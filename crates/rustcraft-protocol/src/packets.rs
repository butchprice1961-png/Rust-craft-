//! Play-state packets of protocol 47 that RustCraft's engine uses. Other
//! packets (entities, sounds, effects, scoreboards) are skipped.

use glam::{DVec3, IVec3};

use rustcraft_world::block::BlockState;
use rustcraft_world::{Chunk, Section};

use crate::codec::{Buf, Item, Put, chat_text};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl GameMode {
    pub fn from_id(id: u8) -> Self {
        match id & 0x7 {
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => Self::Survival,
        }
    }
}

pub enum Clientbound {
    KeepAlive(i32),
    JoinGame {
        entity_id: i32,
        mode: GameMode,
        dimension: i8,
    },
    Chat {
        text: String,
        action_bar: bool,
    },
    Time {
        world_age: i64,
        time_of_day: i64,
    },
    SpawnPosition(IVec3),
    Health {
        health: f32,
        food: i32,
    },
    Respawn {
        dimension: i32,
        mode: GameMode,
    },
    /// `relative` bits: x, y, z, yaw, pitch.
    Teleport {
        pos: DVec3,
        yaw: f32,
        pitch: f32,
        relative: u8,
    },
    HeldItem(u8),
    /// Full chunk columns, from Chunk Data or Map Chunk Bulk.
    Chunks(Vec<Chunk>),
    /// New contents for some sections of a loaded column.
    Sections {
        x: i32,
        z: i32,
        sections: Vec<(usize, Box<Section>)>,
    },
    Unload(i32, i32),
    Blocks(Vec<(IVec3, BlockState)>),
    GameMode(GameMode),
    Abilities {
        flying: bool,
        may_fly: bool,
    },
    /// Player inventory (window 0) slots, 0–44; hotbar is 36–44.
    Slots(Vec<(usize, Option<Item>)>),
    /// A player, mob, object or experience orb came into view.
    SpawnEntity {
        id: i32,
        kind: EntityKind,
        pos: DVec3,
        yaw: f32,
    },
    /// Relative movement and, if it changed, the new yaw.
    MoveEntity {
        id: i32,
        delta: DVec3,
        yaw: Option<f32>,
    },
    TeleportEntity {
        id: i32,
        pos: DVec3,
        yaw: f32,
    },
    RemoveEntities(Vec<i32>),
    /// Tab-list changes: players added (UUID, name) and removed.
    PlayerList {
        added: Vec<(u128, String)>,
        removed: Vec<u128>,
    },
    Disconnect(String),
    SetCompression(Option<usize>),
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player(u128),
    /// Mob type id (50 creeper … 120 villager).
    Mob(u8),
    /// Object type id (1 boat, 2 item, 60 arrow …).
    Object(u8),
    ExperienceOrb,
}

impl EntityKind {
    /// Bounding box (width, height) in blocks, as in 1.8.
    pub fn size(self) -> (f32, f32) {
        match self {
            EntityKind::Player(_) => (0.6, 1.8),
            EntityKind::Mob(id) => match id {
                50 => (0.6, 1.7),
                52 => (1.4, 0.9),
                53 => (3.6, 10.8),
                55 | 62 => (1.0, 1.0),
                56 => (4.0, 4.0),
                58 => (0.6, 2.9),
                59 => (0.7, 0.5),
                60 => (0.4, 0.3),
                63 => (16.0, 8.0),
                64 => (0.9, 3.5),
                65 => (0.5, 0.9),
                67 => (0.4, 0.3),
                68 => (0.85, 0.85),
                90 => (0.9, 0.9),
                91 | 92 | 96 => (0.9, 1.3),
                93 => (0.4, 0.7),
                94 => (0.95, 0.95),
                95 | 98 => (0.6, 0.8),
                97 => (0.7, 1.9),
                99 => (1.4, 2.9),
                100 => (1.4, 1.6),
                101 => (0.6, 0.7),
                _ => (0.6, 1.95),
            },
            EntityKind::Object(1) => (1.5, 0.6),
            EntityKind::Object(10) => (0.98, 0.7),
            EntityKind::Object(50 | 70) => (0.98, 0.98),
            EntityKind::Object(78) => (0.5, 1.975),
            EntityKind::Object(_) => (0.25, 0.25),
            EntityKind::ExperienceOrb => (0.5, 0.5),
        }
    }
}

