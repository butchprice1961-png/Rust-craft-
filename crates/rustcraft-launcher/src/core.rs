//! State shared by every launcher window session and the supervisor loop.

use std::io;
use std::sync::Mutex;

use rustcraft_assets::{Downloader, Installer};
use rustcraft_auth::{Accounts, AuthConfig, Authenticator, OsKeyring};
use rustcraft_core::DataDir;
use rustcraft_core::fs::LoadOutcome;

use crate::config::Settings;
use crate::game::{GameOutcome, RunningGame};

pub struct Core {
    pub dirs: DataDir,
    pub rt: tokio::runtime::Runtime,
    pub http: reqwest::Client,
    pub installer: Installer,
    /// `None` when this build has no Azure client id configured.
    pub auth: Option<Authenticator>,
    pub accounts: tokio::sync::Mutex<Accounts>,
    pub settings: Mutex<Settings>,
    /// Shown once if `config.toml` was invalid and has been reset.
    pub settings_notice: Mutex<Option<String>>,
    /// The game process, handed from the window to the supervisor loop.
    pub game: Mutex<Option<RunningGame>>,
    pub last_outcome: Mutex<Option<GameOutcome>>,
}

impl Core {
    pub fn new(dirs: DataDir) -> io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("rustcraft-io")
            .enable_all()
            .build()?;
        let http = rustcraft_assets::http_client();
        let installer = Installer::new(dirs.clone(), Downloader::new(http.clone()));
        let auth = AuthConfig::from_env().map(Authenticator::new);
        let accounts = Accounts::load(dirs.accounts_file(), Box::new(OsKeyring));
        let (settings, outcome) = Settings::load(&dirs);
        let notice = match outcome {
            LoadOutcome::Recovered { backup, error } => Some(format!(
                "Your settings file was invalid and has been reset. The old file was kept at {} ({error}).",
                backup.display()
            )),
            _ => None,
        };
        Ok(Self {
            dirs,
            rt,
            http,
            installer,
            auth,
            accounts: tokio::sync::Mutex::new(accounts),
            settings: Mutex::new(settings),
            settings_notice: Mutex::new(notice),
            game: Mutex::new(None),
            last_outcome: Mutex::new(None),
        })
    }

    pub fn save_settings(&self) {
        let settings = self.settings.lock().unwrap().clone();
        if let Err(e) = settings.save(&self.dirs) {
            tracing::warn!("could not save settings: {e}");
        }
    }
}
