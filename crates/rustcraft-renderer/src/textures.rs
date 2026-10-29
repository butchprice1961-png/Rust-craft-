//! Block textures: loaded from resource packs (directories or zips, such as
//! the user's own Minecraft client jar), with RustCraft's built-in,
//! procedurally drawn art for anything missing. No Mojang art ships with
//! RustCraft.

use std::collections::{BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

use rustcraft_world::block::{self, BlockState, COLORS, Face};

/// A place resource-pack files are read from.
pub enum Source {
    Dir(PathBuf),
    Zip(PathBuf),
}

impl Source {
    /// Reads `assets/minecraft/<rel>`.
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        let rel = format!("assets/minecraft/{rel}");
        match self {
            Source::Dir(dir) => std::fs::read(dir.join(&rel)).ok(),
            Source::Zip(path) => {
                let mut archive = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
                let mut file = archive.by_name(&rel).ok()?;
                // Textures are small; cap reads so a hostile pack cannot exhaust memory.
                let mut out = Vec::new();
                (&mut file).take(8 << 20).read_to_end(&mut out).ok()?;
                Some(out)
            }
        }
    }

    pub fn from_path(path: &Path) -> Source {
        if path.is_dir() { Source::Dir(path.into()) } else { Source::Zip(path.into()) }
    }
}

#[derive(Deserialize)]
struct McMeta {
    animation: Option<AnimationMeta>,
}

#[derive(Deserialize)]
struct AnimationMeta {
    #[serde(default = "one")]
    frametime: u32,
    #[serde(default)]
    frames: Vec<FrameRef>,
}

fn one() -> u32 {
    1
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FrameRef {
    Index(u32),
    Timed { index: u32, time: u32 },
}

/// An animated layer: frames (RGBA) and the (frame, ticks) sequence.
pub struct Animation {
    pub layer: u32,
    pub frames: Vec<Vec<u8>>,
    pub sequence: Vec<(usize, u32)>,
}

impl Animation {
    /// Frame index shown at `tick`.
    pub fn frame_at(&self, tick: u64) -> usize {
        let total: u64 = self.sequence.iter().map(|(_, t)| *t as u64).sum::<u64>().max(1);
        let mut t = tick % total;
        for (frame, ticks) in &self.sequence {
            if t < *ticks as u64 {
                return *frame;
            }
            t -= *ticks as u64;
        }
        0
    }
}

pub type Lookup = Arc<HashMap<String, u32>>;

/// Decoded block textures, all scaled to one tile size, ready for upload
/// as a texture array.
pub struct TextureData {
    pub size: u32,
    pub names: Vec<String>,
    pub layers: Vec<Vec<u8>>,
    pub animations: Vec<Animation>,
    pub lookup: Lookup,
    pub grass_colormap: Option<Vec<u8>>,
    pub foliage_colormap: Option<Vec<u8>>,
    /// How many textures came from a resource pack rather than built-in art.
    pub from_packs: usize,
}

/// Every texture name the block set can ask for.
pub fn required_names() -> BTreeSet<&'static str> {
    let mut names = BTreeSet::new();
    for id in 0..200u16 {
        for meta in 0..16u8 {
            for face in Face::ALL {
                names.insert(block::texture(BlockState::new(id, meta), face).0);
            }
        }
    }
    for kind in 0..6 {
        for upper in [false, true] {
            names.insert(block::double_plant(kind, upper).0);
        }
    }
    names.extend(["grass_side_overlay", "grass_side_snowed", "missing"]);
    names
}

fn decode(png: &[u8]) -> Option<image::RgbaImage> {
    Some(image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8())
}

