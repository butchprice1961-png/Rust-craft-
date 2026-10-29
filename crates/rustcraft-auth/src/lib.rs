//! Microsoft account sign-in for Minecraft Java Edition.
//!
//! Flow: Microsoft identity platform OAuth 2.0 authorization code with PKCE
//! in the system browser (loopback redirect, `state` checked) → Xbox Live
//! user token → XSTS token → Minecraft services login → ownership
//! entitlement → profile → multiplayer privileges.
//!
//! Only the renewable Microsoft refresh token is persisted, and only in the
//! OS credential store. Every session is issued by Microsoft and Mojang;
//! nothing here bypasses ownership, account restrictions or bans.

mod oauth;
mod services;
pub mod store;

use std::fmt;
use std::time::{Duration, SystemTime};

use tokio_util::sync::CancellationToken;

pub use services::XboxIssue;
pub use store::{AccountRecord, Accounts, MemoryStore, OsKeyring, SecretStore};

/// A credential. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub xbox_user: String,
    pub xsts: String,
    pub minecraft_login: String,
    pub entitlements: String,
    pub profile: String,
    pub attributes: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            authorize: "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize".into(),
            token: "https://login.microsoftonline.com/consumers/oauth2/v2.0/token".into(),
            xbox_user: "https://user.auth.xboxlive.com/user/authenticate".into(),
            xsts: "https://xsts.auth.xboxlive.com/xsts/authorize".into(),
            minecraft_login: "https://api.minecraftservices.com/authentication/login_with_xbox".into(),
            entitlements: "https://api.minecraftservices.com/entitlements/license".into(),
            profile: "https://api.minecraftservices.com/minecraft/profile".into(),
            attributes: "https://api.minecraftservices.com/player/attributes".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Azure application (client) id of a public client. Not a secret; no
    /// client secret is ever used.
    pub client_id: String,
    pub endpoints: Endpoints,
    pub sign_in_timeout: Duration,
}

impl AuthConfig {
    /// Client id from `RUSTCRAFT_MSA_CLIENT_ID` at run time, else the value
    /// baked in at build time. `None` means sign-in is not configured.
    pub fn from_env() -> Option<Self> {
        let id = std::env::var("RUSTCRAFT_MSA_CLIENT_ID")
            .ok()
            .or_else(|| option_env!("RUSTCRAFT_MSA_CLIENT_ID").map(Into::into))
            .filter(|id| !id.trim().is_empty())?;
        Some(Self::new(id.trim()))
    }

    pub fn new(client_id: impl Into<String>) -> Self {
        Self { client_id: client_id.into(), endpoints: Endpoints::default(), sign_in_timeout: Duration::from_secs(300) }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Undashed UUID.
    pub id: String,
    pub name: String,
    pub skin_url: Option<String>,
}

/// What the account may do online. `None` means the privilege service could
/// not be reached; the game server's session check still applies.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Multiplayer {
    pub servers: Option<bool>,
    pub chat: Option<bool>,
    pub ban_reason: Option<String>,
}

impl Multiplayer {
    pub fn blocked_reason(&self) -> Option<String> {
        if let Some(reason) = &self.ban_reason {
            return Some(format!("This account is banned from multiplayer: {reason}"));
        }
        (self.servers == Some(false))
            .then(|| "Multiplayer is disabled for this account (Xbox privacy or family settings).".into())
    }
}

/// A verified Minecraft session for an account that owns Java Edition.
#[derive(Debug, Clone)]
pub struct Session {
    pub profile: Profile,
    pub access_token: Secret,
    pub expires_at: SystemTime,
    pub multiplayer: Multiplayer,
}

impl Session {
    /// True if the token is valid for at least `margin` longer.
    pub fn valid_for(&self, margin: Duration) -> bool {
        SystemTime::now() + margin < self.expires_at
    }
}

