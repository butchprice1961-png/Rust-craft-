//! Reads mod jars as untrusted archives (metadata only: nothing is run or
//! unpacked to disk) and checks them against an instance: loader, game
//! version, dependencies, duplicates and declared incompatibilities.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use serde_json::Value;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::version::{minecraft, satisfies};

/// Largest metadata file read from a jar.
const MAX_META: u64 = 1 << 20;
/// Largest nested jar (Fabric's jar-in-jar) read into memory.
const MAX_NESTED: u64 = 64 << 20;
const MAX_NESTING: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    /// Forge for 1.12 and older (`mcmod.info`).
    LegacyForge,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub kind: Kind,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Fabric's `environment`: `client`, `server` or `*`.
    pub environment: String,
    /// Dependency id and its version predicates (any may match).
    pub depends: Vec<(String, Vec<String>)>,
    pub breaks: Vec<(String, Vec<String>)>,
    /// Ids this jar supplies besides its own: `provides` and nested jars.
    pub provides: Vec<(String, String)>,
    pub has_code: bool,
}

#[derive(Debug, Clone)]
pub struct ModFile {
    pub path: PathBuf,
    pub enabled: bool,
    /// The jar's metadata, or why it could not be read.
    pub meta: Result<Meta, String>,
}

impl ModFile {
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }

    pub fn title(&self) -> String {
        match &self.meta {
            Ok(m) if !m.name.is_empty() => m.name.clone(),
            _ => self.file_name(),
        }
    }
}

/// Every `*.jar` and `*.jar.disabled` in `dir`, sorted by file name.
pub fn scan(dir: &Path) -> Vec<ModFile> {
    let mut mods: Vec<ModFile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?.to_ascii_lowercase();
            let enabled = name.ends_with(".jar");
            (enabled || name.ends_with(".jar.disabled")).then(|| ModFile { meta: read_jar(&path), path, enabled })
        })
        .collect();
    mods.sort_by_key(|m| m.file_name().to_lowercase());
    mods
}

pub fn read_jar(path: &Path) -> Result<Meta, String> {
    let file = File::open(path).map_err(|e| format!("cannot be read ({e})"))?;
    let mut zip = ZipArchive::new(file).map_err(|_| "is not a valid jar file".to_string())?;
    read_archive(&mut zip, 0)
}

/// `None` if the archive has no such entry.
fn read_entry<R: Read + Seek>(zip: &mut ZipArchive<R>, name: &str, limit: u64) -> Option<Result<Vec<u8>, String>> {
    let entry = match zip.by_name(name) {
        Ok(e) => e,
        Err(ZipError::FileNotFound) => return None,
        Err(e) => return Some(Err(format!("has a damaged {name} ({e})"))),
    };
    // The header's size is a claim; cap the real read as well.
    let mut out = Vec::new();
    let read = entry.take(limit + 1).read_to_end(&mut out);
    Some(match read {
        Ok(n) if n as u64 > limit => Err(format!("has an oversized {name}")),
        Ok(_) => Ok(out),
        Err(e) => Err(format!("has a damaged {name} ({e})")),
    })
}

fn json(bytes: &[u8], name: &str) -> Result<Value, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|e| format!("has an invalid {name} ({e})"))
}

fn read_archive<R: Read + Seek>(zip: &mut ZipArchive<R>, depth: usize) -> Result<Meta, String> {
    let has_code = zip.file_names().any(|n| n.is_ok_and(|n| n.ends_with(".class")));
    let mut meta = if let Some(bytes) = read_entry(zip, "fabric.mod.json", MAX_META) {
        fabric(&json(&bytes?, "fabric.mod.json")?, zip, depth)?
    } else if let Some(bytes) = read_entry(zip, "quilt.mod.json", MAX_META) {
        quilt(&json(&bytes?, "quilt.mod.json")?)
    } else if let Some(bytes) = read_entry(zip, "META-INF/neoforge.mods.toml", MAX_META) {
        forge(&bytes?, Kind::NeoForge, zip)
    } else if let Some(bytes) = read_entry(zip, "META-INF/mods.toml", MAX_META) {
        forge(&bytes?, Kind::Forge, zip)
    } else if let Some(bytes) = read_entry(zip, "mcmod.info", MAX_META) {
        legacy_forge(&bytes?)
    } else {
        Meta::default()
    };
    meta.has_code = has_code;
    Ok(meta)
}

