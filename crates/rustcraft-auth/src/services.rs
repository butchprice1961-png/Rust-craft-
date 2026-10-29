//! Xbox Live, XSTS and Minecraft services calls.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::{AuthError, Endpoints, Multiplayer, Profile, Secret, summarize};

/// Account problems Xbox reports as `XErr` codes during XSTS authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XboxIssue {
    Banned,
    GuardianPermissionRequired,
    NoXboxProfile,
    TermsNotAccepted,
    RegionUnavailable,
    VerificationRequired,
    ChildNotInFamily,
    Other(u64),
}

impl XboxIssue {
    pub fn from_code(code: u64) -> Self {
        match code {
            2148916227 => Self::Banned,
            2148916229 => Self::GuardianPermissionRequired,
            2148916233 => Self::NoXboxProfile,
            2148916234 => Self::TermsNotAccepted,
            2148916235 => Self::RegionUnavailable,
            2148916236 | 2148916237 => Self::VerificationRequired,
            2148916238 => Self::ChildNotInFamily,
            other => Self::Other(other),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::Banned => "This Microsoft account is banned from Xbox services.".into(),
            Self::GuardianPermissionRequired => {
                "This account is restricted: a parent or guardian must allow online play at account.xbox.com.".into()
            }
            Self::NoXboxProfile => {
                "This Microsoft account has no Xbox profile. Sign in once at xbox.com to create one, then retry.".into()
            }
            Self::TermsNotAccepted => "Accept the Xbox terms of service at xbox.com, then retry.".into(),
            Self::RegionUnavailable => "Xbox Live is not available in this account's country or region.".into(),
            Self::VerificationRequired => {
                "Xbox requires additional verification for this account (for example age verification). Complete it at xbox.com.".into()
            }
            Self::ChildNotInFamily => {
                "This is a child account: an adult must add it to a Microsoft family group before it can sign in.".into()
            }
            Self::Other(code) => format!("Xbox Live rejected the sign-in (XErr {code})."),
        }
    }
}

#[derive(Debug)]
pub(crate) struct XboxToken {
    pub token: Secret,
    pub user_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct XboxResponse {
    token: String,
    display_claims: DisplayClaims,
}

#[derive(Deserialize)]
struct DisplayClaims {
    xui: Vec<UserClaim>,
}

#[derive(Deserialize)]
struct UserClaim {
    uhs: String,
}

#[derive(Deserialize)]
struct XboxErrorBody {
    #[serde(rename = "XErr")]
    xerr: u64,
}

async fn post_json(
    http: &reqwest::Client,
    stage: &'static str,
    url: &str,
    body: serde_json::Value,
) -> Result<(u16, String), AuthError> {
    let response = http
        .post(url)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|source| AuthError::Network { stage, source })?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|source| AuthError::Network { stage, source })?;
    Ok((status, text))
}

fn parse_xbox(stage: &'static str, status: u16, body: &str) -> Result<XboxToken, AuthError> {
    if status != 200 {
        if let Ok(e) = serde_json::from_str::<XboxErrorBody>(body) {
            return Err(AuthError::Xbox(XboxIssue::from_code(e.xerr)));
        }
        return Err(AuthError::Http {
            stage,
            status,
            body: summarize(body),
        });
    }
    let r: XboxResponse = serde_json::from_str(body).map_err(|e| AuthError::Protocol {
        stage,
        detail: e.to_string(),
    })?;
    let user_hash = r
        .display_claims
        .xui
        .into_iter()
        .next()
        .ok_or_else(|| AuthError::Protocol {
            stage,
            detail: "no user hash".into(),
        })?
        .uhs;
    Ok(XboxToken {
        token: Secret::new(r.token),
        user_hash,
    })
}

pub(crate) async fn xbox_user(
    http: &reqwest::Client,
    ep: &Endpoints,
    ms_access: &Secret,
) -> Result<XboxToken, AuthError> {
    const STAGE: &str = "Xbox Live authentication";
    let body = json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={}", ms_access.expose()),
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT",
    });
    let (status, text) = post_json(http, STAGE, &ep.xbox_user, body).await?;
    parse_xbox(STAGE, status, &text)
}

pub(crate) async fn xsts(
    http: &reqwest::Client,
    ep: &Endpoints,
    user_token: &Secret,
) -> Result<XboxToken, AuthError> {
    const STAGE: &str = "Xbox XSTS authorization";
    let body = json!({
        "Properties": { "SandboxId": "RETAIL", "UserTokens": [user_token.expose()] },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT",
    });
    let (status, text) = post_json(http, STAGE, &ep.xsts, body).await?;
    parse_xbox(STAGE, status, &text)
}

#[derive(Deserialize)]
struct LoginResponse {
    access_token: String,
    expires_in: u64,
}

pub(crate) async fn minecraft_login(
    http: &reqwest::Client,
    ep: &Endpoints,
    xsts: &XboxToken,
) -> Result<(Secret, Duration), AuthError> {
    const STAGE: &str = "Minecraft services login";
    let body =
        json!({ "identityToken": format!("XBL3.0 x={};{}", xsts.user_hash, xsts.token.expose()) });
    let (status, text) = post_json(http, STAGE, &ep.minecraft_login, body).await?;
    match status {
        200 => {
            let r: LoginResponse =
                serde_json::from_str(&text).map_err(|e| AuthError::Protocol {
                    stage: STAGE,
                    detail: e.to_string(),
                })?;
            Ok((
                Secret::new(r.access_token),
                Duration::from_secs(r.expires_in),
            ))
        }
        403 if text.contains("Invalid app registration") => Err(AuthError::AppNotApproved),
        429 => Err(AuthError::RateLimited),
        _ => Err(AuthError::Http {
            stage: STAGE,
            status,
            body: summarize(&text),
        }),
    }
}