impl TextureData {
    /// Loads textures from `sources` in priority order (first wins).
    pub fn load(sources: &[Source]) -> TextureData {
        let names = required_names();
        let mut loaded: Vec<(String, image::RgbaImage, Option<AnimationMeta>)> = Vec::new();
        let mut from_packs = 0;
        for name in &names {
            let rel = format!("textures/blocks/{name}.png");
            let found = sources.iter().find_map(|s| {
                let img = decode(&s.read(&rel)?)?;
                let meta = s.read(&format!("{rel}.mcmeta")).and_then(|m| serde_json::from_slice::<McMeta>(&m).ok());
                Some((img, meta.and_then(|m| m.animation)))
            });
            match found {
                Some((img, anim)) if img.width() > 0 && img.height() >= img.width() => {
                    from_packs += 1;
                    loaded.push((name.to_string(), img, anim));
                }
                _ => loaded.push((name.to_string(), fallback(name), None)),
            }
        }
        // One tile size for the whole array: the largest frame width, as a power of two.
        let size = loaded.iter().map(|(_, img, _)| img.width()).max().unwrap_or(16).next_power_of_two().clamp(16, 512);
        let mut out = TextureData {
            size,
            names: Vec::new(),
            layers: Vec::new(),
            animations: Vec::new(),
            lookup: Arc::default(),
            grass_colormap: None,
            foliage_colormap: None,
            from_packs,
        };
        let mut lookup = HashMap::new();
        for (name, img, anim) in loaded {
            let w = img.width();
            let frames: Vec<Vec<u8>> = (0..(img.height() / w).max(1))
                .map(|i| {
                    let frame = image::imageops::crop_imm(&img, 0, i * w, w, w).to_image();
                    image::imageops::resize(&frame, size, size, image::imageops::FilterType::Nearest).into_raw()
                })
                .collect();
            let layer = out.layers.len() as u32;
            lookup.insert(name.clone(), layer);
            out.names.push(name);
            out.layers.push(frames[0].clone());
            if frames.len() > 1 {
                let meta = anim.unwrap_or(AnimationMeta { frametime: 1, frames: Vec::new() });
                let sequence: Vec<(usize, u32)> = if meta.frames.is_empty() {
                    (0..frames.len()).map(|i| (i, meta.frametime.max(1))).collect()
                } else {
                    meta.frames
                        .iter()
                        .map(|f| match *f {
                            FrameRef::Index(i) => (i as usize, meta.frametime.max(1)),
                            FrameRef::Timed { index, time } => (index as usize, time.max(1)),
                        })
                        .filter(|(i, _)| *i < frames.len())
                        .collect()
                };
                if !sequence.is_empty() {
                    out.animations.push(Animation { layer, frames, sequence });
                }
            }
        }
        out.lookup = Arc::new(lookup);
        let colormap = |name: &str| {
            sources.iter().find_map(|s| {
                let img = decode(&s.read(&format!("textures/colormap/{name}.png"))?)?;
                (img.dimensions() == (256, 256)).then(|| img.into_raw())
            })
        };
        out.grass_colormap = colormap("grass");
        out.foliage_colormap = colormap("foliage");
        out
    }

    pub fn biome_colors(&self) -> BiomeColors {
        BiomeColors { grass: self.grass_colormap.clone(), foliage: self.foliage_colormap.clone() }
    }
}

/// Grass and foliage tints per biome, from colormaps when a pack provides
/// them, otherwise from a built-in approximation.
#[derive(Clone, Default)]
pub struct BiomeColors {
    grass: Option<Vec<u8>>,
    foliage: Option<Vec<u8>>,
}

/// (temperature, rainfall) of 1.8 biome ids.
fn climate(biome: u8) -> (f32, f32) {
    match biome & 127 {
        1 | 16 => (0.8, 0.4),
        2 | 8 | 17 | 37..=39 => (2.0, 0.0),
        3 | 20 | 25 | 34 => (0.2, 0.3),
        4 | 18 | 29 => (0.7, 0.8),
        5 | 19 => (0.25, 0.8),
        6 => (0.8, 0.9),
        10..=13 => (0.0, 0.5),
        14 | 15 => (0.9, 1.0),
        21 | 22 => (0.95, 0.9),
        23 => (0.95, 0.8),
        26 => (0.05, 0.3),
        27 | 28 => (0.6, 0.6),
        30 | 31 => (0.0, 0.4),
        32 | 33 => (0.3, 0.8),
        35 => (1.2, 0.0),
        36 => (1.0, 0.0),
        _ => (0.5, 0.5),
    }
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> f32 { ((a >> s) & 255) as f32 * (1.0 - t) + ((b >> s) & 255) as f32 * t };
    ((ch(16) as u32) << 16) | ((ch(8) as u32) << 8) | ch(0) as u32
}

