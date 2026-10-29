//! Mojang-distributed Java runtimes, so players do not need a system Java.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use rustcraft_core::fs::safe_join;

use crate::AssetsError;
use crate::download::{FileDownload, Progress};
use crate::launch::Installer;
use crate::version::{Os, Platform};

pub const RUNTIME_INDEX_URL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

/// This machine's key in the runtime index.
pub fn runtime_platform(p: Platform) -> Option<&'static str> {
    Some(match (p.os, p.arch) {
        (Os::Linux, "x86_64") => "linux",
        (Os::Linux, "x86") => "linux-i386",
        (Os::MacOs, "x86_64") => "mac-os",
        (Os::MacOs, "arm64") => "mac-os-arm64",
        (Os::Windows, "x86_64") => "windows-x64",
        (Os::Windows, "x86") => "windows-x86",
        (Os::Windows, "arm64") => "windows-arm64",
        _ => return None,
    })
}

#[derive(Deserialize)]
struct RuntimeEntry {
    manifest: Remote,
}

#[derive(Deserialize)]
struct Remote {
    sha1: String,
    size: u64,
    url: String,
}

#[derive(Deserialize)]
struct RuntimeManifest {
    files: BTreeMap<String, RuntimeFile>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum RuntimeFile {
    Directory,
    File {
        #[serde(default)]
        executable: bool,
        downloads: RuntimeDownloads,
    },
    Link {
        target: String,
    },
}

#[derive(Deserialize)]
struct RuntimeDownloads {
    raw: Remote,
}

pub fn java_executable(root: &Path, p: Platform) -> PathBuf {
    match p.os {
        Os::Windows => root.join("bin").join("javaw.exe"),
        Os::MacOs => root.join("jre.bundle/Contents/Home/bin/java"),
        Os::Linux => root.join("bin/java"),
    }
}

impl Installer {
    /// Installs (or verifies) Mojang's runtime `component`, e.g. `jre-legacy`
    /// or `java-runtime-delta`, and returns its `java` executable.
    pub async fn install_java(
        &self,
        index_url: &str,
        component: &str,
        progress: &Arc<Progress>,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, AssetsError> {
        let platform = runtime_platform(self.platform).ok_or_else(|| {
            AssetsError::Metadata("Mojang publishes no Java runtime for this platform".into())
        })?;
        let index: BTreeMap<String, BTreeMap<String, Vec<RuntimeEntry>>> = self
            .json(
                index_url,
                &self.dirs.cache().join("java-runtimes.json"),
                None,
            )
            .await?;
        let entry = index
            .get(platform)
            .and_then(|c| c.get(component))
            .and_then(|v| v.first())
            .ok_or_else(|| {
                AssetsError::Metadata(format!(
                    "Mojang does not publish {component} for {platform}"
                ))
            })?;

        let root = safe_join(&self.dirs.runtimes(), component)?;
        let manifest_path = safe_join(&self.dirs.runtimes(), &format!("{component}.json"))?;
        let manifest: RuntimeManifest = self
            .json(
                &entry.manifest.url,
                &manifest_path,
                Some(&entry.manifest.sha1),
            )
            .await?;

        let (mut files, mut executables, mut links) = (Vec::new(), Vec::new(), Vec::new());
        for (rel, file) in &manifest.files {
            let dest = safe_join(&root, rel)?;
            match file {
                RuntimeFile::Directory => std::fs::create_dir_all(&dest)?,
                RuntimeFile::File {
                    executable,
                    downloads,
                } => {
                    files.push(FileDownload {
                        url: downloads.raw.url.clone(),
                        path: dest.clone(),
                        sha1: Some(downloads.raw.sha1.clone()),
                        size: Some(downloads.raw.size),
                    });
                    if *executable {
                        executables.push(dest);
                    }
                }
                RuntimeFile::Link { target } => links.push((dest, target.clone())),
            }
        }
        self.downloader.fetch_all(files, progress, cancel).await?;
        for exe in executables {
            make_executable(&exe)?;
        }
        for (link, target) in links {
            if !link_stays_inside(&root, &link, &target) {
                return Err(AssetsError::Metadata(format!(
                    "runtime link {} escapes the runtime",
                    link.display()
                )));
            }
            create_link(&link, &target)?;
        }
        Ok(java_executable(&root, self.platform))
    }
}

/// Lexically resolves `target` relative to `link` and checks it stays under `root`.
fn link_stays_inside(root: &Path, link: &Path, target: &str) -> bool {
    let Some(mut out) = link.parent().map(Path::to_path_buf) else {
        return false;
    };
    for c in Path::new(target).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir if out.pop() => {}
            _ => return false,
        }
    }
    out.starts_with(root)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_link(link: &Path, target: &str) -> std::io::Result<()> {
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_file(link);
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn create_link(link: &Path, _: &str) -> std::io::Result<()> {
    // Windows runtimes ship no links; nothing depends on one existing.
    tracing::warn!("skipping symlink {} on this platform", link.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::{Downloader, http_client_builder, sha1_hex};
    use rustcraft_core::DataDir;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn links_cannot_escape() {
        let root = Path::new("/rt/jre");
        assert!(link_stays_inside(
            root,
            Path::new("/rt/jre/lib/a.so"),
            "../bin/b.so"
        ));
        assert!(!link_stays_inside(
            root,
            Path::new("/rt/jre/lib/a.so"),
            "../../../etc/passwd"
        ));
        assert!(!link_stays_inside(
            root,
            Path::new("/rt/jre/a"),
            "/etc/passwd"
        ));
    }

    #[tokio::test]
    async fn installs_runtime_from_manifest() {
        let server = MockServer::start().await;
        let java = b"#!/bin/sh\necho java\n";
        let manifest = serde_json::json!({ "files": {
            "bin": { "type": "directory" },
            "bin/java": { "type": "file", "executable": true,
                "downloads": { "raw": { "sha1": sha1_hex(java), "size": java.len(), "url": format!("{}/java", server.uri()) } } },
            "bin/java-alias": { "type": "link", "target": "java" }
        }})
        .to_string();
        let index = serde_json::json!({ "linux": { "jre-legacy": [ { "manifest": {
            "sha1": sha1_hex(manifest.as_bytes()), "size": manifest.len(), "url": format!("{}/manifest", server.uri()) } } ] } });
        Mock::given(path("/all.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(index))
            .mount(&server)
            .await;
        Mock::given(path("/manifest"))
            .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
            .mount(&server)
            .await;
        Mock::given(path("/java"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(java.as_slice()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let mut installer = Installer::new(
            DataDir::at(dir.path()),
            Downloader::new(http_client_builder().build().unwrap()),
        );
        installer.platform = Platform {
            os: Os::Linux,
            arch: "x86_64",
        };
        let exe = installer
            .install_java(
                &format!("{}/all.json", server.uri()),
                "jre-legacy",
                &Arc::default(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(exe, dir.path().join("runtimes/jre-legacy/bin/java"));
        assert_eq!(std::fs::read(&exe).unwrap(), java);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
                0o111
            );
            let alias = dir.path().join("runtimes/jre-legacy/bin/java-alias");
            assert_eq!(std::fs::read(alias).unwrap(), java);
        }

        let err = installer
            .install_java(
                &format!("{}/all.json", server.uri()),
                "java-runtime-zeta",
                &Arc::default(),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not publish"), "{err}");
    }
}
