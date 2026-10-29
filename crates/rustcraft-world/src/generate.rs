//! RustCraft's own terrain generator for new local worlds: hills,
//! mountains, oceans, a handful of biomes, caves, ores, trees and plants.
//!
//! It is not Mojang's algorithm, so a seed does not reproduce vanilla
//! terrain. Output depends only on the seed and chunk position, so chunks
//! can be generated in any order and on any thread.

use crate::block::BlockState;
use crate::chunk::Chunk;

pub const SEA_LEVEL: i32 = 62;

const STONE: BlockState = BlockState::new(1, 0);
const GRASS: BlockState = BlockState::new(2, 0);
const DIRT: BlockState = BlockState::new(3, 0);
const BEDROCK: BlockState = BlockState::new(7, 0);
const WATER: BlockState = BlockState::new(9, 0);
const LAVA: BlockState = BlockState::new(11, 0);
const SAND: BlockState = BlockState::new(12, 0);
const GRAVEL: BlockState = BlockState::new(13, 0);
const SANDSTONE: BlockState = BlockState::new(24, 0);
const ICE: BlockState = BlockState::new(79, 0);
const SNOW_LAYER: BlockState = BlockState::new(78, 0);
const SNOW: BlockState = BlockState::new(80, 0);
const AIR: BlockState = BlockState::AIR;

/// 1.8 biome ids written into chunks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Biome {
    Ocean = 0,
    Plains = 1,
    Desert = 2,
    Mountains = 3,
    Forest = 4,
    Taiga = 5,
    IcePlains = 12,
    Beach = 16,
    BirchForest = 27,
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Small deterministic PRNG for per-chunk features.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = splitmix(self.0);
        self.0
    }
    fn below(&mut self, n: u32) -> i32 {
        (self.next() % n as u64) as i32
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
}

/// Improved Perlin noise with a seeded permutation.
struct Perlin {
    perm: [u8; 512],
}

impl Perlin {
    fn new(seed: u64) -> Self {
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut s = seed;
        for i in (1..256).rev() {
            s = splitmix(s);
            p.swap(i, (s % (i as u64 + 1)) as usize);
        }
        let mut perm = [0u8; 512];
        for i in 0..512 {
            perm[i] = p[i & 255];
        }
        Self { perm }
    }

