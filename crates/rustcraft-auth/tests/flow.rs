//! Full sign-in chain against mock Microsoft, Xbox and Minecraft services,
//! with a scripted "browser" hitting the loopback redirect.

use std::time::Duration;

use rustcraft_auth::{Accounts, AuthConfig, AuthError, Authenticator, Endpoints, MemoryStore, SecretStore, XboxIssue};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROFILE_ID: &str = "069a79f444e94726a5befca90e38aaf5";

fn config(server: &MockServer) -> AuthConfig {
    let u = server.uri();
    let mut cfg = AuthConfig::new("test-client-id");
    cfg.endpoints = Endpoints {
        authorize: format!("{u}/authorize"),
        token: format!("{u}/token"),
        xbox_user: format!("{u}/xbl"),
        xsts: format!("{u}/xsts"),
        minecraft_login: format!("{u}/mc-login"),
        entitlements: format!("{u}/entitlements"),
        profile: format!("{u}/profile"),
        attributes: format!("{u}/attributes"),
    };
    cfg.sign_in_timeout = Duration::from_secs(20);
    cfg
}

async fn mount(server: &MockServer, m: &str, p: &str, status: u16, body: serde_json::Value) {
    Mock::given(method(m))
        .and(path(p))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

/// Mounts a working service chain from the Xbox step onward.
async fn services_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/xbl"))
        .and(body_string_contains("\"RpsTicket\":\"d=ms-access\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Token": "xbl-token", "DisplayClaims": { "xui": [ { "uhs": "uhs123" } ] } })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/xsts"))
        .and(body_string_contains("xbl-token"))
        .and(body_string_contains("rp://api.minecraftservices.com/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Token": "xsts-token", "DisplayClaims": { "xui": [ { "uhs": "uhs123" } ] } })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mc-login"))
        .and(body_string_contains("XBL3.0 x=uhs123;xsts-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "username": "not-the-player", "access_token": "mc-access", "token_type": "Bearer", "expires_in": 86400 })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/entitlements"))
        .and(header("authorization", "Bearer mc-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [ { "name": "product_minecraft", "source": "PURCHASE" }, { "name": "game_minecraft", "source": "PURCHASE" } ] })))
        .mount(server)
        .await;
    mount(server, "GET", "/profile", 200, json!({
        "id": PROFILE_ID, "name": "Notch",
        "skins": [ { "id": "1", "state": "ACTIVE", "url": "https://textures.minecraft.net/texture/abc", "variant": "CLASSIC" } ],
        "capes": [] }))
    .await;
    mount(
        server,
        "GET",
        "/attributes",
        200,
        json!({
        "privileges": { "onlineChat": { "enabled": false }, "multiplayerServer": { "enabled": true } },
        "banStatus": { "bannedScopes": {} } }),
    )
    .await;
}

async fn token_ok(server: &MockServer, grant: &str) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(format!("grant_type={grant}")))
        .and(body_string_contains("client_id=test-client-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token_type": "bearer", "scope": "XboxLive.signin offline_access", "expires_in": 3600,
            "access_token": "ms-access", "refresh_token": format!("ms-refresh-{grant}") })))
        .mount(server)
        .await;
}

/// Plays the browser: checks the sign-in URL, sends a forged request with a
/// wrong state, then the real redirect with `query`.
fn browser(query: &'static str) -> impl FnOnce(&str) {
    move |sign_in_url: &str| {
        let url = url::Url::parse(sign_in_url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "test-client-id");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["scope"], "XboxLive.signin offline_access");
        assert!(q["redirect_uri"].starts_with("http://localhost:"));
        let redirect = q["redirect_uri"].replace("localhost", "127.0.0.1");
        let state = q["state"].clone();
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            let forged = http.get(format!("{redirect}/?code=evil&state=wrong")).send().await.unwrap();
            assert_eq!(forged.status(), 400);
            let real = http.get(format!("{redirect}/?{query}&state={state}")).send().await.unwrap();
            assert_eq!(real.status(), 200);
        });
    }
}

fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[tokio::test]
async fn sign_in_store_and_renew() {
    install_crypto();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=good-code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "ms-access", "refresh_token": "ms-refresh-1", "expires_in": 3600 })))
        .expect(1)
        .mount(&server)
        .await;
    services_ok(&server).await;
    let auth = Authenticator::new(config(&server));

    let signed_in = auth.sign_in(browser("code=good-code"), &CancellationToken::new()).await.unwrap();
    let s = &signed_in.session;
    assert_eq!(s.profile.name, "Notch");
    assert_eq!(s.profile.id, PROFILE_ID);
    assert_eq!(s.access_token.expose(), "mc-access");
    assert!(s.valid_for(Duration::from_secs(80_000)));
    assert_eq!(s.multiplayer.servers, Some(true));
    assert_eq!(s.multiplayer.chat, Some(false));
    assert_eq!(s.multiplayer.blocked_reason(), None);
    assert!(!format!("{signed_in:?}").contains("mc-access"));

    // Persist: tokens go to the secret store only.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("accounts.toml");
    let store = std::sync::Arc::new(MemoryStore::default());
    let mut accounts = Accounts::load(file.clone(), Box::new(SharedStore(store.clone())));
    assert_eq!(accounts.add(signed_in).unwrap(), None);
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("Notch") && !on_disk.contains("ms-refresh") && !on_disk.contains("mc-access"));
    assert_eq!(accounts.active().unwrap().id, PROFILE_ID);
    assert_eq!(accounts.session(&auth, PROFILE_ID).await.unwrap().access_token.expose(), "mc-access");

    // Next launch: no cached session, so the stored refresh token renews it.
    token_ok(&server, "refresh_token").await;
    let mut accounts = Accounts::load(file.clone(), Box::new(SharedStore(store.clone())));
    assert!(accounts.cached_session(PROFILE_ID).is_none());
    let renewed = accounts.session(&auth, PROFILE_ID).await.unwrap();
    assert_eq!(renewed.profile.name, "Notch");
    assert_eq!(
        store.get("msa-refresh:069a79f444e94726a5befca90e38aaf5").unwrap().unwrap().expose(),
        "ms-refresh-refresh_token"
    );

    // Sign out removes the credential and the record.
    accounts.remove(PROFILE_ID).unwrap();
    assert!(accounts.records().is_empty());
    assert!(store.get(&format!("msa-refresh:{PROFILE_ID}")).unwrap().is_none());
}

#[tokio::test]
async fn expired_refresh_token_requires_new_sign_in() {
    install_crypto();
    let server = MockServer::start().await;
    mount(
        &server,
        "POST",
        "/token",
        400,
        json!({
        "error": "invalid_grant", "error_description": "AADSTS70000: The refresh token has expired.\r\nTrace ID: x" }),
    )
    .await;
    let auth = Authenticator::new(config(&server));
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(MemoryStore::default());
    store.set(&format!("msa-refresh:{PROFILE_ID}"), &rustcraft_auth::Secret::new("old")).unwrap();
    let mut accounts = Accounts::load(dir.path().join("a.toml"), Box::new(SharedStore(store.clone())));
    let err = accounts.session(&auth, PROFILE_ID).await.unwrap_err();
    assert!(matches!(err, AuthError::SessionExpired), "{err}");
    assert!(store.get(&format!("msa-refresh:{PROFILE_ID}")).unwrap().is_none());
}

#[tokio::test]
async fn user_declining_in_browser_is_reported() {
    install_crypto();
    let server = MockServer::start().await;
    let auth = Authenticator::new(config(&server));
    let err = auth
        .sign_in(browser("error=access_denied&error_description=The+user+declined"), &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Denied), "{err}");
}

#[tokio::test]
async fn cancel_stops_waiting_for_browser() {
    install_crypto();
    let server = MockServer::start().await;
    let auth = Authenticator::new(config(&server));
    let cancel = CancellationToken::new();
    let c = cancel.clone();
    let err = auth.sign_in(move |_| c.cancel(), &cancel).await.unwrap_err();
    assert!(matches!(err, AuthError::Cancelled));
}

