//! Per-version launch metadata (`versions/<id>/<id>.json`), including loader
//! profiles (Fabric, Quilt, legacy Forge) that extend a vanilla version via
//! `inheritsFrom`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::AssetsError;
use crate::download::FileDownload;
use crate::manifest::VersionKind;

pub const LIBRARIES_URL: &str = "https://libraries.minecraft.net/";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    pub inherits_from: Option<String>,
    /// Id of the version whose client jar is used (loader profiles reuse
    /// their parent's jar).
    pub jar: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<VersionKind>,
    pub main_class: Option<String>,
    /// Pre-1.13 space-separated game arguments.
    pub minecraft_arguments: Option<String>,
    /// 1.13+ structured arguments.
    pub arguments: Option<Arguments>,
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default)]
    pub downloads: BTreeMap<String, Artifact>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    pub java_version: Option<JavaVersion>,
    pub logging: Option<Logging>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional { rules: Vec<Rule>, value: OneOrMany },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub action: RuleAction,
    pub os: Option<OsRule>,
    pub features: Option<BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsRule {
    pub name: Option<String>,
    pub version: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub path: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Logging {
    pub client: Option<LoggingConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    /// Contains `${path}`, replaced by the downloaded config file.
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Legacy native classifier per OS, e.g. `"windows": "natives-windows-${arch}"`.
    pub natives: Option<BTreeMap<String, String>>,
    pub extract: Option<Extract>,
    /// Maven repository base for loader libraries that omit `downloads`.
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
    #[serde(default)]
    pub classifiers: BTreeMap<String, Artifact>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

/// The machine a version is being resolved for, in the vocabulary the
/// metadata rules use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    /// `"x86"`, `"x86_64"` or `"arm64"`.
    pub arch: &'static str,
}

impl Platform {
    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::MacOs,
            _ => Os::Linux,
        };
        let arch = match std::env::consts::ARCH {
            "x86" => "x86",
            "aarch64" => "arm64",
            _ => "x86_64",
        };
        Self { os, arch }
    }

    pub fn os_name(self) -> &'static str {
        match self.os {
            Os::Windows => "windows",
            Os::MacOs => "osx",
            Os::Linux => "linux",
        }
    }

    fn pointer_bits(self) -> &'static str {
        if self.arch == "x86" { "32" } else { "64" }
    }

    pub fn classpath_separator(self) -> &'static str {
        if self.os == Os::Windows { ";" } else { ":" }
    }
}

impl Rule {
    fn matches(&self, platform: Platform, features: &[&str]) -> bool {
        if let Some(os) = &self.os {
            if os.name.as_deref().is_some_and(|n| n != platform.os_name())
                || os.arch.as_deref().is_some_and(|a| a != platform.arch)
            {
                return false;
            }
            // ponytail: OS-version regexes only appear in workarounds for
            // macOS 10.5 and old Java on Windows 10; treated as non-matching.
            // Add a regex + OS version probe if a needed library depends on one.
            if os.version.is_some() {
                return false;
            }
        }
        self.features.iter().flatten().all(|(name, want)| features.contains(&name.as_str()) == *want)
    }
}

/// Mojang semantics: no rules means allowed; otherwise start disallowed and
/// let the last matching rule decide.
pub fn rules_allow(rules: &[Rule], platform: Platform, features: &[&str]) -> bool {
    if rules.is_empty() {
        return true;
    }
    rules.iter().rfind(|r| r.matches(platform, features)).is_some_and(|r| r.action == RuleAction::Allow)
}

