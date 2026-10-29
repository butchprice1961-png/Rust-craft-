//! The official version list (`version_manifest_v2.json`).

use serde::Deserialize;

pub const VERSION_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

/// Oldest release RustCraft manages. Older versions use asset and launch
/// formats the launcher does not implement.
pub const OLDEST_SUPPORTED: &str = "1.8";

#[derive(Debug, Clone, Deserialize)]
pub struct VersionManifest {
    pub latest: Latest,
    /// Newest first, as published.
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Latest {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: VersionKind,
    pub url: String,
    pub release_time: String,
    pub sha1: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Snapshot,
    OldBeta,
    OldAlpha,
    #[serde(other)]
    Other,
}

impl VersionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::Snapshot => "Snapshot",
            Self::OldBeta => "Beta",
            Self::OldAlpha => "Alpha",
            Self::Other => "Other",
        }
    }
}

impl VersionManifest {
    pub fn get(&self, id: &str) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// Versions from [`OLDEST_SUPPORTED`] onward (newest first). Uses the
    /// manifest's own ordering, so snapshots that preceded 1.8 are excluded.
    pub fn supported(&self) -> &[VersionEntry] {
        let end = self
            .versions
            .iter()
            .position(|v| v.id == OLDEST_SUPPORTED)
            .map_or(self.versions.len(), |i| i + 1);
        &self.versions[..end]
    }

    pub fn is_supported(&self, id: &str) -> bool {
        self.supported().iter().any(|v| v.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_window_uses_manifest_order() {
        let m: VersionManifest =
            serde_json::from_str(include_str!("../tests/fixtures/version_manifest_v2.json"))
                .unwrap();
        assert_eq!(m.latest.release, "1.21.4");
        let ids: Vec<_> = m.supported().iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["25w03a", "1.21.4", "1.12.2", "1.8.9", "1.8"]);
        assert!(!m.is_supported("14w02a"));
        assert!(!m.is_supported("1.7.10"));
        assert_eq!(m.get("25w03a").unwrap().kind, VersionKind::Snapshot);
        assert_eq!(m.get("experimental-x").unwrap().kind, VersionKind::Other);
    }
}