fn str_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or_default().to_string()
}

fn fabric<R: Read + Seek>(v: &Value, zip: &mut ZipArchive<R>, depth: usize) -> Result<Meta, String> {
    let id = str_of(v.get("id"));
    if id.is_empty() {
        return Err("has a fabric.mod.json without a mod id".into());
    }
    let version = str_of(v.get("version"));
    let deps = |key: &str| -> Vec<(String, Vec<String>)> {
        let Some(map) = v.get(key).and_then(Value::as_object) else { return Vec::new() };
        map.iter()
            .map(|(id, p)| {
                let preds = match p {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                    _ => Vec::new(),
                };
                (id.clone(), preds)
            })
            .collect()
    };
    let mut provides: Vec<(String, String)> = v
        .get("provides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.as_str()?.to_string(), version.clone())))
        .collect();
    let nested: Vec<String> = v
        .get("jars")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|j| Some(j.get("file")?.as_str()?.to_string()))
        .collect();
    if depth < MAX_NESTING {
        for file in nested {
            let Some(Ok(bytes)) = read_entry(zip, &file, MAX_NESTED) else { continue };
            let Ok(mut inner) = ZipArchive::new(Cursor::new(bytes)) else { continue };
            if let Ok(m) = read_archive(&mut inner, depth + 1)
                && m.kind == Kind::Fabric
            {
                provides.push((m.id, m.version));
                provides.extend(m.provides);
            }
        }
    }
    let name = str_of(v.get("name"));
    let environment = str_of(v.get("environment"));
    Ok(Meta {
        kind: Kind::Fabric,
        name: if name.is_empty() { id.clone() } else { name },
        id,
        version,
        environment: if environment.is_empty() { "*".into() } else { environment },
        depends: deps("depends"),
        breaks: deps("breaks"),
        provides,
        has_code: false,
    })
}

fn quilt(v: &Value) -> Meta {
    let q = v.get("quilt_loader");
    let field = |k: &str| str_of(q.and_then(|q| q.get(k)));
    let name = str_of(q.and_then(|q| q.get("metadata")?.get("name")));
    Meta { kind: Kind::Quilt, id: field("id"), name, version: field("version"), ..Meta::default() }
}

fn forge<R: Read + Seek>(bytes: &[u8], kind: Kind, zip: &mut ZipArchive<R>) -> Meta {
    let parsed: Option<toml::Table> = std::str::from_utf8(bytes).ok().and_then(|t| t.parse().ok());
    let first =
        parsed.as_ref().and_then(|t| t.get("mods")?.as_array()?.first()?.as_table().cloned()).unwrap_or_default();
    let field = |k: &str| first.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let mut version = field("version");
    if version.contains("${file.jarVersion}") {
        let manifest = read_entry(zip, "META-INF/MANIFEST.MF", MAX_META).and_then(Result::ok).unwrap_or_default();
        version = String::from_utf8_lossy(&manifest)
            .lines()
            .find_map(|l| l.strip_prefix("Implementation-Version:").map(|v| v.trim().to_string()))
            .unwrap_or_default();
    }
    Meta { kind, id: field("modId"), name: field("displayName"), version, ..Meta::default() }
}

fn legacy_forge(bytes: &[u8]) -> Meta {
    let v = json(bytes, "mcmod.info").unwrap_or_default();
    let list = v.get("modList").unwrap_or(&v);
    let first = list.as_array().and_then(|a| a.first());
    let field = |k: &str| str_of(first.and_then(|m| m.get(k)));
    Meta {
        kind: Kind::LegacyForge,
        id: field("modid"),
        name: field("name"),
        version: field("version"),
        ..Meta::default()
    }
}

