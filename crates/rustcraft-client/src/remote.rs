//! A multiplayer session. A network thread logs in, answers keep-alives
//! and decodes packets; the game thread applies them and sends what the
//! player does.

use std::collections::VecDeque;
use std::net::{Shutdown, TcpStream};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use glam::DVec3;

use rustcraft_protocol::conn::PacketWriter;
use rustcraft_protocol::login::SESSION_JOIN;
use rustcraft_protocol::packets::decode;
use rustcraft_protocol::{Clientbound, GameMode, Item, Profile, Session, join, send};
use rustcraft_world::block::{BlockState, props};

type Writer = Arc<Mutex<PacketWriter<TcpStream>>>;

enum Net {
    Joined(Writer),
    Packet(Clientbound),
    Closed(String),
}

pub enum Status {
    Connecting,
    Playing,
    /// Why the session ended, for the disconnect screen.
    Closed(String),
}

/// A block being dug in survival mode.
pub struct Digging {
    pub pos: glam::IVec3,
    pub face: u8,
    pub elapsed: u32,
    pub total: u32,
}

pub struct Remote {
    pub address: String,
    pub status: Status,
    events: Receiver<Net>,
    writer: Option<Writer>,
    pub entity_id: i32,
    pub mode: GameMode,
    /// Player inventory slots 0–44 as the server last sent them.
    pub inventory: [Option<Item>; 45],
    pub chat: VecDeque<(Instant, String)>,
    pub action_bar: Option<(Instant, String)>,
    pub health: f32,
    pub food: i32,
    /// The server has placed the player (first teleport received).
    pub positioned: bool,
    pub digging: Option<Digging>,
    pub last_pos: DVec3,
    pub last_look: (f32, f32),
    pub since_position: u32,
    pub sent_flying: bool,
    pub sent_sneaking: bool,
    pub sent_sprinting: bool,
}

impl Remote {
    /// Starts connecting in the background; `session` is (access token,
    /// undashed UUID) for online-mode servers.
    pub fn connect(address: String, name: String, session: Option<(String, String)>) -> Remote {
        let (tx, events) = channel();
        let target = address.clone();
        let spawned = std::thread::Builder::new().name("network".into()).spawn(move || {
            let profile = Profile {
                name: &name,
                session: session.as_ref().map(|(token, uuid)| Session {
                    access_token: token,
                    uuid,
                    join_url: SESSION_JOIN,
                }),
            };
            let joined = match join(&target, &profile) {
                Ok(j) => j,
                Err(e) => {
                    let _ = tx.send(Net::Closed(format!("Could not join {target}: {e}")));
                    return;
                }
            };
            tracing::info!("joined {target} as {} ({})", joined.name, joined.uuid);
            let writer = Arc::new(Mutex::new(joined.writer));
            let mut reader = joined.reader;
            let _ = tx.send(Net::Joined(writer.clone()));
            let mut sky = true;
            loop {
                let closed = |reason: String| {
                    let _ = tx.send(Net::Closed(reason));
                };
                let (id, body) = match reader.read() {
                    Ok(p) => p,
                    Err(e) => return closed(format!("Connection lost: {e}")),
                };
                let packet = match decode(id, &body, sky) {
                    Ok(p) => p,
                    Err(e) => return closed(format!("{e} (packet 0x{id:02X})")),
                };
                match &packet {
                    // Answered here, so a busy or hidden game window cannot time out.
                    Clientbound::KeepAlive(n) => {
                        let (id, body) = send::keep_alive(*n);
                        if let Err(e) = writer.lock().unwrap().write(id, &body) {
                            return closed(format!("Connection lost: {e}"));
                        }
                        continue;
                    }
                    Clientbound::SetCompression(t) => {
                        reader.set_compression(*t);
                        writer.lock().unwrap().set_compression(*t);
                        continue;
                    }
                    Clientbound::Disconnect(reason) => return closed(format!("Disconnected: {reason}")),
                    Clientbound::Ignored => continue,
                    Clientbound::JoinGame { dimension, .. } => sky = *dimension == 0,
                    Clientbound::Respawn { dimension, .. } => sky = *dimension == 0,
                    _ => {}
                }
                if tx.send(Net::Packet(packet)).is_err() {
                    return;
                }
            }
        });
        let status = match spawned {
            Ok(_) => Status::Connecting,
            Err(e) => Status::Closed(format!("Could not start networking: {e}")),
        };
        Remote {
            address,
            status,
            events,
            writer: None,
            entity_id: 0,
            mode: GameMode::Survival,
            inventory: [None; 45],
            chat: VecDeque::new(),
            action_bar: None,
            health: 20.0,
            food: 20,
            positioned: false,
            digging: None,
            last_pos: DVec3::ZERO,
            last_look: (0.0, 0.0),
            since_position: 0,
            sent_flying: false,
            sent_sneaking: false,
            sent_sprinting: false,
        }
    }

