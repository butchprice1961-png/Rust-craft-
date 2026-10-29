//! Minecraft: Java Edition 1.8.x networking (protocol 47) for RustCraft's
//! engine: framing, compression, encryption, login (offline-mode servers,
//! and online-mode servers through Mojang's session service) and the play
//! packets the engine uses.

pub mod codec;
pub mod conn;
pub mod login;
pub mod packets;

pub use codec::Item;
pub use login::{Joined, Profile, Session, join};
pub use packets::{Clientbound, GameMode, send};

/// Minecraft 1.8 to 1.8.9.
pub const PROTOCOL_VERSION: i32 = 47;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connection failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("the server sent a bad packet: {0}")]
    Malformed(&'static str),
    #[error("not a valid server address")]
    BadAddress,
    #[error("the server disconnected: {0}")]
    Disconnected(String),
    #[error("this server is in online mode and needs a signed-in Minecraft account")]
    NeedsAccount,
    #[error("joining failed: {0}")]
    Session(String),
}

pub type Result<T> = std::result::Result<T, Error>;
