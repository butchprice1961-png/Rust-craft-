//! Signed-in accounts. The account list (`accounts.toml`) holds only public
//! profile data; refresh tokens live in the OS credential store (Windows
//! Credential Manager, macOS Keychain, Secret Service on Linux) and nowhere
//! else on disk.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use rustcraft_core::fs::{load_toml_or_default, save_toml, unix_time};

use crate::{AuthError, Authenticator, Secret, Session, SignedIn};

const SERVICE: &str = "RustCraft";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Secret>, String>;
    fn set(&self, key: &str, value: &Secret) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The operating system's credential store.
pub struct OsKeyring;

impl SecretStore for OsKeyring {
    fn get(&self, key: &str) -> Result<Option<Secret>, String> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        // Raw bytes: Windows caps blobs at 2560 bytes and `set_password`
        // would spend two of them per character.
        match entry.get_secret() {
            Ok(bytes) => String::from_utf8(bytes)
                .map(|s| Some(Secret::new(s)))
                .map_err(|_| "stored credential is not UTF-8".into()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, key: &str, value: &Secret) -> Result<(), String> {
        keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.set_secret(value.expose().as_bytes()))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Process-lifetime store for tests and for systems without a keyring.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, Secret>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<Secret>, String> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &Secret) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRecord {
    /// Minecraft profile UUID (undashed).
    pub id: String,
    pub name: String,
    pub skin_url: Option<String>,
    /// Unix time ownership was last verified with Minecraft services.
    pub verified_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AccountsFile {
    active: Option<String>,
    #[serde(default)]
    accounts: Vec<AccountRecord>,
}

pub struct Accounts {
    path: PathBuf,
    file: AccountsFile,
    secrets: Box<dyn SecretStore>,
    /// Refresh tokens the OS store refused; kept for this run only.
    unsaved: HashMap<String, Secret>,
    sessions: HashMap<String, Session>,
}

fn key(id: &str) -> String {
    format!("msa-refresh:{id}")
}

impl Accounts {
    pub fn load(path: PathBuf, secrets: Box<dyn SecretStore>) -> Self {
        let (file, _) = load_toml_or_default(&path);
        Self {
            path,
            file,
            secrets,
            unsaved: HashMap::new(),
            sessions: HashMap::new(),
        }
    }

    pub fn records(&self) -> &[AccountRecord] {
        &self.file.accounts
    }

    pub fn active(&self) -> Option<&AccountRecord> {
        let id = self.file.active.as_deref()?;
        self.file.accounts.iter().find(|a| a.id == id)
    }

    pub fn set_active(&mut self, id: &str) -> io::Result<()> {
        if self.file.accounts.iter().any(|a| a.id == id) {
            self.file.active = Some(id.into());
            self.save()?;
        }
        Ok(())
    }

    /// Records a sign-in and makes it the active account. Returns a warning
    /// when the refresh token could only be kept in memory.
    pub fn add(&mut self, signed_in: SignedIn) -> io::Result<Option<String>> {
        let id = signed_in.session.profile.id.clone();
        let warning = self.store(signed_in)?;
        self.file.active = Some(id);
        self.save()?;
        Ok(warning)
    }

    fn store(&mut self, signed_in: SignedIn) -> io::Result<Option<String>> {
        let profile = &signed_in.session.profile;
        let record = AccountRecord {
            id: profile.id.clone(),
            name: profile.name.clone(),
            skin_url: profile.skin_url.clone(),
            verified_at: unix_time(),
        };
        match self.file.accounts.iter_mut().find(|a| a.id == record.id) {
            Some(existing) => *existing = record,
            None => self.file.accounts.push(record),
        }
        let id = profile.id.clone();
        let warning = match self.secrets.set(&key(&id), &signed_in.refresh_token) {
            Ok(()) => {
                self.unsaved.remove(&id);
                None
            }
            Err(e) => {
                tracing::warn!(
                    "credential store unavailable, keeping sign-in for this session only: {e}"
                );
                self.unsaved
                    .insert(id.clone(), signed_in.refresh_token.clone());
                Some(format!(
                    "Your system credential store is unavailable ({e}); you will need to sign in again next time."
                ))
            }
        };
        self.sessions.insert(id, signed_in.session);
        self.save()?;
        Ok(warning)
    }

    /// Signs out: forgets the account and deletes its stored credential.
    pub fn remove(&mut self, id: &str) -> io::Result<()> {
        if let Err(e) = self.secrets.delete(&key(id)) {
            tracing::warn!("could not delete stored credential: {e}");
        }
        self.unsaved.remove(id);
        self.sessions.remove(id);
        self.file.accounts.retain(|a| a.id != id);
        if self.file.active.as_deref() == Some(id) {
            self.file.active = self.file.accounts.first().map(|a| a.id.clone());
        }
        self.save()
    }

    pub fn cached_session(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }

    /// A session valid for at least five more minutes, renewing it with the
    /// stored refresh token when needed.
    pub async fn session(&mut self, auth: &Authenticator, id: &str) -> Result<Session, AuthError> {
        if let Some(s) = self.sessions.get(id)
            && s.valid_for(Duration::from_secs(300))
        {
            return Ok(s.clone());
        }
        let token = match self.unsaved.get(id) {
            Some(t) => Some(t.clone()),
            None => self.secrets.get(&key(id)).unwrap_or_else(|e| {
                tracing::warn!("credential store unavailable: {e}");
                None
            }),
        };
        let token = token.ok_or(AuthError::SessionExpired)?;
        match auth.refresh(&token).await {
            Ok(signed_in) if signed_in.session.profile.id == id => {
                let session = signed_in.session.clone();
                self.store(signed_in)?;
                Ok(session)
            }
            Ok(_) => Err(AuthError::Protocol {
                stage: "session renewal",
                detail: "renewed a different profile".into(),
            }),
            Err(AuthError::SessionExpired) => {
                let _ = self.secrets.delete(&key(id));
                self.unsaved.remove(id);
                Err(AuthError::SessionExpired)
            }
            Err(e) => Err(e),
        }
    }

    fn save(&self) -> io::Result<()> {
        save_toml(&self.path, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the real OS store. Needs an unlocked keyring (on Linux a
    /// Secret Service daemon on the session bus), so it only runs when
    /// `RUSTCRAFT_TEST_KEYRING=1`.
    #[test]
    fn os_keyring_roundtrip() {
        if std::env::var("RUSTCRAFT_TEST_KEYRING").as_deref() != Ok("1") {
            return;
        }
        let k = format!("test:{}", unix_time());
        // Longer than Windows' 1280-character set_password limit.
        let value = Secret::new("M.C5".to_string() + &"x".repeat(1800));
        OsKeyring.set(&k, &value).unwrap();
        assert_eq!(OsKeyring.get(&k).unwrap(), Some(value));
        OsKeyring.delete(&k).unwrap();
        assert_eq!(OsKeyring.get(&k).unwrap(), None);
    }
}