/// The instance mods are checked against.
pub struct Target<'a> {
    pub game_version: &'a str,
    /// Fabric Loader version; `None` for an instance without a mod loader.
    pub fabric_loader: Option<&'a str>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    /// Reasons the mod will not load or will break the game.
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

fn display(id: &str) -> String {
    match id {
        "fabric-api" | "fabric" => "Fabric API".into(),
        _ => id.into(),
    }
}

/// One result per mod, in the same order. Disabled mods are not checked.
pub fn check(mods: &[ModFile], target: &Target) -> Vec<Check> {
    let enabled_fabric = || {
        mods.iter()
            .filter(|m| m.enabled)
            .filter_map(|m| Some((m, m.meta.as_ref().ok()?)))
            .filter(|(_, m)| m.kind == Kind::Fabric)
    };
    let mut provided: HashMap<&str, &str> = HashMap::new();
    for (_, m) in enabled_fabric() {
        provided.insert(&m.id, &m.version);
        for (id, v) in &m.provides {
            provided.entry(id).or_insert(v);
        }
    }
    let disabled: Vec<&str> = mods
        .iter()
        .filter(|m| !m.enabled)
        .filter_map(|m| m.meta.as_ref().ok())
        .flat_map(|m| std::iter::once(m.id.as_str()).chain(m.provides.iter().map(|(id, _)| id.as_str())))
        .collect();
    let game = minecraft(target.game_version);

    let check_one = |file: &ModFile| -> Check {
        let mut c = Check::default();
        let meta = match &file.meta {
            Ok(meta) => meta,
            Err(e) => {
                c.problems.push(format!("This file {e}."));
                return c;
            }
        };
        let Some(loader) = target.fabric_loader else {
            c.problems.push("This instance has no mod loader, so the game does not load mods.".into());
            return c;
        };
        let wrong_loader = match meta.kind {
            Kind::Fabric => None,
            Kind::Quilt => Some("Needs Quilt Loader, which RustCraft does not support."),
            Kind::Forge | Kind::LegacyForge => Some("Made for Forge, which RustCraft does not support."),
            Kind::NeoForge => Some("Made for NeoForge, which RustCraft does not support."),
            Kind::Unknown if meta.has_code => Some("Not a Fabric mod: the jar has no fabric.mod.json."),
            Kind::Unknown => Some("Not a mod: the jar has no mod metadata."),
        };
        if let Some(reason) = wrong_loader {
            c.problems.push(reason.into());
            return c;
        }
        if let Some((other, _)) = enabled_fabric().find(|(o, m)| !std::ptr::eq(*o, file) && m.id == meta.id) {
            c.problems.push(format!("{} is installed twice (also {}).", meta.name, other.file_name()));
        }
        if meta.environment == "server" {
            c.warnings.push("Server-only mod: Fabric does not load it in the game client.".into());
        }
        let installed = |dep: &str| match dep {
            "minecraft" => game.as_deref(),
            "fabricloader" => Some(loader),
            _ => provided.get(dep).copied(),
        };
        for (dep, preds) in &meta.depends {
            // Java is chosen at launch; snapshots need Fabric's own table.
            if dep == "java" || (dep == "minecraft" && game.is_none()) {
                continue;
            }
            let wanted = preds.join(" or ");
            match installed(dep) {
                None if disabled.contains(&dep.as_str()) => {
                    c.problems.push(format!("Needs {}, which is disabled.", display(dep)))
                }
                None => c.problems.push(format!("Needs {}, which is not installed.", display(dep))),
                Some(v) if satisfies(preds, v) == Some(false) => c.problems.push(match dep.as_str() {
                    "minecraft" => format!("Made for Minecraft {wanted}; this instance is {}.", target.game_version),
                    "fabricloader" => format!("Needs Fabric Loader {wanted}; this instance has {loader}."),
                    _ => format!("Needs {} {wanted}; version {v} is installed.", display(dep)),
                }),
                Some(_) => {}
            }
        }
        for (dep, preds) in &meta.breaks {
            if let Some(v) = installed(dep)
                && satisfies(preds, v) == Some(true)
            {
                c.problems.push(format!("Does not work with {} {v}.", display(dep)));
            }
        }
        c
    };
    mods.iter().map(|m| if m.enabled { check_one(m) } else { Check::default() }).collect()
}

