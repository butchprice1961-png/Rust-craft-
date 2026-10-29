use std::io;
use std::path::{Path, PathBuf};

use crate::fs::safe_join;

/// Root of all RustCraft-managed data. `RUSTCRAFT_HOME` overrides the
/// platform default so tests and portable installs can relocate it.
///
/// Layout mirrors the official launcher where files are shared
/// (`versions/`, `libraries/`, `assets/`) so caches stay compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    pub fn from_env() -> io::Result<Self> {
        if let Some(home) = std::env::var_os("RUSTCRAFT_HOME") {
            return Ok(Self::at(home));
        }
        let base = dirs::data_dir()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no platform data directory"))?;
        Ok(Self::at(base.join("RustCraft")))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("accounts.toml")
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// `versions/<id>/`; `id` may come from remote metadata so it is checked.
    pub fn version_dir(&self, id: &str) -> io::Result<PathBuf> {
        safe_join(&self.versions(), id)
    }

    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtimes")
    }

    pub fn instances(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn instance_dir(&self, name: &str) -> io::Result<PathBuf> {
        safe_join(&self.instances(), name)
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn natives(&self) -> PathBuf {
        self.root.join("natives")
    }

    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
}
