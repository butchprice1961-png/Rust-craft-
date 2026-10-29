//! Turns one 16×16×16 chunk section into vertex data. Runs on worker
//! threads: the main thread captures a [`Snapshot`] (the section plus a
//! one-block border of neighbours, light and biome tints) and the mesher
//! needs nothing else.

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

use rustcraft_world::block::{self, Aabb, BlockState, Face, Layer, Neighbors, Shape, Tint};
use rustcraft_world::chunk::index;
use rustcraft_world::{Chunk, World};

use crate::textures::{BiomeColors, Lookup};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Vertex {
    /// Section-local position (0–16).
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    /// Tint × directional shade × ambient occlusion.
    pub color: [u8; 4],
    /// Sky and block light, 0–15 scaled to 0–255.
    pub light: [u8; 4],
}

#[derive(Default, Debug)]
pub struct Geometry {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Geometry {
    fn quad(&mut self, v: [Vertex; 4], flip: bool) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(v);
        let order = if flip { [1, 2, 3, 1, 3, 0] } else { [0, 1, 2, 0, 2, 3] };
        self.indices.extend(order.map(|i| base + i));
    }

    fn double_quad(&mut self, v: [Vertex; 4]) {
        self.quad(v, false);
        self.quad([v[3], v[2], v[1], v[0]], false);
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

pub struct MeshData {
    pub section: IVec3,
    pub solid: Geometry,
    pub translucent: Geometry,
}

const SIDE: i32 = 18;

#[inline]
fn idx(x: i32, y: i32, z: i32) -> usize {
    ((y + 1) * SIDE * SIDE + (z + 1) * SIDE + (x + 1)) as usize
}

/// A section and its immediate surroundings, copied out of the world.
pub struct Snapshot {
    pub section: IVec3,
    blocks: Vec<BlockState>,
    sky: Vec<u8>,
    block_light: Vec<u8>,
    grass: Vec<u32>,
    foliage: Vec<u32>,
}

impl Snapshot {
    /// `None` if the section is missing or contains only air.
    pub fn capture(world: &World, section: IVec3, colors: &BiomeColors) -> Option<Snapshot> {
        if !(0..16).contains(&section.y) {
            return None;
        }
        let own = world.chunk(section.x, section.z)?;
        if own.sections[section.y as usize].as_ref().is_none_or(|s| s.is_empty()) {
            return None;
        }
        let neighbours: [[Option<&Chunk>; 3]; 3] = std::array::from_fn(|dz| {
            std::array::from_fn(|dx| world.chunk(section.x + dx as i32 - 1, section.z + dz as i32 - 1))
        });
        let n = (SIDE * SIDE * SIDE) as usize;
        let mut s = Snapshot {
            section,
            blocks: vec![BlockState::AIR; n],
            sky: vec![0; n],
            block_light: vec![0; n],
            grass: vec![0; (SIDE * SIDE) as usize],
            foliage: vec![0; (SIDE * SIDE) as usize],
        };
        let sky_default = if world.has_sky { 15 } else { 0 };
        for z in -1..17 {
            for x in -1..17 {
                let (cdx, lx) = (((x + 16) / 16) as usize, ((x + 16) % 16) as usize);
                let (cdz, lz) = (((z + 16) / 16) as usize, ((z + 16) % 16) as usize);
                let chunk = neighbours[cdz][cdx];
                let biome = chunk.map_or(1, |c| c.biomes[lz * 16 + lx]);
                let col = ((z + 1) * SIDE + (x + 1)) as usize;
                s.grass[col] = colors.grass(biome);
                s.foliage[col] = colors.foliage(biome);
                for y in -1..17 {
                    let wy = section.y * 16 + y;
                    let i = idx(x, y, z);
                    if wy >= 256 {
                        s.sky[i] = sky_default;
                        continue;
                    }
                    let Some(chunk) = chunk else {
                        s.sky[i] = sky_default;
                        continue;
                    };
                    if wy < 0 {
                        continue;
                    }
                    match &chunk.sections[(wy >> 4) as usize] {
                        Some(sec) => {
                            let j = index(lx, (wy & 15) as usize, lz);
                            s.blocks[i] = sec.block(j);
                            s.sky[i] = sec.sky_light(j);
                            s.block_light[i] = sec.block_light(j);
                        }
                        None => s.sky[i] = sky_default,
                    }
                }
            }
        }
        // Blend tints over 3×3 columns so biome edges are soft.
        let (grass, foliage) = (s.grass.clone(), s.foliage.clone());
        for z in 0..16 {
            for x in 0..16 {
                let col = ((z + 1) * SIDE + (x + 1)) as usize;
                s.grass[col] = blend(&grass, x, z);
                s.foliage[col] = blend(&foliage, x, z);
            }
        }
        Some(s)
    }

    fn at(&self, x: i32, y: i32, z: i32) -> BlockState {
        if (-1..17).contains(&x) && (-1..17).contains(&y) && (-1..17).contains(&z) {
            self.blocks[idx(x, y, z)]
        } else {
            BlockState::AIR
        }
    }

    fn light(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let i = idx(x.clamp(-1, 16), y.clamp(-1, 16), z.clamp(-1, 16));
        (self.sky[i], self.block_light[i])
    }
}

fn blend(colors: &[u32], x: i32, z: i32) -> u32 {
    let (mut r, mut g, mut b) = (0, 0, 0);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let c = colors[((z + 1 + dz) * SIDE + (x + 1 + dx)) as usize];
            r += (c >> 16) & 255;
            g += (c >> 8) & 255;
            b += c & 255;
        }
    }
    ((r / 9) << 16) | ((g / 9) << 8) | (b / 9)
}

