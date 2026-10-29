//! Installing a version's files and building the Java command that runs it.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use rustcraft_core::DataDir;
use rustcraft_core::fs::{safe_join, write_atomic};

use crate::AssetsError;
use crate::download::{Downloader, FileDownload, Progress};
use crate::manifest::{VERSION_MANIFEST_URL, VersionManifest};
use crate::version::{
    Argument, NativeJar, OneOrMany, Platform, VersionJson, resolve_libraries, rules_allow,
};

pub const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

/// `assets/indexes/<id>.json`
#[derive(Debug, Clone, Deserialize)]
pub struct AssetIndex {
    pub objects: BTreeMap<String, AssetObject>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

impl AssetObject {
    /// `<first two hex digits>/<hash>`, the content-addressed object path.
    pub fn rel_path(&self) -> Result<String, AssetsError> {
        if self.hash.len() != 40 || !self.hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(AssetsError::Metadata(format!(
                "bad asset hash {:?}",
                self.hash
            )));
        }
        Ok(format!("{}/{}", &self.hash[..2], self.hash))
    }
}

impl AssetIndex {
    pub fn downloads(
        &self,
        objects_dir: &Path,
        base_url: &str,
    ) -> Result<Vec<FileDownload>, AssetsError> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for object in self.objects.values() {
            if !seen.insert(object.hash.as_str()) {
                continue;
            }
            let rel = object.rel_path()?;
            out.push(FileDownload {
                url: format!("{base_url}/{rel}"),
                path: objects_dir.join(&rel),
                sha1: Some(object.hash.clone()),
                size: Some(object.size),
            });
        }
        Ok(out)
    }
}

/// Remote endpoints, overridable for tests and mirrors.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub manifest: String,
    pub resources: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            manifest: VERSION_MANIFEST_URL.into(),
            resources: RESOURCES_URL.into(),
        }
    }
}

/// A version whose files are all present and verified.
#[derive(Debug, Clone)]
pub struct Installed {
    pub version: VersionJson,
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub logging_config: Option<PathBuf>,
    pub assets_index: String,
}

#[derive(Debug, Clone)]
pub struct Installer {
    pub dirs: DataDir,
    pub downloader: Downloader,
    pub platform: Platform,
    pub endpoints: Endpoints,
}

impl Installer {
    pub fn new(dirs: DataDir, downloader: Downloader) -> Self {
        Self {
            dirs,
            downloader,
            platform: Platform::current(),
            endpoints: Endpoints::default(),
        }
    }

    /// Fresh manifest when online, last cached copy otherwise.
    pub async fn manifest(&self) -> Result<VersionManifest, AssetsError> {
        let cache = self.dirs.cache().join("version_manifest_v2.json");
        self.json(&self.endpoints.manifest, &cache, None).await
    }

    /// Loads `id` and every `inheritsFrom` ancestor, merged into one.
    /// Versions missing from the manifest (loader profiles) must already be
    /// installed locally.
    pub async fn resolve(
        &self,
        id: &str,
        manifest: Option<&VersionManifest>,
    ) -> Result<VersionJson, AssetsError> {
        let mut chain = vec![self.load_version(id, manifest).await?];
        while let Some(parent) = chain.last().and_then(|v| v.inherits_from.clone()) {
            if chain.len() >= 8 {
                return Err(AssetsError::Metadata(format!(
                    "inheritsFrom chain of {id} is too deep"
                )));
            }
            chain.push(self.load_version(&parent, manifest).await?);
        }
        let mut merged = chain.pop().expect("chain starts non-empty");
        while let Some(child) = chain.pop() {
            merged = child.merged_onto(merged);
        }
        Ok(merged)
    }

    async fn load_version(
        &self,
        id: &str,
        manifest: Option<&VersionManifest>,
    ) -> Result<VersionJson, AssetsError> {
        let path = self.dirs.version_dir(id)?.join(format!("{id}.json"));
        match manifest.and_then(|m| m.get(id)) {
            Some(entry) => self.json(&entry.url, &path, entry.sha1.as_deref()).await,
            None => {
                let bytes = std::fs::read(&path).map_err(|e| {
                    AssetsError::Metadata(format!(
                        "version {id} is not installed ({}: {e})",
                        path.display()
                    ))
                })?;
                Ok(serde_json::from_slice(&bytes)?)
            }
        }
    }