#[tokio::test]
async fn account_problems_map_to_clear_errors() {
    install_crypto();

    // No Xbox profile.
    let server = MockServer::start().await;
    token_ok(&server, "refresh_token").await;
    mount(
        &server,
        "POST",
        "/xbl",
        200,
        json!({ "Token": "xbl-token", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } }),
    )
    .await;
    mount(&server, "POST", "/xsts", 401, json!({ "Identity": "0", "XErr": 2148916233u64, "Message": "", "Redirect": "https://start.ui.xboxlive.com/CreateAccount" })).await;
    let err = Authenticator::new(config(&server)).refresh(&rustcraft_auth::Secret::new("rt")).await.unwrap_err();
    assert!(matches!(err, AuthError::Xbox(XboxIssue::NoXboxProfile)), "{err}");
    assert!(err.to_string().contains("xbox.com"));

    // Azure app not approved for Minecraft services.
    let server = MockServer::start().await;
    token_ok(&server, "refresh_token").await;
    mount(&server, "POST", "/xbl", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } })).await;
    mount(&server, "POST", "/xsts", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } })).await;
    mount(
        &server,
        "POST",
        "/mc-login",
        403,
        json!({ "path": "/authentication/login_with_xbox",
        "errorMessage": "Invalid app registration, see https://aka.ms/AppRegInfo for more information" }),
    )
    .await;
    let err = Authenticator::new(config(&server)).refresh(&rustcraft_auth::Secret::new("rt")).await.unwrap_err();
    assert!(matches!(err, AuthError::AppNotApproved), "{err}");

    // Does not own the game; then owns it but has no profile.
    for (items, expect_not_owned) in [(json!([]), true), (json!([ { "name": "game_minecraft" } ]), false)] {
        let server = MockServer::start().await;
        token_ok(&server, "refresh_token").await;
        mount(&server, "POST", "/xbl", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } }))
            .await;
        mount(&server, "POST", "/xsts", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } }))
            .await;
        mount(&server, "POST", "/mc-login", 200, json!({ "access_token": "mc", "expires_in": 86400 })).await;
        mount(&server, "GET", "/entitlements", 200, json!({ "items": items })).await;
        mount(
            &server,
            "GET",
            "/profile",
            404,
            json!({ "path": "/minecraft/profile", "errorType": "NOT_FOUND", "error": "NOT_FOUND" }),
        )
        .await;
        let err = Authenticator::new(config(&server)).refresh(&rustcraft_auth::Secret::new("rt")).await.unwrap_err();
        if expect_not_owned {
            assert!(matches!(err, AuthError::NotOwned), "{err}");
        } else {
            assert!(matches!(err, AuthError::NoProfile), "{err}");
        }
    }
}

#[tokio::test]
async fn multiplayer_ban_is_surfaced() {
    install_crypto();
    let server = MockServer::start().await;
    token_ok(&server, "refresh_token").await;
    mount(&server, "POST", "/xbl", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } })).await;
    mount(&server, "POST", "/xsts", 200, json!({ "Token": "x", "DisplayClaims": { "xui": [ { "uhs": "u" } ] } })).await;
    mount(&server, "POST", "/mc-login", 200, json!({ "access_token": "mc", "expires_in": 86400 })).await;
    mount(&server, "GET", "/entitlements", 200, json!({ "items": [ { "name": "product_minecraft" } ] })).await;
    mount(&server, "GET", "/profile", 200, json!({ "id": PROFILE_ID, "name": "Notch" })).await;
    mount(&server, "GET", "/attributes", 200, json!({
        "privileges": { "multiplayerServer": { "enabled": true } },
        "banStatus": { "bannedScopes": { "MULTIPLAYER": { "banId": "b", "expires": 0, "reason": "false_reporting", "reasonMessage": "Account suspended" } } } }))
    .await;
    let s = Authenticator::new(config(&server)).refresh(&rustcraft_auth::Secret::new("rt")).await.unwrap().session;
    assert!(s.multiplayer.blocked_reason().unwrap().contains("Account suspended"));
}

/// Lets a test inspect the store after handing it to `Accounts`.
struct SharedStore(std::sync::Arc<MemoryStore>);

impl SecretStore for SharedStore {
    fn get(&self, key: &str) -> Result<Option<rustcraft_auth::Secret>, String> {
        self.0.get(key)
    }
    fn set(&self, key: &str, value: &rustcraft_auth::Secret) -> Result<(), String> {
        self.0.set(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.delete(key)
    }
}
