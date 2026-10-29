//! Minecraft Java Edition distribution metadata: the version list, per-version
//! launch metadata, verified downloads, asset objects and Java runtimes.
//!
//! Files are fetched only from the official endpoints named in the metadata
//! (or a loader's own Maven repository for loader profiles); nothing
//! proprietary is bundled with RustCraft.

pub mod download;
pub mod java;
pub mod launch;
pub mod manifest;
pub mod version;

pub use download::{DownloadError, Downloader, FileDownload, Progress, http_client, http_client_builder};
pub use launch::{Installed, Installer, LaunchAuth, LaunchCommand, LaunchOptions, build_command};
pub use manifest::{VersionEntry, VersionKind, VersionManifest};
pub use version::{Platform, VersionJson};

#[derive(Debug, thiserror::Error)]
pub enum AssetsError {
    #[error(transparent)]
    Download(#[from] DownloadError),
    #[error("invalid metadata: {0}")]
    Metadata(String),
    #[error("malformed JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
