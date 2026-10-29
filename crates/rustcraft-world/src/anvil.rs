//! Reading and writing 1.8 worlds in the Anvil format: `region/r.X.Z.mca`
//! files of zlib-compressed chunk NBT, and the gzip-compressed `level.dat`.
//!
//! Unknown chunk data (entities, tile entities, ticks) is kept and written
//! back unchanged. Region files are replaced atomically, and the first
//! write to an existing region file in a session keeps a copy in
//! `rustcraft-backups/`, so a bug or crash cannot destroy a world.

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::{GzDecoder, ZlibDecoder};
use flate2::write::{GzEncoder, ZlibEncoder};

use rustcraft_core::fs::{unix_time, write_atomic};
use rustcraft_core::nbt::{self, Compound, CompoundExt, Tag};

use crate::block::BlockState;
use crate::chunk::{Chunk, Section};

#[derive(Debug, thiserror::Error)]
pub enum AnvilError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("corrupt chunk data: {0}")]
    Corrupt(String),
}

impl From<nbt::NbtError> for AnvilError {
    fn from(e: nbt::NbtError) -> Self {
        AnvilError::Corrupt(e.to_string())
    }
}

/// Uncompressed chunk NBT is capped to keep a hostile file from exhausting memory.
const MAX_CHUNK_BYTES: u64 = 16 << 20;

fn region_path(dir: &Path, cx: i32, cz: i32) -> PathBuf {
    dir.join("region").join(format!("r.{}.{}.mca", cx >> 5, cz >> 5))
}

fn slot(cx: i32, cz: i32) -> usize {
    ((cx & 31) + (cz & 31) * 32) as usize
}

/// Raw (still compressed) chunk payloads of one region file.
fn read_region(path: &Path) -> io::Result<HashMap<usize, (Vec<u8>, u32)>> {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let mut out = HashMap::new();
    if data.len() < 8192 {
        return Ok(out);
    }
    for i in 0..1024 {
        let loc = u32::from_be_bytes(data[i * 4..i * 4 + 4].try_into().unwrap());
        let (offset, sectors) = ((loc >> 8) as usize * 4096, (loc & 0xff) as usize);
        if offset < 8192 || sectors == 0 || offset + 5 > data.len() {
            continue;
        }
        let len = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        if len == 0 || offset + 4 + len > data.len() || len > sectors * 4096 {
            tracing::warn!("{}: chunk slot {i} is damaged; skipping it", path.display());
            continue;
        }
        let stamp = u32::from_be_bytes(data[4096 + i * 4..4096 + i * 4 + 4].try_into().unwrap());
        out.insert(i, (data[offset + 4..offset + 4 + len].to_vec(), stamp));
    }
    Ok(out)
}

fn inflate(payload: &[u8]) -> Result<Vec<u8>, AnvilError> {
    let (kind, body) = payload.split_first().ok_or_else(|| AnvilError::Corrupt("empty chunk".into()))?;
    let mut out = Vec::new();
    let read = match kind {
        1 => GzDecoder::new(body).take(MAX_CHUNK_BYTES).read_to_end(&mut out),
        2 => ZlibDecoder::new(body).take(MAX_CHUNK_BYTES).read_to_end(&mut out),
        other => return Err(AnvilError::Corrupt(format!("unknown compression {other}"))),
    };
    read.map_err(|e| AnvilError::Corrupt(e.to_string()))?;
    Ok(out)
}

/// Loads one chunk, or `None` if it has never been generated.
pub fn load_chunk(world_dir: &Path, cx: i32, cz: i32) -> Result<Option<Chunk>, AnvilError> {
    let region = read_region(&region_path(world_dir, cx, cz))?;
    let Some((payload, _)) = region.get(&slot(cx, cz)) else { return Ok(None) };
    let (_, root, _) = nbt::read(&inflate(payload)?)?;
    let level = root.compound("Level").ok_or_else(|| AnvilError::Corrupt("no Level tag".into()))?;
    Ok(Some(chunk_from_nbt(level, cx, cz)?))
}

fn nibble_array(bytes: Option<&[u8]>) -> [u8; 2048] {
    let mut out = [0u8; 2048];
    if let Some(b) = bytes
        && b.len() == 2048
    {
        out.copy_from_slice(b);
    }
    out
}