async fn get(
    http: &reqwest::Client,
    stage: &'static str,
    url: &str,
    token: &Secret,
    query: &[(&str, &str)],
) -> Result<(u16, String), AuthError> {
    let response = http
        .get(url)
        .query(query)
        .bearer_auth(token.expose())
        .send()
        .await
        .map_err(|source| AuthError::Network { stage, source })?;
    let status = response.status().as_u16();
    let text = response
        .text()
        .await
        .map_err(|source| AuthError::Network { stage, source })?;
    if status == 429 {
        return Err(AuthError::RateLimited);
    }
    Ok((status, text))
}

#[derive(Deserialize)]
struct Entitlements {
    #[serde(default)]
    items: Vec<Entitlement>,
}

#[derive(Deserialize)]
struct Entitlement {
    name: String,
}

pub(crate) async fn owns_java_edition(
    http: &reqwest::Client,
    ep: &Endpoints,
    token: &Secret,
) -> Result<bool, AuthError> {
    const STAGE: &str = "Minecraft entitlement check";
    let request_id = uuid_v4();
    let (status, text) = get(
        http,
        STAGE,
        &ep.entitlements,
        token,
        &[("requestId", &request_id)],
    )
    .await?;
    if status != 200 {
        return Err(AuthError::Http {
            stage: STAGE,
            status,
            body: summarize(&text),
        });
    }
    let e: Entitlements = serde_json::from_str(&text).map_err(|e| AuthError::Protocol {
        stage: STAGE,
        detail: e.to_string(),
    })?;
    Ok(e.items
        .iter()
        .any(|i| i.name == "game_minecraft" || i.name == "product_minecraft"))
}

#[derive(Deserialize)]
struct ProfileResponse {
    id: String,
    name: String,
    #[serde(default)]
    skins: Vec<Skin>,
}

#[derive(Deserialize)]
struct Skin {
    state: String,
    url: String,
}

pub(crate) async fn profile(
    http: &reqwest::Client,
    ep: &Endpoints,
    token: &Secret,
) -> Result<Option<Profile>, AuthError> {
    const STAGE: &str = "Minecraft profile lookup";
    let (status, text) = get(http, STAGE, &ep.profile, token, &[]).await?;
    match status {
        200 => {
            let p: ProfileResponse =
                serde_json::from_str(&text).map_err(|e| AuthError::Protocol {
                    stage: STAGE,
                    detail: e.to_string(),
                })?;
            let skin_url = p
                .skins
                .into_iter()
                .find(|s| s.state == "ACTIVE")
                .map(|s| s.url);
            Ok(Some(Profile {
                id: p.id,
                name: p.name,
                skin_url,
            }))
        }
        404 => Ok(None),
        _ => Err(AuthError::Http {
            stage: STAGE,
            status,
            body: summarize(&text),
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attributes {
    privileges: Option<Privileges>,
    ban_status: Option<BanStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Privileges {
    online_chat: Option<Privilege>,
    multiplayer_server: Option<Privilege>,
}

#[derive(Deserialize)]
struct Privilege {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BanStatus {
    #[serde(default)]
    banned_scopes: std::collections::BTreeMap<String, Ban>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ban {
    reason_message: Option<String>,
    reason: Option<String>,
}

/// Never fails: an unreachable privilege service leaves the fields unknown.
pub(crate) async fn multiplayer(
    http: &reqwest::Client,
    ep: &Endpoints,
    token: &Secret,
) -> Multiplayer {
    let fetched = get(http, "player attributes", &ep.attributes, token, &[]).await;
    let attrs = match fetched {
        Ok((200, text)) => serde_json::from_str::<Attributes>(&text).ok(),
        _ => None,
    };
    let Some(attrs) = attrs else {
        tracing::warn!("could not read multiplayer privileges; leaving them unknown");
        return Multiplayer::default();
    };
    let privileges = attrs.privileges;
    let ban = attrs.ban_status.and_then(|b| {
        b.banned_scopes.get("MULTIPLAYER").map(|ban| {
            ban.reason_message
                .clone()
                .or_else(|| ban.reason.clone())
                .unwrap_or_else(|| "no reason given".into())
        })
    });
    Multiplayer {
        servers: privileges
            .as_ref()
            .and_then(|p| p.multiplayer_server.as_ref())
            .map(|p| p.enabled),
        chat: privileges
            .as_ref()
            .and_then(|p| p.online_chat.as_ref())
            .map(|p| p.enabled),
        ban_reason: ban,
    }
}

fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random number generator unavailable");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xerr_codes_map_to_actionable_messages() {
        assert_eq!(XboxIssue::from_code(2148916233), XboxIssue::NoXboxProfile);
        assert_eq!(
            XboxIssue::from_code(2148916238),
            XboxIssue::ChildNotInFamily
        );
        assert!(XboxIssue::from_code(1).message().contains("XErr 1"));
        let err = parse_xbox(
            "x",
            401,
            r#"{"Identity":"0","XErr":2148916235,"Message":"","Redirect":""}"#,
        )
        .unwrap_err();
        assert!(matches!(err, AuthError::Xbox(XboxIssue::RegionUnavailable)));
    }

    #[test]
    fn uuid_v4_shape() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }
}
