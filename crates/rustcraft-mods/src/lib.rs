//! Mod support for RustCraft.
//!
//! [`loader`] installs Java mod loaders whose launch metadata is published
//! as a version profile (Fabric). Such mods then run in the JVM
//! compatibility path: the official game plus the loader in a separate
//! Java process.

pub mod loader;

#[derive(Debug, thiserror::Error)]
pub enum ModsError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
