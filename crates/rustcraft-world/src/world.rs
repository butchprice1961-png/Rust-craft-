//! Loaded chunks, block access, light propagation, ray casts and collision
//! queries in world coordinates.

use std::collections::{HashMap, VecDeque};

use glam::{DVec3, IVec3};

use crate::block::{self, Aabb, BlockState, Face, Neighbors, Shape};
use crate::chunk::{Chunk, index};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LightKind {
    Block,
    Sky,
}

/// An axis-aligned box in world space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Bounds {
    pub min: DVec3,
    pub max: DVec3,
}

impl Bounds {
    pub fn new(min: DVec3, max: DVec3) -> Self {
        Self { min, max }
    }

    fn of(b: Aabb, at: IVec3) -> Self {
        let o = at.as_dvec3();
        Self { min: o + b.min.as_dvec3(), max: o + b.max.as_dvec3() }
    }

    pub fn intersects(&self, o: &Bounds) -> bool {
        self.min.cmplt(o.max).all() && self.max.cmpgt(o.min).all()
    }

    pub fn expand(&self, d: DVec3) -> Bounds {
        Bounds { min: self.min + d.min(DVec3::ZERO), max: self.max + d.max(DVec3::ZERO) }
    }

    pub fn offset(&self, d: DVec3) -> Bounds {
        Bounds { min: self.min + d, max: self.max + d }
    }

    /// How far `mover` can travel by `offset` along `axis` before touching
    /// this box (the vanilla `calculate{X,Y,Z}Offset`).
    pub fn clip(&self, mover: &Bounds, axis: usize, offset: f64) -> f64 {
        let (a, b) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let overlaps = |i: usize| mover.max[i] > self.min[i] && mover.min[i] < self.max[i];
        if !overlaps(a) || !overlaps(b) {
            return offset;
        }
        if offset > 0.0 && mover.max[axis] <= self.min[axis] {
            offset.min(self.min[axis] - mover.max[axis])
        } else if offset < 0.0 && mover.min[axis] >= self.max[axis] {
            offset.max(self.max[axis] - mover.min[axis])
        } else {
            offset
        }
    }

    /// Ray entry distance and face, if the ray hits this box.
    fn ray(&self, from: DVec3, inv: DVec3) -> Option<(f64, Face)> {
        let t1 = (self.min - from) * inv;
        let t2 = (self.max - from) * inv;
        let lo = t1.min(t2);
        let hi = t1.max(t2);
        let enter = lo.max_element();
        let exit = hi.min_element();
        if enter > exit || exit < 0.0 {
            return None;
        }
        let face = if enter == lo.x {
            if inv.x > 0.0 { Face::West } else { Face::East }
        } else if enter == lo.y {
            if inv.y > 0.0 { Face::Down } else { Face::Up }
        } else if inv.z > 0.0 {
            Face::North
        } else {
            Face::South
        };
        Some((enter.max(0.0), face))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hit {
    pub block: IVec3,
    pub face: Face,
    pub point: DVec3,
    pub distance: f64,
}

/// Neighbour lookups around one block, for shape and model functions.
pub struct At<'a> {
    pub world: &'a World,
    pub pos: IVec3,
}

impl Neighbors for At<'_> {
    fn get(&self, dx: i32, dy: i32, dz: i32) -> BlockState {
        self.world.block(self.pos + IVec3::new(dx, dy, dz))
    }
}

#[derive(Default)]
pub struct World {
    chunks: HashMap<(i32, i32), Chunk>,
    /// Whether this dimension has sky light (overworld).
    pub has_sky: bool,
}

#[inline]
fn split(p: IVec3) -> ((i32, i32), usize, usize) {
    ((p.x >> 4, p.z >> 4), (p.x & 15) as usize, (p.z & 15) as usize)
}

impl World {
    pub fn new(has_sky: bool) -> Self {
        Self { chunks: HashMap::new(), has_sky }
    }

    pub fn chunk(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        self.chunks.get(&(cx, cz))
    }

