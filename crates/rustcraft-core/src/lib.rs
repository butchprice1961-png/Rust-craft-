//! Shared foundations for every RustCraft crate: the managed data-directory
//! layout, path-safety checks for untrusted names, and crash-safe
//! configuration persistence.

pub mod fs;
pub mod nbt;
pub mod paths;

pub use paths::DataDir;