impl BiomeColors {
    fn sample(map: &Option<Vec<u8>>, biome: u8, fallback: (u32, u32, u32)) -> u32 {
        let (t, r) = climate(biome);
        let t = t.clamp(0.0, 1.0);
        let r = r.clamp(0.0, 1.0) * t;
        match map {
            Some(px) => {
                let (x, y) = (((1.0 - t) * 255.0) as usize, ((1.0 - r) * 255.0) as usize);
                let i = (y * 256 + x) * 4;
                ((px[i] as u32) << 16) | ((px[i + 1] as u32) << 8) | px[i + 2] as u32
            }
            None => lerp_rgb(lerp_rgb(fallback.0, fallback.1, r), fallback.2, (1.0 - t) * 0.7),
        }
    }

    pub fn grass(&self, biome: u8) -> u32 {
        match biome & 127 {
            6 => 0x6A7039,
            37..=39 => 0x90814D,
            29 => lerp_rgb(Self::sample(&self.grass, biome, (0xBFB755, 0x59C93C, 0x80B497)), 0x28340A, 0.5),
            _ => Self::sample(&self.grass, biome, (0xBFB755, 0x59C93C, 0x80B497)),
        }
    }

    pub fn foliage(&self, biome: u8) -> u32 {
        match biome & 127 {
            6 => 0x6A7039,
            37..=39 => 0x9E814D,
            _ => Self::sample(&self.foliage, biome, (0xAEA42A, 0x30BB0B, 0x60A17B)),
        }
    }
}

