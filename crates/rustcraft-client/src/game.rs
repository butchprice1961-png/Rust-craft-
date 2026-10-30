//! Game state and rules: ticks, chunk streaming, meshing and block
//! interaction, for a local creative world or a server.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{DVec3, IVec3};

use rustcraft_protocol::packets::send::{Action, Dig};
use rustcraft_protocol::{Clientbound, GameMode, send};
use rustcraft_renderer::mesh::{MeshData, Snapshot, mesh};
use rustcraft_renderer::{BiomeColors, Lookup, Renderer};
use rustcraft_world::block::{BlockState, Face, Shape, break_ticks};
use rustcraft_world::world::{At, selection};
use rustcraft_world::{Hit, World};

use crate::local::LocalWorld;
use crate::player::{MoveInput, Player};
use crate::remote::{Digging, Entity, Remote, block_of, item_of};

/// Where the world comes from.
pub enum Source {
    Local(Box<LocalWorld>),
    Remote(Box<Remote>),
}

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
    pub source: Source,
    /// Server packets not yet applied; applied within a time budget per frame.
    pending: VecDeque<Clientbound>,
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
    /// The player's chunk at the last update.
    last_center: Option<(i32, i32)>,
    /// Renderer work from server packets, done in the next update.
    clear_renderer: bool,
    unloaded: Vec<(i32, i32)>,
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
        let (day_time, ticks) = (local.info.day_time, local.info.time.max(0) as u64);
        Self::with_source(Source::Local(Box::new(local)), player, day_time, ticks, lookup, colors, render_distance)
    }

    /// A game on a server; the server places the player once it has joined.
    pub fn new_remote(remote: Remote, lookup: Lookup, colors: BiomeColors, render_distance: u32) -> Self {
        let player = Player::new(DVec3::new(0.5, 100.0, 0.5));
        Self::with_source(Source::Remote(Box::new(remote)), player, 6000, 0, lookup, colors, render_distance)
    }

    fn with_source(
        source: Source,
        player: Player,
        day_time: i64,
        ticks: u64,
        lookup: Lookup,
        colors: BiomeColors,
        render_distance: u32,
    ) -> Self {
        Self {
            day_time,
            ticks,
            world: World::new(true),
            player,
            source,
            pending: VecDeque::new(),
            hotbar: HOTBAR,
            selected: 0,
            render_distance,
            target: None,
            controls: Controls::default(),
            paused: false,
            dirty: HashSet::new(),
            in_flight: HashSet::new(),
            last_center: None,
            clear_renderer: false,
            unloaded: Vec::new(),
            mesher: MeshPool::new(lookup),
            colors,
            last_jump: None,
            attack_cooldown: 0,
        }
    }

    pub fn remote(&mut self) -> Option<&mut Remote> {
        match &mut self.source {
            Source::Remote(r) => Some(r),
            Source::Local(_) => None,
        }
    }

    pub fn remote_ref(&self) -> Option<&Remote> {
        match &self.source {
            Source::Remote(r) => Some(r),
            Source::Local(_) => None,
        }
    }

    pub fn player_chunk(&self) -> (i32, i32) {
        ((self.player.pos.x.floor() as i32) >> 4, (self.player.pos.z.floor() as i32) >> 4)
    }

    /// The chunks around the player are present (so the player may move).
    pub fn spawn_area_ready(&self) -> bool {
        let (cx, cz) = self.player_chunk();
        self.remote_ref().is_none_or(|r| r.positioned)
            && (-1..=1).all(|dx| (-1..=1).all(|dz| self.world.is_loaded(cx + dx, cz + dz)))
    }

    /// Everything within the render distance is loaded and meshed (on a
    /// server: everything received so far).
    pub fn fully_loaded(&self) -> bool {
        if self.remote_ref().is_some() {
            // Edge sections wait for neighbours the server may never send.
            return self.spawn_area_ready()
                && self.pending.is_empty()
                && self.in_flight.is_empty()
                && self.dirty.iter().all(|s| !self.neighbours_loaded(*s));
        }
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
        // A server's world keeps running while the menu is open.
        let multiplayer = self.remote_ref().is_some();
        if (self.paused && !multiplayer) || !self.spawn_area_ready() {
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
        if multiplayer {
            self.remote_actions();
            self.clear_presses();
            return;
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
        if let Source::Local(l) = &self.source
            && l.wants_save()
        {
            self.save();
        }
    }

    /// Breaking, placing and movement on a server: sent to the server and
    /// shown at once; the server's block updates have the last word.
    fn remote_actions(&mut self) {
        let creative = self.remote_ref().is_some_and(Remote::creative);
        if let Some(r) = self.remote_ref()
            && !creative
        {
            self.hotbar = r.hotbar();
        }
        if creative {
            if (self.controls.attack_pressed || (self.controls.attack && self.attack_cooldown == 0))
                && let Some(hit) = self.target
            {
                let r = self.remote().unwrap();
                r.send(send::dig(Dig::Start, hit.block, hit.face.index() as u8));
                r.send(send::swing());
                self.set_block(hit.block, BlockState::AIR);
                self.attack_cooldown = 5;
                self.update_target();
            }
        } else if self.remote_ref().is_some_and(|r| r.mode != GameMode::Spectator) {
            self.survival_dig();
        }
        if self.controls.use_pressed
            && let Some(hit) = self.target
        {
            self.remote_place(hit, creative);
        }
        if self.controls.pick_pressed
            && creative
            && let Some(hit) = self.target
        {
            self.hotbar[self.selected] = self.world.block(hit.block);
        }
        self.sync_movement();
    }

    /// Survival digging: progress per tick as in 1.8, then "finished".
    fn survival_dig(&mut self) {
        let slot = 36 + self.selected;
        let held = self.remote_ref().and_then(|r| r.inventory[slot]).map_or(0, |i| i.id);
        let (on_ground, in_water) = (self.player.on_ground, self.player.in_water);
        let target = self.target.filter(|_| self.controls.attack);
        let current = self.remote_ref().and_then(|r| r.digging.as_ref().map(|d| d.pos));
        let mut broken = None;
        match (target, current) {
            (None, Some(_)) => {
                let r = self.remote().unwrap();
                let d = r.digging.take().unwrap();
                r.send(send::dig(Dig::Cancel, d.pos, d.face));
            }
            (Some(hit), current) if current != Some(hit.block) && self.attack_cooldown == 0 => {
                let state = self.world.block(hit.block);
                let ticks = break_ticks(state, held, on_ground, in_water);
                let face = hit.face.index() as u8;
                let r = self.remote().unwrap();
                if let Some(old) = r.digging.take() {
                    r.send(send::dig(Dig::Cancel, old.pos, old.face));
                }
                if let Some(total) = ticks {
                    r.send(send::dig(Dig::Start, hit.block, face));
                    r.send(send::swing());
                    // Fast enough to break on the first tick: the server does it.
                    if total <= 1 {
                        broken = Some(hit.block);
                    } else {
                        r.digging = Some(Digging { pos: hit.block, face, elapsed: 0, total });
                    }
                }
            }
            (Some(_), Some(_)) => {
                let r = self.remote().unwrap();
                let d = r.digging.as_mut().unwrap();
                d.elapsed += 1;
                let (done, swing) = (d.elapsed >= d.total, d.elapsed.is_multiple_of(4));
                if swing {
                    r.send(send::swing());
                }
                if done {
                    let d = r.digging.take().unwrap();
                    r.send(send::dig(Dig::Finish, d.pos, d.face));
                    broken = Some(d.pos);
                }
            }
            _ => {}
        }
        if let Some(pos) = broken {
            self.set_block(pos, BlockState::AIR);
            self.attack_cooldown = 5;
            self.update_target();
        }
    }

    fn remote_place(&mut self, hit: Hit, creative: bool) {
        let slot = 36 + self.selected;
        let held = if creative {
            let state = self.hotbar[self.selected];
            (!state.is_air()).then(|| item_of(state))
        } else {
            self.remote_ref().and_then(|r| r.inventory[slot])
        };
        let offset = hit.point - hit.block.as_dvec3();
        let cursor = (offset * 16.0).to_array().map(|v| v.clamp(0.0, 15.0) as u8);
        let r = self.remote().unwrap();
        if creative && r.inventory[slot] != held {
            // Creative players choose their own items; tell the server first.
            r.send(send::creative_slot(slot as i16, held));
            r.inventory[slot] = held;
        }
        r.send(send::place(hit.block, hit.face.index() as u8, held, cursor));
        r.send(send::swing());
        if held.is_some_and(|i| !block_of(i).is_air()) {
            self.place();
        }
    }

    /// Movement packets as the 1.8 client sends them: every tick, with the
    /// position when it changed (and at least once a second).
    fn sync_movement(&mut self) {
        let p = &self.player;
        let (pos, look, on_ground) = (p.pos, (p.yaw, p.pitch), p.on_ground);
        let (flying, sneaking, sprinting) = (p.flying, p.sneaking, p.sprinting);
        let r = self.remote().unwrap();
        let moved = (pos - r.last_pos).length_squared() > 9e-4 || r.since_position >= 20;
        let rotated = look != r.last_look;
        r.send(match (moved, rotated) {
            (true, true) => send::position_look(pos, look.0, look.1, on_ground),
            (true, false) => send::position(pos, on_ground),
            (false, true) => send::look(look.0, look.1, on_ground),
            (false, false) => send::on_ground(on_ground),
        });
        if moved {
            (r.last_pos, r.since_position) = (pos, 0);
        } else {
            r.since_position += 1;
        }
        r.last_look = look;
        let id = r.entity_id;
        if flying != r.sent_flying {
            r.send(send::abilities(flying));
            r.sent_flying = flying;
        }
        if sneaking != r.sent_sneaking {
            r.send(send::entity_action(id, if sneaking { Action::StartSneaking } else { Action::StopSneaking }));
            r.sent_sneaking = sneaking;
        }
        if sprinting != r.sent_sprinting {
            r.send(send::entity_action(id, if sprinting { Action::StartSprinting } else { Action::StopSprinting }));
            r.sent_sprinting = sprinting;
        }
    }

    pub fn send_chat(&mut self, text: &str) {
        if let Some(r) = self.remote()
            && !text.trim().is_empty()
        {
            r.send(send::chat(text.trim()));
        }
    }

    pub fn respawn(&mut self) {
        if let Some(r) = self.remote() {
            r.send(send::respawn());
        }
    }

    /// Tells the server about a hotbar change.
    pub fn select(&mut self, slot: usize) {
        if slot != self.selected {
            self.selected = slot;
            if let Some(r) = self.remote() {
                r.send(send::held_item(slot as u8));
            }
        }
    }

    pub fn set_render_distance(&mut self, distance: u32) {
        self.render_distance = distance;
        if let Some(r) = self.remote() {
            r.send(send::settings(distance.min(32) as u8));
        }
    }

    fn set_mode(&mut self, mode: GameMode) {
        if let Some(r) = self.remote() {
            r.mode = mode;
        }
        self.player.can_fly = matches!(mode, GameMode::Creative | GameMode::Spectator);
        if !self.player.can_fly {
            self.player.flying = false;
        }
    }

    /// Applies server packets for at most `budget`.
    pub fn apply_pending(&mut self, budget: Duration) {
        let start = Instant::now();
        while start.elapsed() < budget
            && let Some(packet) = self.pending.pop_front()
        {
            self.apply(packet);
        }
    }

    /// Applies one server packet.
    fn apply(&mut self, packet: Clientbound) {
        match packet {
            Clientbound::JoinGame { entity_id, mode, dimension } => {
                self.reset_world(dimension == 0);
                self.set_mode(mode);
                let distance = self.render_distance.min(32) as u8;
                let r = self.remote().unwrap();
                r.entity_id = entity_id;
                r.send(send::settings(distance));
            }
            Clientbound::Respawn { dimension, mode } => {
                self.reset_world(dimension == 0);
                self.set_mode(mode);
                self.remote().unwrap().positioned = false;
            }
            Clientbound::Chunks(chunks) => {
                for c in chunks {
                    let (x, z) = (c.x, c.z);
                    self.world.insert_lit_chunk(c);
                    self.mark_chunk_dirty(x, z);
                }
            }
            Clientbound::Sections { x, z, sections } => {
                if let Some(c) = self.world.chunk_mut(x, z) {
                    for (y, s) in sections {
                        c.sections[y] = Some(s);
                    }
                    self.mark_chunk_dirty(x, z);
                }
            }
            Clientbound::Unload(x, z) => {
                self.world.remove_chunk(x, z);
                self.unloaded.push((x, z));
            }
            Clientbound::Blocks(changes) => {
                for (pos, state) in changes {
                    if self.world.block(pos) != state {
                        self.set_block(pos, state);
                    }
                }
                self.update_target();
            }
            Clientbound::Teleport { pos, yaw, pitch, relative } => {
                let p = &mut self.player;
                let base = |bit: u8, old: f64, new: f64| if relative & bit != 0 { old + new } else { new };
                p.pos = DVec3::new(base(1, p.pos.x, pos.x), base(2, p.pos.y, pos.y), base(4, p.pos.z, pos.z));
                p.yaw = base(8, p.yaw as f64, yaw as f64) as f32;
                p.pitch = base(16, p.pitch as f64, pitch as f64) as f32;
                (p.prev_pos, p.vel) = (p.pos, DVec3::ZERO);
                let (pos, yaw, pitch) = (p.pos, p.yaw, p.pitch);
                let r = self.remote().unwrap();
                r.positioned = true;
                (r.last_pos, r.last_look, r.since_position) = (pos, (yaw, pitch), 0);
                // The 1.8 client confirms every teleport with its new position.
                r.send(send::position_look(pos, yaw, pitch, false));
            }
            Clientbound::Time { time_of_day, .. } => self.day_time = time_of_day.abs(),
            Clientbound::Health { health, food } => {
                let r = self.remote().unwrap();
                (r.health, r.food) = (health, food);
            }
            Clientbound::HeldItem(slot) => self.selected = slot as usize,
            Clientbound::GameMode(mode) => self.set_mode(mode),
            Clientbound::Abilities { flying, may_fly } => {
                self.player.can_fly = may_fly;
                self.player.flying = flying && may_fly;
                self.remote().unwrap().sent_flying = self.player.flying;
            }
            Clientbound::Slots(slots) => {
                let r = self.remote().unwrap();
                for (i, item) in slots {
                    r.inventory[i] = item;
                }
            }
            Clientbound::Chat { text, action_bar: true } => {
                self.remote().unwrap().action_bar = Some((Instant::now(), text));
            }
            Clientbound::Chat { text, .. } => self.remote().unwrap().say(text),
            Clientbound::SpawnEntity { id, kind, pos, .. } => {
                self.remote().unwrap().entities.insert(id, Entity { kind, pos });
            }
            Clientbound::MoveEntity { id, delta, .. } => {
                if let Some(e) = self.remote().unwrap().entities.get_mut(&id) {
                    e.pos += delta;
                }
            }
            Clientbound::TeleportEntity { id, pos, .. } => {
                if let Some(e) = self.remote().unwrap().entities.get_mut(&id) {
                    e.pos = pos;
                }
            }
            Clientbound::RemoveEntities(ids) => {
                let r = self.remote().unwrap();
                for id in ids {
                    r.entities.remove(&id);
                }
            }
            Clientbound::PlayerList { added, removed } => {
                let r = self.remote().unwrap();
                r.names.extend(added);
                for uuid in removed {
                    r.names.remove(&uuid);
                }
            }
            _ => {}
        }
    }

    fn reset_world(&mut self, sky: bool) {
        self.world = World::new(sky);
        self.dirty.clear();
        self.clear_renderer = true;
        if let Some(r) = self.remote() {
            r.entities.clear();
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
            if let Source::Local(l) = &mut self.source {
                l.mark_modified(pos.x >> 4, pos.z >> 4);
            }
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

    fn neighbours_loaded(&self, s: IVec3) -> bool {
        (-1..=1).all(|dx| (-1..=1).all(|dz| self.world.is_loaded(s.x + dx, s.z + dz)))
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
        let added = match &mut self.source {
            Source::Local(l) => l.stream(&mut self.world, center, self.render_distance as i32 + 1, 6),
            Source::Remote(r) => {
                self.pending.extend(r.poll());
                Vec::new()
            }
        };
        for (cx, cz) in &added {
            self.mark_chunk_dirty(*cx, *cz);
        }
        self.apply_pending(Duration::from_millis(8));
        if std::mem::take(&mut self.clear_renderer) {
            renderer.clear();
        }
        for (x, z) in self.unloaded.drain(..) {
            renderer.remove_column(x, z);
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
        // Sections leave the set when their chunk unloads or goes out of
        // range; loading the chunk, or coming back into range, marks them again.
        let range = self.render_distance as i32;
        if let Some((ox, oz)) = self.last_center
            && (ox, oz) != center
        {
            for cx in pcx - range..=pcx + range {
                for cz in pcz - range..=pcz + range {
                    if ((cx - ox).abs() > range || (cz - oz).abs() > range) && self.world.is_loaded(cx, cz) {
                        self.dirty.extend((0..16).map(|sy| IVec3::new(cx, sy, cz)));
                    }
                }
            }
        }
        self.last_center = Some(center);
        let world = &self.world;
        self.dirty.retain(|s| {
            (0..16).contains(&s.y)
                && world.is_loaded(s.x, s.z)
                && (s.x - pcx).abs() <= range
                && (s.z - pcz).abs() <= range
        });
        // Only mesh once all neighbours exist, so borders are final; the
        // rest wait in the set without blocking nearer work.
        let mut queue: Vec<IVec3> =
            self.dirty.iter().copied().filter(|s| !self.in_flight.contains(s) && self.neighbours_loaded(*s)).collect();
        queue.sort_by_key(|s| (s.x - pcx).pow(2) + (s.z - pcz).pow(2) + (s.y - py).pow(2));
        for s in queue.into_iter().take(48) {
            self.dirty.remove(&s);
            match Snapshot::capture(&self.world, s, &self.colors) {
                Some(snapshot) => {
                    self.in_flight.insert(s);
                    let _ = self.mesher.jobs.send(snapshot);
                }
                None => renderer.remove_section(s),
            }
        }
        while let Ok(mesh) = self.mesher.results.try_recv() {
            self.in_flight.remove(&mesh.section);
            if self.world.is_loaded(mesh.section.x, mesh.section.z) {
                renderer.upload(&mesh);
            }
        }
    }

    /// Saves a local world; on a server there is nothing to save.
    pub fn save(&mut self) {
        if let Source::Local(l) = &mut self.source
            && let Err(e) = l.save(&self.world, &self.player, self.day_time, self.ticks as i64)
        {
            tracing::error!("saving the world failed: {e:#}");
        }
    }

    /// Saves a local world, or leaves the server.
    pub fn leave(&mut self) {
        self.save();
        if let Some(r) = self.remote() {
            r.disconnect();
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

#[cfg(test)]
mod multiplayer_tests {
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use rustcraft_protocol::codec::{Buf, Put};
    use rustcraft_protocol::conn::{PacketReader, PacketWriter};
    use rustcraft_protocol::{Clientbound, GameMode, Item};
    use rustcraft_renderer::TextureData;
    use rustcraft_world::Chunk;

    use super::*;
    use crate::remote::{Remote, Status};

    /// An offline-mode server that logs the player in, then reports every
    /// packet it receives.
    fn server() -> (String, mpsc::Receiver<(i32, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = PacketReader::new(s.try_clone().unwrap());
            let mut w = PacketWriter::new(s);
            r.read().unwrap();
            r.read().unwrap();
            let mut ok = Vec::new();
            ok.string("00000000-0000-3000-8000-000000000001").string("Tester");
            w.write(0x02, &ok).unwrap();
            while let Ok(p) = r.read() {
                if tx.send(p).is_err() {
                    return;
                }
            }
        });
        (address, rx)
    }

    fn flat(x: i32, z: i32) -> Chunk {
        let mut c = Chunk::new(x, z);
        c.needs_light = false;
        for bx in 0..16 {
            for bz in 0..16 {
                for y in 0..4 {
                    c.set_block(bx, y, bz, BlockState::new(1, 0));
                }
            }
        }
        c
    }

    fn digs(packets: &[(i32, Vec<u8>)]) -> Vec<(u8, IVec3, u8)> {
        packets
            .iter()
            .filter(|(id, _)| *id == 0x07)
            .map(|(_, b)| {
                let mut b = Buf(b);
                (b.u8().unwrap(), b.position().unwrap(), b.u8().unwrap())
            })
            .collect()
    }

    #[test]
    fn breaking_placing_and_chat_reach_the_server() {
        let (address, received) = server();
        let textures = TextureData::load(&[]);
        let remote = Remote::connect(address, "Tester".into(), None);
        let mut game = Game::new_remote(remote, textures.lookup.clone(), textures.biome_colors(), 4);
        for _ in 0..200 {
            let packets = game.remote().unwrap().poll();
            game.pending.extend(packets);
            if matches!(game.remote_ref().unwrap().status, Status::Playing) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(game.remote_ref().unwrap().status, Status::Playing));

        game.pending.push_back(Clientbound::JoinGame { entity_id: 7, mode: GameMode::Creative, dimension: 0 });
        game.pending.push_back(Clientbound::Chunks((-1..=1).flat_map(|x| (-1..=1).map(move |z| flat(x, z))).collect()));
        // Stand at the origin and look down-east at the top of block (1, 3, 0).
        game.pending.push_back(Clientbound::Teleport {
            pos: DVec3::new(0.5, 4.0, 0.5),
            yaw: -90.0,
            pitch: 58.0,
            relative: 0,
        });
        game.apply_pending(Duration::MAX);
        assert!(game.spawn_area_ready());
        game.tick();
        assert_eq!(game.target.map(|t| (t.block, t.face)), Some((IVec3::new(1, 3, 0), Face::Up)));

        // Creative: breaking is instant.
        game.controls.attack_pressed = true;
        game.tick();
        assert!(game.world.block(IVec3::new(1, 3, 0)).is_air());
        // The ray now reaches the west face of (2, 3, 0); place cobblestone into the hole.
        game.select(1);
        game.controls.use_pressed = true;
        game.tick();
        assert_eq!(game.world.block(IVec3::new(1, 3, 0)), BlockState::new(4, 0));
        game.send_chat("hello");

        // Survival with a diamond pickaxe: cobblestone takes 8 ticks.
        game.pending.push_back(Clientbound::GameMode(GameMode::Survival));
        game.pending.push_back(Clientbound::Slots(vec![(36, Some(Item { id: 278, count: 1, damage: 0 }))]));
        game.apply_pending(Duration::MAX);
        game.select(0);
        game.controls.attack = true;
        for _ in 0..12 {
            game.tick();
        }
        assert!(game.world.block(IVec3::new(1, 3, 0)).is_air());

        // Entities: spawned, moved, named through the tab list, removed.
        let steve = 7u128;
        game.pending.push_back(Clientbound::PlayerList { added: vec![(steve, "Steve".into())], removed: vec![] });
        game.pending.push_back(Clientbound::SpawnEntity {
            id: 2,
            kind: rustcraft_protocol::EntityKind::Player(steve),
            pos: DVec3::new(3.0, 4.0, 3.0),
            yaw: 0.0,
        });
        game.pending.push_back(Clientbound::MoveEntity { id: 2, delta: DVec3::new(0.5, 0.0, 0.0), yaw: None });
        game.apply_pending(Duration::MAX);
        let r = game.remote_ref().unwrap();
        assert_eq!(r.entities[&2].pos, DVec3::new(3.5, 4.0, 3.0));
        assert_eq!(r.names[&steve], "Steve");
        game.pending.push_back(Clientbound::RemoveEntities(vec![2]));
        game.apply_pending(Duration::MAX);
        assert!(game.remote_ref().unwrap().entities.is_empty());

        std::thread::sleep(Duration::from_millis(200));
        let packets: Vec<(i32, Vec<u8>)> = received.try_iter().collect();
        let ids: Vec<i32> = packets.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&0x15), "client settings after joining: {ids:x?}");
        assert!(ids.contains(&0x06), "teleport confirmed");
        let up = Face::Up.index() as u8;
        assert_eq!(
            digs(&packets),
            vec![(0, IVec3::new(1, 3, 0), up), (0, IVec3::new(1, 3, 0), up), (2, IVec3::new(1, 3, 0), up)],
            "creative start, then survival start and finish"
        );
        let place = packets.iter().find(|(id, _)| *id == 0x08).expect("a placement");
        let mut b = Buf(&place.1);
        assert_eq!((b.position().unwrap(), b.u8().unwrap()), (IVec3::new(2, 3, 0), Face::West.index() as u8));
        assert_eq!(b.slot().unwrap().map(|i| i.id), Some(4));
        let creative_slot = packets.iter().find(|(id, _)| *id == 0x10).expect("creative slot");
        assert_eq!(Buf(&creative_slot.1).i16().unwrap(), 37);
        let chat = packets.iter().find(|(id, _)| *id == 0x01).expect("chat");
        assert_eq!(Buf(&chat.1).string(100).unwrap(), "hello");
        // Every tick reports the player's state.
        assert!(ids.iter().filter(|id| (0x03..=0x06).contains(*id)).count() >= 15);
    }
}