    pub fn chunk_mut(&mut self, cx: i32, cz: i32) -> Option<&mut Chunk> {
        self.chunks.get_mut(&(cx, cz))
    }

    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_loaded(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&(cx, cz))
    }

    pub fn block(&self, p: IVec3) -> BlockState {
        let (key, x, z) = split(p);
        self.chunks.get(&key).map_or(BlockState::AIR, |c| c.block(x, p.y, z))
    }

    pub fn light(&self, kind: LightKind, p: IVec3) -> u8 {
        if p.y >= 256 {
            return if kind == LightKind::Sky && self.has_sky { 15 } else { 0 };
        }
        if p.y < 0 {
            return 0;
        }
        let (key, x, z) = split(p);
        let Some(chunk) = self.chunks.get(&key) else {
            return if kind == LightKind::Sky && self.has_sky { 15 } else { 0 };
        };
        match &chunk.sections[(p.y >> 4) as usize] {
            Some(s) => {
                let i = index(x, (p.y & 15) as usize, z);
                if kind == LightKind::Sky { s.sky_light(i) } else { s.block_light(i) }
            }
            None => {
                if kind == LightKind::Sky && self.has_sky {
                    15
                } else {
                    0
                }
            }
        }
    }

    fn set_light(&mut self, kind: LightKind, p: IVec3, v: u8) {
        if !(0..256).contains(&p.y) {
            return;
        }
        let sky = self.has_sky;
        let (key, x, z) = split(p);
        let Some(chunk) = self.chunks.get_mut(&key) else {
            return;
        };
        let section = chunk.sections[(p.y >> 4) as usize]
            .get_or_insert_with(|| Box::new(crate::chunk::Section::new(if sky { 15 } else { 0 })));
        let i = index(x, (p.y & 15) as usize, z);
        if kind == LightKind::Sky { section.set_sky_light(i, v) } else { section.set_block_light(i, v) }
    }

    fn opacity(&self, p: IVec3) -> u8 {
        let props = self.block(p).props();
        if props.opaque { 15 } else { props.light_opacity }
    }

    fn in_world(&self, p: IVec3) -> bool {
        (0..256).contains(&p.y) && self.chunks.contains_key(&(p.x >> 4, p.z >> 4))
    }

    /// Adds a chunk. Chunks without light data (generated or loaded from
    /// disk) are lit; light then flows across the seams to loaded
    /// neighbours.
    pub fn insert_chunk(&mut self, mut chunk: Chunk) {
        let (cx, cz) = (chunk.x, chunk.z);
        if chunk.needs_light {
            crate::light::light_chunk_local(&mut chunk, self.has_sky);
        }
        self.chunks.insert((cx, cz), chunk);
        self.stitch_light(cx, cz);
    }

    pub fn remove_chunk(&mut self, cx: i32, cz: i32) -> Option<Chunk> {
        self.chunks.remove(&(cx, cz))
    }

    /// Propagates light across the borders between a chunk and its neighbours.
    fn stitch_light(&mut self, cx: i32, cz: i32) {
        let mut seeds = VecDeque::new();
        let (x0, z0) = (cx * 16, cz * 16);
        for y in 0..256 {
            for i in 0..16 {
                for p in [
                    IVec3::new(x0 + i, y, z0),
                    IVec3::new(x0 + i, y, z0 + 15),
                    IVec3::new(x0, y, z0 + i),
                    IVec3::new(x0 + 15, y, z0 + i),
                    IVec3::new(x0 + i, y, z0 - 1),
                    IVec3::new(x0 + i, y, z0 + 16),
                    IVec3::new(x0 - 1, y, z0 + i),
                    IVec3::new(x0 + 16, y, z0 + i),
                ] {
                    if self.in_world(p) {
                        seeds.push_back(p);
                    }
                }
            }
        }
        for kind in self.kinds() {
            let mut q = seeds.clone();
            self.propagate(kind, &mut q);
        }
    }

    fn kinds(&self) -> Vec<LightKind> {
        if self.has_sky { vec![LightKind::Block, LightKind::Sky] } else { vec![LightKind::Block] }
    }

    /// Spreads light outward from every queued position.
    fn propagate(&mut self, kind: LightKind, queue: &mut VecDeque<IVec3>) {
        while let Some(p) = queue.pop_front() {
            let level = self.light(kind, p);
            if level <= 1 {
                continue;
            }
            for face in Face::ALL {
                let n = p + IVec3::from_array(face.offset());
                if !self.in_world(n) {
                    continue;
                }
                let op = self.opacity(n);
                if op >= 15 {
                    continue;
                }
                let new = if kind == LightKind::Sky && face == Face::Down && level == 15 && op == 0 {
                    15
                } else {
                    level.saturating_sub(op.max(1))
                };
                if new > self.light(kind, n) {
                    self.set_light(kind, n, new);
                    queue.push_back(n);
                }
            }
        }
    }

    /// Darkens everything lit from `start`, collecting independently lit
    /// border cells to re-propagate from.
    fn unlight(&mut self, kind: LightKind, start: IVec3, old: u8, relight: &mut VecDeque<IVec3>) {
        let mut q = VecDeque::from([(start, old)]);
        while let Some((p, level)) = q.pop_front() {
            for face in Face::ALL {
                let n = p + IVec3::from_array(face.offset());
                if !self.in_world(n) {
                    continue;
                }
                let nl = self.light(kind, n);
                if nl == 0 {
                    continue;
                }
                let sunbeam = kind == LightKind::Sky && face == Face::Down && level == 15 && nl == 15;
                if nl < level || sunbeam {
                    self.set_light(kind, n, 0);
                    q.push_back((n, nl));
                    let emission = self.block(n).props().light_emission;
                    if kind == LightKind::Block && emission > 0 {
                        self.set_light(kind, n, emission);
                        relight.push_back(n);
                    }
                } else {
                    relight.push_back(n);
                }
            }
        }
    }

    /// Sets a block and updates light around it. Returns false if unchanged
    /// or the chunk is not loaded.
    pub fn set_block(&mut self, p: IVec3, state: BlockState) -> bool {
        let (key, x, z) = split(p);
        let Some(chunk) = self.chunks.get_mut(&key) else {
            return false;
        };
        if !(0..256).contains(&p.y) || chunk.block(x, p.y, z) == state {
            return false;
        }
        chunk.set_block(x, p.y, z, state);
        for kind in self.kinds() {
            let old = self.light(kind, p);
            let mut relight = VecDeque::new();
            if old > 0 {
                self.set_light(kind, p, 0);
                self.unlight(kind, p, old, &mut relight);
            }
            let own = match kind {
                LightKind::Block => state.props().light_emission,
                LightKind::Sky => {
                    let open = p.y == 255 || self.light(LightKind::Sky, p + IVec3::Y) == 15;
                    if open && self.opacity(p) == 0 { 15 } else { 0 }
                }
            };
            self.set_light(kind, p, own);
            relight.push_back(p);
            for face in Face::ALL {
                let n = p + IVec3::from_array(face.offset());
                if self.in_world(n) {
                    relight.push_back(n);
                }
            }
            self.propagate(kind, &mut relight);
        }
        true
    }

    /// The first block a ray hits within `max` blocks, using each block's
    /// selection shape.
    pub fn raycast(&self, from: DVec3, dir: DVec3, max: f64) -> Option<Hit> {
        let dir = dir.normalize_or_zero();
        if dir == DVec3::ZERO {
            return None;
        }
        let inv = DVec3::ONE / dir;
        let mut cell = from.floor().as_ivec3();
        let step = dir.signum().as_ivec3();
        let next_boundary = |c: i32, s: i32| if s > 0 { (c + 1) as f64 } else { c as f64 };
        let mut t_max = DVec3::new(
            if dir.x != 0.0 { (next_boundary(cell.x, step.x) - from.x) * inv.x } else { f64::INFINITY },
            if dir.y != 0.0 { (next_boundary(cell.y, step.y) - from.y) * inv.y } else { f64::INFINITY },
            if dir.z != 0.0 { (next_boundary(cell.z, step.z) - from.z) * inv.z } else { f64::INFINITY },
        );
        let t_delta = inv.abs();
        loop {
            let state = self.block(cell);
            if !state.is_air() {
                let best = selection(state, &At { world: self, pos: cell })
                    .into_iter()
                    .filter_map(|b| Bounds::of(b, cell).ray(from, inv))
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((t, face)) = best
                    && t <= max
                {
                    return Some(Hit { block: cell, face, point: from + dir * t, distance: t });
                }
            }
            let axis = if t_max.x < t_max.y && t_max.x < t_max.z {
                0
            } else if t_max.y < t_max.z {
                1
            } else {
                2
            };
            if t_max[axis] > max {
                return None;
            }
            cell[axis] += step[axis];
            t_max[axis] += t_delta[axis];
        }
    }

    /// Collision boxes of every block overlapping `area`.
    pub fn collisions(&self, area: &Bounds) -> Vec<Bounds> {
        let lo = area.min.floor().as_ivec3() - IVec3::Y;
        let hi = area.max.ceil().as_ivec3();
        let mut out = Vec::new();
        for x in lo.x..hi.x {
            for z in lo.z..hi.z {
                for y in lo.y.max(0)..hi.y.min(256) {
                    let p = IVec3::new(x, y, z);
                    let state = self.block(p);
                    if !state.props().solid {
                        continue;
                    }
                    for b in block::collision(state, &At { world: self, pos: p }) {
                        let wb = Bounds::of(b, p);
                        if wb.intersects(area) {
                            out.push(wb);
                        }
                    }
                }
            }
        }
        out
    }
}

