//! Client options, kept in `<game dir>/rustcraft-options.toml`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use rustcraft_core::fs::{load_toml_or_default, save_toml};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub render_distance: u32,
    pub fov: f32,
    pub sensitivity: f32,
    pub vsync: bool,
    pub max_fps: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self { render_distance: 8, fov: 70.0, sensitivity: 1.0, vsync: true, max_fps: 240 }
    }
}

const FILE: &str = "rustcraft-options.toml";

impl Settings {
    pub fn load(game_dir: &Path) -> Settings {
        let (mut s, _) = load_toml_or_default::<Settings>(&game_dir.join(FILE));
        s.render_distance = s.render_distance.clamp(2, 32);
        s.fov = s.fov.clamp(30.0, 120.0);
        s.max_fps = s.max_fps.clamp(10, 1000);
        s
    }

    pub fn save(&self, game_dir: &Path) {
        if let Err(e) = save_toml(&game_dir.join(FILE), self) {
            tracing::warn!("could not save options: {e}");
        }
    }
}