#[derive(Debug, Clone)]
pub struct SignedIn {
    pub session: Session,
    pub refresh_token: Secret,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Microsoft sign-in is not configured: set RUSTCRAFT_MSA_CLIENT_ID (see docs/AUTHENTICATION.md)")]
    NotConfigured,
    #[error("sign-in was cancelled")]
    Cancelled,
    #[error("sign-in timed out waiting for the browser")]
    Timeout,
    #[error("sign-in was declined in the browser")]
    Denied,
    #[error("Microsoft sign-in failed: {error}{}", description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    OAuth { error: String, description: Option<String> },
    #[error("the saved sign-in has expired; please sign in again")]
    SessionExpired,
    #[error("{}", .0.message())]
    Xbox(XboxIssue),
    #[error("this build's Azure application is not approved for Minecraft sign-in (see docs/AUTHENTICATION.md)")]
    AppNotApproved,
    #[error("this Microsoft account does not own Minecraft: Java Edition")]
    NotOwned,
    #[error("this account owns Minecraft but has no profile yet; create one at minecraft.net")]
    NoProfile,
    #[error("Minecraft services are rate limiting sign-ins; try again in a minute")]
    RateLimited,
    #[error("{stage} failed with HTTP {status}{}", if body.is_empty() { String::new() } else { format!(": {body}") })]
    Http { stage: &'static str, status: u16, body: String },
    #[error("network error during {stage}: {source}")]
    Network {
        stage: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("unexpected response during {stage}: {detail}")]
    Protocol { stage: &'static str, detail: String },
    #[error("local sign-in server error: {0}")]
    Io(#[from] std::io::Error),
}

/// Orchestrates the full Microsoft → Xbox → Minecraft chain.
#[derive(Debug, Clone)]
pub struct Authenticator {
    http: reqwest::Client,
    config: AuthConfig,
}

impl Authenticator {
    pub fn new(config: AuthConfig) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(concat!("RustCraft/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client configuration is static");
        Self { http, config }
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    /// Interactive sign-in. `on_url` receives the Microsoft sign-in page URL
    /// to open in the system browser (see [`open_in_browser`]) and to show as
    /// a fallback link.
    pub async fn sign_in(&self, on_url: impl FnOnce(&str), cancel: &CancellationToken) -> Result<SignedIn, AuthError> {
        let tokens = oauth::authorize_interactive(&self.http, &self.config, on_url, cancel).await?;
        self.finish(tokens).await
    }

    /// Silent renewal with a stored refresh token.
    pub async fn refresh(&self, refresh_token: &Secret) -> Result<SignedIn, AuthError> {
        let tokens = oauth::refresh(&self.http, &self.config, refresh_token).await?;
        self.finish(tokens).await
    }

    async fn finish(&self, ms: oauth::MsTokens) -> Result<SignedIn, AuthError> {
        let ep = &self.config.endpoints;
        let xbl = services::xbox_user(&self.http, ep, &ms.access_token).await?;
        let xsts = services::xsts(&self.http, ep, &xbl.token).await?;
        let (access_token, lifetime) = services::minecraft_login(&self.http, ep, &xsts).await?;
        let owns = services::owns_java_edition(&self.http, ep, &access_token).await?;
        let profile = services::profile(&self.http, ep, &access_token).await?;
        let profile = match (owns, profile) {
            (true, Some(p)) => p,
            (true, None) => return Err(AuthError::NoProfile),
            (false, _) => return Err(AuthError::NotOwned),
        };
        let multiplayer = services::multiplayer(&self.http, ep, &access_token).await;
        Ok(SignedIn {
            session: Session { profile, access_token, expires_at: SystemTime::now() + lifetime, multiplayer },
            refresh_token: ms.refresh_token,
        })
    }
}

/// Opens `url` in the user's default browser.
pub fn open_in_browser(url: &str) -> std::io::Result<()> {
    open::that_detached(url)
}

/// Keeps error bodies short and single-line for display.
fn summarize(body: &str) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 300 { flat.chars().take(300).collect::<String>() + "…" } else { flat }
}