    /// Fetches a JSON document. With a known hash, a matching cached copy is
    /// used without touching the network; without one, the network is tried
    /// first and the cache is the offline fallback.
    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        url: &str,
        cache: &Path,
        sha1: Option<&str>,
    ) -> Result<T, AssetsError> {
        if let Some(sha1) = sha1 {
            let file = FileDownload {
                url: url.into(),
                path: cache.into(),
                sha1: Some(sha1.into()),
                size: None,
            };
            self.downloader
                .fetch_one(&file, &Progress::default(), &CancellationToken::new())
                .await?;
            return Ok(serde_json::from_slice(&tokio::fs::read(cache).await?)?);
        }
        match self.downloader.get_bytes(url).await {
            Ok(bytes) => {
                let value = serde_json::from_slice(&bytes)?;
                write_atomic(cache, &bytes)?;
                Ok(value)
            }
            Err(e) => match tokio::fs::read(cache).await {
                Ok(bytes) => {
                    tracing::warn!("using cached {} because {e}", cache.display());
                    Ok(serde_json::from_slice(&bytes)?)
                }
                Err(_) => Err(e.into()),
            },
        }
    }

    /// Downloads and verifies everything `version` needs, then extracts its
    /// legacy natives.
    pub async fn install(
        &self,
        version: &VersionJson,
        progress: &Arc<Progress>,
        cancel: &CancellationToken,
    ) -> Result<Installed, AssetsError> {
        let libs = resolve_libraries(&version.libraries, self.platform, &self.dirs.libraries())?;
        let mut files = libs.downloads;

        let jar_id = version.jar_id();
        let jar = self.dirs.version_dir(jar_id)?.join(format!("{jar_id}.jar"));
        let client = version.downloads.get("client");
        files.push(FileDownload {
            url: client.map(|a| a.url.clone()).unwrap_or_default(),
            path: jar.clone(),
            sha1: client.and_then(|a| a.sha1.clone()),
            size: client.and_then(|a| a.size),
        });

        let mut logging_config = None;
        if let Some(cfg) = version.logging.as_ref().and_then(|l| l.client.as_ref()) {
            let path = safe_join(&self.dirs.assets().join("log_configs"), &cfg.file.id)?;
            files.push(FileDownload {
                url: cfg.file.url.clone(),
                path: path.clone(),
                sha1: Some(cfg.file.sha1.clone()),
                size: Some(cfg.file.size),
            });
            logging_config = Some(path);
        }

        let index_ref = version
            .asset_index
            .as_ref()
            .ok_or_else(|| AssetsError::Metadata(format!("{} has no assetIndex", version.id)))?;
        let index_path = safe_join(
            &self.dirs.assets().join("indexes"),
            &format!("{}.json", index_ref.id),
        )?;
        let index: AssetIndex = self
            .json(&index_ref.url, &index_path, Some(&index_ref.sha1))
            .await?;
        files.extend(index.downloads(
            &self.dirs.assets().join("objects"),
            &self.endpoints.resources,
        )?);

        self.downloader.fetch_all(files, progress, cancel).await?;

        let natives_dir = safe_join(&self.dirs.natives(), &version.id)?;
        let (natives, dir) = (libs.natives, natives_dir.clone());
        tokio::task::spawn_blocking(move || extract_natives(&natives, &dir))
            .await
            .expect("natives extraction panicked")?;

        let mut classpath = libs.classpath;
        classpath.push(jar);
        Ok(Installed {
            version: version.clone(),
            classpath,
            natives_dir,
            logging_config,
            assets_index: index_ref.id.clone(),
        })
    }
}