fn hash(seed: &str, x: u32, y: u32) -> u32 {
    let mut h: u32 = 2166136261;
    for b in seed.bytes().chain(x.to_le_bytes()).chain(y.to_le_bytes()) {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    h ^ (h >> 15)
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

fn shade(c: [u8; 3], f: f32) -> [u8; 3] {
    c.map(|v| (v as f32 * f).clamp(0.0, 255.0) as u8)
}

const DYES: [u32; 16] = [
    0xE9ECEC, 0xF07613, 0xBD44B3, 0x3AAFD9, 0xF8C627, 0x70B919, 0xED8DAC, 0x3E4447, 0x8E8E86, 0x158991, 0x792AAC,
    0x35399D, 0x724728, 0x546D1B, 0xA12722, 0x141519,
];

fn wood(name: &str) -> u32 {
    if name.contains("spruce") {
        0x7A5A34
    } else if name.contains("birch") {
        0xD7C185
    } else if name.contains("jungle") {
        0xB1805C
    } else if name.contains("acacia") {
        0xBA6337
    } else if name.contains("big_oak") || name.contains("dark_oak") || name.contains("roofed") {
        0x4F3218
    } else {
        0xB8945F
    }
}

/// RustCraft's own 16×16 art, drawn from simple rules per texture name so
/// worlds are recognisable without any game assets installed.
pub fn fallback(name: &str) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(16, 16);
    let noise = |x: u32, y: u32| (hash(name, x, y) % 1000) as f32 / 1000.0;
    let fill = |img: &mut image::RgbaImage, base: u32, amount: f32| {
        for (x, y, p) in img.enumerate_pixels_mut() {
            let c = shade(rgb(base), 1.0 - amount / 2.0 + noise(x, y) * amount);
            *p = image::Rgba([c[0], c[1], c[2], 255]);
        }
    };
    // Longest matching colour suffix, so "_light_blue" wins over "_blue".
    let dye = COLORS
        .iter()
        .enumerate()
        .filter(|(_, c)| name.ends_with(&format!("_{c}")))
        .max_by_key(|(_, c)| c.len())
        .map(|(i, _)| i);
    let plant = name.starts_with("sapling")
        || name.starts_with("flower")
        || name.starts_with("double_plant")
        || name.starts_with("wheat")
        || name.starts_with("carrots")
        || name.starts_with("potatoes")
        || name.starts_with("nether_wart")
        || matches!(
            name,
            "tallgrass"
                | "fern"
                | "deadbush"
                | "reeds"
                | "mushroom_brown"
                | "mushroom_red"
                | "web"
                | "vine"
                | "waterlily"
        )
        || name.contains("stem");

    if name == "missing" {
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = if (x / 8 + y / 8) % 2 == 0 { image::Rgba([248, 0, 248, 255]) } else { image::Rgba([0, 0, 0, 255]) };
        }
    } else if plant {
        let (stem, bloom): (u32, Option<u32>) = match name {
            n if n.starts_with("flower_rose") || n.contains("tulip_red") || n.contains("rose") => {
                (0x3E7A27, Some(0xC8102E))
            }
            n if n.contains("dandelion") || n.contains("sunflower") => (0x3E7A27, Some(0xF2D531)),
            n if n.contains("orchid") => (0x3E7A27, Some(0x2BB3E0)),
            n if n.contains("allium") || n.contains("syringa") => (0x3E7A27, Some(0xB664E3)),
            n if n.contains("houstonia") || n.contains("white") || n.contains("daisy") => (0x3E7A27, Some(0xE8E8E8)),
            n if n.contains("orange") => (0x3E7A27, Some(0xE8731C)),
            n if n.contains("pink") || n.contains("paeonia") => (0x3E7A27, Some(0xEFA3C8)),
            "mushroom_brown" => (0xC9AB8C, Some(0x8B6646)),
            "mushroom_red" => (0xD9D3C8, Some(0xD42A2A)),
            "deadbush" => (0x8A6533, None),
            "reeds" => (0x89C064, None),
            "web" => (0xDDDDDD, None),
            n if n.starts_with("wheat") || n.starts_with("potatoes") || n.starts_with("carrots") => {
                (0x5F9E2E, Some(0xC8B23E))
            }
            n if n.starts_with("nether_wart") => (0x9C2424, Some(0x6E1515)),
            // Grass-like plants are grey so the biome tint colours them.
            _ => (0x9A9A9A, None),
        };
        for x in 0..16 {
            let height = 6 + hash(name, x, 99) % 9;
            if x % 3 == 1 || name == "web" {
                for y in (16 - height)..16 {
                    let c = rgb(stem);
                    img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
                }
            }
        }
        if let Some(b) = bloom {
            for (x, y) in [(6, 3), (7, 3), (8, 3), (6, 4), (7, 4), (8, 4), (7, 2), (7, 5), (5, 4), (9, 4)] {
                let c = shade(rgb(b), 0.85 + noise(x, y) * 0.3);
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    } else if name.starts_with("leaves") {
        for (x, y, p) in img.enumerate_pixels_mut() {
            let n = noise(x, y);
            let c = shade(rgb(0xA0A0A0), 0.7 + n * 0.5);
            *p = image::Rgba([c[0], c[1], c[2], if n < 0.22 { 0 } else { 255 }]);
        }
    } else if name.starts_with("water") {
        for (x, y, p) in img.enumerate_pixels_mut() {
            let c = shade(rgb(0x2F5AFA), 0.85 + noise(x, y) * 0.25);
            *p = image::Rgba([c[0], c[1], c[2], 170]);
        }
    } else if name.starts_with("glass") || name == "ice" || name == "slime" {
        let (base, alpha) = match dye {
            Some(i) => (DYES[i], 140),
            None if name == "ice" => (0x9DB8F0, 190),
            None if name == "slime" => (0x7EBF6E, 180),
            None => (0xDDEEFF, 0),
        };
        for (x, y, p) in img.enumerate_pixels_mut() {
            let edge = x == 0 || y == 0 || x == 15 || y == 15;
            let c = rgb(if edge && name.starts_with("glass") { 0xF4F8FC } else { base });
            *p = image::Rgba([c[0], c[1], c[2], if edge && name.starts_with("glass") { 255 } else { alpha }]);
        }
    } else if name == "torch_on" || name.starts_with("redstone_torch") {
        for y in 6..16 {
            img.put_pixel(7, y, image::Rgba([116, 86, 50, 255]));
            img.put_pixel(8, y, image::Rgba([96, 70, 40, 255]));
        }
        let flame = if name == "torch_on" {
            [255, 214, 92, 255]
        } else if name.ends_with("_on") {
            [255, 40, 30, 255]
        } else {
            [110, 30, 25, 255]
        };
        for (x, y) in [(7, 5), (8, 5), (7, 6), (8, 6)] {
            img.put_pixel(x, y, image::Rgba(flame));
        }
    } else if name == "grass_side_overlay" {
        for x in 0..16 {
            for y in 0..(3 + hash(name, x, 0) % 3) {
                let c = shade(rgb(0xA0A0A0), 0.8 + noise(x, y) * 0.3);
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    } else if name.starts_with("rail")
        || name == "ladder"
        || name.starts_with("redstone_dust")
        || name.starts_with("trip_wire")
        || name == "fire_layer_0"
        || name.starts_with("door")
        || name.contains("trapdoor")
        || name == "iron_bars"
        || name == "lever"
    {
        let c = if name.starts_with("redstone_dust") {
            rgb(0xE0E0E0)
        } else if name == "fire_layer_0" {
            rgb(0xF08C1A)
        } else if name.contains("iron") {
            rgb(0xC8C8C8)
        } else {
            rgb(wood(name))
        };
        for (x, y, p) in img.enumerate_pixels_mut() {
            let on = match name {
                n if n.starts_with("rail") => x == 3 || x == 12 || y % 4 == 1,
                "ladder" => x == 2 || x == 13 || y % 4 == 2,
                "iron_bars" => x % 4 == 1,
                n if n.starts_with("redstone_dust") || n.starts_with("trip_wire") => {
                    (6..10).contains(&x) || (6..10).contains(&y)
                }
                "fire_layer_0" => noise(x, y) > 0.35 + y as f32 / 40.0,
                "lever" => (7..9).contains(&x) && y > 3,
                _ => true,
            };
            if on {
                let s = shade(c, 0.8 + noise(x, y) * 0.3);
                *p = image::Rgba([s[0], s[1], s[2], 255]);
            }
        }
        if name.starts_with("door") && name.ends_with("upper") {
            for x in 3..13 {
                for y in 3..8 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
                }
            }
        }
    } else {
        let base = if let Some(i) = dye {
            if name.starts_with("hardened_clay") { lerp_rgb(DYES[i], 0x985E43, 0.45) } else { DYES[i] }
        } else if name.starts_with("planks") {
            wood(name)
        } else if name.starts_with("log") {
            if name.ends_with("_top") {
                lerp_rgb(wood(name), 0xFFFFFF, 0.1)
            } else {
                lerp_rgb(wood(name), 0x2A1C0C, 0.45)
            }
        } else {
            match name {
                n if n.contains("grass_top") => 0x9A9A9A,
                n if n.contains("mycelium") => 0x6F6265,
                n if n.contains("podzol") => 0x5A3D1F,
                "dirt" | "farmland_dry" | "grass_side" => 0x866043,
                "farmland_wet" => 0x51391F,
                "coarse_dirt" => 0x77553B,
                "grass_side_snowed" | "snow" => 0xF0FAFA,
                n if n.starts_with("stone_granite") => 0x9A6B5A,
                n if n.starts_with("stone_diorite") => 0xC4C4C4,
                n if n.starts_with("stone_andesite") => 0x88888C,
                "stone" | "stone_slab_side" | "stone_slab_top" => 0x7F7F7F,
                n if n.starts_with("cobblestone") => 0x7A7A7A,
                "bedrock" => 0x555555,
                "sand" => 0xDBD3A0,
                "red_sand" => 0xA95821,
                "gravel" => 0x837F7E,
                n if n.starts_with("sandstone") => 0xD8CB9B,
                n if n.starts_with("red_sandstone") => 0xB25B22,
                "clay" => 0xA0A6B3,
                "hardened_clay" => 0x985E43,
                n if n.starts_with("ice") => 0x8DB4FA,
                n if n.starts_with("lava") => 0xD96514,
                "obsidian" => 0x150F20,
                "netherrack" => 0x6F3634,
                "soul_sand" => 0x544033,
                "glowstone" | "redstone_lamp_on" => 0xFBDA74,
                "redstone_lamp_off" => 0x5F3B24,
                "end_stone" => 0xDDDFA5,
                n if n.contains("brick") && n.contains("nether") => 0x2D1519,
                "brick" => 0x96574A,
                n if n.starts_with("stonebrick") => 0x7A7A7A,
                n if n.starts_with("quartz") => 0xECE6DF,
                n if n.starts_with("prismarine") => 0x63A898,
                "sea_lantern" => 0xD1E4DC,
                "gold_block" => 0xF9D849,
                "iron_block" | "anvil_base" => 0xDCDCDC,
                "diamond_block" => 0x62EDE4,
                "emerald_block" => 0x51D975,
                "lapis_block" => 0x264EA8,
                "redstone_block" => 0xAB1B09,
                "coal_block" => 0x141414,
                "sponge" | "sponge_wet" => 0xC8C34A,
                n if n.starts_with("cactus") => 0x57802A,
                n if n.starts_with("pumpkin") => 0xC5701A,
                n if n.starts_with("melon") => 0x6E9A2B,
                n if n.starts_with("hay_block") => 0xB59A26,
                n if n.starts_with("tnt") => 0xC5371B,
                "bookshelf" => 0x6C5437,
                n if n.starts_with("crafting_table") => 0x7E5A35,
                n if n.starts_with("furnace") || n.starts_with("dispenser") || n.starts_with("dropper") => 0x6F6F6F,
                n if n.starts_with("piston") => 0x9C8E70,
                n if n.starts_with("mushroom_block_skin_red") => 0xB52E2A,
                n if n.starts_with("mushroom_block_skin_brown") => 0x8E6B4E,
                n if n.starts_with("mushroom_block") => 0xCFC8AE,
                "portal" => 0x5B1EB8,
                "noteblock" | "jukebox_side" | "jukebox_top" => 0x6B4934,
                "mob_spawner" => 0x1D2A38,
                _ => 0x808080 | (hash(name, 0, 0) & 0x3F3F3F),
            }
        };
        let rough =
            if name.ends_with("_smooth") || name.starts_with("quartz") || name.contains("wool") { 0.12 } else { 0.35 };
        fill(&mut img, base, rough);
        if name.contains("_ore") {
            let speck = match name {
                "coal_ore" => 0x2B2B2B,
                "iron_ore" => 0xD8AF93,
                "gold_ore" => 0xFCEE4B,
                "diamond_ore" => 0x5DECF5,
                "lapis_ore" => 0x1546A8,
                "emerald_ore" => 0x17DD62,
                "quartz_ore" => 0xE8E1D8,
                _ => 0xFF2010,
            };
            for (x, y, p) in img.enumerate_pixels_mut() {
                if hash(name, x / 3, y / 3).is_multiple_of(5) && noise(x, y) > 0.3 {
                    let c = rgb(speck);
                    *p = image::Rgba([c[0], c[1], c[2], 255]);
                }
            }
        }
        if name.starts_with("planks")
            || name.contains("brick")
            || name.starts_with("cobblestone")
            || name == "bookshelf"
        {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let mortar = match name {
                    n if n.starts_with("planks") => y % 4 == 3,
                    n if n.starts_with("cobblestone") => {
                        hash(name, x / 4, y / 4).is_multiple_of(3) && (x % 4 == 0 || y % 4 == 0)
                    }
                    "bookshelf" => y == 0 || y == 15 || y == 7 || y == 8,
                    _ => y % 4 == 3 || (x + if (y / 4) % 2 == 0 { 0 } else { 4 }) % 8 == 7,
                };
                if mortar {
                    let c = shade([p[0], p[1], p[2]], 0.65);
                    *p = image::Rgba([c[0], c[1], c[2], 255]);
                }
            }
        }
        if name.starts_with("log") && name.ends_with("_top") {
            for (x, y, p) in img.enumerate_pixels_mut() {
                let d = ((x as f32 - 7.5).powi(2) + (y as f32 - 7.5).powi(2)).sqrt() as u32;
                if d.is_multiple_of(3) || d >= 7 {
                    let c = shade([p[0], p[1], p[2]], if d >= 7 { 0.55 } else { 0.8 });
                    *p = image::Rgba([c[0], c[1], c[2], 255]);
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_texture_has_built_in_art() {
        let data = TextureData::load(&[]);
        assert_eq!(data.size, 16);
        assert_eq!(data.from_packs, 0);
        assert!(data.names.len() > 250, "{}", data.names.len());
        for name in ["stone", "grass_top", "water_still", "leaves_oak", "wool_colored_red", "missing"] {
            let layer = data.lookup[name] as usize;
            assert_eq!(data.layers[layer].len(), 16 * 16 * 4);
        }
        // Leaves have see-through holes; stone is opaque.
        let leaves = &data.layers[data.lookup["leaves_oak"] as usize];
        assert!(leaves.chunks(4).any(|p| p[3] == 0));
        assert!(data.layers[data.lookup["stone"] as usize].chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn pack_textures_override_and_animate() {
        let dir = std::env::temp_dir().join(format!("rc-pack-{}", std::process::id()));
        let blocks = dir.join("assets/minecraft/textures/blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        // A 32px stone texture and a 2-frame lava animation.
        image::RgbaImage::from_pixel(32, 32, image::Rgba([1, 2, 3, 255])).save(blocks.join("stone.png")).unwrap();
        let mut lava = image::RgbaImage::new(16, 32);
        for (_, y, p) in lava.enumerate_pixels_mut() {
            *p = if y < 16 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
        }
        lava.save(blocks.join("lava_still.png")).unwrap();
        std::fs::write(blocks.join("lava_still.png.mcmeta"), r#"{"animation":{"frametime":3}}"#).unwrap();

        let data = TextureData::load(&[Source::Dir(dir.clone())]);
        assert_eq!(data.size, 32, "tile size follows the largest texture");
        assert_eq!(data.from_packs, 2);
        assert_eq!(&data.layers[data.lookup["stone"] as usize][..4], &[1, 2, 3, 255]);
        let anim = data.animations.iter().find(|a| a.layer == data.lookup["lava_still"]).unwrap();
        assert_eq!(anim.frames.len(), 2);
        assert_eq!((anim.frame_at(0), anim.frame_at(3), anim.frame_at(6)), (0, 1, 0));
        // Smaller textures are scaled up to the tile size.
        assert_eq!(data.layers[data.lookup["dirt"] as usize].len(), 32 * 32 * 4);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn biome_tints_without_colormaps() {
        let c = BiomeColors::default();
        let (g_desert, g_jungle) = (c.grass(2), c.grass(21));
        assert_ne!(g_desert, g_jungle);
        assert_eq!(c.grass(6), 0x6A7039);
        // Green channel dominates for lush biomes.
        assert!((g_jungle >> 8) & 255 > (g_jungle >> 16) & 255);
    }
}
