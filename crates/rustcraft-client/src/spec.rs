//! How the client was asked to start: command-line flags for development,
//! or a JSON launch spec on stdin from the launcher (so the access token
//! never appears in the process list).

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LaunchSpec {
    /// Instance directory: `saves/`, `resourcepacks/`, `screenshots/`, `crash-reports/`.
    pub game_dir: PathBuf,
    /// Resource packs in priority order (directories or zips), e.g. the
    /// user's own 1.8.9 client jar last.
    pub resource_packs: Vec<PathBuf>,
    /// Local world folder name under `saves/`.
    pub world: String,
    /// Seed for a new local world.
    pub seed: Option<i64>,
    /// `host[:port]` to join instead of playing locally.
    pub server: Option<String>,
    pub player_name: String,
    /// Undashed UUID of the signed-in profile; empty when not signed in.
    pub player_uuid: String,
    /// Minecraft services token, for online-mode servers. Only the launcher
    /// passes it, on standard input.
    pub access_token: Option<Secret>,
    pub render_distance: u32,
    /// Automation: save a screenshot once the world has loaded, then exit.
    pub screenshot: Option<PathBuf>,
    /// Automation: fly a fixed route for this many seconds, report frame
    /// timings as JSON, then exit.
    pub benchmark_seconds: Option<u32>,
}

impl Default for LaunchSpec {
    fn default() -> Self {
        Self {
            game_dir: PathBuf::from("."),
            resource_packs: Vec::new(),
            world: "New World".into(),
            seed: None,
            server: None,
            player_name: "Player".into(),
            player_uuid: String::new(),
            access_token: None,
            render_distance: 8,
            screenshot: None,
            benchmark_seconds: None,
        }
    }
}

pub const USAGE: &str = "\
rustcraft-client [options]
  --game-dir <dir>         instance directory (default: .)
  --world <name>           local world under <game-dir>/saves (default: New World)
  --seed <n>               seed when creating a new local world
  --server <host[:port]>   join a server (offline-mode servers only from the command line)
  --name <player>          player name (default: Player)
  --pack <path>            resource pack directory or zip; repeat for more
  --render-distance <n>    chunks (2-32, default 8)
  --screenshot <png>       save a screenshot after loading, then exit
  --benchmark <seconds>    fly a fixed route, print frame timings, then exit
  --spec-stdin             read a JSON launch spec from standard input
";

/// A token that never appears in logs or debug output.
#[derive(Clone, Deserialize)]
#[serde(transparent)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl LaunchSpec {
    pub fn from_args(args: &[String]) -> Result<LaunchSpec, String> {
        let mut spec = LaunchSpec::default();
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let mut value = || it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
            match flag.as_str() {
                "--game-dir" => spec.game_dir = value()?.into(),
                "--world" => spec.world = value()?,
                "--seed" => spec.seed = Some(value()?.parse().map_err(|_| "--seed needs a number")?),
                "--server" => spec.server = Some(value()?),
                "--name" => spec.player_name = value()?,
                "--pack" => spec.resource_packs.push(value()?.into()),
                "--render-distance" => {
                    spec.render_distance = value()?.parse().map_err(|_| "--render-distance needs a number")?
                }
                "--screenshot" => spec.screenshot = Some(value()?.into()),
                "--benchmark" => {
                    spec.benchmark_seconds = Some(value()?.parse().map_err(|_| "--benchmark needs seconds")?)
                }
                "--spec-stdin" => {
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text).map_err(|e| e.to_string())?;
                    spec = serde_json::from_str(&text).map_err(|e| format!("bad launch spec: {e}"))?;
                }
                other => return Err(format!("unknown option {other}\n\n{USAGE}")),
            }
        }
        spec.render_distance = spec.render_distance.clamp(2, 32);
        if spec.world.is_empty() || spec.world.contains(['/', '\\', ':']) || spec.world.starts_with('.') {
            return Err(format!("invalid world name {:?}", spec.world));
        }
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_flags_and_validates() {
        let s =
            LaunchSpec::from_args(&args("--world Test --seed -5 --render-distance 99 --pack a.zip --pack b")).unwrap();
        assert_eq!((s.world.as_str(), s.seed, s.render_distance), ("Test", Some(-5), 32));
        assert_eq!(s.resource_packs.len(), 2);
        assert!(LaunchSpec::from_args(&args("--world ../x")).is_err());
        assert!(LaunchSpec::from_args(&args("--seed")).is_err());
        assert!(LaunchSpec::from_args(&args("--bogus")).is_err());
        let json: LaunchSpec =
            serde_json::from_str(r#"{"world":"W","server":"localhost:25565","access_token":"T0K3N"}"#).unwrap();
        assert_eq!(json.access_token.as_ref().unwrap().0, "T0K3N");
        assert!(!format!("{json:?}").contains("T0K3N"));
        assert_eq!(json.render_distance, 8);
        assert_eq!(json.server.as_deref(), Some("localhost:25565"));
    }
}