    fn grad(h: u8, x: f64, y: f64, z: f64) -> f64 {
        let h = h & 15;
        let u = if h < 8 { x } else { y };
        let v = if h < 4 {
            y
        } else if h == 12 || h == 14 {
            x
        } else {
            z
        };
        (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
    }

    fn noise(&self, x: f64, y: f64, z: f64) -> f64 {
        let fade = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let (xi, yi, zi) = (x.floor() as i32 & 255, y.floor() as i32 & 255, z.floor() as i32 & 255);
        let (x, y, z) = (x - x.floor(), y - y.floor(), z - z.floor());
        let (u, v, w) = (fade(x), fade(y), fade(z));
        let p = &self.perm;
        let a = p[xi as usize] as usize + yi as usize;
        let (aa, ab) = (p[a] as usize + zi as usize, p[a + 1] as usize + zi as usize);
        let b = p[xi as usize + 1] as usize + yi as usize;
        let (ba, bb) = (p[b] as usize + zi as usize, p[b + 1] as usize + zi as usize);
        lerp(
            w,
            lerp(
                v,
                lerp(u, Self::grad(p[aa], x, y, z), Self::grad(p[ba], x - 1.0, y, z)),
                lerp(u, Self::grad(p[ab], x, y - 1.0, z), Self::grad(p[bb], x - 1.0, y - 1.0, z)),
            ),
            lerp(
                v,
                lerp(u, Self::grad(p[aa + 1], x, y, z - 1.0), Self::grad(p[ba + 1], x - 1.0, y, z - 1.0)),
                lerp(u, Self::grad(p[ab + 1], x, y - 1.0, z - 1.0), Self::grad(p[bb + 1], x - 1.0, y - 1.0, z - 1.0)),
            ),
        )
    }

    /// Fractal noise, roughly in -1..1.
    fn fbm(&self, x: f64, y: f64, z: f64, octaves: u32) -> f64 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for _ in 0..octaves {
            sum += self.noise(x * freq, y * freq, z * freq) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
}

pub struct Generator {
    seed: u64,
    continent: Perlin,
    hills: Perlin,
    ridges: Perlin,
    climate: Perlin,
    caves: Perlin,
}

pub struct Column {
    pub height: i32,
    pub biome: Biome,
}

impl Generator {
    pub fn new(seed: i64) -> Self {
        let s = seed as u64;
        Self {
            seed: s,
            continent: Perlin::new(splitmix(s ^ 1)),
            hills: Perlin::new(splitmix(s ^ 2)),
            ridges: Perlin::new(splitmix(s ^ 3)),
            climate: Perlin::new(splitmix(s ^ 4)),
            caves: Perlin::new(splitmix(s ^ 5)),
        }
    }

    pub fn column(&self, x: i32, z: i32) -> Column {
        let (fx, fz) = (x as f64, z as f64);
        let continent = self.continent.fbm(fx / 700.0, 0.0, fz / 700.0, 4);
        let hills = self.hills.fbm(fx / 140.0, 3.7, fz / 140.0, 5);
        let ridge = 1.0 - self.ridges.fbm(fx / 320.0, 9.1, fz / 320.0, 4).abs();
        let mountain_mask = (self.ridges.fbm(fx / 900.0, 1.3, fz / 900.0, 2) * 2.5 - 0.3).clamp(0.0, 1.0);
        let land = (continent * 3.0 + 0.6).clamp(-1.0, 1.0);
        let height = 64.0 + land * 14.0 + hills * 10.0 * land.max(0.2) + ridge.powi(3) * 55.0 * mountain_mask;
        let height = (height as i32).clamp(8, 200);

        let temperature = self.climate.fbm(fx / 900.0, 17.0, fz / 900.0, 3);
        let humidity = self.climate.fbm(fx / 700.0, 61.0, fz / 700.0, 3);
        let biome = if height < SEA_LEVEL - 1 {
            Biome::Ocean
        } else if height > 100 {
            Biome::Mountains
        } else if temperature < -0.3 {
            Biome::IcePlains
        } else if height <= SEA_LEVEL + 1 {
            Biome::Beach
        } else if temperature > 0.3 && humidity < 0.0 {
            Biome::Desert
        } else if temperature < -0.1 {
            Biome::Taiga
        } else if humidity > 0.2 {
            if temperature < 0.1 { Biome::BirchForest } else { Biome::Forest }
        } else {
            Biome::Plains
        };
        Column { height, biome }
    }

    /// A dry land position near the origin, with the height to stand at.
    pub fn spawn_point(&self) -> (i32, i32, i32) {
        for r in 0..64 {
            for (x, z) in [(r * 16, 0), (0, r * 16), (-r * 16, 0), (0, -r * 16)] {
                let c = self.column(x, z);
                if c.height > SEA_LEVEL && c.biome != Biome::Ocean {
                    return (x, c.height + 1, z);
                }
            }
        }
        (0, 100, 0)
    }

    pub fn generate(&self, cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        let mut rng = Rng(splitmix(
            self.seed
                ^ (cx as u64).wrapping_mul(0x632B_E59B_D9B4_E019)
                ^ (cz as u64).wrapping_mul(0x8CB9_2BA7_2F3D_8DD7),
        ));
        let mut columns = Vec::with_capacity(256);
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (cx * 16 + lx as i32, cz * 16 + lz as i32);
                let col = self.column(x, z);
                chunk.biomes[lz * 16 + lx] = col.biome as u8;
                self.fill_column(&mut chunk, &mut rng, lx, lz, x, z, &col);
                columns.push(col);
            }
        }
        self.ores(&mut chunk, &mut rng);
        self.decorate(&mut chunk, &mut rng, &columns);
        chunk
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_column(&self, c: &mut Chunk, rng: &mut Rng, lx: usize, lz: usize, x: i32, z: i32, col: &Column) {
        let h = col.height;
        let (top, filler, depth) = match col.biome {
            Biome::Desert => (SAND, SANDSTONE, 6),
            Biome::Beach => (SAND, SAND, 4),
            Biome::Ocean => (if h < SEA_LEVEL - 12 { GRAVEL } else { SAND }, DIRT, 3),
            Biome::Mountains if h > 125 => (SNOW, STONE, 1),
            Biome::Mountains if h > 108 => (STONE, STONE, 1),
            _ => (GRASS, DIRT, 4),
        };
        for y in 0..=h.max(SEA_LEVEL) {
            let state = if y == 0 || (y < 5 && rng.below(5) >= y) {
                BEDROCK
            } else if y > h {
                if col.biome == Biome::IcePlains && y == SEA_LEVEL { ICE } else { WATER }
            } else if y == h {
                if h < SEA_LEVEL && top == GRASS { DIRT } else { top }
            } else if y > h - depth {
                filler
            } else {
                STONE
            };
            c.set_block(lx, y, lz, state);
        }
        if col.biome == Biome::IcePlains && h >= SEA_LEVEL {
            c.set_block(lx, h + 1, lz, SNOW_LAYER);
        }
        // Caves: carve where two noise fields cross zero, below the surface
        // and never under water.
        let ceiling = if h < SEA_LEVEL + 2 { h - 6 } else { h - 1 };
        for y in 6..ceiling {
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            let a = self.caves.noise(fx / 42.0, fy / 26.0, fz / 42.0);
            let b = self.caves.noise(fx / 42.0 + 100.0, fy / 26.0, fz / 42.0);
            if a.abs() < 0.07 && b.abs() < 0.11 {
                c.set_block(lx, y, lz, if y < 11 { LAVA } else { AIR });
            }
        }
    }

    fn ores(&self, c: &mut Chunk, rng: &mut Rng) {
        // (block, veins, min y, max y, vein size)
        for (ore, veins, lo, hi, size) in [
            (16u16, 20, 5, 128, 8),
            (15, 20, 5, 64, 6),
            (14, 2, 5, 32, 6),
            (73, 8, 5, 16, 5),
            (56, 1, 5, 16, 5),
            (21, 1, 10, 30, 4),
        ] {
            for _ in 0..veins {
                let (mut x, mut y, mut z) = (rng.below(16), lo + rng.below((hi - lo) as u32), rng.below(16));
                for _ in 0..size {
                    if (0..16).contains(&x) && (0..16).contains(&z) && c.block(x as usize, y, z as usize) == STONE {
                        c.set_block(x as usize, y, z as usize, BlockState::new(ore, 0));
                    }
                    match rng.below(6) {
                        0 => x += 1,
                        1 => x -= 1,
                        2 => y += 1,
                        3 => y -= 1,
                        4 => z += 1,
                        _ => z -= 1,
                    }
                }
            }
        }
    }

    fn decorate(&self, c: &mut Chunk, rng: &mut Rng, columns: &[Column]) {
        let biome = columns[8 * 16 + 8].biome;
        let trees = match biome {
            Biome::Forest | Biome::BirchForest => 7,
            Biome::Taiga => 5,
            Biome::Plains => i32::from(rng.chance(0.3)),
            Biome::Mountains => 1,
            _ => 0,
        };
        for _ in 0..trees {
            let (x, z) = (2 + rng.below(12) as usize, 2 + rng.below(12) as usize);
            let col = &columns[z * 16 + x];
            if c.block(x, col.height, z) != GRASS {
                continue;
            }
            let kind = match col.biome {
                Biome::Taiga | Biome::Mountains => Tree::Spruce,
                Biome::BirchForest => Tree::Birch,
                Biome::Forest if rng.chance(0.2) => Tree::Birch,
                _ => Tree::Oak,
            };
            tree(c, rng, x, col.height + 1, z, kind);
        }
        for i in 0..256 {
            let (x, z) = (i % 16, i / 16);
            let col = &columns[i];
            let y = col.height + 1;
            if !c.block(x, y, z).is_air() {
                continue;
            }
            match (c.block(x, col.height, z), col.biome) {
                (GRASS, Biome::Plains | Biome::Forest | Biome::BirchForest) if rng.chance(0.12) => {
                    c.set_block(x, y, z, BlockState::new(31, 1));
                }
                (GRASS, Biome::Plains | Biome::Forest) if rng.chance(0.01) => {
                    c.set_block(x, y, z, if rng.chance(0.5) { BlockState::new(37, 0) } else { BlockState::new(38, 0) });
                }
                (GRASS, Biome::Taiga) if rng.chance(0.04) => c.set_block(x, y, z, BlockState::new(31, 2)),
                (SAND, Biome::Desert) if rng.chance(0.004) => {
                    for dy in 0..(1 + rng.below(3)) {
                        c.set_block(x, y + dy, z, BlockState::new(81, 0));
                    }
                }
                (SAND, Biome::Desert) if rng.chance(0.004) => c.set_block(x, y, z, BlockState::new(32, 0)),
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Tree {
    Oak,
    Birch,
    Spruce,
}

fn tree(c: &mut Chunk, rng: &mut Rng, x: usize, y: i32, z: usize, kind: Tree) {
    let (log, leaves) = match kind {
        Tree::Oak => (BlockState::new(17, 0), BlockState::new(18, 0)),
        Tree::Birch => (BlockState::new(17, 2), BlockState::new(18, 2)),
        Tree::Spruce => (BlockState::new(17, 1), BlockState::new(18, 1)),
    };
    let height = 4 + rng.below(3);
    let top = y + height;
    let set_leaf = |c: &mut Chunk, lx: i32, ly: i32, lz: i32| {
        if (0..16).contains(&lx) && (0..16).contains(&lz) && c.block(lx as usize, ly, lz as usize).is_air() {
            c.set_block(lx as usize, ly, lz as usize, leaves);
        }
    };
    match kind {
        Tree::Spruce => {
            for ly in (y + 1)..=(top + 1) {
                let r = ((top + 1 - ly) / 2).min(2) + i32::from(ly % 2 == 0 && ly < top);
                for dx in -r..=r {
                    for dz in -r..=r {
                        if dx.abs() + dz.abs() <= r + 1 {
                            set_leaf(c, x as i32 + dx, ly, z as i32 + dz);
                        }
                    }
                }
            }
        }
        _ => {
            for ly in (top - 3)..=top {
                let r: i32 = if ly >= top - 1 { 1 } else { 2 };
                for dx in -r..=r {
                    for dz in -r..=r {
                        let corner = dx.abs() == r && dz.abs() == r;
                        if !corner || (ly < top && rng.chance(0.5)) {
                            set_leaf(c, x as i32 + dx, ly, z as i32 + dz);
                        }
                    }
                }
            }
        }
    }
    for ly in y..top {
        c.set_block(x, ly, z, log);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_and_plausible() {
        let g = Generator::new(12345);
        let a = g.generate(3, -7);
        let b = Generator::new(12345).generate(3, -7);
        for y in 0..256 {
            for i in 0..256 {
                assert_eq!(a.block(i % 16, y, i / 16), b.block(i % 16, y, i / 16));
            }
        }
        // Bedrock floor everywhere, and something at sea level or above.
        for i in 0..256 {
            assert_eq!(a.block(i % 16, 0, i / 16), BEDROCK);
            assert!(a.top(i % 16, i / 16) >= SEA_LEVEL - 40);
        }
        let other = Generator::new(999).generate(3, -7);
        let same = (0..256).filter(|&i| a.top(i % 16, i / 16) == other.top(i % 16, i / 16)).count();
        assert!(same < 256, "different seeds produced identical terrain");
    }

    #[test]
    fn spawn_is_on_land() {
        let g = Generator::new(42);
        let (x, y, z) = g.spawn_point();
        let col = g.column(x, z);
        assert!(col.height > SEA_LEVEL);
        assert_eq!(y, col.height + 1);
    }
}