fn chunk_from_nbt(level: &Compound, cx: i32, cz: i32) -> Result<Chunk, AnvilError> {
    let mut chunk = Chunk::new(cx, cz);
    if let Some(b) = level.bytes("Biomes")
        && b.len() == 256
    {
        chunk.biomes.copy_from_slice(b);
    }
    let mut has_light = level.int("LightPopulated").unwrap_or(0) != 0;
    for tag in level.list("Sections").unwrap_or_default() {
        let Tag::Compound(s) = tag else { continue };
        let y = s.int("Y").unwrap_or(-1);
        if !(0..16).contains(&y) {
            continue;
        }
        let blocks = s
            .bytes("Blocks")
            .filter(|b| b.len() == 4096)
            .ok_or_else(|| AnvilError::Corrupt("section without Blocks".into()))?;
        let data = nibble_array(s.bytes("Data"));
        let add = s.bytes("Add").filter(|a| a.len() == 2048);
        let mut section = Section::new(0);
        for i in 0..4096 {
            let shift = (i & 1) << 2;
            let high = add.map_or(0, |a| ((a[i >> 1] >> shift) & 15) as u16);
            let meta = (data[i >> 1] >> shift) & 15;
            section.blocks[i] = BlockState::new(blocks[i] as u16 | (high << 8), meta);
        }
        section.recount();
        match (s.bytes("BlockLight"), s.bytes("SkyLight")) {
            (Some(bl), Some(sl)) if bl.len() == 2048 && sl.len() == 2048 => {
                section.block_light.copy_from_slice(bl);
                section.sky_light.copy_from_slice(sl);
            }
            _ => has_light = false,
        }
        chunk.sections[y as usize] = Some(Box::new(section));
    }
    chunk.needs_light = !has_light;
    let mut extra = level.clone();
    for key in ["Sections", "Biomes", "HeightMap", "xPos", "zPos", "LastUpdate", "LightPopulated"] {
        extra.remove(key);
    }
    chunk.extra = Some(Box::new(extra));
    Ok(chunk)
}

fn chunk_to_nbt(chunk: &Chunk) -> Compound {
    let mut level = chunk.extra.as_deref().cloned().unwrap_or_default();
    level.insert("xPos".into(), Tag::Int(chunk.x));
    level.insert("zPos".into(), Tag::Int(chunk.z));
    level.insert("LastUpdate".into(), Tag::Long(0));
    level.insert("LightPopulated".into(), Tag::Byte(1));
    level.entry("TerrainPopulated".into()).or_insert(Tag::Byte(1));
    level.entry("V".into()).or_insert(Tag::Byte(1));
    level.entry("InhabitedTime".into()).or_insert(Tag::Long(0));
    level.entry("Entities".into()).or_insert(Tag::List(Vec::new()));
    level.entry("TileEntities".into()).or_insert(Tag::List(Vec::new()));
    level.insert("Biomes".into(), Tag::ByteArray(chunk.biomes.to_vec()));
    let heights: Vec<i32> = (0..256).map(|i| chunk.top(i % 16, i / 16) + 1).collect();
    level.insert("HeightMap".into(), Tag::IntArray(heights));
    let mut sections = Vec::new();
    for (y, section) in chunk.sections.iter().enumerate() {
        let Some(s) = section else { continue };
        if s.is_empty() {
            continue;
        }
        let mut blocks = vec![0u8; 4096];
        let mut data = vec![0u8; 2048];
        let mut add = vec![0u8; 2048];
        let mut needs_add = false;
        for i in 0..4096 {
            let state = s.blocks[i];
            let id = state.id();
            let shift = (i & 1) << 2;
            blocks[i] = (id & 0xff) as u8;
            data[i >> 1] |= state.meta() << shift;
            if id > 0xff {
                needs_add = true;
                add[i >> 1] |= (((id >> 8) & 15) as u8) << shift;
            }
        }
        let mut c = Compound::new();
        c.insert("Y".into(), Tag::Byte(y as i8));
        c.insert("Blocks".into(), Tag::ByteArray(blocks));
        c.insert("Data".into(), Tag::ByteArray(data));
        if needs_add {
            c.insert("Add".into(), Tag::ByteArray(add));
        }
        c.insert("BlockLight".into(), Tag::ByteArray(s.block_light.to_vec()));
        c.insert("SkyLight".into(), Tag::ByteArray(s.sky_light.to_vec()));
        sections.push(Tag::Compound(c));
    }
    level.insert("Sections".into(), Tag::List(sections));
    let mut root = Compound::new();
    root.insert("Level".into(), Tag::Compound(level));
    root
}

/// Writes chunks into their region files. Each region file is rewritten
/// atomically; `backed_up` tracks which files already have a session backup.
pub fn save_chunks<'a>(
    world_dir: &Path,
    chunks: impl IntoIterator<Item = &'a Chunk>,
    backed_up: &mut HashSet<PathBuf>,
) -> Result<(), AnvilError> {
    let mut by_region: HashMap<PathBuf, Vec<&Chunk>> = HashMap::new();
    for c in chunks {
        by_region.entry(region_path(world_dir, c.x, c.z)).or_default().push(c);
    }
    for (path, chunks) in by_region {
        if path.exists() && backed_up.insert(path.clone()) {
            let backups = world_dir.join("rustcraft-backups");
            std::fs::create_dir_all(&backups)?;
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            std::fs::copy(&path, backups.join(format!("{name}.{}", unix_time())))?;
        }
        let mut region = read_region(&path)?;
        for c in chunks {
            let raw = nbt::write("", &chunk_to_nbt(c));
            let mut z = ZlibEncoder::new(vec![2u8], Compression::default());
            z.write_all(&raw)?;
            region.insert(slot(c.x, c.z), (z.finish()?, unix_time() as u32));
        }
        write_atomic(&path, &encode_region(&region))?;
    }
    Ok(())
}

