//! Mod support for RustCraft.
//!
//! [`loader`] installs Java mod loaders whose launch metadata is published
//! as a version profile (Fabric). Such mods then run in the JVM
//! compatibility path: the official game plus the loader in a separate
//! Java process. [`scan`] reads mod jars' metadata and reports, per mod,
//! whether it can load in an instance and why not.

pub mod loader;
pub mod scan;
pub mod version;

#[derive(Debug, thiserror::Error)]
pub enum ModsError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
