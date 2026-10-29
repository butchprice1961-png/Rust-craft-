//! File helpers that keep user data intact when the process dies mid-write
//! and keep untrusted names from escaping managed directories.

use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Joins `untrusted` (a version id, library path or archive entry name) onto
/// `root`, rejecting anything that is not a plain relative path.
pub fn safe_join(root: &Path, untrusted: &str) -> io::Result<PathBuf> {
    let rel = Path::new(untrusted);
    let plain = !untrusted.is_empty()
        && !untrusted.contains(['\\', ':', '\0'])
        && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe path {untrusted:?}"),
        ));
    }
    Ok(root.join(rel))
}

/// Writes via a sibling temp file and rename, so readers see either the old
/// or the new contents, never a torn file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

/// What happened while loading a config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    Missing,
    /// The file was unreadable or malformed. It was moved to `backup` and
    /// defaults are in use, so the user's original text is never lost.
    Recovered {
        backup: PathBuf,
        error: String,
    },
}

/// Loads TOML into `T`, falling back to `T::default()` if the file is absent
/// or broken. A broken file is renamed aside rather than overwritten.
pub fn load_toml_or_default<T: DeserializeOwned + Default>(path: &Path) -> (T, LoadOutcome) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return (T::default(), LoadOutcome::Missing);
        }
        Err(e) => return recover(path, e.to_string()),
    };
    match toml::from_str(&text) {
        Ok(value) => (value, LoadOutcome::Loaded),
        Err(e) => recover(path, e.to_string()),
    }
}

fn recover<T: Default>(path: &Path, error: String) -> (T, LoadOutcome) {
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".broken-{}", unix_time()));
    let backup = PathBuf::from(backup);
    if let Err(e) = std::fs::rename(path, &backup) {
        tracing::warn!("could not move broken config {}: {e}", path.display());
    }
    tracing::warn!(
        "config {} was invalid ({error}); using defaults",
        path.display()
    );
    (T::default(), LoadOutcome::Recovered { backup, error })
}

pub fn save_toml<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let text = toml::to_string_pretty(value).map_err(io::Error::other)?;
    write_atomic(path, text.as_bytes())
}

pub fn unix_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn safe_join_rejects_escapes() {
        let root = Path::new("/data");
        assert_eq!(
            safe_join(root, "org/lwjgl/lwjgl.jar").unwrap(),
            root.join("org/lwjgl/lwjgl.jar")
        );
        for bad in [
            "",
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:\\x",
            "a\\..\\b",
            "a:stream",
            "./a",
        ] {
            assert!(safe_join(root, bad).is_err(), "{bad} accepted");
        }
    }

    #[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
    struct Cfg {
        name: String,
        memory_mb: u32,
    }

    #[test]
    fn broken_config_is_backed_up_and_defaults_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let (cfg, outcome) = load_toml_or_default::<Cfg>(&path);
        assert_eq!((cfg, outcome), (Cfg::default(), LoadOutcome::Missing));

        let good = Cfg {
            name: "a".into(),
            memory_mb: 2048,
        };
        save_toml(&path, &good).unwrap();
        assert_eq!(
            load_toml_or_default::<Cfg>(&path),
            (good, LoadOutcome::Loaded)
        );

        std::fs::write(&path, "memory_mb = \"lots\"").unwrap();
        let (cfg, outcome) = load_toml_or_default::<Cfg>(&path);
        assert_eq!(cfg, Cfg::default());
        let LoadOutcome::Recovered { backup, .. } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(
            std::fs::read_to_string(backup).unwrap(),
            "memory_mb = \"lots\""
        );
        assert!(!path.exists());
    }
}
