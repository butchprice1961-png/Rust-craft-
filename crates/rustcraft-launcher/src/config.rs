//! Launcher settings (`config.toml`) and game instances
//! (`instances/<name>/instance.toml`, with the instance's game files beside it).

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use rustcraft_core::DataDir;
use rustcraft_core::fs::{LoadOutcome, load_toml_or_default, save_toml, unix_time, write_atomic};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Settings {
    pub selected_instance: Option<String>,
    pub reduced_motion: bool,
    pub show_snapshots: bool,
    /// Used for every instance without its own Java override. `None` lets
    /// RustCraft pick Mojang's runtime or a matching installed Java.
    pub java_path: Option<PathBuf>,
}

impl Settings {
    pub fn load(dirs: &DataDir) -> (Self, LoadOutcome) {
        load_toml_or_default(&dirs.config_file())
    }

    pub fn save(&self, dirs: &DataDir) -> io::Result<()> {
        save_toml(&dirs.config_file(), self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Loader {
    Vanilla,
    Fabric { version: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Graphics {
    Fast,
    Fancy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    #[serde(skip)]
    pub name: String,
    pub game_version: String,
    pub loader: Loader,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    #[serde(default)]
    pub jvm_args: String,
    #[serde(default)]
    pub java_path: Option<PathBuf>,
    #[serde(default)]
    pub resolution: Option<(u32, u32)>,
    /// `host[:port]` to join on launch.
    #[serde(default)]
    pub server: Option<String>,
    pub created: u64,
    #[serde(default)]
    pub last_played: Option<u64>,
}

impl Instance {
    pub fn new(name: &str, game_version: &str, loader: Loader) -> Self {
        Self {
            name: name.into(),
            game_version: game_version.into(),
            loader,
            min_memory_mb: 512,
            max_memory_mb: 2048,
            jvm_args: String::new(),
            java_path: None,
            resolution: None,
            server: None,
            created: unix_time(),
            last_played: None,
        }
    }

    pub fn dir(&self, dirs: &DataDir) -> io::Result<PathBuf> {
        dirs.instance_dir(&self.name)
    }

    pub fn loader_label(&self) -> String {
        match &self.loader {
            Loader::Vanilla => "Vanilla".into(),
            Loader::Fabric { version } => format!("Fabric {version}"),
        }
    }

    pub fn save(&self, dirs: &DataDir) -> io::Result<()> {
        save_toml(&self.dir(dirs)?.join("instance.toml"), self)
    }

    /// Creates the instance directory and settings. Initial game options
    /// are written only if the instance has none, so existing worlds and
    /// settings are never overwritten.
    pub fn create(&self, dirs: &DataDir, view_distance: u32, graphics: Graphics) -> io::Result<()> {
        let dir = self.dir(dirs)?;
        if dir.join("instance.toml").exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("an instance named {:?} already exists", self.name),
            ));
        }
        std::fs::create_dir_all(dir.join("mods"))?;
        let options = dir.join("options.txt");
        if !options.exists() {
            // Both keys are written: 1.8–1.15 read `fancyGraphics`, 1.16+
            // read `graphicsMode`; each version ignores the other.
            let fancy = graphics == Graphics::Fancy;
            let text =
                format!("renderDistance:{view_distance}\nfancyGraphics:{fancy}\ngraphicsMode:{}\n", u8::from(fancy));
            write_atomic(&options, text.as_bytes())?;
        }
        self.save(dirs)
    }

    pub fn world_count(&self, dirs: &DataDir) -> usize {
        self.dir(dirs)
            .and_then(|d| std::fs::read_dir(d.join("saves")))
            .map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count())
            .unwrap_or(0)
    }
}

/// Every instance with a readable `instance.toml`, sorted by name.
pub fn load_instances(dirs: &DataDir) -> Vec<Instance> {
    let Ok(entries) = std::fs::read_dir(dirs.instances()) else {
        return Vec::new();
    };
    let mut out: Vec<Instance> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let text = std::fs::read_to_string(e.path().join("instance.toml")).ok()?;
            match toml::from_str::<Instance>(&text) {
                Ok(mut i) => {
                    i.name = name;
                    Some(i)
                }
                Err(err) => {
                    tracing::warn!("skipping instance {name}: {err}");
                    None
                }
            }
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Checks a user-entered instance name.
pub fn validate_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Enter a name.");
    }
    if trimmed.len() > 48 {
        return Err("Use at most 48 characters.");
    }
    if trimmed != name || name.starts_with('.') || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("Use letters, numbers, spaces, - or _ (no leading/trailing spaces or dots).");
    }
    Ok(())
}

/// Deletes an instance and everything in it, including its worlds. The UI
/// asks for confirmation (naming the world count) before calling this.
pub fn delete_instance(dirs: &DataDir, name: &str) -> io::Result<()> {
    std::fs::remove_dir_all(dirs.instance_dir(name)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_roundtrip_preserves_existing_options() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = DataDir::at(tmp.path());
        let mut inst = Instance::new("PvP 1.8", "1.8.9", Loader::Vanilla);
        inst.create(&dirs, 10, Graphics::Fancy).unwrap();
        let options = tmp.path().join("instances/PvP 1.8/options.txt");
        assert!(std::fs::read_to_string(&options).unwrap().contains("renderDistance:10"));
        assert!(inst.create(&dirs, 10, Graphics::Fancy).is_err(), "duplicate name accepted");

        inst.max_memory_mb = 4096;
        inst.loader = Loader::Fabric { version: "0.16.10".into() };
        inst.save(&dirs).unwrap();
        let loaded = load_instances(&dirs);
        assert_eq!(loaded, [inst.clone()]);

        // A second instance in a dir that already has options keeps them.
        std::fs::create_dir_all(tmp.path().join("instances/Old")).unwrap();
        std::fs::write(tmp.path().join("instances/Old/options.txt"), "renderDistance:2\n").unwrap();
        Instance::new("Old", "1.12.2", Loader::Vanilla).create(&dirs, 16, Graphics::Fast).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("instances/Old/options.txt")).unwrap(),
            "renderDistance:2\n"
        );

        // Unreadable instance files are skipped, not fatal.
        std::fs::create_dir_all(tmp.path().join("instances/Broken")).unwrap();
        std::fs::write(tmp.path().join("instances/Broken/instance.toml"), "not = [valid").unwrap();
        assert_eq!(load_instances(&dirs).len(), 2);
    }

    #[test]
    fn names_are_validated() {
        assert!(validate_name("My World 1.8").is_ok());
        for bad in ["", " x", "x ", "../x", "a/b", ".hidden", "a:b", &"x".repeat(49)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }
}
