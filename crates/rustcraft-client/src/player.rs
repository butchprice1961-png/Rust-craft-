//! Player movement following Minecraft 1.8's rules, tick by tick (20 per
//! second): input acceleration, block slipperiness, gravity and drag,
//! swimming, ladders, creative flight, sneaking at edges and 0.6-block
//! step-up. Servers check movement, so matching vanilla matters.

use glam::{DVec3, IVec3};

use rustcraft_world::{Bounds, World};

pub const WIDTH: f64 = 0.6;
pub const HEIGHT: f64 = 1.8;
pub const EYE: f64 = 1.62;
const STEP: f64 = 0.6;

#[derive(Debug, Clone, Copy, Default)]
pub struct MoveInput {
    /// -1 (back) to 1 (forward).
    pub forward: f32,
    /// -1 (right) to 1 (left), as vanilla's strafe axis.
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Debug, Clone)]
pub struct Player {
    /// Feet position (bottom centre of the bounding box).
    pub pos: DVec3,
    pub prev_pos: DVec3,
    pub vel: DVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub collided_horizontally: bool,
    pub sneaking: bool,
    pub sprinting: bool,
    pub flying: bool,
    pub can_fly: bool,
    pub in_water: bool,
    pub in_lava: bool,
    jump_ticks: u32,
    /// Movement speed attribute (0.1 when walking).
    pub walk_speed: f64,
    pub fly_speed: f64,
}

impl Player {
    pub fn new(pos: DVec3) -> Self {
        Self {
            pos,
            prev_pos: pos,
            vel: DVec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            collided_horizontally: false,
            sneaking: false,
            sprinting: false,
            flying: false,
            can_fly: false,
            in_water: false,
            in_lava: false,
            jump_ticks: 0,
            walk_speed: 0.1,
            fly_speed: 0.05,
        }
    }

    pub fn bounds_at(pos: DVec3) -> Bounds {
        Bounds::new(pos - DVec3::new(WIDTH / 2.0, 0.0, WIDTH / 2.0), pos + DVec3::new(WIDTH / 2.0, HEIGHT, WIDTH / 2.0))
    }

    pub fn bounds(&self) -> Bounds {
        Self::bounds_at(self.pos)
    }

    pub fn eye_height(&self) -> f64 {
        if self.sneaking { EYE - 0.08 } else { EYE }
    }

    pub fn eye(&self) -> DVec3 {
        self.pos + DVec3::new(0.0, self.eye_height(), 0.0)
    }