struct Local<'a> {
    s: &'a Snapshot,
    x: i32,
    y: i32,
    z: i32,
}

impl Neighbors for Local<'_> {
    fn get(&self, dx: i32, dy: i32, dz: i32) -> BlockState {
        self.s.at(self.x + dx, self.y + dy, self.z + dz)
    }
}

/// Unit-cube corners per face, counter-clockwise seen from outside.
const CORNERS: [[[f32; 3]; 4]; 6] = [
    [[0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0]],
    [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]],
    [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]],
    [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]],
    [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0]],
    [[1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0]],
];

/// Texture coordinates projected from a block-local point, as vanilla does
/// for model elements without explicit UVs.
fn auto_uv(face: Face, p: Vec3) -> [f32; 2] {
    match face {
        Face::Down => [p.x, 1.0 - p.z],
        Face::Up => [p.x, p.z],
        Face::North => [1.0 - p.x, 1.0 - p.y],
        Face::South => [p.x, 1.0 - p.y],
        Face::West => [p.z, 1.0 - p.y],
        Face::East => [1.0 - p.z, 1.0 - p.y],
    }
}

const AO: [f32; 4] = [0.5, 0.65, 0.82, 1.0];

fn rgb(c: u32) -> Vec3 {
    Vec3::new(((c >> 16) & 255) as f32, ((c >> 8) & 255) as f32, (c & 255) as f32) / 255.0
}

fn pack_color(c: Vec3) -> [u8; 4] {
    let c = (c.clamp(Vec3::ZERO, Vec3::ONE) * 255.0).round();
    [c.x as u8, c.y as u8, c.z as u8, 255]
}

fn pack_light(sky: f32, block: f32) -> [u8; 4] {
    [(sky * 17.0).round() as u8, (block * 17.0).round() as u8, 0, 0]
}

struct Mesher<'a> {
    s: &'a Snapshot,
    lookup: &'a Lookup,
    missing: u32,
    solid: Geometry,
    translucent: Geometry,
}

pub fn mesh(s: &Snapshot, lookup: &Lookup) -> MeshData {
    let mut m = Mesher {
        s,
        lookup,
        missing: lookup.get("missing").copied().unwrap_or(0),
        solid: Geometry::default(),
        translucent: Geometry::default(),
    };
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                m.block(x, y, z);
            }
        }
    }
    MeshData { section: s.section, solid: m.solid, translucent: m.translucent }
}

fn culls(me: BlockState, other: BlockState) -> bool {
    other.props().opaque || (other.id() == me.id() && matches!(me.id(), 20 | 79 | 95 | 160 | 165))
}