/// Entity positions are fixed-point numbers with 5 fraction bits.
fn fixed(b: &mut Buf) -> Result<DVec3> {
    Ok(DVec3::new(b.i32()? as f64, b.i32()? as f64, b.i32()? as f64) / 32.0)
}

fn angle(b: &mut Buf) -> Result<f32> {
    Ok(b.u8()? as f32 * 360.0 / 256.0)
}

/// Decodes one play packet. `sky` says whether the current dimension has
/// sky light, which decides the layout of chunk data.
pub fn decode(id: i32, body: &[u8], sky: bool) -> Result<Clientbound> {
    let mut b = Buf(body);
    Ok(match id {
        0x00 => Clientbound::KeepAlive(b.varint()?),
        0x01 => {
            let entity_id = b.i32()?;
            let mode = GameMode::from_id(b.u8()?);
            Clientbound::JoinGame { entity_id, mode, dimension: b.i8()? }
        }
        0x02 => {
            let text = chat_text(&b.string(32767)?);
            Clientbound::Chat { text, action_bar: b.i8()? == 2 }
        }
        0x03 => Clientbound::Time { world_age: b.i64()?, time_of_day: b.i64()? },
        0x05 => Clientbound::SpawnPosition(b.position()?),
        0x06 => {
            let health = b.f32()?;
            Clientbound::Health { health, food: b.varint()? }
        }
        0x07 => {
            let dimension = b.i32()?;
            let _difficulty = b.u8()?;
            Clientbound::Respawn { dimension, mode: GameMode::from_id(b.u8()?) }
        }
        0x08 => {
            let pos = DVec3::new(b.f64()?, b.f64()?, b.f64()?);
            Clientbound::Teleport { pos, yaw: b.f32()?, pitch: b.f32()?, relative: b.u8()? }
        }
        0x09 => Clientbound::HeldItem(b.u8()?.min(8)),
        0x21 => {
            let (x, z, full, mask) = (b.i32()?, b.i32()?, b.bool()?, b.u16()?);
            let data = b.bytes(crate::conn::MAX_PACKET)?;
            if full && mask == 0 {
                Clientbound::Unload(x, z)
            } else if full {
                Clientbound::Chunks(vec![column(x, z, mask, sky, &mut Buf(data))?])
            } else {
                Clientbound::Sections { x, z, sections: sections(mask, sky, &mut Buf(data))? }
            }
        }
        0x22 => {
            let (cx, cz) = (b.i32()?, b.i32()?);
            let n = b.length(65536)?;
            let mut changes = Vec::with_capacity(n);
            for _ in 0..n {
                let xz = b.u8()?;
                let y = b.u8()?;
                let state = BlockState(b.varint()? as u16);
                changes.push((IVec3::new(cx * 16 + (xz >> 4) as i32, y as i32, cz * 16 + (xz & 15) as i32), state));
            }
            Clientbound::Blocks(changes)
        }
        0x23 => {
            let pos = b.position()?;
            Clientbound::Blocks(vec![(pos, BlockState(b.varint()? as u16))])
        }
        0x26 => {
            let sky = b.bool()?;
            let n = b.length(1024)?;
            let mut heads = Vec::with_capacity(n);
            for _ in 0..n {
                heads.push((b.i32()?, b.i32()?, b.u16()?));
            }
            let chunks =
                heads.into_iter().map(|(x, z, mask)| column(x, z, mask, sky, &mut b)).collect::<Result<_>>()?;
            Clientbound::Chunks(chunks)
        }
        0x2B => match (b.u8()?, b.f32()?) {
            (3, mode) => Clientbound::GameMode(GameMode::from_id(mode as u8)),
            _ => Clientbound::Ignored,
        },
        0x2F => {
            let (window, slot) = (b.i8()?, b.i16()?);
            let item = b.slot()?;
            match (window, usize::try_from(slot)) {
                (0, Ok(slot)) if slot < 45 => Clientbound::Slots(vec![(slot, item)]),
                _ => Clientbound::Ignored,
            }
        }
        0x30 => {
            if b.u8()? != 0 {
                return Ok(Clientbound::Ignored);
            }
            let n = usize::try_from(b.i16()?).map_err(|_| Error::Malformed("negative slot count"))?;
            let mut slots = Vec::with_capacity(n.min(45));
            for i in 0..n {
                let item = b.slot()?;
                if i < 45 {
                    slots.push((i, item));
                }
            }
            Clientbound::Slots(slots)
        }
        0x0C => {
            let id = b.varint()?;
            let uuid = ((b.u64()? as u128) << 64) | b.u64()? as u128;
            let pos = fixed(&mut b)?;
            Clientbound::SpawnEntity { id, kind: EntityKind::Player(uuid), pos, yaw: angle(&mut b)? }
        }
        0x0E => {
            let (id, kind) = (b.varint()?, EntityKind::Object(b.u8()?));
            let pos = fixed(&mut b)?;
            let _pitch = b.u8()?;
            Clientbound::SpawnEntity { id, kind, pos, yaw: angle(&mut b)? }
        }
        0x0F => {
            let (id, kind) = (b.varint()?, EntityKind::Mob(b.u8()?));
            let pos = fixed(&mut b)?;
            Clientbound::SpawnEntity { id, kind, pos, yaw: angle(&mut b)? }
        }
        0x11 => {
            let id = b.varint()?;
            Clientbound::SpawnEntity { id, kind: EntityKind::ExperienceOrb, pos: fixed(&mut b)?, yaw: 0.0 }
        }
        0x13 => {
            let n = b.length(65536)?;
            Clientbound::RemoveEntities((0..n).map(|_| b.varint()).collect::<Result<_>>()?)
        }
        0x15 | 0x17 => {
            let entity = b.varint()?;
            let delta = DVec3::new(b.i8()? as f64, b.i8()? as f64, b.i8()? as f64) / 32.0;
            let yaw = if id == 0x17 { Some(angle(&mut b)?) } else { None };
            Clientbound::MoveEntity { id: entity, delta, yaw }
        }
        0x16 => {
            let id = b.varint()?;
            Clientbound::MoveEntity { id, delta: DVec3::ZERO, yaw: Some(angle(&mut b)?) }
        }
        0x18 => {
            let id = b.varint()?;
            let pos = fixed(&mut b)?;
            Clientbound::TeleportEntity { id, pos, yaw: angle(&mut b)? }
        }
        0x38 => {
            let action = b.varint()?;
            let n = b.length(4096)?;
            let (mut added, mut removed) = (Vec::new(), Vec::new());
            for _ in 0..n {
                let uuid = ((b.u64()? as u128) << 64) | b.u64()? as u128;
                match action {
                    0 => {
                        let name = b.string(16)?;
                        for _ in 0..b.length(64)? {
                            b.string(32767)?;
                            b.string(32767)?;
                            if b.bool()? {
                                b.string(32767)?;
                            }
                        }
                        b.varint()?;
                        b.varint()?;
                        if b.bool()? {
                            b.string(32767)?;
                        }
                        added.push((uuid, name));
                    }
                    1 | 2 => {
                        b.varint()?;
                    }
                    3 => {
                        if b.bool()? {
                            b.string(32767)?;
                        }
                    }
                    _ => removed.push(uuid),
                }
            }
            Clientbound::PlayerList { added, removed }
        }
        0x39 => {
            let flags = b.u8()?;
            Clientbound::Abilities { flying: flags & 2 != 0, may_fly: flags & 4 != 0 }
        }
        0x40 => Clientbound::Disconnect(chat_text(&b.string(32767)?)),
        0x46 => Clientbound::SetCompression(usize::try_from(b.varint()?).ok()),
        _ => Clientbound::Ignored,
    })
}