/// Unpacks native libraries, skipping excluded prefixes and any entry whose
/// path would escape `dest`.
pub fn extract_natives(natives: &[NativeJar], dest: &Path) -> Result<(), AssetsError> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::create_dir_all(dest)?;
    for jar in natives {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&jar.path)?)?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = String::from_utf8_lossy(entry.name_raw()).into_owned();
            if entry.is_dir() || jar.exclude.iter().any(|e| name.starts_with(e.as_str())) {
                continue;
            }
            let Some(rel) = entry.enclosed_name() else {
                tracing::warn!("skipping unsafe entry {name:?} in {}", jar.path.display());
                continue;
            };
            let out = dest.join(rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            io::copy(&mut entry, &mut std::fs::File::create(out)?)?;
        }
    }
    Ok(())
}

/// Who is playing. `access_token` is secret and is redacted from logs.
#[derive(Clone)]
pub struct LaunchAuth {
    pub player_name: String,
    /// Undashed hex UUID.
    pub uuid: String,
    pub access_token: String,
    /// `"msa"` for Microsoft accounts.
    pub user_type: String,
    pub xuid: String,
    pub client_id: String,
}

impl std::fmt::Debug for LaunchAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LaunchAuth")
            .field("player_name", &self.player_name)
            .field("uuid", &self.uuid)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub java: PathBuf,
    pub game_dir: PathBuf,
    pub assets_dir: PathBuf,
    pub libraries_dir: PathBuf,
    pub auth: LaunchAuth,
    pub min_memory_mb: u32,
    pub max_memory_mb: u32,
    pub extra_jvm_args: Vec<String>,
    pub resolution: Option<(u32, u32)>,
    /// `host[:port]` to join directly.
    pub server: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    secret: String,
}

impl LaunchCommand {
    pub fn to_command(&self) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.program);
        cmd.args(&self.args).current_dir(&self.cwd);
        cmd
    }

    /// Arguments safe to log or show.
    pub fn redacted_args(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| {
                if self.secret.is_empty() {
                    a.clone()
                } else {
                    a.replace(&self.secret, "<redacted>")
                }
            })
            .collect()
    }
}