/// Shapes the cursor can target: real geometry, or a small box for plants.
pub fn selection(state: BlockState, n: &impl Neighbors) -> Vec<Aabb> {
    let p = state.props();
    match p.shape {
        Shape::None | Shape::Liquid => Vec::new(),
        Shape::Cross | Shape::Crop => vec![Aabb::new(0.2, 0.0, 0.2, 0.8, 0.8, 0.8)],
        Shape::Rail | Shape::Flat => vec![Aabb::new(0.0, 0.0, 0.0, 1.0, 0.125, 1.0)],
        Shape::Vine => vec![Aabb::FULL],
        _ => {
            let b = block::boxes(state, n);
            if b.is_empty() { vec![Aabb::FULL] } else { b }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_world() -> World {
        let mut w = World::new(true);
        for cx in -1..=1 {
            for cz in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for x in 0..16 {
                    for z in 0..16 {
                        for y in 0..4 {
                            c.set_block(x, y, z, BlockState::new(1, 0));
                        }
                    }
                }
                w.insert_chunk(c);
            }
        }
        w
    }

    #[test]
    fn sky_light_and_shadows() {
        let mut w = flat_world();
        assert_eq!(w.light(LightKind::Sky, IVec3::new(0, 4, 0)), 15);
        assert_eq!(w.light(LightKind::Sky, IVec3::new(0, 3, 0)), 0);
        // A roof over (0,4,0) shades it; light then falls off sideways.
        w.set_block(IVec3::new(0, 6, 0), BlockState::new(1, 0));
        assert_eq!(w.light(LightKind::Sky, IVec3::new(0, 5, 0)), 14);
        assert_eq!(w.light(LightKind::Sky, IVec3::new(0, 4, 0)), 14);
        // Removing it restores full sunlight.
        w.set_block(IVec3::new(0, 6, 0), BlockState::AIR);
        assert_eq!(w.light(LightKind::Sky, IVec3::new(0, 4, 0)), 15);
    }

    #[test]
    fn block_light_spreads_and_clears_across_chunks() {
        let mut w = flat_world();
        let torch = IVec3::new(15, 4, 0);
        w.set_block(torch, BlockState::new(50, 5));
        assert_eq!(w.light(LightKind::Block, torch), 14);
        assert_eq!(w.light(LightKind::Block, IVec3::new(16, 4, 0)), 13);
        assert_eq!(w.light(LightKind::Block, IVec3::new(20, 4, 0)), 9);
        w.set_block(torch, BlockState::AIR);
        assert_eq!(w.light(LightKind::Block, IVec3::new(16, 4, 0)), 0);
        assert_eq!(w.light(LightKind::Block, torch), 0);
    }

    #[test]
    fn raycast_hits_faces_and_partial_blocks() {
        let mut w = flat_world();
        let hit = w.raycast(DVec3::new(0.5, 10.0, 0.5), DVec3::NEG_Y, 20.0).unwrap();
        assert_eq!((hit.block, hit.face), (IVec3::new(0, 3, 0), Face::Up));
        assert!((hit.distance - 6.0).abs() < 1e-9);
        // A bottom slab is hit at its half height.
        w.set_block(IVec3::new(2, 4, 0), BlockState::new(44, 0));
        let hit = w.raycast(DVec3::new(2.5, 10.0, 0.5), DVec3::NEG_Y, 20.0).unwrap();
        assert_eq!(hit.block, IVec3::new(2, 4, 0));
        assert!((hit.point.y - 4.5).abs() < 1e-9);
        // Sideways into a wall.
        w.set_block(IVec3::new(5, 5, 0), BlockState::new(1, 0));
        let hit = w.raycast(DVec3::new(0.5, 5.5, 0.5), DVec3::X, 10.0).unwrap();
        assert_eq!((hit.block, hit.face), (IVec3::new(5, 5, 0), Face::West));
        assert!(w.raycast(DVec3::new(0.5, 10.0, 0.5), DVec3::Y, 50.0).is_none());
    }

    #[test]
    fn collisions_include_fence_height() {
        let mut w = flat_world();
        w.set_block(IVec3::new(0, 4, 0), BlockState::new(85, 0));
        let area = Bounds::new(DVec3::new(0.0, 5.2, 0.0), DVec3::new(1.0, 5.4, 1.0));
        let boxes = w.collisions(&area);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].max.y, 5.5);
    }
}
