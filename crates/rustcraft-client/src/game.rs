//! Game state and rules for a local creative world: ticks, chunk
//! streaming, meshing, block interaction.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use glam::{DVec3, IVec3};

use rustcraft_renderer::mesh::{MeshData, Snapshot, mesh};
use rustcraft_renderer::{BiomeColors, Lookup, Renderer};
use rustcraft_world::block::{BlockState, Face, Shape};
use rustcraft_world::world::{At, selection};
use rustcraft_world::{Hit, World};

use crate::local::LocalWorld;
use crate::player::{MoveInput, Player};

/// Held keys and one-shot presses, filled by the window layer.
#[derive(Debug, Default, Clone)]
pub struct Controls {
    pub forward: bool,
    pub back: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
    pub attack: bool,
    pub attack_pressed: bool,
    pub use_pressed: bool,
    pub pick_pressed: bool,
    pub jump_pressed: bool,
}

struct MeshPool {
    jobs: Sender<Snapshot>,
    results: Receiver<MeshData>,
}

impl MeshPool {
    fn new(lookup: Lookup) -> Self {
        let (jobs, job_rx) = channel::<Snapshot>();
        let (result_tx, results) = channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get().saturating_sub(1).clamp(1, 6));
        for i in 0..workers {
            let (job_rx, result_tx, lookup) = (job_rx.clone(), result_tx.clone(), lookup.clone());
            std::thread::Builder::new()
                .name(format!("mesher-{i}"))
                .spawn(move || {
                    loop {
                        let job = job_rx.lock().unwrap().recv();
                        let Ok(snapshot) = job else { break };
                        if result_tx.send(mesh(&snapshot, &lookup)).is_err() {
                            break;
                        }
                    }
                })
                .expect("spawn mesher");
        }
        Self { jobs, results }
    }
}

pub const HOTBAR: [BlockState; 9] = [
    BlockState::new(1, 0),
    BlockState::new(4, 0),
    BlockState::new(3, 0),
    BlockState::new(2, 0),
    BlockState::new(5, 0),
    BlockState::new(17, 0),
    BlockState::new(20, 0),
    BlockState::new(50, 5),
    BlockState::new(89, 0),
];

pub struct Game {
    pub world: World,
    pub player: Player,
    pub local: LocalWorld,
    pub hotbar: [BlockState; 9],
    pub selected: usize,
    pub day_time: i64,
    pub ticks: u64,
    pub render_distance: u32,
    pub target: Option<Hit>,
    pub controls: Controls,
    pub paused: bool,
    dirty: HashSet<IVec3>,
    in_flight: HashSet<IVec3>,
    mesher: MeshPool,
    colors: BiomeColors,
    last_jump: Option<u64>,
    attack_cooldown: u32,
}

impl Game {
    pub fn new(local: LocalWorld, lookup: Lookup, colors: BiomeColors, render_distance: u32) -> Self {
        let (pos, yaw, pitch, flying) = local.spawn_position();
        let mut player = Player::new(pos);
        player.yaw = yaw;
        player.pitch = pitch;
        player.can_fly = true;
        player.flying = flying;
        Self {
            day_time: local.info.day_time,
            ticks: local.info.time.max(0) as u64,
            world: World::new(true),
            player,
            local,
            hotbar: HOTBAR,
            selected: 0,
            render_distance,
            target: None,
            controls: Controls::default(),
            paused: false,
            dirty: HashSet::new(),
            in_flight: HashSet::new(),
            mesher: MeshPool::new(lookup),
            colors,
            last_jump: None,
            attack_cooldown: 0,
        }
    }

    pub fn player_chunk(&self) -> (i32, i32) {
        ((self.player.pos.x.floor() as i32) >> 4, (self.player.pos.z.floor() as i32) >> 4)
    }

    /// The chunks around the player are present (so the player may move).
    pub fn spawn_area_ready(&self) -> bool {
        let (cx, cz) = self.player_chunk();
        (-1..=1).all(|dx| (-1..=1).all(|dz| self.world.is_loaded(cx + dx, cz + dz)))
    }

    /// Everything within the render distance is loaded and meshed.
    pub fn fully_loaded(&self) -> bool {
        let (cx, cz) = self.player_chunk();
        let r = self.render_distance as i32;
        self.dirty.is_empty()
            && self.in_flight.is_empty()
            && (-r..=r).all(|dx| (-r..=r).all(|dz| self.world.is_loaded(cx + dx, cz + dz)))
    }