/// Sections listed in `mask`, in the 1.8 layout: all block states (two
/// bytes each, little endian), then all block light, then all sky light.
fn sections(mask: u16, sky: bool, b: &mut Buf) -> Result<Vec<(usize, Box<Section>)>> {
    let ys: Vec<usize> = (0..16).filter(|y| mask & (1 << y) != 0).collect();
    let mut out: Vec<(usize, Box<Section>)> = ys.iter().map(|&y| (y, Box::new(Section::new(0)))).collect();
    for (_, s) in &mut out {
        let data = b.take(8192)?;
        for (i, pair) in data.chunks_exact(2).enumerate() {
            s.blocks[i] = BlockState(u16::from_le_bytes([pair[0], pair[1]]));
        }
        s.recount();
    }
    for (_, s) in &mut out {
        s.block_light.copy_from_slice(b.take(2048)?);
    }
    if sky {
        for (_, s) in &mut out {
            s.sky_light.copy_from_slice(b.take(2048)?);
        }
    }
    Ok(out)
}

fn column(x: i32, z: i32, mask: u16, sky: bool, b: &mut Buf) -> Result<Chunk> {
    let mut chunk = Chunk::new(x, z);
    chunk.needs_light = false;
    for (y, s) in sections(mask, sky, b)? {
        chunk.sections[y] = Some(s);
    }
    chunk.biomes.copy_from_slice(b.take(256)?);
    Ok(chunk)
}

