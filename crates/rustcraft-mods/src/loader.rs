//! Fabric Loader installation via Fabric's metadata service, which publishes
//! a ready-made version profile that inherits from the vanilla version.
//!
//! Forge and NeoForge are not installed: they require running their own
//! installer (which patches the game jar) and are reported as unsupported.

use serde::Deserialize;

use rustcraft_assets::VersionJson;
use rustcraft_core::DataDir;
use rustcraft_core::fs::write_atomic;

use crate::ModsError;

pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FabricLoader {
    pub version: String,
    pub stable: bool,
}

#[derive(Deserialize)]
struct LoaderEntry {
    loader: FabricLoader,
}

fn segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Loader versions that support `game`, newest first.
pub async fn fabric_loaders(http: &reqwest::Client, meta: &str, game: &str) -> Result<Vec<FabricLoader>, ModsError> {
    let url = format!("{meta}/versions/loader/{}", segment(game));
    let entries: Vec<LoaderEntry> = http.get(url).send().await?.error_for_status()?.json().await?;
    Ok(entries.into_iter().map(|e| e.loader).collect())
}

/// Writes the Fabric profile for `game` + `loader` into `versions/` and
/// returns the profile's version id.
pub async fn install_fabric(
    http: &reqwest::Client,
    meta: &str,
    dirs: &DataDir,
    game: &str,
    loader: &str,
) -> Result<String, ModsError> {
    let url = format!("{meta}/versions/loader/{}/{}/profile/json", segment(game), segment(loader));
    let bytes = http.get(url).send().await?.error_for_status()?.bytes().await?;
    let profile: VersionJson =
        serde_json::from_slice(&bytes).map_err(|e| ModsError::Invalid(format!("bad Fabric profile: {e}")))?;
    if profile.inherits_from.as_deref() != Some(game) {
        return Err(ModsError::Invalid(format!(
            "Fabric profile {} targets {:?}, expected {game}",
            profile.id, profile.inherits_from
        )));
    }
    let dir = dirs.version_dir(&profile.id)?;
    write_atomic(&dir.join(format!("{}.json", profile.id)), &bytes)?;
    Ok(profile.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn installs_profile_and_rejects_mismatch() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let server = MockServer::start().await;
        Mock::given(path("/versions/loader/1.21.4"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "loader": { "separator": ".", "build": 10, "maven": "net.fabricmc:fabric-loader:0.16.10", "version": "0.16.10", "stable": true },
                  "intermediary": { "version": "1.21.4", "stable": true } }
            ])))
            .mount(&server)
            .await;
        Mock::given(path("/versions/loader/1.21.4/0.16.10/profile/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "fabric-loader-0.16.10-1.21.4", "inheritsFrom": "1.21.4", "type": "release",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "arguments": { "game": [], "jvm": [] },
                "libraries": [ { "name": "net.fabricmc:fabric-loader:0.16.10", "url": "https://maven.fabricmc.net/" } ]
            })))
            .mount(&server)
            .await;
        Mock::given(path("/versions/loader/1.8.9/0.16.10/profile/json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "../../evil", "inheritsFrom": "1.21.4", "libraries": [] })))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let loaders = fabric_loaders(&http, &server.uri(), "1.21.4").await.unwrap();
        assert_eq!(loaders, [FabricLoader { version: "0.16.10".into(), stable: true }]);

        let dir = tempfile::tempdir().unwrap();
        let dirs = DataDir::at(dir.path());
        let id = install_fabric(&http, &server.uri(), &dirs, "1.21.4", "0.16.10").await.unwrap();
        assert_eq!(id, "fabric-loader-0.16.10-1.21.4");
        assert!(dir.path().join("versions/fabric-loader-0.16.10-1.21.4/fabric-loader-0.16.10-1.21.4.json").exists());

        let err = install_fabric(&http, &server.uri(), &dirs, "1.8.9", "0.16.10").await.unwrap_err();
        assert!(err.to_string().contains("expected 1.8.9"), "{err}");
    }
}
