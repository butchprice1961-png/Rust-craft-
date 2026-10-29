//! Lighting a single chunk in isolation (used for generated or loaded
//! chunks before they join the world, where seams are then stitched).

use std::collections::VecDeque;

use crate::block::Face;
use crate::chunk::{Chunk, Section, index};

fn opacity(c: &Chunk, x: usize, y: i32, z: usize) -> u8 {
    let p = c.block(x, y, z).props();
    if p.opaque { 15 } else { p.light_opacity }
}

/// Computes sky light (top-down with sideways spread) and block light
/// (from emitters) inside one chunk.
pub fn light_chunk_local(c: &mut Chunk, has_sky: bool) {
    let sky_fill = if has_sky { 15 } else { 0 };
    for s in c.sections.iter_mut().flatten() {
        s.block_light.fill(0);
        s.sky_light.fill(sky_fill | (sky_fill << 4));
    }
    // Sections that are empty but lie below terrain still need light storage.
    let top = (0..16).flat_map(|x| (0..16).map(move |z| (x, z))).map(|(x, z)| c.top(x, z)).max().unwrap_or(-1);
    for sy in 0..=((top.max(0) as usize) >> 4) {
        c.sections[sy].get_or_insert_with(|| Box::new(Section::new(sky_fill)));
    }

    let mut sky_q = VecDeque::new();
    let mut block_q = VecDeque::new();
    for x in 0..16 {
        for z in 0..16 {
            let mut level = sky_fill;
            for y in (0..256).rev() {
                let op = opacity(c, x, y, z);
                level = if op >= 15 { 0 } else { level.saturating_sub(op) };
                let emission = c.block(x, y, z).props().light_emission;
                if let Some(s) = c.sections[(y >> 4) as usize].as_mut() {
                    let i = index(x, (y & 15) as usize, z);
                    s.set_sky_light(i, level);
                    if emission > 0 {
                        s.set_block_light(i, emission);
                        block_q.push_back((x as i32, y, z as i32));
                    }
                }
                if level > 1 && y <= top + 1 {
                    sky_q.push_back((x as i32, y, z as i32));
                }
            }
        }
    }
    spread(c, &mut sky_q, true);
    spread(c, &mut block_q, false);
    c.needs_light = false;
}

fn get(c: &Chunk, sky: bool, x: i32, y: i32, z: i32) -> u8 {
    match &c.sections[(y >> 4) as usize] {
        Some(s) => {
            let i = index(x as usize, (y & 15) as usize, z as usize);
            if sky { s.sky_light(i) } else { s.block_light(i) }
        }
        None => 0,
    }
}

fn spread(c: &mut Chunk, q: &mut VecDeque<(i32, i32, i32)>, sky: bool) {
    while let Some((x, y, z)) = q.pop_front() {
        let level = get(c, sky, x, y, z);
        if level <= 1 {
            continue;
        }
        for face in Face::ALL {
            let [dx, dy, dz] = face.offset();
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if !(0..16).contains(&nx) || !(0..16).contains(&nz) || !(0..256).contains(&ny) {
                continue;
            }
            let op = opacity(c, nx as usize, ny, nz as usize);
            if op >= 15 {
                continue;
            }
            let new =
                if sky && face == Face::Down && level == 15 && op == 0 { 15 } else { level.saturating_sub(op.max(1)) };
            if new > get(c, sky, nx, ny, nz) {
                let Some(s) = c.sections[(ny >> 4) as usize].as_mut() else {
                    continue;
                };
                let i = index(nx as usize, (ny & 15) as usize, nz as usize);
                if sky {
                    s.set_sky_light(i, new)
                } else {
                    s.set_block_light(i, new)
                }
                q.push_back((nx, ny, nz));
            }
        }
    }
}