/// Serverbound play packets as (id, body).
pub mod send {
    use super::*;

    pub fn keep_alive(id: i32) -> (i32, Vec<u8>) {
        (0x00, std::mem::take(Vec::new().varint(id)))
    }

    pub fn chat(message: &str) -> (i32, Vec<u8>) {
        let message: String = message.chars().take(100).collect();
        (0x01, std::mem::take(Vec::new().string(&message)))
    }

    pub fn on_ground(on_ground: bool) -> (i32, Vec<u8>) {
        (0x03, vec![on_ground as u8])
    }

    pub fn position(pos: DVec3, on_ground: bool) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        v.be(&pos.x.to_be_bytes()).be(&pos.y.to_be_bytes()).be(&pos.z.to_be_bytes()).be(&[on_ground as u8]);
        (0x04, v)
    }

    pub fn look(yaw: f32, pitch: f32, on_ground: bool) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        v.be(&yaw.to_be_bytes()).be(&pitch.to_be_bytes()).be(&[on_ground as u8]);
        (0x05, v)
    }

    pub fn position_look(pos: DVec3, yaw: f32, pitch: f32, on_ground: bool) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        v.be(&pos.x.to_be_bytes()).be(&pos.y.to_be_bytes()).be(&pos.z.to_be_bytes());
        v.be(&yaw.to_be_bytes()).be(&pitch.to_be_bytes()).be(&[on_ground as u8]);
        (0x06, v)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Dig {
        Start = 0,
        Cancel = 1,
        Finish = 2,
    }

    pub fn dig(action: Dig, pos: IVec3, face: u8) -> (i32, Vec<u8>) {
        let mut v = vec![action as u8];
        v.position(pos).be(&[face]);
        (0x07, v)
    }

    /// Right click on a block face; `cursor` is where on the face (0–15).
    pub fn place(pos: IVec3, face: u8, held: Option<Item>, cursor: [u8; 3]) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        v.position(pos).be(&[face]).slot(held).be(&cursor);
        (0x08, v)
    }

    /// Tells the server whether the player is flying (the only flag it reads).
    pub fn abilities(flying: bool) -> (i32, Vec<u8>) {
        let mut v = vec![if flying { 2 } else { 0 }];
        v.be(&0.05f32.to_be_bytes()).be(&0.1f32.to_be_bytes());
        (0x13, v)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Action {
        StartSneaking = 0,
        StopSneaking = 1,
        StartSprinting = 3,
        StopSprinting = 4,
    }

    pub fn entity_action(entity_id: i32, action: Action) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        v.varint(entity_id).varint(action as i32).varint(0);
        (0x0B, v)
    }

    pub fn held_item(slot: u8) -> (i32, Vec<u8>) {
        (0x09, (slot as i16).to_be_bytes().to_vec())
    }

    pub fn swing() -> (i32, Vec<u8>) {
        (0x0A, Vec::new())
    }

    /// Creative mode: put `item` into inventory slot `slot` (hotbar: 36–44).
    pub fn creative_slot(slot: i16, item: Option<Item>) -> (i32, Vec<u8>) {
        let mut v = slot.to_be_bytes().to_vec();
        v.slot(item);
        (0x10, v)
    }

    pub fn settings(view_distance: u8) -> (i32, Vec<u8>) {
        let mut v = Vec::new();
        // Locale, view distance, chat enabled, chat colours, all skin parts.
        v.string("en_US").be(&[view_distance, 0, 1, 0x7F]);
        (0x15, v)
    }

    pub fn respawn() -> (i32, Vec<u8>) {
        (0x16, std::mem::take(Vec::new().varint(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column_bytes(mask: u16, sky: bool, biomes: bool, fill: impl Fn(usize, usize) -> u16) -> Vec<u8> {
        let ys: Vec<usize> = (0..16).filter(|y| mask & (1 << y) != 0).collect();
        let mut v = Vec::new();
        for &y in &ys {
            for i in 0..4096 {
                v.extend_from_slice(&fill(y, i).to_le_bytes());
            }
        }
        for _ in &ys {
            v.extend_from_slice(&[0x21; 2048]);
        }
        if sky {
            for _ in &ys {
                v.extend_from_slice(&[0xF0; 2048]);
            }
        }
        if biomes {
            v.extend_from_slice(&[4; 256]);
        }
        v
    }

    #[test]
    fn chunk_data_decodes_into_sections_and_light() {
        // Stone (1:0) in section 0, oak logs (17:0) in section 4.
        let data = column_bytes(0b1_0001, true, true, |y, _| if y == 0 { 1 << 4 } else { 17 << 4 });
        let mut body = Vec::new();
        body.be(&3i32.to_be_bytes()).be(&(-2i32).to_be_bytes()).be(&[1]).be(&0b1_0001u16.to_be_bytes());
        body.varint(data.len() as i32).be(&data);
        let Clientbound::Chunks(chunks) = decode(0x21, &body, true).unwrap() else { panic!("not a chunk") };
        let c = &chunks[0];
        assert_eq!((c.x, c.z, c.needs_light), (3, -2, false));
        assert_eq!(c.block(0, 0, 0), BlockState::new(1, 0));
        assert_eq!(c.block(15, 70, 15), BlockState::new(17, 0));
        assert_eq!(c.block(0, 20, 0), BlockState::AIR);
        let s = c.sections[0].as_ref().unwrap();
        assert_eq!((s.block_light(0), s.block_light(1), s.sky_light(0), s.sky_light(1)), (1, 2, 0, 15));
        assert_eq!(c.biomes[0], 4);

        // The same column without its last byte is an error, not a panic.
        body.truncate(body.len() - 1);
        assert!(decode(0x21, &body, true).is_err());
    }

    #[test]
    fn bulk_and_partial_chunks_and_block_changes() {
        let a = column_bytes(0b1, false, true, |_, _| 2 << 4);
        let b = column_bytes(0b10, false, true, |_, _| 3 << 4);
        let mut body = vec![0];
        body.varint(2);
        body.be(&0i32.to_be_bytes()).be(&0i32.to_be_bytes()).be(&1u16.to_be_bytes());
        body.be(&1i32.to_be_bytes()).be(&0i32.to_be_bytes()).be(&2u16.to_be_bytes());
        body.be(&a).be(&b);
        let Clientbound::Chunks(chunks) = decode(0x26, &body, true).unwrap() else { panic!() };
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].block(0, 16, 0), BlockState::new(3, 0));
        // No sky light in this dimension: sections start dark.
        assert_eq!(chunks[0].sections[0].as_ref().unwrap().sky_light(0), 0);

        let part = column_bytes(0b100, true, false, |_, _| 1 << 4);
        let mut body = Vec::new();
        body.be(&0i32.to_be_bytes()).be(&0i32.to_be_bytes()).be(&[0]).be(&4u16.to_be_bytes());
        body.varint(part.len() as i32).be(&part);
        let Clientbound::Sections { sections, .. } = decode(0x21, &body, true).unwrap() else { panic!() };
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, 2);

        let mut body = Vec::new();
        body.be(&(-1i32).to_be_bytes()).be(&2i32.to_be_bytes()).varint(1).be(&[0x3F, 70]).varint(4 << 4);
        let Clientbound::Blocks(changes) = decode(0x22, &body, true).unwrap() else { panic!() };
        assert_eq!(changes, vec![(IVec3::new(-16 + 3, 70, 32 + 15), BlockState::new(4, 0))]);

        let mut unload = Vec::new();
        unload.be(&5i32.to_be_bytes()).be(&6i32.to_be_bytes()).be(&[1]).be(&0u16.to_be_bytes()).varint(0);
        assert!(matches!(decode(0x21, &unload, true).unwrap(), Clientbound::Unload(5, 6)));
    }

    #[test]
    fn entities_and_the_player_list() {
        let uuid: u128 = 0x069a79f4_44e9_4726_a5be_fca90e38aaf5;
        let mut body = Vec::new();
        body.varint(42).be(&uuid.to_be_bytes());
        body.be(&(32 * 10i32).to_be_bytes()).be(&(32 * 64i32 + 16).to_be_bytes()).be(&(-32 * 5i32).to_be_bytes());
        body.be(&[64, 0]).be(&0i16.to_be_bytes()).be(&[0x7f]); // yaw 90°, pitch, item, metadata end
        let Clientbound::SpawnEntity { id, kind, pos, yaw } = decode(0x0C, &body, true).unwrap() else { panic!() };
        assert_eq!((id, kind, pos, yaw), (42, EntityKind::Player(uuid), DVec3::new(10.0, 64.5, -5.0), 90.0));

        let mut body = Vec::new();
        body.varint(42).be(&[16, (-32i8) as u8, 0]).be(&[128, 0]).be(&[1]);
        let Clientbound::MoveEntity { delta, yaw, .. } = decode(0x17, &body, true).unwrap() else { panic!() };
        assert_eq!((delta, yaw), (DVec3::new(0.5, -1.0, 0.0), Some(180.0)));
        let Clientbound::MoveEntity { yaw, .. } = decode(0x15, &body[..5], true).unwrap() else { panic!() };
        assert_eq!(yaw, None);

        let mut body = Vec::new();
        body.varint(2).varint(42).varint(7);
        assert!(matches!(decode(0x13, &body, true).unwrap(), Clientbound::RemoveEntities(ids) if ids == vec![42, 7]));

        let mut body = Vec::new();
        body.varint(0).varint(1).be(&uuid.to_be_bytes()).string("Notch");
        body.varint(1).string("textures").string("e30=").be(&[1]).string("sig");
        body.varint(0).varint(35).be(&[0]);
        let Clientbound::PlayerList { added, removed } = decode(0x38, &body, true).unwrap() else { panic!() };
        assert_eq!((added, removed.len()), (vec![(uuid, "Notch".to_string())], 0));
        let mut body = Vec::new();
        body.varint(4).varint(1).be(&uuid.to_be_bytes());
        let Clientbound::PlayerList { removed, .. } = decode(0x38, &body, true).unwrap() else { panic!() };
        assert_eq!(removed, vec![uuid]);
        assert_eq!(EntityKind::Mob(91).size(), (0.9, 1.3));
    }

    #[test]
    fn teleports_inventories_and_serverbound_packets() {
        let mut body = Vec::new();
        for v in [1.5f64, 64.0, -3.25] {
            body.be(&v.to_be_bytes());
        }
        body.be(&90f32.to_be_bytes()).be(&10f32.to_be_bytes()).be(&[0b11000]);
        let Clientbound::Teleport { pos, yaw, relative, .. } = decode(0x08, &body, true).unwrap() else { panic!() };
        assert_eq!((pos, yaw, relative), (DVec3::new(1.5, 64.0, -3.25), 90.0, 0b11000));

        let mut body = vec![0];
        body.be(&45i16.to_be_bytes());
        for i in 0..45 {
            body.slot((i == 36).then_some(Item { id: 1, count: 64, damage: 0 }));
        }
        let Clientbound::Slots(slots) = decode(0x30, &body, true).unwrap() else { panic!() };
        assert_eq!(slots.len(), 45);
        assert_eq!(slots[36].1.unwrap().id, 1);

        let (id, body) = send::dig(send::Dig::Start, IVec3::new(1, 2, 3), 1);
        assert_eq!(id, 0x07);
        assert_eq!(body.len(), 10);
        let (id, body) = send::chat(&"x".repeat(300));
        assert_eq!((id, Buf(&body).string(100).unwrap().len()), (0x01, 100));
    }
}