fn encode_region(chunks: &HashMap<usize, (Vec<u8>, u32)>) -> Vec<u8> {
    let mut out = vec![0u8; 8192];
    let mut slots: Vec<_> = chunks.iter().collect();
    slots.sort_by_key(|(i, _)| **i);
    for (&i, (payload, stamp)) in slots {
        let sector = out.len() / 4096;
        let len = payload.len() + 4;
        let sectors = len.div_ceil(4096);
        if sectors > 255 {
            tracing::warn!("chunk slot {i} exceeds the region format's 1 MiB limit; not saved");
            continue;
        }
        out[i * 4..i * 4 + 4].copy_from_slice(&(((sector as u32) << 8) | sectors as u32).to_be_bytes());
        out[4096 + i * 4..4096 + i * 4 + 4].copy_from_slice(&stamp.to_be_bytes());
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out.resize((sector + sectors) * 4096, 0);
    }
    out
}

/// The parts of `level.dat` RustCraft uses. Other fields are preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelInfo {
    pub name: String,
    pub seed: i64,
    pub spawn: [i32; 3],
    pub time: i64,
    pub day_time: i64,
    pub game_type: i32,
    pub player: Option<PlayerInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfo {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
}

pub fn read_level(world_dir: &Path) -> Result<(LevelInfo, Compound), AnvilError> {
    let mut raw = Vec::new();
    GzDecoder::new(std::fs::File::open(world_dir.join("level.dat"))?).take(MAX_CHUNK_BYTES).read_to_end(&mut raw)?;
    let (_, root, _) = nbt::read(&raw)?;
    let data = root.compound("Data").ok_or_else(|| AnvilError::Corrupt("level.dat has no Data".into()))?;
    let player = data.compound("Player").and_then(|p| {
        let pos = p.list("Pos")?;
        let rot = p.list("Rotation")?;
        Some(PlayerInfo {
            pos: [pos.first()?.as_f64()?, pos.get(1)?.as_f64()?, pos.get(2)?.as_f64()?],
            yaw: rot.first()?.as_f64()? as f32,
            pitch: rot.get(1)?.as_f64()? as f32,
            flying: p.compound("abilities").and_then(|a| a.int("flying")).unwrap_or(0) != 0,
        })
    });
    let info = LevelInfo {
        name: data.string("LevelName").unwrap_or("World").into(),
        seed: data.int("RandomSeed").unwrap_or(0),
        spawn: [
            data.int("SpawnX").unwrap_or(0) as i32,
            data.int("SpawnY").unwrap_or(64) as i32,
            data.int("SpawnZ").unwrap_or(0) as i32,
        ],
        time: data.int("Time").unwrap_or(0),
        day_time: data.int("DayTime").unwrap_or(0),
        game_type: data.int("GameType").unwrap_or(0) as i32,
        player,
    };
    Ok((info, root))
}

