//! Chunk storage in the 1.8 layout: 16 sections of 16×16×16 block states
//! (`id << 4 | meta`), each with block-light and sky-light nibbles.

use crate::block::BlockState;

pub const SECTION_BLOCKS: usize = 4096;

/// Index of a block inside a section, matching 1.8 network and disk order.
#[inline]
pub const fn index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

#[derive(Clone)]
pub struct Section {
    pub blocks: Box<[BlockState; SECTION_BLOCKS]>,
    pub block_light: Box<[u8; 2048]>,
    pub sky_light: Box<[u8; 2048]>,
    non_air: u16,
}

#[inline]
fn nibble(data: &[u8; 2048], i: usize) -> u8 {
    (data[i >> 1] >> ((i & 1) << 2)) & 15
}

#[inline]
fn set_nibble(data: &mut [u8; 2048], i: usize, v: u8) {
    let shift = (i & 1) << 2;
    data[i >> 1] = (data[i >> 1] & !(15 << shift)) | ((v & 15) << shift);
}

impl Section {
    pub fn new(sky: u8) -> Self {
        let fill = sky | (sky << 4);
        Self {
            blocks: Box::new([BlockState::AIR; SECTION_BLOCKS]),
            block_light: Box::new([0; 2048]),
            sky_light: Box::new([fill; 2048]),
            non_air: 0,
        }
    }

    pub fn block(&self, i: usize) -> BlockState {
        self.blocks[i]
    }

    pub fn set_block(&mut self, i: usize, state: BlockState) {
        let old = std::mem::replace(&mut self.blocks[i], state);
        match (old.is_air(), state.is_air()) {
            (true, false) => self.non_air += 1,
            (false, true) => self.non_air -= 1,
            _ => {}
        }
    }

    pub fn block_light(&self, i: usize) -> u8 {
        nibble(&self.block_light, i)
    }

    pub fn sky_light(&self, i: usize) -> u8 {
        nibble(&self.sky_light, i)
    }

    pub fn set_block_light(&mut self, i: usize, v: u8) {
        set_nibble(&mut self.block_light, i, v);
    }

    pub fn set_sky_light(&mut self, i: usize, v: u8) {
        set_nibble(&mut self.sky_light, i, v);
    }

    pub fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    /// Recounts non-air blocks after bulk writes into `blocks`.
    pub fn recount(&mut self) {
        self.non_air = self.blocks.iter().filter(|b| !b.is_air()).count() as u16;
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub sections: [Option<Box<Section>>; 16],
    /// Biome id per column, `z * 16 + x`.
    pub biomes: [u8; 256],
    /// Set by world generation or loading; cleared once lit.
    pub needs_light: bool,
}

impl Chunk {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z, sections: Default::default(), biomes: [1; 256], needs_light: true }
    }

    /// Block at chunk-local coordinates; y outside 0..256 is air.
    pub fn block(&self, x: usize, y: i32, z: usize) -> BlockState {
        if !(0..256).contains(&y) {
            return BlockState::AIR;
        }
        let y = y as usize;
        self.sections[y >> 4].as_ref().map_or(BlockState::AIR, |s| s.block(index(x, y & 15, z)))
    }

    pub fn set_block(&mut self, x: usize, y: i32, z: usize, state: BlockState) {
        if !(0..256).contains(&y) {
            return;
        }
        let y = y as usize;
        let section = self.sections[y >> 4].get_or_insert_with(|| Box::new(Section::new(15)));
        section.set_block(index(x, y & 15, z), state);
    }

    /// Highest non-air block y in a column, or -1.
    pub fn top(&self, x: usize, z: usize) -> i32 {
        for sy in (0..16).rev() {
            let Some(s) = &self.sections[sy] else {
                continue;
            };
            if s.is_empty() {
                continue;
            }
            for y in (0..16).rev() {
                if !s.block(index(x, y, z)).is_air() {
                    return (sy * 16 + y) as i32;
                }
            }
        }
        -1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_storage_and_nibbles() {
        let mut s = Section::new(15);
        assert!(s.is_empty());
        s.set_block(index(1, 2, 3), BlockState::new(1, 0));
        assert!(!s.is_empty());
        s.set_block_light(5, 9);
        s.set_block_light(4, 3);
        assert_eq!((s.block_light(4), s.block_light(5)), (3, 9));
        assert_eq!(s.sky_light(100), 15);
        s.set_block(index(1, 2, 3), BlockState::AIR);
        assert!(s.is_empty());
    }

    #[test]
    fn chunk_top_and_bounds() {
        let mut c = Chunk::new(0, 0);
        assert_eq!(c.top(0, 0), -1);
        c.set_block(0, 70, 0, BlockState::new(2, 0));
        c.set_block(0, 300, 0, BlockState::new(2, 0));
        assert_eq!(c.top(0, 0), 70);
        assert_eq!(c.block(0, -1, 0), BlockState::AIR);
    }
}