/// Renames `x.jar` to `x.jar.disabled` or back. The jar itself is never
/// changed. Returns the new path.
pub fn set_enabled(path: &Path, enabled: bool) -> io::Result<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| io::Error::other("unusable file name"))?;
    let is_disabled = name.to_ascii_lowercase().ends_with(".disabled");
    let new_name = match (enabled, is_disabled) {
        (true, true) => name[..name.len() - ".disabled".len()].to_string(),
        (false, false) => format!("{name}.disabled"),
        _ => return Ok(path.to_path_buf()),
    };
    let target = path.with_file_name(new_name);
    if target.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} already exists", target.display())));
    }
    std::fs::rename(path, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn jar_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn fabric_jar(dir: &Path, file: &str, meta: Value, extra: &[(&str, &[u8])]) {
        let meta = meta.to_string();
        let mut entries: Vec<(&str, &[u8])> = vec![("fabric.mod.json", meta.as_bytes()), ("a/B.class", b"\xCA\xFE")];
        entries.extend_from_slice(extra);
        std::fs::write(dir.join(file), jar_bytes(&entries)).unwrap();
    }

    fn checks(dir: &Path, target: &Target) -> HashMap<String, Check> {
        let mods = scan(dir);
        let results = check(&mods, target);
        mods.iter().map(ModFile::file_name).zip(results).collect()
    }

    #[test]
    fn reports_loader_version_dependency_and_duplicate_problems() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let base = jar_bytes(&[(
            "fabric.mod.json",
            json!({"id": "fabric-api-base", "version": "0.4.42"}).to_string().as_bytes(),
        )]);
        fabric_jar(
            d,
            "fabric-api.jar",
            json!({"id": "fabric-api", "name": "Fabric API", "version": "0.110.0+1.21.4", "provides": ["fabric"],
                   "jars": [{"file": "META-INF/jars/base.jar"}, {"file": "../../escape.jar"}]}),
            &[("META-INF/jars/base.jar", &base)],
        );
        fabric_jar(
            d,
            "sodium.jar",
            json!({"id": "sodium", "name": "Sodium", "version": "0.6.5",
                   "depends": {"minecraft": "~1.21.4", "fabricloader": ">=0.16.0", "fabric-api-base": ">=0.4", "java": ">=21"}}),
            &[],
        );
        fabric_jar(d, "sodium-copy.jar", json!({"id": "sodium", "name": "Sodium", "version": "0.6.4"}), &[]);
        fabric_jar(
            d,
            "old.jar",
            json!({"id": "old", "version": "1.0", "depends": {"minecraft": ["1.20.x", "1.19.4"]}}),
            &[],
        );
        fabric_jar(
            d,
            "new-loader.jar",
            json!({"id": "nl", "version": "1", "depends": {"fabricloader": ">=0.17"}}),
            &[],
        );
        fabric_jar(
            d,
            "needs-lib.jar",
            json!({"id": "x", "version": "1", "depends": {"lib": "*", "missing": "*"}}),
            &[],
        );
        fabric_jar(d, "lib.jar.disabled", json!({"id": "lib", "version": "1"}), &[]);
        fabric_jar(d, "hates-sodium.jar", json!({"id": "h", "version": "1", "breaks": {"sodium": "<0.7"}}), &[]);
        fabric_jar(d, "server.jar", json!({"id": "srv", "version": "1", "environment": "server"}), &[]);
        let toml =
            b"modLoader=\"javafml\"\n[[mods]]\nmodId=\"jei\"\ndisplayName=\"JEI\"\nversion=\"${file.jarVersion}\"\n";
        let manifest = b"Manifest-Version: 1.0\nImplementation-Version: 19.21\n";
        std::fs::write(
            d.join("jei.jar"),
            jar_bytes(&[("META-INF/mods.toml", toml), ("META-INF/MANIFEST.MF", manifest)]),
        )
        .unwrap();
        std::fs::write(d.join("textures.jar"), jar_bytes(&[("assets/x/textures/a.png", b"png")])).unwrap();
        std::fs::write(d.join("broken.jar"), b"not a zip").unwrap();
        std::fs::write(d.join("notes.txt"), b"ignored").unwrap();

        let target = Target { game_version: "1.21.4", fabric_loader: Some("0.16.10") };
        let c = checks(d, &target);
        assert_eq!(c.len(), 12);
        let problems = |f: &str| c[f].problems.join(" | ");
        assert_eq!(c["fabric-api.jar"], Check::default());
        assert!(
            problems("sodium.jar").contains("installed twice (also sodium-copy.jar)"),
            "{}",
            problems("sodium.jar")
        );
        assert_eq!(c["sodium.jar"].problems.len(), 1);
        assert_eq!(problems("old.jar"), "Made for Minecraft 1.20.x or 1.19.4; this instance is 1.21.4.");
        assert_eq!(problems("new-loader.jar"), "Needs Fabric Loader >=0.17; this instance has 0.16.10.");
        assert_eq!(problems("needs-lib.jar"), "Needs lib, which is disabled. | Needs missing, which is not installed.");
        assert_eq!(c["lib.jar.disabled"], Check::default());
        assert_eq!(problems("hates-sodium.jar"), "Does not work with sodium 0.6.5.");
        assert!(c["server.jar"].problems.is_empty() && c["server.jar"].warnings.len() == 1);
        assert_eq!(problems("jei.jar"), "Made for Forge, which RustCraft does not support.");
        assert_eq!(problems("textures.jar"), "Not a mod: the jar has no mod metadata.");
        assert_eq!(problems("broken.jar"), "This file is not a valid jar file.");

        let mods = scan(d);
        let jei = mods.iter().find(|m| m.file_name() == "jei.jar").unwrap().meta.as_ref().unwrap();
        assert_eq!((jei.id.as_str(), jei.name.as_str(), jei.version.as_str()), ("jei", "JEI", "19.21"));
        let api = mods.iter().find(|m| m.file_name() == "fabric-api.jar").unwrap().meta.as_ref().unwrap();
        assert!(api.provides.iter().any(|(id, v)| id == "fabric-api-base" && v == "0.4.42"));
        assert!(api.has_code);

        let vanilla = checks(d, &Target { game_version: "1.21.4", fabric_loader: None });
        assert!(vanilla["sodium.jar"].problems[0].contains("no mod loader"));
    }

    #[test]
    fn hostile_archives_are_bounded() {
        let tmp = tempfile::tempdir().unwrap();
        // Compresses to a few kilobytes, expands past the metadata limit.
        let huge = vec![b' '; (MAX_META + 10) as usize];
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let deflate = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("fabric.mod.json", deflate).unwrap();
        zip.write_all(&huge).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        assert!(bytes.len() < 64 << 10);
        let path = tmp.path().join("bomb.jar");
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(read_jar(&path).unwrap_err(), "has an oversized fabric.mod.json");

        std::fs::write(&path, jar_bytes(&[("fabric.mod.json", b"{\"version\": \"1\"}")])).unwrap();
        assert!(read_jar(&path).unwrap_err().contains("without a mod id"));
    }

    #[test]
    fn enabling_and_disabling_only_renames() {
        let tmp = tempfile::tempdir().unwrap();
        let jar = tmp.path().join("a.jar");
        std::fs::write(&jar, b"data").unwrap();
        let off = set_enabled(&jar, false).unwrap();
        assert!(off.ends_with("a.jar.disabled") && !jar.exists());
        assert_eq!(set_enabled(&off, false).unwrap(), off);
        std::fs::write(&jar, b"other").unwrap();
        assert_eq!(set_enabled(&off, true).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        std::fs::remove_file(&jar).unwrap();
        assert_eq!(std::fs::read(set_enabled(&off, true).unwrap()).unwrap(), b"data");
    }
}