/// Writes `level.dat`, merging into `previous` (an existing root) so fields
/// RustCraft does not model survive.
pub fn write_level(world_dir: &Path, info: &LevelInfo, previous: Option<&Compound>) -> Result<(), AnvilError> {
    let mut root = previous.cloned().unwrap_or_default();
    let mut data = root.compound("Data").cloned().unwrap_or_default();
    data.insert("LevelName".into(), Tag::String(info.name.clone()));
    data.insert("RandomSeed".into(), Tag::Long(info.seed));
    data.insert("SpawnX".into(), Tag::Int(info.spawn[0]));
    data.insert("SpawnY".into(), Tag::Int(info.spawn[1]));
    data.insert("SpawnZ".into(), Tag::Int(info.spawn[2]));
    data.insert("Time".into(), Tag::Long(info.time));
    data.insert("DayTime".into(), Tag::Long(info.day_time));
    data.insert("GameType".into(), Tag::Int(info.game_type));
    data.insert("LastPlayed".into(), Tag::Long(unix_time() as i64 * 1000));
    data.entry("version".into()).or_insert(Tag::Int(19133));
    data.entry("generatorName".into()).or_insert(Tag::String("default".into()));
    data.entry("initialized".into()).or_insert(Tag::Byte(1));
    if let Some(p) = &info.player {
        let mut player = data.compound("Player").cloned().unwrap_or_default();
        player.insert("Pos".into(), Tag::List(p.pos.iter().map(|&v| Tag::Double(v)).collect()));
        player.insert("Rotation".into(), Tag::List(vec![Tag::Float(p.yaw), Tag::Float(p.pitch)]));
        let mut abilities = player.compound("abilities").cloned().unwrap_or_default();
        abilities.insert("flying".into(), Tag::Byte(p.flying as i8));
        abilities.insert("mayfly".into(), Tag::Byte((info.game_type == 1) as i8));
        abilities.insert("instabuild".into(), Tag::Byte((info.game_type == 1) as i8));
        player.insert("abilities".into(), Tag::Compound(abilities));
        data.insert("Player".into(), Tag::Compound(player));
    }
    root.insert("Data".into(), Tag::Compound(data));
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&nbt::write("", &root))?;
    write_atomic(&world_dir.join("level.dat"), &gz.finish()?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chunk(cx: i32, cz: i32) -> Chunk {
        let mut c = Chunk::new(cx, cz);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block(x, 0, z, BlockState::new(7, 0));
                c.set_block(x, 1, z, BlockState::new(35, (x as u8) & 15));
            }
        }
        c.set_block(3, 100, 4, BlockState::new(54, 2));
        c.biomes[5] = 4;
        let mut extra = Compound::new();
        extra.insert(
            "TileEntities".into(),
            Tag::List(vec![Tag::Compound(Compound::from([("id".to_string(), Tag::String("Chest".into()))]))]),
        );
        c.extra = Some(Box::new(extra));
        c
    }

    #[test]
    fn chunks_roundtrip_through_region_files() {
        let dir = tempfile::tempdir().unwrap();
        let chunks = [sample_chunk(0, 0), sample_chunk(-1, 5), sample_chunk(31, 31)];
        let mut backed_up = HashSet::new();
        save_chunks(dir.path(), chunks.iter(), &mut backed_up).unwrap();
        for c in &chunks {
            let loaded = load_chunk(dir.path(), c.x, c.z).unwrap().unwrap();
            assert_eq!(loaded.block(3, 100, 4), BlockState::new(54, 2));
            assert_eq!(loaded.block(9, 1, 0), BlockState::new(35, 9));
            assert_eq!(loaded.biomes[5], 4);
            assert!(!loaded.needs_light);
            // Tile entities survive a load/save cycle.
            assert_eq!(loaded.extra.as_ref().unwrap().list("TileEntities").unwrap().len(), 1);
        }
        assert!(load_chunk(dir.path(), 1, 0).unwrap().is_none());

        // Saving into an existing region keeps one backup and the other chunks.
        let mut changed = sample_chunk(0, 0);
        changed.set_block(0, 50, 0, BlockState::new(1, 0));
        save_chunks(dir.path(), [&changed], &mut backed_up).unwrap();
        assert_eq!(load_chunk(dir.path(), 0, 0).unwrap().unwrap().block(0, 50, 0), BlockState::new(1, 0));
        assert!(load_chunk(dir.path(), 31, 31).unwrap().is_some());
        assert_eq!(std::fs::read_dir(dir.path().join("rustcraft-backups")).unwrap().count(), 1);
    }

    #[test]
    fn damaged_region_data_is_reported_not_panicking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("region")).unwrap();
        // Header points chunk (0,0) at sector 2 with garbage payload.
        let mut bad = vec![0u8; 8192 + 4096];
        bad[0..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
        bad[8192..8196].copy_from_slice(&100u32.to_be_bytes());
        bad[8196] = 2;
        std::fs::write(dir.path().join("region/r.0.0.mca"), &bad).unwrap();
        assert!(load_chunk(dir.path(), 0, 0).is_err());
        // Truncated file.
        std::fs::write(dir.path().join("region/r.0.0.mca"), &bad[..100]).unwrap();
        assert!(load_chunk(dir.path(), 0, 0).unwrap().is_none());
    }

    #[test]
    fn level_dat_roundtrip_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let info = LevelInfo {
            name: "Test".into(),
            seed: -42,
            spawn: [10, 70, -3],
            time: 1000,
            day_time: 6000,
            game_type: 1,
            player: Some(PlayerInfo { pos: [1.5, 65.0, -2.5], yaw: 90.0, pitch: 10.0, flying: true }),
        };
        let mut prev = Compound::new();
        prev.insert(
            "Data".into(),
            Tag::Compound(Compound::from([("GameRules".to_string(), Tag::Compound(Compound::new()))])),
        );
        write_level(dir.path(), &info, Some(&prev)).unwrap();
        let (read, root) = read_level(dir.path()).unwrap();
        assert_eq!(read, info);
        assert!(root.compound("Data").unwrap().compound("GameRules").is_some());
    }
}