pub fn build_command(
    installed: &Installed,
    opts: &LaunchOptions,
    platform: Platform,
) -> Result<LaunchCommand, AssetsError> {
    let v = &installed.version;
    let main_class = v
        .main_class
        .as_deref()
        .ok_or_else(|| AssetsError::Metadata(format!("{} has no mainClass", v.id)))?;

    let classpath = installed
        .classpath
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join(platform.classpath_separator());
    let (host, port) = match opts
        .server
        .as_deref()
        .map(|s| s.rsplit_once(':').unwrap_or((s, "25565")))
    {
        Some((h, p)) => (h.to_string(), p.to_string()),
        None => Default::default(),
    };
    let quick_play = opts.server.is_some()
        && v.arguments.iter().flat_map(|a| &a.game).any(|a| match a {
            Argument::Conditional {
                value: OneOrMany::Many(vals),
                ..
            } => vals.iter().any(|s| s.contains("quickPlayMultiplayer")),
            _ => false,
        });

    let mut features = Vec::new();
    if opts.resolution.is_some() {
        features.push("has_custom_resolution");
    }
    if quick_play {
        features.push("is_quick_play_multiplayer");
    }

    let (width, height) = opts.resolution.unwrap_or((854, 480));
    let vars: BTreeMap<&str, String> = [
        ("auth_player_name", opts.auth.player_name.clone()),
        ("version_name", v.id.clone()),
        (
            "game_directory",
            opts.game_dir.to_string_lossy().into_owned(),
        ),
        (
            "assets_root",
            opts.assets_dir.to_string_lossy().into_owned(),
        ),
        (
            "game_assets",
            opts.assets_dir.to_string_lossy().into_owned(),
        ),
        ("assets_index_name", installed.assets_index.clone()),
        ("auth_uuid", opts.auth.uuid.clone()),
        ("auth_access_token", opts.auth.access_token.clone()),
        ("auth_session", opts.auth.access_token.clone()),
        ("auth_xuid", opts.auth.xuid.clone()),
        ("clientid", opts.auth.client_id.clone()),
        ("user_type", opts.auth.user_type.clone()),
        ("user_properties", "{}".into()),
        (
            "version_type",
            v.kind.map_or("release", |k| k.label()).to_lowercase(),
        ),
        (
            "natives_directory",
            installed.natives_dir.to_string_lossy().into_owned(),
        ),
        ("launcher_name", "RustCraft".into()),
        ("launcher_version", env!("CARGO_PKG_VERSION").into()),
        ("classpath", classpath),
        ("classpath_separator", platform.classpath_separator().into()),
        (
            "library_directory",
            opts.libraries_dir.to_string_lossy().into_owned(),
        ),
        ("resolution_width", width.to_string()),
        ("resolution_height", height.to_string()),
        ("quickPlayMultiplayer", format!("{host}:{port}")),
    ]
    .into_iter()
    .collect();

    let expand = |args: &[Argument]| -> Vec<String> {
        let mut out = Vec::new();
        for arg in args {
            match arg {
                Argument::Plain(s) => out.push(substitute(s, &vars)),
                Argument::Conditional { rules, value }
                    if rules_allow(rules, platform, &features) =>
                {
                    match value {
                        OneOrMany::One(s) => out.push(substitute(s, &vars)),
                        OneOrMany::Many(list) => {
                            out.extend(list.iter().map(|s| substitute(s, &vars)))
                        }
                    }
                }
                Argument::Conditional { .. } => {}
            }
        }
        out
    };

    let mut args = match &v.arguments {
        Some(a) if !a.jvm.is_empty() => expand(&a.jvm),
        _ => vec![
            substitute("-Djava.library.path=${natives_directory}", &vars),
            "-cp".into(),
            vars["classpath"].clone(),
        ],
    };
    if let (Some(cfg), Some(path)) = (
        v.logging.as_ref().and_then(|l| l.client.as_ref()),
        &installed.logging_config,
    ) {
        args.push(cfg.argument.replace("${path}", &path.to_string_lossy()));
    }
    args.push(format!(
        "-Xms{}M",
        opts.min_memory_mb.min(opts.max_memory_mb)
    ));
    args.push(format!("-Xmx{}M", opts.max_memory_mb));
    args.extend(opts.extra_jvm_args.iter().cloned());
    args.push(main_class.into());

    match &v.minecraft_arguments {
        Some(legacy) if v.arguments.as_ref().is_none_or(|a| a.game.is_empty()) => {
            args.extend(legacy.split_whitespace().map(|s| substitute(s, &vars)));
            if opts.resolution.is_some() {
                args.extend(["--width".into(), vars["resolution_width"].clone()]);
                args.extend(["--height".into(), vars["resolution_height"].clone()]);
            }
        }
        _ => args.extend(expand(
            &v.arguments
                .as_ref()
                .map(|a| a.game.clone())
                .unwrap_or_default(),
        )),
    }
    if opts.server.is_some() && !quick_play {
        args.extend(["--server".into(), host, "--port".into(), port]);
    }

    Ok(LaunchCommand {
        program: opts.java.clone(),
        args,
        cwd: opts.game_dir.clone(),
        secret: opts.auth.access_token.clone(),
    })
}