impl VersionJson {
    /// Applies a loader profile (`self`, which has `inheritsFrom`) on top of
    /// its parent. Profile libraries come first so they win classpath order
    /// and de-duplication.
    pub fn merged_onto(self, parent: VersionJson) -> VersionJson {
        let mut libraries = self.libraries;
        libraries.extend(parent.libraries);
        let arguments = match (parent.arguments, self.arguments) {
            (Some(mut p), Some(c)) => {
                p.game.extend(c.game);
                p.jvm.extend(c.jvm);
                Some(p)
            }
            (p, c) => c.or(p),
        };
        VersionJson {
            id: self.id,
            inherits_from: parent.inherits_from,
            jar: self.jar.or(parent.jar).or(Some(parent.id)),
            kind: self.kind.or(parent.kind),
            main_class: self.main_class.or(parent.main_class),
            minecraft_arguments: self.minecraft_arguments.or(parent.minecraft_arguments),
            arguments,
            asset_index: self.asset_index.or(parent.asset_index),
            downloads: if self.downloads.is_empty() { parent.downloads } else { self.downloads },
            libraries,
            java_version: self.java_version.or(parent.java_version),
            logging: self.logging.or(parent.logging),
        }
    }

    pub fn jar_id(&self) -> &str {
        self.jar.as_deref().unwrap_or(&self.id)
    }

    /// Java major version required, defaulting to 8 for old metadata.
    pub fn java_major(&self) -> u32 {
        self.java_version.as_ref().map_or(8, |j| j.major_version)
    }
}

/// `group:artifact:version[:classifier][@ext]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoord<'a> {
    pub group: &'a str,
    pub artifact: &'a str,
    pub version: &'a str,
    pub classifier: Option<&'a str>,
    pub ext: &'a str,
}

impl<'a> MavenCoord<'a> {
    pub fn parse(name: &'a str) -> Option<Self> {
        let (coords, ext) = name.split_once('@').unwrap_or((name, "jar"));
        let mut parts = coords.split(':');
        let (group, artifact, version) = (parts.next()?, parts.next()?, parts.next()?);
        let classifier = parts.next();
        if parts.next().is_some() || [group, artifact, version].contains(&"") {
            return None;
        }
        Some(Self { group, artifact, version, classifier, ext })
    }

    pub fn path(&self) -> String {
        let classifier = self.classifier.map(|c| format!("-{c}")).unwrap_or_default();
        format!(
            "{}/{}/{}/{}-{}{classifier}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            self.artifact,
            self.version,
            self.ext
        )
    }

    fn with_classifier(&self, classifier: &'a str) -> Self {
        Self { classifier: Some(classifier), ..self.clone() }
    }