    fn move_input(&self) -> MoveInput {
        let c = &self.controls;
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        MoveInput {
            forward: axis(c.forward, c.back),
            strafe: axis(c.left, c.right),
            jump: c.jump,
            sneak: c.sneak,
            sprint: c.sprint,
        }
    }

    pub fn tick(&mut self) {
        if self.paused || !self.spawn_area_ready() {
            self.clear_presses();
            return;
        }
        self.ticks += 1;
        self.day_time += 1;
        if self.controls.jump_pressed && self.player.can_fly {
            // Double-tapping jump toggles creative flight.
            if self.last_jump.is_some_and(|t| self.ticks - t <= 7) {
                self.player.flying = !self.player.flying;
                self.last_jump = None;
            } else {
                self.last_jump = Some(self.ticks);
            }
        }
        let input = self.move_input();
        self.player.tick(&self.world, &input);
        self.update_target();

        if self.attack_cooldown > 0 {
            self.attack_cooldown -= 1;
        }
        if (self.controls.attack_pressed || (self.controls.attack && self.attack_cooldown == 0))
            && let Some(hit) = self.target
        {
            self.set_block(hit.block, BlockState::AIR);
            self.attack_cooldown = 5;
            self.update_target();
        }
        if self.controls.use_pressed {
            self.place();
        }
        if self.controls.pick_pressed
            && let Some(hit) = self.target
        {
            self.hotbar[self.selected] = self.world.block(hit.block);
        }
        self.clear_presses();
        if self.local.wants_save() {
            self.save();
        }
    }

    fn clear_presses(&mut self) {
        self.controls.attack_pressed = false;
        self.controls.use_pressed = false;
        self.controls.pick_pressed = false;
        self.controls.jump_pressed = false;
    }

    pub fn update_target(&mut self) {
        let reach = 5.0;
        self.target = self.world.raycast(self.player.eye(), self.player.look(), reach);
    }

    fn place(&mut self) {
        let Some(hit) = self.target else { return };
        let item = self.hotbar[self.selected];
        let existing = self.world.block(hit.block);
        let replaceable = |s: BlockState| s.is_air() || s.is_liquid() || matches!(s.id(), 31 | 51 | 78 | 106);
        let cell = if replaceable(existing) { hit.block } else { hit.block + IVec3::from_array(hit.face.offset()) };
        if !replaceable(self.world.block(cell)) || !(0..256).contains(&cell.y) {
            return;
        }
        let state = oriented(item, hit.face, hit.point.y - hit.point.y.floor(), self.player.yaw);
        let local = At { world: &self.world, pos: cell };
        if state.props().solid {
            let player = self.player.bounds();
            let blocked = rustcraft_world::block::collision(state, &local).into_iter().any(|b| {
                let o = cell.as_dvec3();
                rustcraft_world::Bounds::new(o + b.min.as_dvec3(), o + b.max.as_dvec3()).intersects(&player)
            });
            if blocked {
                return;
            }
        }
        // Torches, flowers and similar need solid ground or a wall.
        if matches!(state.props().shape, Shape::Torch | Shape::Cross) {
            let support = cell - IVec3::from_array(hit.face.offset());
            if !self.world.block(support).props().opaque {
                return;
            }
        }
        self.set_block(cell, state);
        self.update_target();
    }

