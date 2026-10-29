//! Joining a server: handshake, login, and for online-mode servers the
//! encryption handshake with Mojang's session service, as the official
//! client does it.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use sha1::{Digest, Sha1};

use crate::codec::{Buf, Put, chat_text};
use crate::conn::{PacketReader, PacketWriter};
use crate::{Error, PROTOCOL_VERSION, Result};

pub const SESSION_JOIN: &str = "https://sessionserver.mojang.com/session/minecraft/join";

/// Who joins. Without `session`, only offline-mode servers accept the
/// player; RustCraft never pretends to be authenticated.
pub struct Profile<'a> {
    pub name: &'a str,
    pub session: Option<Session<'a>>,
}

pub struct Session<'a> {
    pub access_token: &'a str,
    /// Undashed profile UUID.
    pub uuid: &'a str,
    /// Mojang's join endpoint; [`SESSION_JOIN`] except in tests.
    pub join_url: &'a str,
}

pub struct Joined {
    pub reader: PacketReader<TcpStream>,
    pub writer: PacketWriter<TcpStream>,
    /// The UUID and name the server assigned.
    pub uuid: String,
    pub name: String,
}

/// `host`, `host:port` or `[v6]:port`; the default port is 25565.
pub fn split_address(address: &str) -> Result<(String, u16)> {
    let address = address.trim();
    let (host, port) = match address.strip_prefix('[') {
        Some(rest) => {
            let (host, tail) = rest.split_once(']').ok_or(Error::BadAddress)?;
            (host, tail.strip_prefix(':'))
        }
        None => match address.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h, Some(p)),
            _ => (address, None),
        },
    };
    let port = match port {
        Some(p) => p.parse().map_err(|_| Error::BadAddress)?,
        None => 25565,
    };
    if host.is_empty() {
        return Err(Error::BadAddress);
    }
    Ok((host.to_string(), port))
}

/// Minecraft's server hash: SHA-1 as a signed (two's complement) number
/// in hexadecimal.
pub fn server_hash(server_id: &str, secret: &[u8], public_key: &[u8]) -> String {
    let mut digest: [u8; 20] =
        Sha1::new().chain_update(server_id).chain_update(secret).chain_update(public_key).finalize().into();
    let negative = digest[0] & 0x80 != 0;
    if negative {
        let mut carry = true;
        for b in digest.iter_mut().rev() {
            *b = !*b;
            if carry {
                (*b, carry) = b.overflowing_add(1);
            }
        }
    }
    let hex = hex::encode(digest);
    let digits = hex.trim_start_matches('0');
    format!("{}{}", if negative { "-" } else { "" }, if digits.is_empty() { "0" } else { digits })
}

fn session_join(session: &Session, server_hash: &str) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::blocking::Client::builder()
        .user_agent(concat!("RustCraft/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| Error::Session(e.to_string()))?;
    let body = serde_json::json!({
        "accessToken": session.access_token,
        "selectedProfile": session.uuid,
        "serverId": server_hash,
    });
    let response = http
        .post(session.join_url)
        .json(&body)
        .send()
        .map_err(|e| Error::Session(format!("could not reach Mojang ({e})")))?;
    match response.status().as_u16() {
        200..=299 => Ok(()),
        403 => Err(Error::Session("Mojang refused the session; sign in again in the launcher".into())),
        429 => Err(Error::Session("Mojang is rate limiting joins; try again in a minute".into())),
        code => Err(Error::Session(format!("Mojang's session service answered HTTP {code}"))),
    }
}

/// Connects and logs in; on success the connection is in the play state.
pub fn join(address: &str, profile: &Profile) -> Result<Joined> {
    let (host, port) = split_address(address)?;
    let addr = (host.as_str(), port).to_socket_addrs()?.next().ok_or(Error::BadAddress)?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut writer = PacketWriter::new(stream.try_clone()?);
    let mut reader = PacketReader::new(stream);

    let mut handshake = Vec::new();
    handshake.varint(PROTOCOL_VERSION).string(&host).be(&port.to_be_bytes()).varint(2);
    writer.write(0x00, &handshake)?;
    let mut start = Vec::new();
    start.string(profile.name);
    writer.write(0x00, &start)?;

    loop {
        let (id, body) = reader.read()?;
        let mut b = Buf(&body);
        match id {
            0x00 => return Err(Error::Disconnected(chat_text(&b.string(32767)?))),
            0x01 => {
                let server_id = b.string(20)?;
                let public_key = b.bytes(1024)?;
                let token = b.bytes(1024)?;
                let session = profile.session.as_ref().ok_or(Error::NeedsAccount)?;
                let mut secret = [0u8; 16];
                getrandom::fill(&mut secret).map_err(|e| Error::Session(format!("no random numbers: {e}")))?;
                session_join(session, &server_hash(&server_id, &secret, public_key))?;
                let key = RsaPublicKey::from_public_key_der(public_key)
                    .map_err(|_| Error::Malformed("server sent an unusable public key"))?;
                let encrypt = |data: &[u8]| {
                    key.encrypt(&mut rsa::rand_core::OsRng, Pkcs1v15Encrypt, data)
                        .map_err(|_| Error::Malformed("public key too small"))
                };
                let (secret_enc, token_enc) = (encrypt(&secret)?, encrypt(token)?);
                let mut response = Vec::new();
                response.varint(secret_enc.len() as i32).be(&secret_enc).varint(token_enc.len() as i32).be(&token_enc);
                writer.write(0x01, &response)?;
                reader.enable_encryption(&secret);
                writer.enable_encryption(&secret);
            }
            0x02 => {
                let uuid = b.string(36)?;
                let name = b.string(16)?;
                return Ok(Joined { reader, writer, uuid, name });
            }
            0x03 => {
                let threshold = usize::try_from(b.varint()?).ok();
                reader.set_compression(threshold);
                writer.set_compression(threshold);
            }
            _ => return Err(Error::Malformed("unexpected packet during login")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_hashes_match_known_values() {
        // The examples Minecraft's protocol documentation gives for sha1(name).
        assert_eq!(server_hash("Notch", &[], &[]), "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48");
        assert_eq!(server_hash("jeb_", &[], &[]), "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1");
        assert_eq!(server_hash("simon", &[], &[]), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn addresses_parse_with_default_port() {
        assert_eq!(split_address("play.example.org").unwrap(), ("play.example.org".into(), 25565));
        assert_eq!(split_address("localhost:25570").unwrap(), ("localhost".into(), 25570));
        assert_eq!(split_address("[::1]:25566").unwrap(), ("::1".into(), 25566));
        assert_eq!(split_address("::1").unwrap(), ("::1".into(), 25565));
        assert!(split_address("host:notaport").is_err());
        assert!(split_address(":25565").is_err());
    }
}