impl Mesher<'_> {
    fn layer(&self, name: &str) -> u32 {
        self.lookup.get(name).copied().unwrap_or(self.missing)
    }

    fn tint(&self, tint: Tint, x: i32, z: i32) -> Vec3 {
        let col = ((z + 1) * SIDE + (x + 1)) as usize;
        match tint {
            Tint::None => Vec3::ONE,
            Tint::Grass => rgb(self.s.grass[col]),
            Tint::Foliage => rgb(self.s.foliage[col]),
            Tint::Rgb(c) => rgb(c),
            Tint::Redstone(power) => {
                let f = power as f32 / 15.0;
                Vec3::new(
                    if power == 0 { 0.3 } else { f * 0.6 + 0.4 },
                    (f * f * 0.7 - 0.5).max(0.0),
                    (f * f * 0.6 - 0.7).max(0.0),
                )
            }
        }
    }

    fn geometry(&mut self, layer: Layer) -> &mut Geometry {
        if layer == Layer::Translucent { &mut self.translucent } else { &mut self.solid }
    }

    fn face_texture(&self, state: BlockState, face: Face, x: i32, y: i32, z: i32) -> (&'static str, Tint) {
        if state.id() == 2 && face.horizontal() && matches!(self.s.at(x, y + 1, z).id(), 78 | 80) {
            return ("grass_side_snowed", Tint::None);
        }
        if state.id() == 175 && state.meta() & 8 != 0 {
            return block::double_plant(self.s.at(x, y - 1, z).meta() & 7, true);
        }
        block::texture(state, face)
    }

    fn block(&mut self, x: i32, y: i32, z: i32) {
        let state = self.s.at(x, y, z);
        let props = state.props();
        match props.shape {
            Shape::None => {}
            Shape::Cube => self.cube(state, x, y, z),
            Shape::Cross => self.cross(state, x, y, z),
            Shape::Crop => self.crop(state, x, y, z),
            Shape::Liquid => self.liquid(state, x, y, z),
            Shape::Vine => self.vine(state, x, y, z),
            Shape::Rail => self.rail(state, x, y, z),
            Shape::Flat => self.flat(state, x, y, z),
            _ => {
                let local = Local { s: self.s, x, y, z };
                for b in block::boxes(state, &local) {
                    self.cuboid(state, x, y, z, b);
                }
            }
        }
    }

    /// Ambient occlusion and smoothed light at one corner of a full face.
    fn corner_light(&self, x: i32, y: i32, z: i32, face: Face, corner: [f32; 3]) -> (usize, f32, f32) {
        let n = IVec3::from_array(face.offset());
        let front = IVec3::new(x, y, z) + n;
        let axis = if n.x != 0 {
            0
        } else if n.y != 0 {
            1
        } else {
            2
        };
        let (a1, a2) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let mut e1 = IVec3::ZERO;
        e1[a1] = if corner[a1] > 0.5 { 1 } else { -1 };
        let mut e2 = IVec3::ZERO;
        e2[a2] = if corner[a2] > 0.5 { 1 } else { -1 };
        let cells = [front, front + e1, front + e2, front + e1 + e2];
        let opaque = cells.map(|c| self.s.at(c.x, c.y, c.z).props().opaque);
        let ao = if opaque[1] && opaque[2] { 0 } else { 3 - opaque[1..].iter().filter(|o| **o).count() };
        let (mut sky, mut blk, mut count) = (0.0, 0.0, 0.0);
        for (i, c) in cells.iter().enumerate() {
            if i == 0 || !opaque[i] {
                if i == 3 && opaque[1] && opaque[2] {
                    continue;
                }
                let (s, b) = self.s.light(c.x, c.y, c.z);
                sky += s as f32;
                blk += b as f32;
                count += 1.0;
            }
        }
        (ao, sky / count, blk / count)
    }

    fn cube(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let layer = state.props().layer;
        for face in Face::ALL {
            let [dx, dy, dz] = face.offset();
            if culls(state, self.s.at(x + dx, y + dy, z + dz)) {
                continue;
            }
            let (name, tint) = self.face_texture(state, face, x, y, z);
            self.full_face(x, y, z, face, name, tint, layer);
            if state.id() == 2 && face.horizontal() && name == "grass_side" {
                self.full_face(x, y, z, face, "grass_side_overlay", Tint::Grass, layer);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn full_face(&mut self, x: i32, y: i32, z: i32, face: Face, name: &str, tint: Tint, layer: Layer) {
        let tex = self.layer(name);
        let color = self.tint(tint, x, z) * face.shade();
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let mut aos = [0usize; 4];
        let verts: [Vertex; 4] = std::array::from_fn(|i| {
            let c = CORNERS[face.index()][i];
            let (ao, sky, blk) = self.corner_light(x, y, z, face, c);
            aos[i] = ao;
            let p = Vec3::from_array(c);
            Vertex {
                pos: (origin + p).to_array(),
                uv: auto_uv(face, p),
                layer: tex,
                color: pack_color(color * AO[ao]),
                light: pack_light(sky, blk),
            }
        });
        // Split the quad along the brighter diagonal to avoid AO artefacts.
        let flip = aos[0] + aos[2] < aos[1] + aos[3];
        self.geometry(layer).quad(verts, flip);
    }

    fn flat_light(&self, x: i32, y: i32, z: i32, face: Option<Face>) -> [u8; 4] {
        let (s0, b0) = self.s.light(x, y, z);
        let (s1, b1) = match face {
            Some(f) => {
                let [dx, dy, dz] = f.offset();
                self.s.light(x + dx, y + dy, z + dz)
            }
            None => (0, 0),
        };
        pack_light(s0.max(s1) as f32, b0.max(b1) as f32)
    }

    fn cuboid(&mut self, state: BlockState, x: i32, y: i32, z: i32, b: Aabb) {
        let layer = state.props().layer;
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        for face in Face::ALL {
            let on_edge = match face {
                Face::Down => b.min.y <= 0.0,
                Face::Up => b.max.y >= 1.0,
                Face::North => b.min.z <= 0.0,
                Face::South => b.max.z >= 1.0,
                Face::West => b.min.x <= 0.0,
                Face::East => b.max.x >= 1.0,
            };
            let [dx, dy, dz] = face.offset();
            if on_edge && culls(state, self.s.at(x + dx, y + dy, z + dz)) {
                continue;
            }
            let (name, tint) = self.face_texture(state, face, x, y, z);
            let tex = self.layer(name);
            let color = pack_color(self.tint(tint, x, z) * face.shade());
            let light = self.flat_light(x, y, z, Some(face));
            let verts = CORNERS[face.index()].map(|c| {
                let p = b.min + Vec3::from_array(c) * (b.max - b.min);
                Vertex { pos: (origin + p).to_array(), uv: auto_uv(face, p), layer: tex, color, light }
            });
            self.geometry(layer).quad(verts, false);
        }
    }

    fn plane(
        &mut self,
        state: BlockState,
        x: i32,
        y: i32,
        z: i32,
        corners: [[f32; 3]; 4],
        tint_override: Option<Tint>,
    ) {
        let (name, tint) = self.face_texture(state, Face::North, x, y, z);
        let tex = self.layer(name);
        let color = pack_color(self.tint(tint_override.unwrap_or(tint), x, z));
        let light = self.flat_light(x, y, z, None);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let verts: [Vertex; 4] = std::array::from_fn(|i| Vertex {
            pos: (origin + Vec3::from_array(corners[i])).to_array(),
            uv: uvs[i],
            layer: tex,
            color,
            light,
        });
        let layer = state.props().layer;
        self.geometry(layer).double_quad(verts);
    }

    fn cross(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let (a, b) = (0.05, 0.95);
        self.plane(state, x, y, z, [[a, 0.0, a], [b, 0.0, b], [b, 1.0, b], [a, 1.0, a]], None);
        self.plane(state, x, y, z, [[a, 0.0, b], [b, 0.0, a], [b, 1.0, a], [a, 1.0, b]], None);
    }

    fn crop(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let tint = if matches!(state.id(), 104 | 105) { None } else { Some(Tint::None) };
        for d in [0.25, 0.75] {
            self.plane(state, x, y, z, [[d, 0.0, 0.0], [d, 0.0, 1.0], [d, 1.0, 1.0], [d, 1.0, 0.0]], tint);
            self.plane(state, x, y, z, [[0.0, 0.0, d], [1.0, 0.0, d], [1.0, 1.0, d], [0.0, 1.0, d]], tint);
        }
    }

    fn vine(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let m = state.meta();
        let e = 0.05;
        let mut planes = Vec::new();
        if m & 1 != 0 {
            planes.push([[0.0, 0.0, 1.0 - e], [1.0, 0.0, 1.0 - e], [1.0, 1.0, 1.0 - e], [0.0, 1.0, 1.0 - e]]);
        }
        if m & 2 != 0 {
            planes.push([[e, 0.0, 0.0], [e, 0.0, 1.0], [e, 1.0, 1.0], [e, 1.0, 0.0]]);
        }
        if m & 4 != 0 {
            planes.push([[0.0, 0.0, e], [1.0, 0.0, e], [1.0, 1.0, e], [0.0, 1.0, e]]);
        }
        if m & 8 != 0 {
            planes.push([[1.0 - e, 0.0, 0.0], [1.0 - e, 0.0, 1.0], [1.0 - e, 1.0, 1.0], [1.0 - e, 1.0, 0.0]]);
        }
        if m == 0 {
            planes.push([[0.0, 1.0 - e, 0.0], [1.0, 1.0 - e, 0.0], [1.0, 1.0 - e, 1.0], [0.0, 1.0 - e, 1.0]]);
        }
        for p in planes {
            self.plane(state, x, y, z, p, None);
        }
    }

    fn rail(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let shape = if state.id() == 66 { state.meta() } else { state.meta() & 7 };
        let h = 1.0 / 16.0;
        // Corner heights NW, NE, SE, SW for slopes.
        let (nw, ne, se, sw) = match shape {
            2 => (h, 1.0 + h, 1.0 + h, h),
            3 => (1.0 + h, h, h, 1.0 + h),
            4 => (1.0 + h, 1.0 + h, h, h),
            5 => (h, h, 1.0 + h, 1.0 + h),
            _ => (h, h, h, h),
        };
        // Quarter turns of the texture: straight rails run north–south in it.
        let turns = match shape {
            1..=3 => 1,
            7 => 1,
            8 => 2,
            9 => 3,
            _ => 0,
        };
        let base = [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]];
        let uv: [[f32; 2]; 4] = std::array::from_fn(|i| base[(i + turns) % 4]);
        let (name, _) = self.face_texture(state, Face::Up, x, y, z);
        let tex = self.layer(name);
        let light = self.flat_light(x, y, z, None);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let corners = [[0.0, nw, 0.0], [0.0, sw, 1.0], [1.0, se, 1.0], [1.0, ne, 0.0]];
        let verts: [Vertex; 4] = std::array::from_fn(|i| Vertex {
            pos: (origin + Vec3::from_array(corners[i])).to_array(),
            uv: uv[i],
            layer: tex,
            color: [255; 4],
            light,
        });
        self.solid.double_quad(verts);
    }

    fn flat(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let h = 1.0 / 64.0;
        let (name, tint) = self.face_texture(state, Face::Up, x, y, z);
        let tex = self.layer(name);
        let color = pack_color(self.tint(tint, x, z));
        let light = self.flat_light(x, y, z, None);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let verts = CORNERS[Face::Up.index()].map(|c| {
            let p = Vec3::new(c[0], h, c[2]);
            Vertex { pos: (origin + p).to_array(), uv: auto_uv(Face::Up, p), layer: tex, color, light }
        });
        self.solid.double_quad(verts);
    }

    fn liquid(&mut self, state: BlockState, x: i32, y: i32, z: i32) {
        let water = state.is_water();
        let same = |s: BlockState| s.is_liquid() && s.is_water() == water;
        let layer = if water { Layer::Translucent } else { Layer::Solid };
        let above = self.s.at(x, y + 1, z);
        let local = Local { s: self.s, x, y, z };
        let [nw, ne, se, sw] = if same(above) { [1.0; 4] } else { block::liquid_corners(state, &local) };
        let still = self.layer(block::texture(state, Face::Up).0);
        let flow = self.layer(block::texture(state, Face::North).0);
        let origin = Vec3::new(x as f32, y as f32, z as f32);
        let vertex = |p: [f32; 3], uv: [f32; 2], tex: u32, shade: f32, light: [u8; 4]| Vertex {
            pos: (origin + Vec3::from_array(p)).to_array(),
            uv,
            layer: tex,
            color: pack_color(Vec3::splat(shade)),
            light,
        };
        if !same(above) {
            let light = self.flat_light(x, y, z, Some(Face::Up));
            let top = [[0.0, nw, 0.0], [0.0, sw, 1.0], [1.0, se, 1.0], [1.0, ne, 0.0]];
            let v = top.map(|p| vertex(p, [p[0], p[2]], still, 1.0, light));
            self.geometry(layer).double_quad(v);
        }
        for (face, quad) in [
            (Face::North, [[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, nw, 0.0], [1.0, ne, 0.0]]),
            (Face::South, [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, se, 1.0], [0.0, sw, 1.0]]),
            (Face::West, [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, sw, 1.0], [0.0, nw, 0.0]]),
            (Face::East, [[1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [1.0, ne, 0.0], [1.0, se, 1.0]]),
        ] {
            let [dx, _, dz] = face.offset();
            let other = self.s.at(x + dx, y, z + dz);
            if same(other) || other.props().opaque {
                continue;
            }
            let light = self.flat_light(x, y, z, Some(face));
            let v = quad.map(|p| vertex(p, auto_uv(face, Vec3::from_array(p)), flow, face.shade(), light));
            self.geometry(layer).quad(v, false);
        }
        let below = self.s.at(x, y - 1, z);
        if !same(below) && !below.props().opaque {
            let light = self.flat_light(x, y, z, Some(Face::Down));
            let v = CORNERS[Face::Down.index()]
                .map(|p| vertex(p, auto_uv(Face::Down, Vec3::from_array(p)), still, 0.5, light));
            self.geometry(layer).quad(v, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::textures::TextureData;
    use rustcraft_world::Chunk;

    fn world_with(blocks: &[(i32, i32, i32, BlockState)]) -> World {
        let mut w = World::new(true);
        w.insert_chunk(Chunk::new(0, 0));
        for &(x, y, z, s) in blocks {
            w.set_block(IVec3::new(x, y, z), s);
        }
        w
    }

    fn mesh_of(w: &World) -> MeshData {
        let data = TextureData::load(&[]);
        let snap = Snapshot::capture(w, IVec3::new(0, 4, 0), &data.biome_colors()).unwrap();
        mesh(&snap, &data.lookup)
    }

    #[test]
    fn single_cube_has_six_faces_and_neighbours_cull() {
        let stone = BlockState::new(1, 0);
        let one = mesh_of(&world_with(&[(5, 70, 5, stone)]));
        assert_eq!(one.solid.indices.len(), 6 * 6);
        assert!(one.translucent.is_empty());
        let two = mesh_of(&world_with(&[(5, 70, 5, stone), (6, 70, 5, stone)]));
        assert_eq!(two.solid.indices.len(), 10 * 6, "shared face is culled");
        assert!(Snapshot::capture(&world_with(&[]), IVec3::new(0, 4, 0), &BiomeColors::default()).is_none());
    }

    #[test]
    fn ambient_occlusion_darkens_corners() {
        let stone = BlockState::new(1, 0);
        // A floor with a wall: the floor's top corners next to the wall are darker.
        let w = world_with(&[(5, 70, 5, stone), (5, 71, 6, stone)]);
        let m = mesh_of(&w);
        let top: Vec<&Vertex> = m.solid.vertices.iter().filter(|v| v.pos[1] == 7.0 && v.color[0] > 0).collect();
        let near_wall = top.iter().filter(|v| v.pos[2] == 6.0).map(|v| v.color[0]).max().unwrap();
        let far = top.iter().filter(|v| v.pos[2] == 5.0 && v.pos[1] == 7.0).map(|v| v.color[0]).max().unwrap();
        assert!(near_wall < far, "{near_wall} vs {far}");
    }

    #[test]
    fn grass_gets_overlay_and_plants_are_double_sided() {
        let m = mesh_of(&world_with(&[(5, 70, 5, BlockState::new(2, 0))]));
        // 6 faces + 4 tinted side overlays.
        assert_eq!(m.solid.indices.len(), 10 * 6);
        let m = mesh_of(&world_with(&[(5, 70, 5, BlockState::new(31, 1))]));
        assert_eq!(m.solid.indices.len(), 4 * 6);
    }

    #[test]
    fn water_is_translucent_and_lowered() {
        let m = mesh_of(&world_with(&[(5, 70, 5, BlockState::new(9, 0))]));
        assert!(m.solid.is_empty());
        let max_y = m.translucent.vertices.iter().map(|v| v.pos[1]).fold(0.0, f32::max);
        assert!(max_y > 6.0 && max_y < 7.0, "{max_y}");
    }

    #[test]
    fn light_is_carried_to_vertices() {
        let w = world_with(&[(5, 70, 5, BlockState::new(1, 0)), (5, 71, 5, BlockState::new(50, 5))]);
        let m = mesh_of(&w);
        assert!(m.solid.vertices.iter().any(|v| v.light[1] >= 13 * 17));
        // Open sky above the stone.
        assert!(m.solid.vertices.iter().any(|v| v.light[0] == 255));
    }
}