/// Replaces `${name}` placeholders; unknown names are left as written.
fn substitute(template: &str, vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let value = after
            .find('}')
            .and_then(|end| Some((end, vars.get(&after[..end])?)));
        match value {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Os;

    const LINUX: Platform = Platform {
        os: Os::Linux,
        arch: "x86_64",
    };

    fn installed(fixture: &str) -> Installed {
        let version: VersionJson = serde_json::from_str(fixture).unwrap();
        Installed {
            assets_index: version.asset_index.as_ref().unwrap().id.clone(),
            version,
            classpath: vec!["/l/a.jar".into(), "/v/client.jar".into()],
            natives_dir: "/n".into(),
            logging_config: Some("/a/log_configs/client-1.12.xml".into()),
        }
    }

    fn opts(server: Option<&str>) -> LaunchOptions {
        LaunchOptions {
            java: "/java/bin/java".into(),
            game_dir: "/game".into(),
            assets_dir: "/assets".into(),
            libraries_dir: "/l".into(),
            auth: LaunchAuth {
                player_name: "Steve".into(),
                uuid: "0123456789abcdef0123456789abcdef".into(),
                access_token: "SECRET-TOKEN".into(),
                user_type: "msa".into(),
                xuid: "2535".into(),
                client_id: "cid".into(),
            },
            min_memory_mb: 512,
            max_memory_mb: 2048,
            extra_jvm_args: vec!["-XX:+UseG1GC".into()],
            resolution: None,
            server: server.map(Into::into),
        }
    }

    fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
        let i = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} missing in {args:?}"));
        &args[i + 1]
    }

    #[test]
    fn legacy_command_line() {
        let cmd = build_command(
            &installed(include_str!("../tests/fixtures/1.8.9.json")),
            &opts(Some("mc.example.org")),
            LINUX,
        )
        .unwrap();
        let a = &cmd.args;
        assert_eq!(a[0], "-Djava.library.path=/n");
        assert_eq!(after(a, "-cp"), "/l/a.jar:/v/client.jar");
        let main = a
            .iter()
            .position(|s| s == "net.minecraft.client.main.Main")
            .unwrap();
        assert!(a[..main].contains(&"-Xmx2048M".to_string()));
        assert!(a[..main].contains(&"-XX:+UseG1GC".to_string()));
        assert_eq!(after(a, "--username"), "Steve");
        assert_eq!(after(a, "--assetIndex"), "1.8");
        assert_eq!(after(a, "--userProperties"), "{}");
        assert_eq!(after(a, "--server"), "mc.example.org");
        assert_eq!(after(a, "--port"), "25565");
        assert!(!a.iter().any(|s| s.contains("${")), "{a:?}");
        // 1.8.9 metadata has no logging section, so no log4j argument.
        assert!(!a.iter().any(|s| s.starts_with("-Dlog4j")));
    }

    #[test]
    fn modern_command_line_uses_rules_and_quick_play() {
        let inst = installed(include_str!("../tests/fixtures/1.21.4.json"));
        let mut o = opts(Some("play.example.org:25570"));
        o.resolution = Some((1280, 720));
        let a = build_command(&inst, &o, LINUX).unwrap().args;
        assert!(a.contains(&"-Dminecraft.launcher.brand=RustCraft".to_string()));
        assert!(
            a.contains(&"-Dlog4j.configurationFile=/a/log_configs/client-1.12.xml".to_string())
        );
        assert!(!a.contains(&"-XstartOnFirstThread".to_string()));
        assert!(!a.contains(&"--demo".to_string()));
        assert_eq!(
            after(&a, "--quickPlayMultiplayer"),
            "play.example.org:25570"
        );
        assert!(!a.contains(&"--server".to_string()));
        assert_eq!(after(&a, "--width"), "1280");
        assert_eq!(after(&a, "--versionType"), "release");
        assert!(!a.contains(&"--quickPlayPath".to_string()));

        let mac = build_command(
            &inst,
            &opts(None),
            Platform {
                os: Os::MacOs,
                arch: "arm64",
            },
        )
        .unwrap()
        .args;
        assert_eq!(mac[0], "-XstartOnFirstThread");
        assert!(!mac.contains(&"--quickPlayMultiplayer".to_string()));
    }

    #[test]
    fn token_is_redacted_for_logs() {
        let cmd = build_command(
            &installed(include_str!("../tests/fixtures/1.8.9.json")),
            &opts(None),
            LINUX,
        )
        .unwrap();
        assert!(cmd.args.iter().any(|a| a == "SECRET-TOKEN"));
        assert!(
            !cmd.redacted_args()
                .iter()
                .any(|a| a.contains("SECRET-TOKEN"))
        );
        assert!(!format!("{:?}", opts(None).auth).contains("SECRET"));
    }

    #[test]
    fn substitution_leaves_unknown_placeholders() {
        let vars: BTreeMap<&str, String> = [("a", "1".to_string())].into_iter().collect();
        assert_eq!(substitute("x${a}y${b}z${", &vars), "x1y${b}z${");
    }
}