    /// Identity used to de-duplicate libraries across a profile and its parent.
    fn key(&self) -> String {
        format!("{}:{}:{}", self.group, self.artifact, self.classifier.unwrap_or(""))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeJar {
    pub path: PathBuf,
    pub exclude: Vec<String>,
}

#[derive(Debug, Default, Clone)]
pub struct ResolvedLibraries {
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<NativeJar>,
    pub downloads: Vec<FileDownload>,
}

/// Works out which library files this platform needs, where they live, and
/// how to fetch them.
pub fn resolve_libraries(
    libraries: &[Library],
    platform: Platform,
    root: &Path,
) -> Result<ResolvedLibraries, AssetsError> {
    let mut out = ResolvedLibraries::default();
    let mut seen = HashSet::new();
    for lib in libraries {
        if !rules_allow(&lib.rules, platform, &[]) {
            continue;
        }
        let coord = MavenCoord::parse(&lib.name)
            .ok_or_else(|| AssetsError::Metadata(format!("bad library name {:?}", lib.name)))?;
        if !seen.insert(coord.key()) {
            continue;
        }
        let base = lib.url.as_deref().unwrap_or(LIBRARIES_URL);
        let computed = |coord: &MavenCoord| Artifact {
            path: None,
            url: format!("{}/{}", base.trim_end_matches('/'), coord.path()),
            sha1: lib.sha1.clone(),
            size: lib.size,
        };

        let main = match (&lib.downloads, &lib.natives) {
            (Some(d), _) => d.artifact.clone(),
            (None, None) => Some(computed(&coord)),
            (None, Some(_)) => None,
        };
        if let Some(artifact) = main {
            let file = to_download(&artifact, &coord, root)?;
            out.classpath.push(file.path.clone());
            out.downloads.push(file);
        }

        let Some(classifier) = lib.natives.as_ref().and_then(|n| n.get(platform.os_name())) else {
            continue;
        };
        let classifier = classifier.replace("${arch}", platform.pointer_bits());
        let native_coord = coord.with_classifier(&classifier);
        let artifact = match &lib.downloads {
            Some(d) => match d.classifiers.get(&classifier) {
                Some(a) => a.clone(),
                None => {
                    tracing::warn!("{} lists {classifier} but provides no download", lib.name);
                    continue;
                }
            },
            None => computed(&native_coord),
        };
        let file = to_download(&artifact, &native_coord, root)?;
        out.natives
            .push(NativeJar { path: file.path.clone(), exclude: lib.extract.clone().unwrap_or_default().exclude });
        out.downloads.push(file);
    }
    Ok(out)
}

fn to_download(artifact: &Artifact, coord: &MavenCoord, root: &Path) -> Result<FileDownload, AssetsError> {
    let rel = artifact.path.clone().unwrap_or_else(|| coord.path());
    Ok(FileDownload {
        url: artifact.url.clone(),
        path: rustcraft_core::fs::safe_join(root, &rel)?,
        sha1: artifact.sha1.clone(),
        size: artifact.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy() -> VersionJson {
        serde_json::from_str(include_str!("../tests/fixtures/1.8.9.json")).unwrap()
    }

    fn modern() -> VersionJson {
        serde_json::from_str(include_str!("../tests/fixtures/1.21.4.json")).unwrap()
    }

    const LINUX: Platform = Platform { os: Os::Linux, arch: "x86_64" };
    const MAC_ARM: Platform = Platform { os: Os::MacOs, arch: "arm64" };
    const WIN32: Platform = Platform { os: Os::Windows, arch: "x86" };

    #[test]
    fn maven_paths() {
        let c = MavenCoord::parse("org.lwjgl.lwjgl:lwjgl-platform:2.9.4-nightly-20150209").unwrap();
        assert_eq!(
            c.with_classifier("natives-linux").path(),
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar"
        );
        assert_eq!(MavenCoord::parse("a.b:c:1@zip").unwrap().path(), "a/b/c/1/c-1.zip");
        assert!(MavenCoord::parse("a:b").is_none());
        assert!(MavenCoord::parse("a:b:c:d:e").is_none());
    }

    #[test]
    fn rule_semantics() {
        let rules: Vec<Rule> =
            serde_json::from_str(r#"[{"action":"allow"},{"action":"disallow","os":{"name":"osx"}}]"#).unwrap();
        assert!(rules_allow(&rules, LINUX, &[]));
        assert!(!rules_allow(&rules, MAC_ARM, &[]));
        let only_mac: Vec<Rule> = serde_json::from_str(r#"[{"action":"allow","os":{"name":"osx"}}]"#).unwrap();
        assert!(!rules_allow(&only_mac, LINUX, &[]));
        assert!(rules_allow(&only_mac, MAC_ARM, &[]));
        let x86: Vec<Rule> = serde_json::from_str(r#"[{"action":"allow","os":{"arch":"x86"}}]"#).unwrap();
        assert!(rules_allow(&x86, WIN32, &[]));
        assert!(!rules_allow(&x86, LINUX, &[]));
        let feature: Vec<Rule> =
            serde_json::from_str(r#"[{"action":"allow","features":{"is_demo_user":true}}]"#).unwrap();
        assert!(!rules_allow(&feature, LINUX, &[]));
        assert!(rules_allow(&feature, LINUX, &["is_demo_user"]));
    }

    #[test]
    fn legacy_libraries_and_natives_per_os() {
        let v = legacy();
        let root = Path::new("/libs");
        let linux = resolve_libraries(&v.libraries, LINUX, root).unwrap();
        let names: Vec<_> = linux.classpath.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["lwjgl-2.9.4-nightly-20150209.jar", "gson-2.2.4.jar"]);
        assert_eq!(linux.natives.len(), 1);
        assert!(linux.natives[0].path.ends_with(
            "org/lwjgl/lwjgl/lwjgl-platform/2.9.4-nightly-20150209/lwjgl-platform-2.9.4-nightly-20150209-natives-linux.jar"
        ));
        assert_eq!(linux.natives[0].exclude, ["META-INF/"]);
        assert_eq!(linux.downloads.len(), 3);

        let mac = resolve_libraries(&v.libraries, MAC_ARM, root).unwrap();
        let names: Vec<_> = mac.classpath.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["lwjgl-2.9.2-nightly-20140822.jar", "gson-2.2.4.jar"]);
        assert_eq!(mac.natives.len(), 1);
        assert!(mac.natives[0].path.ends_with("twitch-platform-6.5-natives-osx.jar"));

        let win = resolve_libraries(&v.libraries, WIN32, root).unwrap();
        let natives: Vec<_> = win.natives.iter().map(|n| n.path.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(
            natives,
            ["lwjgl-platform-2.9.4-nightly-20150209-natives-windows.jar", "twitch-platform-6.5-natives-windows-32.jar"]
        );
    }

    #[test]
    fn modern_libraries_put_natives_on_classpath() {
        let v = modern();
        let r = resolve_libraries(&v.libraries, LINUX, Path::new("/libs")).unwrap();
        let names: Vec<_> = r.classpath.iter().map(|p| p.file_name().unwrap().to_str().unwrap()).collect();
        assert_eq!(names, ["lwjgl-3.3.3.jar", "lwjgl-3.3.3-natives-linux.jar"]);
        assert!(r.natives.is_empty());
        let mac = resolve_libraries(&v.libraries, MAC_ARM, Path::new("/libs")).unwrap();
        assert_eq!(mac.classpath.len(), 2);
        assert!(mac.classpath[1].ends_with("lwjgl-3.3.3-natives-macos-arm64.jar"));
    }

    #[test]
    fn hostile_library_path_is_rejected() {
        let libs: Vec<Library> = serde_json::from_str(
            r#"[{"name":"a:b:1","downloads":{"artifact":{"path":"../../../.bashrc","url":"https://x/y","sha1":"00","size":1}}}]"#,
        )
        .unwrap();
        assert!(resolve_libraries(&libs, LINUX, Path::new("/libs")).is_err());
    }

    #[test]
    fn loader_profile_merges_onto_parent() {
        let profile: VersionJson =
            serde_json::from_str(include_str!("../tests/fixtures/fabric-loader-1.21.4.json")).unwrap();
        assert_eq!(profile.inherits_from.as_deref(), Some("1.21.4"));
        let merged = profile.merged_onto(modern());
        assert_eq!(merged.id, "fabric-loader-0.16.10-1.21.4");
        assert_eq!(merged.jar_id(), "1.21.4");
        assert_eq!(merged.main_class.as_deref(), Some("net.fabricmc.loader.impl.launch.knot.KnotClient"));
        assert_eq!(merged.asset_index.as_ref().unwrap().id, "19");
        assert_eq!(merged.java_major(), 21);
        let r = resolve_libraries(&merged.libraries, LINUX, Path::new("/libs")).unwrap();
        let first = &r.downloads[0];
        assert_eq!(
            first.url,
            "https://maven.fabricmc.net/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
        );
        assert_eq!(first.sha1.as_deref(), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        // Profile JVM args are appended after the parent's.
        let jvm = &merged.arguments.as_ref().unwrap().jvm;
        assert!(
            matches!(jvm.last(), Some(Argument::Plain(s)) if s == "-DFabricMcEmu= net.minecraft.client.main.Main ")
        );
    }
}