    pub fn look(&self) -> DVec3 {
        let (yaw, pitch) = (self.yaw.to_radians() as f64, self.pitch.to_radians() as f64);
        DVec3::new(-yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
    }

    fn liquid_at(world: &World, area: &Bounds, water: bool) -> bool {
        let lo = area.min.floor().as_ivec3();
        let hi = area.max.ceil().as_ivec3();
        (lo.x..hi.x).any(|x| {
            (lo.y..hi.y).any(|y| {
                (lo.z..hi.z).any(|z| {
                    let s = world.block(IVec3::new(x, y, z));
                    s.is_liquid() && s.is_water() == water
                })
            })
        })
    }

    fn on_ladder(&self, world: &World) -> bool {
        matches!(world.block(self.pos.floor().as_ivec3()).id(), 65 | 106)
    }

    /// Advances one game tick.
    pub fn tick(&mut self, world: &World, input: &MoveInput) {
        self.prev_pos = self.pos;
        self.sneaking = input.sneak && !self.flying;
        if input.sprint && input.forward > 0.0 && !self.sneaking {
            self.sprinting = true;
        }
        if input.forward <= 0.0 || self.collided_horizontally || self.sneaking {
            self.sprinting = false;
        }

        let bb = self.bounds();
        let inner = Bounds::new(bb.min + DVec3::new(0.001, 0.4, 0.001), bb.max - DVec3::new(0.001, 0.4, 0.001));
        self.in_water = Self::liquid_at(world, &inner, true);
        self.in_lava = Self::liquid_at(world, &inner, false);

        let scale = if self.sneaking { 0.3 } else { 1.0 };
        let mut forward = (input.forward * scale * 0.98) as f64;
        let mut strafe = (input.strafe * scale * 0.98) as f64;
        if self.flying {
            forward = input.forward as f64 * 0.98;
            strafe = input.strafe as f64 * 0.98;
            if input.sneak {
                self.vel.y -= self.fly_speed * 3.0;
            }
            if input.jump {
                self.vel.y += self.fly_speed * 3.0;
            }
        }

        if self.jump_ticks > 0 {
            self.jump_ticks -= 1;
        }
        if input.jump && !self.flying {
            if self.in_water || self.in_lava {
                self.vel.y += 0.04;
            } else if self.on_ground && self.jump_ticks == 0 {
                self.vel.y = 0.42;
                if self.sprinting {
                    let yaw = self.yaw.to_radians() as f64;
                    self.vel.x -= yaw.sin() * 0.2;
                    self.vel.z += yaw.cos() * 0.2;
                }
                self.jump_ticks = 10;
            }
        } else {
            self.jump_ticks = 0;
        }

        if (self.in_water || self.in_lava) && !self.flying {
            let start_y = self.pos.y;
            self.accelerate(strafe, forward, 0.02);
            self.move_by(world, self.vel);
            let drag = if self.in_water { 0.8 } else { 0.5 };
            self.vel *= drag;
            self.vel.y -= 0.02;
            if self.collided_horizontally {
                let up = DVec3::new(self.vel.x, self.vel.y + 0.6 - self.pos.y + start_y, self.vel.z);
                let moved = self.bounds().offset(up);
                if world.collisions(&moved).is_empty() && !Self::liquid_at(world, &moved, self.in_water) {
                    self.vel.y = 0.3;
                }
            }
            return;
        }

        let friction = if self.on_ground { self.slipperiness(world) * 0.91 } else { 0.91 };
        let accel = if self.on_ground {
            let speed = if self.sprinting { self.walk_speed * 1.3 } else { self.walk_speed };
            speed * (0.162_771_36 / (friction * friction * friction))
        } else if self.flying {
            self.fly_speed * if self.sprinting { 2.0 } else { 1.0 }
        } else if self.sprinting {
            0.026
        } else {
            0.02
        };
        self.accelerate(strafe, forward, accel);

        let ladder = !self.flying && self.on_ladder(world);
        if ladder {
            self.vel.x = self.vel.x.clamp(-0.15, 0.15);
            self.vel.z = self.vel.z.clamp(-0.15, 0.15);
            self.vel.y = self.vel.y.max(-0.15);
            if self.sneaking && self.vel.y < 0.0 {
                self.vel.y = 0.0;
            }
        }
        let vertical = self.vel.y;
        self.move_by(world, self.vel);
        if self.collided_horizontally && ladder {
            self.vel.y = 0.2;
        }
        if self.flying {
            self.vel.y = vertical * 0.6;
            if self.on_ground {
                self.flying = false;
            }
        } else {
            self.vel.y -= 0.08;
            self.vel.y *= 0.98;
        }
        self.vel.x *= friction;
        self.vel.z *= friction;
    }

    fn slipperiness(&self, world: &World) -> f64 {
        let below = IVec3::new(self.pos.x.floor() as i32, (self.pos.y - 1.0).floor() as i32, self.pos.z.floor() as i32);
        world.block(below).props().slipperiness as f64
    }

    /// Vanilla `moveFlying`: turns input into acceleration along the yaw.
    fn accelerate(&mut self, strafe: f64, forward: f64, accel: f64) {
        let mut dist = strafe * strafe + forward * forward;
        if dist < 1.0e-4 {
            return;
        }
        dist = dist.sqrt().max(1.0);
        let (s, f) = (strafe * accel / dist, forward * accel / dist);
        let yaw = self.yaw.to_radians() as f64;
        self.vel.x += s * yaw.cos() - f * yaw.sin();
        self.vel.z += f * yaw.cos() + s * yaw.sin();
    }

    /// Vanilla `moveEntity`: collides along Y, X then Z, steps up ledges.
    pub fn move_by(&mut self, world: &World, wanted: DVec3) {
        let start = self.bounds();
        let mut d = wanted;
        if self.on_ground && self.sneaking {
            // Do not walk off edges while sneaking.
            let step = 0.05;
            let unsupported = |dx: f64, dz: f64| world.collisions(&start.offset(DVec3::new(dx, -1.0, dz))).is_empty();
            while d.x != 0.0 && unsupported(d.x, 0.0) {
                d.x = if d.x.abs() < step { 0.0 } else { d.x - step * d.x.signum() };
            }
            while d.z != 0.0 && unsupported(0.0, d.z) {
                d.z = if d.z.abs() < step { 0.0 } else { d.z - step * d.z.signum() };
            }
            while d.x != 0.0 && d.z != 0.0 && unsupported(d.x, d.z) {
                d.x = if d.x.abs() < step { 0.0 } else { d.x - step * d.x.signum() };
                d.z = if d.z.abs() < step { 0.0 } else { d.z - step * d.z.signum() };
            }
        }
        let original = d;
        let boxes = world.collisions(&start.expand(d));
        let collide = |mut bb: Bounds, mut d: DVec3, boxes: &[Bounds]| {
            for axis in [1, 0, 2] {
                for b in boxes {
                    d[axis] = b.clip(&bb, axis, d[axis]);
                }
                let mut shift = DVec3::ZERO;
                shift[axis] = d[axis];
                bb = bb.offset(shift);
            }
            (bb, d)
        };
        let (mut bb, mut moved) = collide(start, d, &boxes);

        let may_step = self.on_ground || (original.y != moved.y && original.y < 0.0);
        if may_step && (original.x != moved.x || original.z != moved.z) {
            let up = DVec3::new(original.x, STEP, original.z);
            let step_boxes = world.collisions(&start.expand(up));
            let (raised, rd) = collide(start, up, &step_boxes);
            // Settle back down onto whatever is underneath.
            let mut down = -rd.y;
            for b in &step_boxes {
                down = b.clip(&raised, 1, down);
            }
            let stepped = raised.offset(DVec3::new(0.0, down, 0.0));
            let stepped_d = DVec3::new(rd.x, rd.y + down, rd.z);
            if rd.x * rd.x + rd.z * rd.z > moved.x * moved.x + moved.z * moved.z {
                bb = stepped;
                moved = stepped_d;
            }
        }

        self.pos = DVec3::new((bb.min.x + bb.max.x) / 2.0, bb.min.y, (bb.min.z + bb.max.z) / 2.0);
        self.collided_horizontally = original.x != moved.x || original.z != moved.z;
        self.on_ground = original.y != moved.y && original.y < 0.0;
        if original.x != moved.x {
            self.vel.x = 0.0;
        }
        if original.z != moved.z {
            self.vel.z = 0.0;
        }
        if original.y != moved.y {
            self.vel.y = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcraft_world::{BlockState, Chunk};

    fn floor_world() -> World {
        let mut w = World::new(true);
        for cx in -2..=2 {
            for cz in -2..=2 {
                let mut c = Chunk::new(cx, cz);
                for x in 0..16 {
                    for z in 0..16 {
                        c.set_block(x, 63, z, BlockState::new(1, 0));
                    }
                }
                w.insert_chunk(c);
            }
        }
        w
    }

    fn run(p: &mut Player, w: &World, input: MoveInput, ticks: usize) {
        for _ in 0..ticks {
            p.tick(w, &input);
        }
    }

    #[test]
    fn falls_and_lands() {
        let w = floor_world();
        let mut p = Player::new(DVec3::new(0.5, 70.0, 0.5));
        run(&mut p, &w, MoveInput::default(), 60);
        assert!(p.on_ground);
        assert!((p.pos.y - 64.0).abs() < 1e-9, "{}", p.pos.y);
        // As in vanilla, gravity is applied after the landing reset each tick.
        assert!((p.vel.y + 0.0784).abs() < 1e-9, "{}", p.vel.y);
    }

    #[test]
    fn walking_and_sprinting_speeds_match_vanilla() {
        let w = floor_world();
        let mut p = Player::new(DVec3::new(0.5, 64.0, 0.5));
        run(&mut p, &w, MoveInput::default(), 5);
        let walk = MoveInput { forward: 1.0, ..Default::default() };
        run(&mut p, &w, walk, 20);
        let start = p.pos;
        run(&mut p, &w, walk, 20);
        // Vanilla walking speed is about 4.317 blocks per second.
        let speed = (p.pos - start).length();
        assert!((speed - 4.317).abs() < 0.05, "walk {speed}");

        let sprint = MoveInput { forward: 1.0, sprint: true, ..Default::default() };
        run(&mut p, &w, sprint, 20);
        let start = p.pos;
        run(&mut p, &w, sprint, 20);
        let speed = (p.pos - start).length();
        assert!((speed - 5.612).abs() < 0.06, "sprint {speed}");
        assert!(p.pos.z > 0.5 && (p.pos.x - 0.5).abs() < 1e-6, "yaw 0 walks south (+Z)");
    }

    #[test]
    fn jump_height_and_step_up() {
        let mut w = floor_world();
        let mut p = Player::new(DVec3::new(0.5, 64.0, 0.5));
        run(&mut p, &w, MoveInput::default(), 2);
        let mut peak: f64 = 0.0;
        p.tick(&w, &MoveInput { jump: true, ..Default::default() });
        for _ in 0..20 {
            p.tick(&w, &MoveInput::default());
            peak = peak.max(p.pos.y - 64.0);
        }
        // A vanilla jump peaks at about 1.25 blocks.
        assert!((peak - 1.2522).abs() < 0.01, "peak {peak}");

        // Walking into a slab steps up onto it; a full block stops you.
        w.set_block(IVec3::new(0, 64, 3), BlockState::new(44, 0));
        w.set_block(IVec3::new(2, 64, 3), BlockState::new(1, 0));
        let mut p = Player::new(DVec3::new(0.5, 64.0, 0.5));
        run(&mut p, &w, MoveInput { forward: 1.0, ..Default::default() }, 40);
        assert!((p.pos.y - 64.5).abs() < 1e-9 || p.pos.z > 4.0, "y {} z {}", p.pos.y, p.pos.z);
        let mut p = Player::new(DVec3::new(2.5, 64.0, 0.5));
        run(&mut p, &w, MoveInput { forward: 1.0, ..Default::default() }, 40);
        assert!(p.pos.z <= 3.0 - WIDTH / 2.0 + 1e-9, "walked into a wall: {}", p.pos.z);
        assert_eq!(p.pos.y, 64.0);
    }

    #[test]
    fn sneaking_stops_at_edges() {
        let mut w = World::new(true);
        let mut c = Chunk::new(0, 0);
        c.set_block(0, 63, 0, BlockState::new(1, 0));
        w.insert_chunk(c);
        let mut p = Player::new(DVec3::new(0.5, 64.0, 0.5));
        run(&mut p, &w, MoveInput::default(), 2);
        run(&mut p, &w, MoveInput { forward: 1.0, sneak: true, ..Default::default() }, 60);
        assert!(p.on_ground);
        assert!(p.pos.z < 1.0 + WIDTH / 2.0, "sneaked off the edge: {}", p.pos.z);
        assert_eq!(p.pos.y, 64.0);
    }

    #[test]
    fn swimming_and_flying() {
        let mut w = floor_world();
        for y in 64..70 {
            w.set_block(IVec3::new(0, y, 0), BlockState::new(9, 0));
        }
        let mut p = Player::new(DVec3::new(0.5, 66.0, 0.5));
        run(&mut p, &w, MoveInput::default(), 3);
        assert!(p.in_water);
        let before = p.pos.y;
        run(&mut p, &w, MoveInput { jump: true, ..Default::default() }, 20);
        assert!(p.pos.y > before, "swimming up");

        let mut p = Player::new(DVec3::new(5.5, 70.0, 5.5));
        p.can_fly = true;
        p.flying = true;
        run(&mut p, &w, MoveInput::default(), 40);
        assert!((p.pos.y - 70.0).abs() < 1e-6, "hovering while flying: {}", p.pos.y);
        run(&mut p, &w, MoveInput { jump: true, ..Default::default() }, 10);
        assert!(p.pos.y > 71.0);
    }
}