    /// Changes a block and schedules re-meshing of everything its light
    /// could have reached.
    pub fn set_block(&mut self, pos: IVec3, state: BlockState) {
        if self.world.set_block(pos, state) {
            self.local.mark_modified(pos.x >> 4, pos.z >> 4);
            let s = pos >> 4;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        self.dirty.insert(s + IVec3::new(dx, dy, dz));
                    }
                }
            }
        }
    }

    fn mark_chunk_dirty(&mut self, cx: i32, cz: i32) {
        for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            for sy in 0..16 {
                self.dirty.insert(IVec3::new(cx + dx, sy, cz + dz));
            }
        }
    }

    /// Per-frame work: stream chunks, queue meshing, upload finished meshes.
    pub fn update(&mut self, renderer: &mut Renderer) {
        let center = self.player_chunk();
        let before: HashSet<(i32, i32)> = self.world.chunks().map(|c| (c.x, c.z)).collect();
        let added = self.local.stream(&mut self.world, center, self.render_distance as i32 + 1, 6);
        for (cx, cz) in &added {
            self.mark_chunk_dirty(*cx, *cz);
        }
        if before.len() + added.len() != self.world.chunk_count() {
            for (cx, cz) in before {
                if !self.world.is_loaded(cx, cz) {
                    renderer.remove_column(cx, cz);
                }
            }
        }

        let (pcx, pcz) = center;
        let py = (self.player.pos.y as i32) >> 4;
        let mut queue: Vec<IVec3> = self.dirty.iter().copied().filter(|s| !self.in_flight.contains(s)).collect();
        queue.sort_by_key(|s| (s.x - pcx).pow(2) + (s.z - pcz).pow(2) + (s.y - py).pow(2));
        for s in queue.into_iter().take(48) {
            self.dirty.remove(&s);
            if !(0..16).contains(&s.y) {
                continue;
            }
            // Only mesh once all neighbours exist, so borders are final.
            let neighbours_loaded = (-1..=1).all(|dx| (-1..=1).all(|dz| self.world.is_loaded(s.x + dx, s.z + dz)));
            if !neighbours_loaded {
                if self.world.is_loaded(s.x, s.z)
                    && (s.x - pcx).abs() <= self.render_distance as i32
                    && (s.z - pcz).abs() <= self.render_distance as i32
                {
                    self.dirty.insert(s);
                }
                continue;
            }
            match Snapshot::capture(&self.world, s, &self.colors) {
                Some(snapshot) => {
                    self.in_flight.insert(s);
                    let _ = self.mesher.jobs.send(snapshot);
                }
                None => renderer.remove_section(s),
            }
        }
        // Keep re-queued edge sections from spinning: retry next frame only.
        while let Ok(mesh) = self.mesher.results.try_recv() {
            self.in_flight.remove(&mesh.section);
            if self.world.is_loaded(mesh.section.x, mesh.section.z) {
                renderer.upload(&mesh);
            }
        }
    }

    pub fn save(&mut self) {
        if let Err(e) = self.local.save(&self.world, &self.player, self.day_time, self.ticks as i64) {
            tracing::error!("saving the world failed: {e:#}");
        }
    }

    /// Interpolated camera position between the last two ticks.
    pub fn camera_pos(&self, alpha: f64) -> DVec3 {
        self.player.prev_pos.lerp(self.player.pos, alpha) + DVec3::new(0.0, self.player.eye_height(), 0.0)
    }

    pub fn selection_boxes(&self) -> Option<(IVec3, Vec<rustcraft_world::block::Aabb>)> {
        let hit = self.target?;
        let state = self.world.block(hit.block);
        Some((hit.block, selection(state, &At { world: &self.world, pos: hit.block })))
    }
}

/// Adjusts placement metadata for blocks that face or align (torches, logs,
/// slabs, stairs, pumpkins, furnaces).
fn oriented(item: BlockState, face: Face, hit_y: f64, yaw: f32) -> BlockState {
    let id = item.id();
    let m = item.meta();
    // Direction the player faces: 0 south, 1 west, 2 north, 3 east.
    let facing = ((yaw / 90.0 + 0.5).floor() as i32).rem_euclid(4) as u8;
    let meta = match id {
        50 | 75 | 76 => match face {
            Face::East => 1,
            Face::West => 2,
            Face::South => 3,
            Face::North => 4,
            _ => 5,
        },
        17 | 162 | 170 => {
            (m & 3)
                | match face {
                    Face::West | Face::East => 4,
                    Face::North | Face::South => 8,
                    _ => 0,
                }
        }
        44 | 126 | 182 => (m & 7) | if face == Face::Down || (face != Face::Up && hit_y > 0.5) { 8 } else { 0 },
        53 | 67 | 108 | 109 | 114 | 128 | 134..=136 | 156 | 163 | 164 | 180 => {
            let dir = [2, 1, 3, 0][facing as usize];
            dir | if face == Face::Down || (face != Face::Up && hit_y > 0.5) { 4 } else { 0 }
        }
        86 | 91 => (facing + 2) % 4,
        23 | 54 | 61 | 62 | 130 | 146 | 158 => [2, 5, 3, 4][facing as usize],
        _ => m,
    };
    BlockState::new(id, meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_orientation() {
        assert_eq!(oriented(BlockState::new(50, 0), Face::Up, 0.0, 0.0).meta(), 5);
        assert_eq!(oriented(BlockState::new(50, 0), Face::North, 0.5, 0.0).meta(), 4);
        assert_eq!(oriented(BlockState::new(17, 1), Face::East, 0.5, 0.0).meta(), 5);
        assert_eq!(oriented(BlockState::new(44, 3), Face::South, 0.7, 0.0).meta(), 11);
        assert_eq!(oriented(BlockState::new(44, 3), Face::Up, 0.0, 0.0).meta(), 3);
        // Facing south, a furnace's front points back at the player (north).
        assert_eq!(oriented(BlockState::new(61, 0), Face::Up, 0.0, 0.0).meta(), 2);
    }
}
