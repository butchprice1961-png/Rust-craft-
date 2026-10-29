//! Microsoft identity platform: authorization code + PKCE (RFC 7636) with a
//! loopback redirect (RFC 8252), and refresh-token renewal.

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

use crate::{AuthConfig, AuthError, Secret, summarize};

pub(crate) const SCOPE: &str = "XboxLive.signin offline_access";

pub(crate) struct MsTokens {
    pub access_token: Secret,
    pub refresh_token: Secret,
}

pub(crate) fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    URL_SAFE_NO_PAD.encode(buf)
}

/// S256 code challenge for `verifier`.
pub(crate) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(crate) async fn authorize_interactive(
    http: &reqwest::Client,
    cfg: &AuthConfig,
    on_url: impl FnOnce(&str),
    cancel: &CancellationToken,
) -> Result<MsTokens, AuthError> {
    // Loopback only: nothing off this machine can reach the redirect server.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect = format!("http://localhost:{}", listener.local_addr()?.port());
    let verifier = random_token(32);
    let state = random_token(16);
    let url = url::Url::parse_with_params(
        &cfg.endpoints.authorize,
        [
            ("client_id", cfg.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("response_mode", "query"),
            ("scope", SCOPE),
            ("state", &state),
            ("code_challenge", &challenge(&verifier)),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ],
    )
    .map_err(|e| AuthError::Protocol { stage: "sign-in URL", detail: e.to_string() })?;
    on_url(url.as_str());

    let code = tokio::select! {
        _ = cancel.cancelled() => return Err(AuthError::Cancelled),
        _ = tokio::time::sleep(cfg.sign_in_timeout) => return Err(AuthError::Timeout),
        code = wait_for_redirect(&listener, &state) => code?,
    };
    token_request(
        http,
        cfg,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ],
    )
    .await
}

pub(crate) async fn refresh(
    http: &reqwest::Client,
    cfg: &AuthConfig,
    refresh_token: &Secret,
) -> Result<MsTokens, AuthError> {
    token_request(http, cfg, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token.expose())])
        .await
        .map_err(|e| match e {
            AuthError::OAuth { error, .. } if error == "invalid_grant" => AuthError::SessionExpired,
            other => other,
        })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

async fn token_request(
    http: &reqwest::Client,
    cfg: &AuthConfig,
    params: &[(&str, &str)],
) -> Result<MsTokens, AuthError> {
    const STAGE: &str = "Microsoft token request";
    let mut form = vec![("client_id", cfg.client_id.as_str()), ("scope", SCOPE)];
    form.extend_from_slice(params);
    let response = http
        .post(&cfg.endpoints.token)
        .form(&form)
        .send()
        .await
        .map_err(|source| AuthError::Network { stage: STAGE, source })?;
    let status = response.status();
    let body = response.text().await.map_err(|source| AuthError::Network { stage: STAGE, source })?;
    if !status.is_success() {
        return Err(match serde_json::from_str::<TokenError>(&body) {
            Ok(e) => AuthError::OAuth {
                error: e.error,
                // Descriptions are multi-line with trace ids; keep the first line.
                description: e.error_description.map(|d| summarize(d.lines().next().unwrap_or_default())),
            },
            Err(_) => AuthError::Http { stage: STAGE, status: status.as_u16(), body: summarize(&body) },
        });
    }
    let tokens: TokenResponse =
        serde_json::from_str(&body).map_err(|e| AuthError::Protocol { stage: STAGE, detail: e.to_string() })?;
    let refresh_token = tokens.refresh_token.ok_or(AuthError::Protocol {
        stage: STAGE,
        detail: "no refresh token (offline_access scope missing)".into(),
    })?;
    Ok(MsTokens { access_token: Secret::new(tokens.access_token), refresh_token: Secret::new(refresh_token) })
}

/// Serves the loopback redirect until a request carrying our `state`
/// arrives. Requests with a wrong or missing state (stray tabs, other local
/// pages) are answered with an error page and ignored.
async fn wait_for_redirect(listener: &TcpListener, state: &str) -> Result<String, AuthError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Ok(Some(target)) = tokio::time::timeout(Duration::from_secs(5), read_target(&mut stream)).await else {
            continue;
        };
        let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
            respond(&mut stream, "400 Bad Request", false).await;
            continue;
        };
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        if params.get("state").map(String::as_str) != Some(state) {
            respond(&mut stream, "400 Bad Request", false).await;
            continue;
        }
        if let Some(error) = params.get("error") {
            respond(&mut stream, "200 OK", false).await;
            return Err(if error == "access_denied" {
                AuthError::Denied
            } else {
                AuthError::OAuth { error: error.clone(), description: params.get("error_description").cloned() }
            });
        }
        if let Some(code) = params.get("code") {
            respond(&mut stream, "200 OK", true).await;
            return Ok(code.clone());
        }
        respond(&mut stream, "400 Bad Request", false).await;
    }
}

/// Reads one request head and returns its target (`/path?query`).
async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 || buf.len() + n > 16 * 1024 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = std::str::from_utf8(&buf).ok()?;
    let mut parts = head.lines().next()?.split(' ');
    (parts.next()? == "GET").then_some(())?;
    Some(parts.next()?.to_string())
}

async fn respond(stream: &mut TcpStream, status: &str, ok: bool) {
    let (title, text) = if ok {
        ("Signed in", "You are signed in to RustCraft. You can close this tab.")
    } else {
        ("Sign-in not completed", "RustCraft did not receive a valid sign-in. Return to RustCraft and try again.")
    };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <body style=\"background:#17181b;color:#e8e8ea;font:16px system-ui;display:grid;place-items:center;height:90vh\">\
         <div><h1 style=\"font-weight:600\">{title}</h1><p>{text}</p></div>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc7636_vector() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let v = random_token(32);
        assert_eq!(v.len(), 43);
        assert_ne!(v, random_token(32));
    }
}