    /// Packets received since the last call.
    pub fn poll(&mut self) -> Vec<Clientbound> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Net::Joined(w) => {
                    self.writer = Some(w);
                    self.status = Status::Playing;
                }
                Net::Packet(p) => out.push(p),
                Net::Closed(reason) => self.close(reason),
            }
        }
        out
    }

    fn close(&mut self, reason: String) {
        if let Some(w) = self.writer.take() {
            let _ = w.lock().unwrap().get_ref().shutdown(Shutdown::Both);
        }
        if !matches!(self.status, Status::Closed(_)) {
            tracing::info!("{reason}");
            self.status = Status::Closed(reason);
        }
    }

    pub fn send(&mut self, (id, body): (i32, Vec<u8>)) {
        let Some(w) = &self.writer else { return };
        let result = w.lock().unwrap().write(id, &body);
        if let Err(e) = result {
            self.close(format!("Connection lost: {e}"));
        }
    }

    /// Leaves the server.
    pub fn disconnect(&mut self) {
        self.close("Disconnected".into());
    }

    pub fn say(&mut self, line: String) {
        self.chat.push_back((Instant::now(), line));
        while self.chat.len() > 100 {
            self.chat.pop_front();
        }
    }

    pub fn creative(&self) -> bool {
        self.mode == GameMode::Creative
    }

    /// The hotbar as block states, for survival and adventure mode.
    pub fn hotbar(&self) -> [BlockState; 9] {
        std::array::from_fn(|i| self.inventory[36 + i].map_or(BlockState::AIR, block_of))
    }

    pub fn dig_progress(&self) -> Option<f32> {
        self.digging.as_ref().filter(|d| d.total > 0).map(|d| d.elapsed as f32 / d.total as f32)
    }
}

/// The block an item places, if it is a plain block item.
pub fn block_of(item: Item) -> BlockState {
    match u16::try_from(item.id) {
        Ok(id @ 1..=255) if props(id).name != "unknown" => BlockState::new(id, (item.damage & 15) as u8),
        _ => BlockState::AIR,
    }
}

/// The item for a block in the creative hotbar. Orientation bits in the
/// metadata are dropped; the server orients the block when it is placed.
pub fn item_of(state: BlockState) -> Item {
    let damage = match state.id() {
        17 | 162 => state.meta() & 3,
        44 | 126 | 182 => state.meta() & 7,
        23
        | 50
        | 53
        | 54
        | 61
        | 62
        | 65
        | 67
        | 75
        | 76
        | 86
        | 91
        | 108
        | 109
        | 114
        | 128
        | 130
        | 134..=136
        | 146
        | 156
        | 158
        | 163
        | 164
        | 180 => 0,
        _ => state.meta(),
    };
    Item { id: state.id() as i16, count: 1, damage: damage as i16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_and_blocks_convert() {
        assert_eq!(block_of(Item { id: 17, count: 3, damage: 2 }), BlockState::new(17, 2));
        assert_eq!(block_of(Item { id: 276, count: 1, damage: 0 }), BlockState::AIR, "a sword places nothing");
        assert_eq!(item_of(BlockState::new(50, 5)), Item { id: 50, count: 1, damage: 0 });
        assert_eq!(item_of(BlockState::new(17, 4 | 1)).damage, 1);
        assert_eq!(item_of(BlockState::new(35, 14)).damage, 14);
    }
}
