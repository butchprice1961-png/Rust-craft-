//! Minecraft 1.8 blocks: numeric ids and metadata, physical properties,
//! textures, render geometry and collision boxes.
//!
//! Data is RustCraft's own description of the 1.8.9 block set (ids 0–197),
//! written from public documentation. Texture names refer to files under
//! `assets/minecraft/textures/blocks/` of a resource pack; RustCraft never
//! ships Mojang's textures.
//!
//! Fidelity: cubes, plants, slabs, straight stairs, fences, panes, walls,
//! doors, trapdoors, liquids, layers and flat blocks follow vanilla
//! geometry. Tile-entity blocks (chests, signs, banners, skulls, beds) and
//! a few complex models are approximated by boxes; see docs/COMPATIBILITY.md.

use glam::Vec3;

/// A 1.8 block state as stored in chunks: `id << 4 | meta`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub struct BlockState(pub u16);

impl BlockState {
    pub const AIR: Self = Self(0);

    pub const fn new(id: u16, meta: u8) -> Self {
        Self((id << 4) | (meta & 15) as u16)
    }

    pub const fn id(self) -> u16 {
        self.0 >> 4
    }

    pub const fn meta(self) -> u8 {
        (self.0 & 15) as u8
    }

    pub fn props(self) -> &'static Props {
        props(self.id())
    }

    pub fn is_air(self) -> bool {
        self.id() == 0
    }

    pub fn is_liquid(self) -> bool {
        matches!(self.id(), 8..=11)
    }

    pub fn is_water(self) -> bool {
        matches!(self.id(), 8 | 9)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Face {
    pub const ALL: [Face; 6] = [Face::Down, Face::Up, Face::North, Face::South, Face::West, Face::East];

    /// Unit offset towards the neighbour on this side. North is -Z.
    pub const fn offset(self) -> [i32; 3] {
        match self {
            Face::Down => [0, -1, 0],
            Face::Up => [0, 1, 0],
            Face::North => [0, 0, -1],
            Face::South => [0, 0, 1],
            Face::West => [-1, 0, 0],
            Face::East => [1, 0, 0],
        }
    }

    pub const fn opposite(self) -> Face {
        match self {
            Face::Down => Face::Up,
            Face::Up => Face::Down,
            Face::North => Face::South,
            Face::South => Face::North,
            Face::West => Face::East,
            Face::East => Face::West,
        }
    }

    /// Fixed directional shading used by the 1.8 renderer.
    pub const fn shade(self) -> f32 {
        match self {
            Face::Down => 0.5,
            Face::Up => 1.0,
            Face::North | Face::South => 0.8,
            Face::West | Face::East => 0.6,
        }
    }

    /// The protocol's face index (0 down … 5 east).
    pub const fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(i: u8) -> Option<Face> {
        Face::ALL.get(i as usize).copied()
    }

    pub fn horizontal(self) -> bool {
        !matches!(self, Face::Up | Face::Down)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Not rendered (air, barrier, moving piston).
    None,
    Cube,
    /// Two crossed planes (flowers, saplings, grass).
    Cross,
    /// Four planes in a `#` (wheat and other crops).
    Crop,
    Liquid,
    Slab,
    Stairs,
    Fence,
    Wall,
    Pane,
    FenceGate,
    Door,
    Trapdoor,
    Torch,
    Ladder,
    Vine,
    Rail,
    /// Floor overlays: redstone wire, tripwire, lily pads.
    Flat,
    Layers,
    /// A single fixed cuboid; see [`fixed_box`].
    Box,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Solid,
    /// Alpha-tested: leaves, plants, glass.
    Cutout,
    /// Alpha-blended: water, ice, stained glass.
    Translucent,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    None,
    Pickaxe,
    Axe,
    Shovel,
    Shears,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    Stone,
    Wood,
    Gravel,
    Grass,
    Sand,
    Snow,
    Glass,
    Cloth,
    Metal,
    Ladder,
    Anvil,
    Slime,
}

#[derive(Clone, Copy, Debug)]
pub struct Props {
    pub name: &'static str,
    pub shape: Shape,
    pub layer: Layer,
    /// A full opaque cube: hides neighbouring faces and blocks all light.
    pub opaque: bool,
    /// Light lost passing through (0–15).
    pub light_opacity: u8,
    pub light_emission: u8,
    /// Has collision boxes.
    pub solid: bool,
    /// Seconds-scale hardness as in vanilla; negative means unbreakable.
    pub hardness: f32,
    pub tool: Tool,
    /// Needs the right tool to drop and to break at full speed.
    pub needs_tool: bool,
    pub sound: Sound,
    pub slipperiness: f32,
}

const fn b(name: &'static str) -> Props {
    Props {
        name,
        shape: Shape::Cube,
        layer: Layer::Solid,
        opaque: true,
        light_opacity: 15,
        light_emission: 0,
        solid: true,
        hardness: 1.0,
        tool: Tool::None,
        needs_tool: false,
        sound: Sound::Stone,
        slipperiness: 0.6,
    }
}

impl Props {
    const fn hard(self, hardness: f32) -> Self {
        Props { hardness, ..self }
    }
    const fn pick(self) -> Self {
        Props { tool: Tool::Pickaxe, needs_tool: true, ..self }
    }
    const fn axe(self) -> Self {
        Props { tool: Tool::Axe, sound: Sound::Wood, ..self }
    }
    const fn shovel(self) -> Self {
        Props { tool: Tool::Shovel, ..self }
    }
    const fn sound(self, sound: Sound) -> Self {
        Props { sound, ..self }
    }
    const fn shape(self, shape: Shape) -> Self {
        Props { shape, opaque: false, ..self }
    }
    const fn cutout(self) -> Self {
        Props { layer: Layer::Cutout, opaque: false, light_opacity: 0, ..self }
    }
    const fn translucent(self, light_opacity: u8) -> Self {
        Props { layer: Layer::Translucent, opaque: false, light_opacity, ..self }
    }
    const fn clear(self) -> Self {
        Props { light_opacity: 0, ..self }
    }
    const fn noclip(self) -> Self {
        Props { solid: false, ..self }
    }
    const fn emit(self, light_emission: u8) -> Self {
        Props { light_emission, ..self }
    }
    const fn plant(self) -> Self {
        self.shape(Shape::Cross).cutout().noclip().hard(0.0).sound(Sound::Grass)
    }
    const fn slippery(self, slipperiness: f32) -> Self {
        Props { slipperiness, ..self }
    }
}

const WOOD_STAIRS: Props = b("wooden_stairs").shape(Shape::Stairs).hard(2.0).axe();
const FENCE: Props = b("fence").shape(Shape::Fence).clear().hard(2.0).axe();
const GATE: Props = b("fence_gate").shape(Shape::FenceGate).clear().hard(2.0).axe();
const WOOD_DOOR: Props = b("wooden_door").shape(Shape::Door).cutout().hard(3.0).axe();
const UNKNOWN: Props = b("unknown");

/// Properties for every 1.8.9 block id; unknown ids render as `unknown`.
static TABLE: [Props; 198] = [
    b("air").shape(Shape::None).clear().noclip().hard(0.0),
    b("stone").hard(1.5).pick(),
    b("grass").hard(0.6).shovel().sound(Sound::Grass),
    b("dirt").hard(0.5).shovel().sound(Sound::Gravel),
    b("cobblestone").hard(2.0).pick(),
    b("planks").hard(2.0).axe(),
    b("sapling").plant(),
    b("bedrock").hard(-1.0),
    b("flowing_water").shape(Shape::Liquid).translucent(3).noclip().hard(100.0),
    b("water").shape(Shape::Liquid).translucent(3).noclip().hard(100.0),
    b("flowing_lava").shape(Shape::Liquid).noclip().emit(15).hard(100.0),
    b("lava").shape(Shape::Liquid).noclip().emit(15).hard(100.0),
    b("sand").hard(0.5).shovel().sound(Sound::Sand),
    b("gravel").hard(0.6).shovel().sound(Sound::Gravel),
    b("gold_ore").hard(3.0).pick(),
    b("iron_ore").hard(3.0).pick(),
    b("coal_ore").hard(3.0).pick(),
    b("log").hard(2.0).axe(),
    Props { light_opacity: 1, ..b("leaves").cutout().hard(0.2).sound(Sound::Grass) },
    b("sponge").hard(0.6).sound(Sound::Grass),
    b("glass").cutout().hard(0.3).sound(Sound::Glass),
    b("lapis_ore").hard(3.0).pick(),
    b("lapis_block").hard(3.0).pick(),
    b("dispenser").hard(3.5).pick(),
    b("sandstone").hard(0.8).pick(),
    b("noteblock").hard(0.8).axe(),
    b("bed").shape(Shape::Box).clear().hard(0.2).axe(),
    b("golden_rail").shape(Shape::Rail).cutout().noclip().hard(0.7).sound(Sound::Metal),
    b("detector_rail").shape(Shape::Rail).cutout().noclip().hard(0.7).sound(Sound::Metal),
    b("sticky_piston").hard(0.5),
    Props { light_opacity: 1, ..b("web").plant().hard(4.0).sound(Sound::Stone) },
    b("tallgrass").plant(),
    b("deadbush").plant(),
    b("piston").hard(0.5),
    b("piston_head").shape(Shape::Box).clear().hard(0.5),
    b("wool").hard(0.8).sound(Sound::Cloth),
    b("piston_extension").shape(Shape::None).clear().noclip().hard(-1.0),
    b("yellow_flower").plant(),
    b("red_flower").plant(),
    b("brown_mushroom").plant().emit(1),
    b("red_mushroom").plant(),
    b("gold_block").hard(3.0).pick().sound(Sound::Metal),
    b("iron_block").hard(5.0).pick().sound(Sound::Metal),
    b("double_stone_slab").hard(2.0).pick(),
    b("stone_slab").shape(Shape::Slab).hard(2.0).pick(),
    b("brick_block").hard(2.0).pick(),
    b("tnt").hard(0.0).sound(Sound::Grass),
    b("bookshelf").hard(1.5).axe(),
    b("mossy_cobblestone").hard(2.0).pick(),
    b("obsidian").hard(50.0).pick(),
    b("torch").shape(Shape::Torch).cutout().noclip().emit(14).hard(0.0).sound(Sound::Wood),
    b("fire").shape(Shape::Cross).cutout().noclip().emit(15).hard(0.0).sound(Sound::Cloth),
    b("mob_spawner").cutout().hard(5.0).pick().sound(Sound::Metal),
    WOOD_STAIRS,
    b("chest").shape(Shape::Box).clear().hard(2.5).axe(),
    b("redstone_wire").shape(Shape::Flat).cutout().noclip().hard(0.0),
    b("diamond_ore").hard(3.0).pick(),
    b("diamond_block").hard(5.0).pick().sound(Sound::Metal),
    b("crafting_table").hard(2.5).axe(),
    b("wheat").shape(Shape::Crop).cutout().noclip().hard(0.0).sound(Sound::Grass),
    b("farmland").shape(Shape::Box).hard(0.6).shovel().sound(Sound::Gravel),
    b("furnace").hard(3.5).pick(),
    b("lit_furnace").hard(3.5).pick().emit(13),
    b("standing_sign").shape(Shape::Box).clear().noclip().hard(1.0).axe(),
    WOOD_DOOR,
    b("ladder").shape(Shape::Ladder).cutout().hard(0.4).axe().sound(Sound::Ladder),
    b("rail").shape(Shape::Rail).cutout().noclip().hard(0.7).sound(Sound::Metal),
    b("stone_stairs").shape(Shape::Stairs).hard(2.0).pick(),
    b("wall_sign").shape(Shape::Box).clear().noclip().hard(1.0).axe(),
    b("lever").shape(Shape::Box).cutout().noclip().hard(0.5).sound(Sound::Wood),
    b("stone_pressure_plate").shape(Shape::Box).clear().noclip().hard(0.5).pick(),
    b("iron_door").shape(Shape::Door).cutout().hard(5.0).pick().sound(Sound::Metal),
    b("wooden_pressure_plate").shape(Shape::Box).clear().noclip().hard(0.5).axe(),
    b("redstone_ore").hard(3.0).pick(),
    b("lit_redstone_ore").hard(3.0).pick().emit(9),
    b("unlit_redstone_torch").shape(Shape::Torch).cutout().noclip().hard(0.0).sound(Sound::Wood),
    b("redstone_torch").shape(Shape::Torch).cutout().noclip().emit(7).hard(0.0).sound(Sound::Wood),
    b("stone_button").shape(Shape::Box).clear().noclip().hard(0.5),
    b("snow_layer").shape(Shape::Layers).clear().hard(0.1).shovel().sound(Sound::Snow),
    b("ice").translucent(3).hard(0.5).pick().sound(Sound::Glass).slippery(0.98),
    b("snow").hard(0.2).shovel().sound(Sound::Snow),
    b("cactus").shape(Shape::Box).cutout().hard(0.4).sound(Sound::Cloth),
    b("clay").hard(0.6).shovel().sound(Sound::Gravel),
    b("reeds").plant(),
    b("jukebox").hard(2.0).axe(),
    FENCE,
    b("pumpkin").hard(1.0).axe(),
    b("netherrack").hard(0.4).pick(),
    b("soul_sand").shape(Shape::Box).hard(0.5).shovel().sound(Sound::Sand),
    b("glowstone").emit(15).hard(0.3).sound(Sound::Glass),
    b("portal").shape(Shape::Box).translucent(0).noclip().emit(11).hard(-1.0).sound(Sound::Glass),
    b("lit_pumpkin").emit(15).hard(1.0).axe(),
    b("cake").shape(Shape::Box).clear().hard(0.5).sound(Sound::Cloth),
    b("unpowered_repeater").shape(Shape::Box).clear().hard(0.0).sound(Sound::Wood),
    b("powered_repeater").shape(Shape::Box).clear().emit(9).hard(0.0).sound(Sound::Wood),
    b("stained_glass").translucent(0).hard(0.3).sound(Sound::Glass),
    b("trapdoor").shape(Shape::Trapdoor).cutout().hard(3.0).axe(),
    b("monster_egg").hard(0.75),
    b("stonebrick").hard(1.5).pick(),
    b("brown_mushroom_block").hard(0.2).axe(),
    b("red_mushroom_block").hard(0.2).axe(),
    b("iron_bars").shape(Shape::Pane).cutout().hard(5.0).pick().sound(Sound::Metal),
    b("glass_pane").shape(Shape::Pane).cutout().hard(0.3).sound(Sound::Glass),
    b("melon_block").hard(1.0).axe(),
    b("pumpkin_stem").shape(Shape::Crop).cutout().noclip().hard(0.0).sound(Sound::Wood),
    b("melon_stem").shape(Shape::Crop).cutout().noclip().hard(0.0).sound(Sound::Wood),
    b("vine").shape(Shape::Vine).cutout().noclip().hard(0.2).sound(Sound::Grass),
    GATE,
    b("brick_stairs").shape(Shape::Stairs).hard(2.0).pick(),
    b("stone_brick_stairs").shape(Shape::Stairs).hard(1.5).pick(),
    b("mycelium").hard(0.6).shovel().sound(Sound::Grass),
    b("waterlily").shape(Shape::Flat).cutout().hard(0.0).sound(Sound::Grass),
    b("nether_brick").hard(2.0).pick(),
    b("nether_brick_fence").shape(Shape::Fence).clear().hard(2.0).pick(),
    b("nether_brick_stairs").shape(Shape::Stairs).hard(2.0).pick(),
    b("nether_wart").shape(Shape::Crop).cutout().noclip().hard(0.0),
    b("enchanting_table").shape(Shape::Box).clear().hard(5.0).pick(),
    b("brewing_stand").shape(Shape::Box).cutout().emit(1).hard(0.5).pick().sound(Sound::Metal),
    b("cauldron").shape(Shape::Box).clear().hard(2.0).pick().sound(Sound::Metal),
    b("end_portal").shape(Shape::Box).clear().noclip().emit(15).hard(-1.0),
    b("end_portal_frame").shape(Shape::Box).emit(1).hard(-1.0).sound(Sound::Glass),
    b("end_stone").hard(3.0).pick(),
    b("dragon_egg").shape(Shape::Box).clear().emit(1).hard(3.0),
    b("redstone_lamp").hard(0.3).sound(Sound::Glass),
    b("lit_redstone_lamp").emit(15).hard(0.3).sound(Sound::Glass),
    b("double_wooden_slab").hard(2.0).axe(),
    b("wooden_slab").shape(Shape::Slab).hard(2.0).axe(),
    b("cocoa").shape(Shape::Box).clear().hard(0.2).axe(),
    b("sandstone_stairs").shape(Shape::Stairs).hard(0.8).pick(),
    b("emerald_ore").hard(3.0).pick(),
    b("ender_chest").shape(Shape::Box).clear().emit(7).hard(22.5).pick(),
    b("tripwire_hook").shape(Shape::Box).clear().noclip().hard(0.0).sound(Sound::Wood),
    b("tripwire").shape(Shape::Flat).cutout().noclip().hard(0.0),
    b("emerald_block").hard(5.0).pick().sound(Sound::Metal),
    WOOD_STAIRS,
    WOOD_STAIRS,
    WOOD_STAIRS,
    b("command_block").hard(-1.0).sound(Sound::Metal),
    b("beacon").cutout().emit(15).hard(3.0).sound(Sound::Glass),
    b("cobblestone_wall").shape(Shape::Wall).clear().hard(2.0).pick(),
    b("flower_pot").shape(Shape::Box).cutout().hard(0.0),
    b("carrots").shape(Shape::Crop).cutout().noclip().hard(0.0).sound(Sound::Grass),
    b("potatoes").shape(Shape::Crop).cutout().noclip().hard(0.0).sound(Sound::Grass),
    b("wooden_button").shape(Shape::Box).clear().noclip().hard(0.5).axe(),
    b("skull").shape(Shape::Box).clear().hard(1.0),
    b("anvil").shape(Shape::Box).clear().hard(5.0).pick().sound(Sound::Anvil),
    b("trapped_chest").shape(Shape::Box).clear().hard(2.5).axe(),
    b("light_weighted_pressure_plate").shape(Shape::Box).clear().noclip().hard(0.5).pick().sound(Sound::Metal),
    b("heavy_weighted_pressure_plate").shape(Shape::Box).clear().noclip().hard(0.5).pick().sound(Sound::Metal),
    b("unpowered_comparator").shape(Shape::Box).clear().hard(0.0).sound(Sound::Wood),
    b("powered_comparator").shape(Shape::Box).clear().emit(9).hard(0.0).sound(Sound::Wood),
    b("daylight_detector").shape(Shape::Box).clear().hard(0.2).axe(),
    b("redstone_block").hard(5.0).pick().sound(Sound::Metal),
    b("quartz_ore").hard(3.0).pick(),
    b("hopper").shape(Shape::Box).clear().hard(3.0).pick().sound(Sound::Metal),
    b("quartz_block").hard(0.8).pick(),
    b("quartz_stairs").shape(Shape::Stairs).hard(0.8).pick(),
    b("activator_rail").shape(Shape::Rail).cutout().noclip().hard(0.7).sound(Sound::Metal),
    b("dropper").hard(3.5).pick(),
    b("stained_hardened_clay").hard(1.25).pick(),
    b("stained_glass_pane").shape(Shape::Pane).translucent(0).hard(0.3).sound(Sound::Glass),
    Props { light_opacity: 1, ..b("leaves2").cutout().hard(0.2).sound(Sound::Grass) },
    b("log2").hard(2.0).axe(),
    WOOD_STAIRS,
    WOOD_STAIRS,
    b("slime").translucent(0).hard(0.0).sound(Sound::Slime).slippery(0.8),
    b("barrier").shape(Shape::None).clear().hard(-1.0),
    b("iron_trapdoor").shape(Shape::Trapdoor).cutout().hard(5.0).pick().sound(Sound::Metal),
    b("prismarine").hard(1.5).pick(),
    b("sea_lantern").emit(15).hard(0.3).sound(Sound::Glass),
    b("hay_block").hard(0.5).sound(Sound::Grass),
    b("carpet").shape(Shape::Box).clear().hard(0.1).sound(Sound::Cloth),
    b("hardened_clay").hard(1.25).pick(),
    b("coal_block").hard(5.0).pick(),
    b("packed_ice").hard(0.5).pick().sound(Sound::Glass).slippery(0.98),
    b("double_plant").plant(),
    b("standing_banner").shape(Shape::Box).clear().noclip().hard(1.0).axe(),
    b("wall_banner").shape(Shape::Box).clear().noclip().hard(1.0).axe(),
    b("daylight_detector_inverted").shape(Shape::Box).clear().hard(0.2).axe(),
    b("red_sandstone").hard(0.8).pick(),
    b("red_sandstone_stairs").shape(Shape::Stairs).hard(0.8).pick(),
    b("double_stone_slab2").hard(2.0).pick(),
    b("stone_slab2").shape(Shape::Slab).hard(2.0).pick(),
    GATE,
    GATE,
    GATE,
    GATE,
    GATE,
    FENCE,
    FENCE,
    FENCE,
    FENCE,
    FENCE,
    WOOD_DOOR,
    WOOD_DOOR,
    WOOD_DOOR,
    WOOD_DOOR,
    WOOD_DOOR,
];

pub fn props(id: u16) -> &'static Props {
    TABLE.get(id as usize).unwrap_or(&UNKNOWN)
}

pub const COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "silver",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

const PLANKS: [&str; 6] =
    ["planks_oak", "planks_spruce", "planks_birch", "planks_jungle", "planks_acacia", "planks_big_oak"];

/// How a face is coloured by the biome or block data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tint {
    None,
    Grass,
    Foliage,
    Rgb(u32),
    /// Redstone wire power 0–15.
    Redstone(u8),
}

fn axis_faces(meta: u8, face: Face, end: &'static str, side: &'static str) -> &'static str {
    let on_axis = match (meta >> 2) & 3 {
        0 => !face.horizontal(),
        1 => matches!(face, Face::West | Face::East),
        2 => matches!(face, Face::North | Face::South),
        _ => false,
    };
    if on_axis { end } else { side }
}

/// Facing for furnaces, dispensers and pistons (meta 0–5 = down, up, N, S, W, E).
fn facing6(meta: u8) -> Face {
    Face::from_index(meta & 7).unwrap_or(Face::North)
}

/// Pumpkins and similar (meta 0–3 = south, west, north, east).
fn facing4(meta: u8) -> Face {
    [Face::South, Face::West, Face::North, Face::East][(meta & 3) as usize]
}

fn slab_texture(kind: u8, face: Face) -> &'static str {
    let top = !face.horizontal();
    match kind & 7 {
        0 => {
            if top {
                "stone_slab_top"
            } else {
                "stone_slab_side"
            }
        }
        1 => match face {
            Face::Up => "sandstone_top",
            Face::Down => "sandstone_bottom",
            _ => "sandstone_normal",
        },
        2 => "planks_oak",
        3 => "cobblestone",
        4 => "brick",
        5 => "stonebrick",
        6 => "nether_brick",
        _ => match face {
            Face::Up => "quartz_block_top",
            Face::Down => "quartz_block_bottom",
            _ => "quartz_block_side",
        },
    }
}

/// The texture on `face` of a block, and its tint.
pub fn texture(state: BlockState, face: Face) -> (&'static str, Tint) {
    let m = state.meta();
    let t = |name| (name, Tint::None);
    let top_side = |top, bottom, side| match face {
        Face::Up => (top, Tint::None),
        Face::Down => (bottom, Tint::None),
        _ => (side, Tint::None),
    };
    match state.id() {
        1 => t([
            "stone",
            "stone_granite",
            "stone_granite_smooth",
            "stone_diorite",
            "stone_diorite_smooth",
            "stone_andesite",
            "stone_andesite_smooth",
        ]
        .get(m as usize)
        .copied()
        .unwrap_or("stone")),
        2 => match face {
            Face::Up => ("grass_top", Tint::Grass),
            Face::Down => t("dirt"),
            _ => t("grass_side"),
        },
        3 => match m {
            1 => t("coarse_dirt"),
            2 => top_side("dirt_podzol_top", "dirt", "dirt_podzol_side"),
            _ => t("dirt"),
        },
        4 => t("cobblestone"),
        5 | 125 | 126 => t(PLANKS[(m & 7).min(5) as usize]),
        6 => t([
            "sapling_oak",
            "sapling_spruce",
            "sapling_birch",
            "sapling_jungle",
            "sapling_acacia",
            "sapling_roofed_oak",
        ][(m & 7).min(5) as usize]),
        7 => t("bedrock"),
        8 | 9 => t(if face.horizontal() { "water_flow" } else { "water_still" }),
        10 | 11 => t(if face.horizontal() { "lava_flow" } else { "lava_still" }),
        12 => t(if m == 1 { "red_sand" } else { "sand" }),
        13 => t("gravel"),
        14 => t("gold_ore"),
        15 => t("iron_ore"),
        16 => t("coal_ore"),
        17 => {
            let w = ["oak", "spruce", "birch", "jungle"][(m & 3) as usize];
            t(axis_faces(m, face, log_name(w, true), log_name(w, false)))
        }
        162 => {
            let w = ["acacia", "big_oak"][(m & 1) as usize];
            t(axis_faces(m, face, log_name(w, true), log_name(w, false)))
        }
        18 => match m & 3 {
            0 => ("leaves_oak", Tint::Foliage),
            1 => ("leaves_spruce", Tint::Rgb(0x619961)),
            2 => ("leaves_birch", Tint::Rgb(0x80A755)),
            _ => ("leaves_jungle", Tint::Foliage),
        },
        161 => (if m & 1 == 0 { "leaves_acacia" } else { "leaves_big_oak" }, Tint::Foliage),
        19 => t(if m == 1 { "sponge_wet" } else { "sponge" }),
        20 => t("glass"),
        21 => t("lapis_ore"),
        22 => t("lapis_block"),
        23 | 158 => {
            let front = facing6(m);
            let name = if state.id() == 23 { "dispenser" } else { "dropper" };
            if face == front {
                t(if front.horizontal() { front_name(name, true) } else { front_name(name, false) })
            } else if !face.horizontal() {
                t("furnace_top")
            } else {
                t("furnace_side")
            }
        }
        24 => match m {
            1 => top_side("sandstone_top", "sandstone_top", "sandstone_carved"),
            2 => top_side("sandstone_top", "sandstone_top", "sandstone_smooth"),
            _ => top_side("sandstone_top", "sandstone_bottom", "sandstone_normal"),
        },
        179 => match m {
            1 => top_side("red_sandstone_top", "red_sandstone_top", "red_sandstone_carved"),
            2 => top_side("red_sandstone_top", "red_sandstone_top", "red_sandstone_smooth"),
            _ => top_side("red_sandstone_top", "red_sandstone_bottom", "red_sandstone_normal"),
        },
        25 => t("noteblock"),
        26 => {
            let head = m & 8 != 0;
            match face {
                Face::Up => t(if head { "bed_head_top" } else { "bed_feet_top" }),
                Face::Down => t("planks_oak"),
                _ => t(if head { "bed_head_side" } else { "bed_feet_side" }),
            }
        }
        27 => t(if m & 8 != 0 { "rail_golden_powered" } else { "rail_golden" }),
        28 => t(if m & 8 != 0 { "rail_detector_powered" } else { "rail_detector" }),
        29 | 33 | 34 => {
            let front = facing6(m);
            if face == front {
                t(if state.id() == 29 { "piston_top_sticky" } else { "piston_top_normal" })
            } else if face == front.opposite() {
                t("piston_bottom")
            } else {
                t("piston_side")
            }
        }
        30 => t("web"),
        31 => match m {
            0 => t("deadbush"),
            2 => ("fern", Tint::Grass),
            _ => ("tallgrass", Tint::Grass),
        },
        32 => t("deadbush"),
        35 => t(wool(m)),
        37 => t("flower_dandelion"),
        38 => t([
            "flower_rose",
            "flower_blue_orchid",
            "flower_allium",
            "flower_houstonia",
            "flower_tulip_red",
            "flower_tulip_orange",
            "flower_tulip_white",
            "flower_tulip_pink",
            "flower_oxeye_daisy",
        ]
        .get(m as usize)
        .copied()
        .unwrap_or("flower_rose")),
        39 => t("mushroom_brown"),
        40 => t("mushroom_red"),
        41 | 147 => t("gold_block"),
        42 | 148 => t("iron_block"),
        43 if m == 8 => t("stone_slab_top"),
        43 | 44 => t(slab_texture(m, face)),
        181 | 182 => top_side("red_sandstone_top", "red_sandstone_top", "red_sandstone_normal"),
        45 => t("brick"),
        46 => top_side("tnt_top", "tnt_bottom", "tnt_side"),
        47 => top_side("planks_oak", "planks_oak", "bookshelf"),
        48 => t("cobblestone_mossy"),
        49 => t("obsidian"),
        50 => t("torch_on"),
        51 => t("fire_layer_0"),
        52 => t("mob_spawner"),
        53 | 54 | 63 | 68 | 72 | 143 | 146 | 176 | 177 => t("planks_oak"),
        55 => ("redstone_dust_cross", Tint::Redstone(m)),
        56 => t("diamond_ore"),
        57 => t("diamond_block"),
        58 => match face {
            Face::Up => t("crafting_table_top"),
            Face::Down => t("planks_oak"),
            Face::North | Face::South => t("crafting_table_front"),
            _ => t("crafting_table_side"),
        },
        59 => t(stage("wheat_stage_", m.min(7))),
        60 => match face {
            Face::Up => t(if m > 0 { "farmland_wet" } else { "farmland_dry" }),
            _ => t("dirt"),
        },
        61 | 62 => {
            let front = facing6(m);
            if face == front {
                t(if state.id() == 62 { "furnace_front_on" } else { "furnace_front_off" })
            } else if face.horizontal() {
                t("furnace_side")
            } else {
                t("furnace_top")
            }
        }
        64 => t(if m & 8 != 0 { "door_wood_upper" } else { "door_wood_lower" }),
        71 => t(if m & 8 != 0 { "door_iron_upper" } else { "door_iron_lower" }),
        193..=197 => {
            let w = ["spruce", "birch", "jungle", "acacia", "dark_oak"][(state.id() - 193) as usize];
            t(door_name(w, m & 8 != 0))
        }
        65 => t("ladder"),
        66 => t(if (6..=9).contains(&m) { "rail_normal_turned" } else { "rail_normal" }),
        157 => t(if m & 8 != 0 { "rail_activator_powered" } else { "rail_activator" }),
        67 => t("cobblestone"),
        69 => t("lever"),
        70 | 77 => t("stone"),
        73 | 74 => t("redstone_ore"),
        75 => t("redstone_torch_off"),
        76 => t("redstone_torch_on"),
        78 | 80 => t("snow"),
        79 => t("ice"),
        81 => top_side("cactus_top", "cactus_bottom", "cactus_side"),
        82 => t("clay"),
        83 => t("reeds"),
        84 => top_side("jukebox_top", "jukebox_side", "jukebox_side"),
        85 => t("planks_oak"),
        188..=192 => t(PLANKS[[1, 2, 3, 5, 4][(state.id() - 188) as usize]]),
        107 => t("planks_oak"),
        183..=187 => t(PLANKS[[1, 2, 3, 5, 4][(state.id() - 183) as usize]]),
        86 | 91 => {
            if face == facing4(m) {
                t(if state.id() == 91 { "pumpkin_face_on" } else { "pumpkin_face_off" })
            } else if face.horizontal() {
                t("pumpkin_side")
            } else {
                t("pumpkin_top")
            }
        }
        87 => t("netherrack"),
        88 => t("soul_sand"),
        89 => t("glowstone"),
        90 => t("portal"),
        92 => top_side("cake_top", "cake_bottom", "cake_side"),
        93 => top_side("repeater_off", "stone_slab_top", "stone_slab_top"),
        94 => top_side("repeater_on", "stone_slab_top", "stone_slab_top"),
        95 | 160 => t(stained_glass(m)),
        96 => t("trapdoor"),
        167 => t("iron_trapdoor"),
        97 => t(["stone", "cobblestone", "stonebrick", "stonebrick_mossy", "stonebrick_cracked", "stonebrick_carved"]
            .get(m as usize)
            .copied()
            .unwrap_or("stone")),
        98 | 109 => t(["stonebrick", "stonebrick_mossy", "stonebrick_cracked", "stonebrick_carved"]
            [if state.id() == 98 { (m & 3) as usize } else { 0 }]),
        99 | 100 => t(mushroom_block(state.id() == 99, m, face)),
        101 => t("iron_bars"),
        102 => t("glass"),
        103 => top_side("melon_top", "melon_top", "melon_side"),
        104 | 105 => (
            if state.id() == 104 { "pumpkin_stem_disconnected" } else { "melon_stem_disconnected" },
            Tint::Rgb(stem_color(m)),
        ),
        106 => ("vine", Tint::Foliage),
        108 => t("brick"),
        110 => top_side("mycelium_top", "dirt", "mycelium_side"),
        111 => ("waterlily", Tint::Rgb(0x208030)),
        112..=114 => t("nether_brick"),
        115 => t(stage("nether_wart_stage_", [0, 1, 1, 2][(m & 3) as usize])),
        116 => top_side("enchanting_table_top", "enchanting_table_bottom", "enchanting_table_side"),
        117 => t("brewing_stand"),
        118 => top_side("cauldron_top", "cauldron_bottom", "cauldron_side"),
        119 => t("portal"),
        120 => top_side(if m & 4 != 0 { "endframe_eye" } else { "endframe_top" }, "end_stone", "endframe_side"),
        121 => t("end_stone"),
        122 => t("dragon_egg"),
        123 => t("redstone_lamp_off"),
        124 => t("redstone_lamp_on"),
        127 => t(stage("cocoa_stage_", ((m >> 2) & 3).min(2))),
        128 => top_side("sandstone_top", "sandstone_bottom", "sandstone_normal"),
        180 => top_side("red_sandstone_top", "red_sandstone_bottom", "red_sandstone_normal"),
        129 => t("emerald_ore"),
        130 => t("obsidian"),
        131 => t("trip_wire_source"),
        132 => t("trip_wire"),
        133 => t("emerald_block"),
        134 => t("planks_spruce"),
        135 => t("planks_birch"),
        136 => t("planks_jungle"),
        163 => t("planks_acacia"),
        164 => t("planks_big_oak"),
        137 => t("command_block"),
        138 => t("beacon"),
        139 => t(if m == 1 { "cobblestone_mossy" } else { "cobblestone" }),
        140 => t("flower_pot"),
        141 => t(stage("carrots_stage_", [0, 0, 1, 1, 2, 2, 2, 3][(m & 7) as usize])),
        142 => t(stage("potatoes_stage_", [0, 0, 1, 1, 2, 2, 2, 3][(m & 7) as usize])),
        144 => t("soul_sand"),
        145 => top_side(
            ["anvil_top_damaged_0", "anvil_top_damaged_1", "anvil_top_damaged_2"][((m >> 2) & 3).min(2) as usize],
            "anvil_base",
            "anvil_base",
        ),
        149 => top_side("comparator_off", "stone_slab_top", "stone_slab_top"),
        150 => top_side("comparator_on", "stone_slab_top", "stone_slab_top"),
        151 => top_side("daylight_detector_top", "daylight_detector_side", "daylight_detector_side"),
        178 => top_side("daylight_detector_inverted_top", "daylight_detector_side", "daylight_detector_side"),
        152 => t("redstone_block"),
        153 => t("quartz_ore"),
        154 => top_side("hopper_top", "hopper_outside", "hopper_outside"),
        155 | 156 => match m {
            1 => top_side("quartz_block_chiseled_top", "quartz_block_chiseled_top", "quartz_block_chiseled"),
            2..=4 => {
                let axis_meta = [0, 4, 8][(m - 2) as usize];
                t(axis_faces(axis_meta, face, "quartz_block_lines_top", "quartz_block_lines"))
            }
            _ => top_side("quartz_block_top", "quartz_block_bottom", "quartz_block_side"),
        },
        159 => t(stained_clay(m)),
        165 => t("slime"),
        168 => t(["prismarine_rough", "prismarine_bricks", "prismarine_dark"][(m.min(2)) as usize]),
        169 => t("sea_lantern"),
        170 => t(axis_faces(m, face, "hay_block_top", "hay_block_side")),
        171 => t(wool(m)),
        172 => t("hardened_clay"),
        173 => t("coal_block"),
        174 => t("ice_packed"),
        175 => double_plant(m & 7, m & 8 != 0),
        _ => t("missing"),
    }
}

/// Upper halves of double plants store only the "upper" bit; the renderer
/// passes the lower half's variant in `kind`.
pub fn double_plant(kind: u8, upper: bool) -> (&'static str, Tint) {
    let half = |lower, top| if upper { top } else { lower };
    match kind {
        0 => (half("double_plant_sunflower_bottom", "double_plant_sunflower_top"), Tint::None),
        1 => (half("double_plant_syringa_bottom", "double_plant_syringa_top"), Tint::None),
        2 => (half("double_plant_grass_bottom", "double_plant_grass_top"), Tint::Grass),
        3 => (half("double_plant_fern_bottom", "double_plant_fern_top"), Tint::Grass),
        4 => (half("double_plant_rose_bottom", "double_plant_rose_top"), Tint::None),
        _ => (half("double_plant_paeonia_bottom", "double_plant_paeonia_top"), Tint::None),
    }
}

fn wool(m: u8) -> &'static str {
    [
        "wool_colored_white",
        "wool_colored_orange",
        "wool_colored_magenta",
        "wool_colored_light_blue",
        "wool_colored_yellow",
        "wool_colored_lime",
        "wool_colored_pink",
        "wool_colored_gray",
        "wool_colored_silver",
        "wool_colored_cyan",
        "wool_colored_purple",
        "wool_colored_blue",
        "wool_colored_brown",
        "wool_colored_green",
        "wool_colored_red",
        "wool_colored_black",
    ][m as usize & 15]
}

fn stained_glass(m: u8) -> &'static str {
    [
        "glass_white",
        "glass_orange",
        "glass_magenta",
        "glass_light_blue",
        "glass_yellow",
        "glass_lime",
        "glass_pink",
        "glass_gray",
        "glass_silver",
        "glass_cyan",
        "glass_purple",
        "glass_blue",
        "glass_brown",
        "glass_green",
        "glass_red",
        "glass_black",
    ][m as usize & 15]
}

fn stained_clay(m: u8) -> &'static str {
    [
        "hardened_clay_stained_white",
        "hardened_clay_stained_orange",
        "hardened_clay_stained_magenta",
        "hardened_clay_stained_light_blue",
        "hardened_clay_stained_yellow",
        "hardened_clay_stained_lime",
        "hardened_clay_stained_pink",
        "hardened_clay_stained_gray",
        "hardened_clay_stained_silver",
        "hardened_clay_stained_cyan",
        "hardened_clay_stained_purple",
        "hardened_clay_stained_blue",
        "hardened_clay_stained_brown",
        "hardened_clay_stained_green",
        "hardened_clay_stained_red",
        "hardened_clay_stained_black",
    ][m as usize & 15]
}

fn log_name(wood: &str, top: bool) -> &'static str {
    match (wood, top) {
        ("oak", false) => "log_oak",
        ("oak", true) => "log_oak_top",
        ("spruce", false) => "log_spruce",
        ("spruce", true) => "log_spruce_top",
        ("birch", false) => "log_birch",
        ("birch", true) => "log_birch_top",
        ("jungle", false) => "log_jungle",
        ("jungle", true) => "log_jungle_top",
        ("acacia", false) => "log_acacia",
        ("acacia", true) => "log_acacia_top",
        (_, false) => "log_big_oak",
        (_, true) => "log_big_oak_top",
    }
}

fn front_name(block: &str, horizontal: bool) -> &'static str {
    match (block, horizontal) {
        ("dispenser", true) => "dispenser_front_horizontal",
        ("dispenser", false) => "dispenser_front_vertical",
        (_, true) => "dropper_front_horizontal",
        (_, false) => "dropper_front_vertical",
    }
}

fn door_name(wood: &str, upper: bool) -> &'static str {
    match (wood, upper) {
        ("spruce", false) => "door_spruce_lower",
        ("spruce", true) => "door_spruce_upper",
        ("birch", false) => "door_birch_lower",
        ("birch", true) => "door_birch_upper",
        ("jungle", false) => "door_jungle_lower",
        ("jungle", true) => "door_jungle_upper",
        ("acacia", false) => "door_acacia_lower",
        ("acacia", true) => "door_acacia_upper",
        (_, false) => "door_dark_oak_lower",
        (_, true) => "door_dark_oak_upper",
    }
}

fn stage(prefix: &str, n: u8) -> &'static str {
    const NAMES: [[&str; 8]; 5] = [
        [
            "wheat_stage_0",
            "wheat_stage_1",
            "wheat_stage_2",
            "wheat_stage_3",
            "wheat_stage_4",
            "wheat_stage_5",
            "wheat_stage_6",
            "wheat_stage_7",
        ],
        ["carrots_stage_0", "carrots_stage_1", "carrots_stage_2", "carrots_stage_3", "", "", "", ""],
        ["potatoes_stage_0", "potatoes_stage_1", "potatoes_stage_2", "potatoes_stage_3", "", "", "", ""],
        ["nether_wart_stage_0", "nether_wart_stage_1", "nether_wart_stage_2", "", "", "", "", ""],
        ["cocoa_stage_0", "cocoa_stage_1", "cocoa_stage_2", "", "", "", "", ""],
    ];
    let row = match prefix {
        "wheat_stage_" => 0,
        "carrots_stage_" => 1,
        "potatoes_stage_" => 2,
        "nether_wart_stage_" => 3,
        _ => 4,
    };
    NAMES[row][n as usize & 7]
}

fn mushroom_block(brown: bool, m: u8, face: Face) -> &'static str {
    let cap = if brown { "mushroom_block_skin_brown" } else { "mushroom_block_skin_red" };
    let (pores, stem) = ("mushroom_block_inside", "mushroom_block_skin_stem");
    let capped = match m {
        14 => true,
        0 => false,
        15 => return stem,
        10 => return if face.horizontal() { stem } else { pores },
        1..=9 => {
            face == Face::Up
                || match face {
                    Face::West => matches!(m, 1 | 4 | 7),
                    Face::East => matches!(m, 3 | 6 | 9),
                    Face::North => matches!(m, 1..=3),
                    Face::South => matches!(m, 7..=9),
                    _ => false,
                }
        }
        _ => false,
    };
    if capped { cap } else { pores }
}

fn stem_color(age: u8) -> u32 {
    let a = (age & 7) as u32;
    let (r, g, bl) = (a * 32, 255 - a * 8, a * 4);
    (r << 16) | (g << 8) | bl
}

/// A cuboid in block-local coordinates (0–1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const fn new(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> Self {
        Self { min: Vec3::new(x0, y0, z0), max: Vec3::new(x1, y1, z1) }
    }

    pub const FULL: Aabb = Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, 1.0);

    fn px(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> Self {
        Self::new(x0 / 16.0, y0 / 16.0, z0 / 16.0, x1 / 16.0, y1 / 16.0, z1 / 16.0)
    }
}

/// Read access to the blocks around the one being modelled.
pub trait Neighbors {
    fn get(&self, dx: i32, dy: i32, dz: i32) -> BlockState;
}

fn connects_fence(s: BlockState, gate_ok: bool) -> bool {
    let p = s.props();
    p.opaque || matches!(p.shape, Shape::Fence) || (gate_ok && p.shape == Shape::FenceGate)
}

fn connects_pane(s: BlockState) -> bool {
    let p = s.props();
    p.opaque || p.shape == Shape::Pane || matches!(s.id(), 20 | 95)
}

fn connects_wall(s: BlockState) -> bool {
    let p = s.props();
    p.opaque || matches!(p.shape, Shape::Wall | Shape::FenceGate)
}

/// North, south, west, east connection flags for connecting shapes.
fn connections(n: &impl Neighbors, f: impl Fn(BlockState) -> bool) -> [bool; 4] {
    [f(n.get(0, 0, -1)), f(n.get(0, 0, 1)), f(n.get(-1, 0, 0)), f(n.get(1, 0, 0))]
}

/// Door geometry needs both halves: returns (facing, open, hinge_right).
fn door_state(state: BlockState, n: &impl Neighbors) -> (u8, bool, bool) {
    let m = state.meta();
    let (lower, upper) = if m & 8 != 0 { (n.get(0, -1, 0).meta(), m) } else { (m, n.get(0, 1, 0).meta()) };
    (lower & 3, lower & 4 != 0, upper & 1 != 0)
}

/// The cuboids of a block, used for both rendering boxes and collision.
/// Shapes that are not made of cuboids (plants, liquids) return none here.
pub fn boxes(state: BlockState, n: &impl Neighbors) -> Vec<Aabb> {
    let m = state.meta();
    let p = state.props();
    match p.shape {
        Shape::Cube => vec![Aabb::FULL],
        Shape::Slab => vec![if m & 8 != 0 {
            Aabb::new(0.0, 0.5, 0.0, 1.0, 1.0, 1.0)
        } else {
            Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)
        }],
        Shape::Stairs => {
            let top = m & 4 != 0;
            let (base, step_y) = if top {
                (Aabb::new(0.0, 0.5, 0.0, 1.0, 1.0, 1.0), 0.0)
            } else {
                (Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0), 0.5)
            };
            let step = match m & 3 {
                0 => Aabb::new(0.5, step_y, 0.0, 1.0, step_y + 0.5, 1.0),
                1 => Aabb::new(0.0, step_y, 0.0, 0.5, step_y + 0.5, 1.0),
                2 => Aabb::new(0.0, step_y, 0.5, 1.0, step_y + 0.5, 1.0),
                _ => Aabb::new(0.0, step_y, 0.0, 1.0, step_y + 0.5, 0.5),
            };
            vec![base, step]
        }
        Shape::Fence => {
            let [north, south, west, east] = connections(n, |s| connects_fence(s, true));
            let mut v = vec![Aabb::px(6.0, 0.0, 6.0, 10.0, 16.0, 10.0)];
            for (on, bx) in [
                (north, (7.0, 0.0, 9.0, 6.0)),
                (south, (7.0, 10.0, 9.0, 16.0)),
                (west, (0.0, 7.0, 6.0, 9.0)),
                (east, (10.0, 7.0, 16.0, 9.0)),
            ] {
                if on {
                    let (x0, z0, x1, z1) = bx;
                    v.push(Aabb::px(x0, 12.0, z0, x1, 15.0, z1));
                    v.push(Aabb::px(x0, 6.0, z0, x1, 9.0, z1));
                }
            }
            v
        }
        Shape::Wall => {
            let [north, south, west, east] = connections(n, connects_wall);
            let mut v = vec![Aabb::px(4.0, 0.0, 4.0, 12.0, 16.0, 12.0)];
            for (on, (x0, z0, x1, z1)) in [
                (north, (5.0, 0.0, 11.0, 4.0)),
                (south, (5.0, 12.0, 11.0, 16.0)),
                (west, (0.0, 5.0, 4.0, 11.0)),
                (east, (12.0, 5.0, 16.0, 11.0)),
            ] {
                if on {
                    v.push(Aabb::px(x0, 0.0, z0, x1, 13.0, z1));
                }
            }
            v
        }
        Shape::Pane => {
            let mut c = connections(n, connects_pane);
            if c == [false; 4] {
                c = [true; 4];
            }
            let [north, south, west, east] = c;
            let mut v = vec![Aabb::px(7.0, 0.0, 7.0, 9.0, 16.0, 9.0)];
            for (on, (x0, z0, x1, z1)) in [
                (north, (7.0, 0.0, 9.0, 7.0)),
                (south, (7.0, 9.0, 9.0, 16.0)),
                (west, (0.0, 7.0, 7.0, 9.0)),
                (east, (9.0, 7.0, 16.0, 9.0)),
            ] {
                if on {
                    v.push(Aabb::px(x0, 0.0, z0, x1, 16.0, z1));
                }
            }
            v
        }
        Shape::FenceGate => {
            let along_x = m & 1 == 0;
            let open = m & 4 != 0;
            let mut v = if along_x {
                vec![Aabb::px(0.0, 5.0, 7.0, 2.0, 16.0, 9.0), Aabb::px(14.0, 5.0, 7.0, 16.0, 16.0, 9.0)]
            } else {
                vec![Aabb::px(7.0, 5.0, 0.0, 9.0, 16.0, 2.0), Aabb::px(7.0, 5.0, 14.0, 9.0, 16.0, 16.0)]
            };
            if !open {
                v.push(if along_x {
                    Aabb::px(2.0, 6.0, 7.0, 14.0, 15.0, 9.0)
                } else {
                    Aabb::px(7.0, 6.0, 2.0, 9.0, 15.0, 14.0)
                });
            }
            v
        }
        Shape::Door => {
            let (facing, open, hinge_right) = door_state(state, n);
            // Closed doors sit against the side they face away from.
            let side = if open { (facing + if hinge_right { 3 } else { 1 }) & 3 } else { facing };
            let t = 3.0 / 16.0;
            vec![match side {
                0 => Aabb::new(0.0, 0.0, 0.0, t, 1.0, 1.0),
                1 => Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, t),
                2 => Aabb::new(1.0 - t, 0.0, 0.0, 1.0, 1.0, 1.0),
                _ => Aabb::new(0.0, 0.0, 1.0 - t, 1.0, 1.0, 1.0),
            }]
        }
        Shape::Trapdoor => {
            let t = 3.0 / 16.0;
            if m & 4 == 0 {
                vec![if m & 8 != 0 {
                    Aabb::new(0.0, 1.0 - t, 0.0, 1.0, 1.0, 1.0)
                } else {
                    Aabb::new(0.0, 0.0, 0.0, 1.0, t, 1.0)
                }]
            } else {
                vec![match m & 3 {
                    0 => Aabb::new(0.0, 0.0, 1.0 - t, 1.0, 1.0, 1.0),
                    1 => Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, t),
                    2 => Aabb::new(1.0 - t, 0.0, 0.0, 1.0, 1.0, 1.0),
                    _ => Aabb::new(0.0, 0.0, 0.0, t, 1.0, 1.0),
                }]
            }
        }
        Shape::Layers => vec![Aabb::new(0.0, 0.0, 0.0, 1.0, ((m & 7) + 1) as f32 / 8.0, 1.0)],
        Shape::Ladder => {
            let t = 2.0 / 16.0;
            vec![match m {
                2 => Aabb::new(0.0, 0.0, 1.0 - t, 1.0, 1.0, 1.0),
                3 => Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0, t),
                4 => Aabb::new(1.0 - t, 0.0, 0.0, 1.0, 1.0, 1.0),
                _ => Aabb::new(0.0, 0.0, 0.0, t, 1.0, 1.0),
            }]
        }
        Shape::Torch => vec![match m {
            1 => Aabb::px(0.0, 3.0, 6.5, 3.0, 13.0, 9.5),
            2 => Aabb::px(13.0, 3.0, 6.5, 16.0, 13.0, 9.5),
            3 => Aabb::px(6.5, 3.0, 0.0, 9.5, 13.0, 3.0),
            4 => Aabb::px(6.5, 3.0, 13.0, 9.5, 13.0, 16.0),
            _ => Aabb::px(7.0, 0.0, 7.0, 9.0, 10.0, 9.0),
        }],
        Shape::Flat if state.id() == 111 => vec![Aabb::new(0.0, 0.0, 0.0, 1.0, 1.0 / 64.0, 1.0)],
        Shape::Box => fixed_box(state).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Single-cuboid blocks (exact or approximate vanilla bounds).
pub fn fixed_box(state: BlockState) -> Option<Aabb> {
    let m = state.meta();
    Some(match state.id() {
        26 => Aabb::px(0.0, 0.0, 0.0, 16.0, 9.0, 16.0),
        34 => Aabb::FULL,
        54 | 130 | 146 => Aabb::px(1.0, 0.0, 1.0, 15.0, 14.0, 15.0),
        60 => Aabb::px(0.0, 0.0, 0.0, 16.0, 15.0, 16.0),
        63 => Aabb::px(4.0, 0.0, 4.0, 12.0, 16.0, 12.0),
        68 => match m {
            2 => Aabb::px(0.0, 4.5, 14.0, 16.0, 12.5, 16.0),
            3 => Aabb::px(0.0, 4.5, 0.0, 16.0, 12.5, 2.0),
            4 => Aabb::px(14.0, 4.5, 0.0, 16.0, 12.5, 16.0),
            _ => Aabb::px(0.0, 4.5, 0.0, 2.0, 12.5, 16.0),
        },
        69 => Aabb::px(5.0, 0.0, 4.0, 11.0, 3.0, 12.0),
        70 | 72 | 147 | 148 => Aabb::px(1.0, 0.0, 1.0, 15.0, 1.0, 15.0),
        77 | 143 => Aabb::px(5.0, 6.0, 6.0, 11.0, 10.0, 10.0),
        81 => Aabb::px(1.0, 0.0, 1.0, 15.0, 16.0, 15.0),
        88 => Aabb::px(0.0, 0.0, 0.0, 16.0, 14.0, 16.0),
        90 => {
            if m & 3 == 2 {
                Aabb::px(6.0, 0.0, 0.0, 10.0, 16.0, 16.0)
            } else {
                Aabb::px(0.0, 0.0, 6.0, 16.0, 16.0, 10.0)
            }
        }
        92 => Aabb::px(1.0 + 2.0 * (m.min(6) as f32), 0.0, 1.0, 15.0, 8.0, 15.0),
        93 | 94 | 149 | 150 => Aabb::px(0.0, 0.0, 0.0, 16.0, 2.0, 16.0),
        116 => Aabb::px(0.0, 0.0, 0.0, 16.0, 12.0, 16.0),
        117 => Aabb::px(7.0, 0.0, 7.0, 9.0, 14.0, 9.0),
        118 => Aabb::FULL,
        119 => Aabb::px(0.0, 0.0, 0.0, 16.0, 12.0, 16.0),
        120 => Aabb::px(0.0, 0.0, 0.0, 16.0, 13.0, 16.0),
        122 => Aabb::px(1.0, 0.0, 1.0, 15.0, 16.0, 15.0),
        127 => Aabb::px(5.0, 3.0, 5.0, 11.0, 12.0, 11.0),
        131 => Aabb::px(6.0, 1.0, 6.0, 10.0, 9.0, 10.0),
        140 => Aabb::px(5.0, 0.0, 5.0, 11.0, 6.0, 11.0),
        144 => Aabb::px(4.0, 0.0, 4.0, 12.0, 8.0, 12.0),
        145 => Aabb::px(2.0, 0.0, 0.0, 14.0, 16.0, 16.0),
        151 | 178 => Aabb::px(0.0, 0.0, 0.0, 16.0, 6.0, 16.0),
        154 => Aabb::px(0.0, 0.0, 0.0, 16.0, 16.0, 16.0),
        171 => Aabb::px(0.0, 0.0, 0.0, 16.0, 1.0, 16.0),
        176 => Aabb::px(7.0, 0.0, 7.0, 9.0, 16.0, 9.0),
        177 => Aabb::px(0.0, 0.0, 14.0, 16.0, 16.0, 16.0),
        _ => return None,
    })
}

/// Collision boxes for movement (empty for passable blocks).
pub fn collision(state: BlockState, n: &impl Neighbors) -> Vec<Aabb> {
    let p = state.props();
    if !p.solid {
        return Vec::new();
    }
    match p.shape {
        // Fences and walls are 1.5 blocks tall for collision.
        Shape::Fence | Shape::Wall => boxes(state, n)
            .into_iter()
            .map(|b| Aabb { min: Vec3::new(b.min.x, 0.0, b.min.z), max: Vec3::new(b.max.x, 1.5, b.max.z) })
            .collect(),
        Shape::FenceGate if state.meta() & 4 == 0 => {
            if state.meta() & 1 == 0 {
                vec![Aabb::new(0.0, 0.0, 0.375, 1.0, 1.5, 0.625)]
            } else {
                vec![Aabb::new(0.375, 0.0, 0.0, 0.625, 1.5, 1.0)]
            }
        }
        Shape::FenceGate => Vec::new(),
        // A single snow layer has no collision height.
        Shape::Layers => {
            let h = (state.meta() & 7) as f32 / 8.0;
            if h > 0.0 { vec![Aabb::new(0.0, 0.0, 0.0, 1.0, h, 1.0)] } else { Vec::new() }
        }
        Shape::Box if state.id() == 81 => vec![Aabb::px(1.0, 0.0, 1.0, 15.0, 15.0, 15.0)],
        _ => boxes(state, n),
    }
}

/// Heights of a liquid's four top corners (NW, NE, SE, SW), averaged with
/// neighbouring columns of the same liquid like the vanilla renderer.
pub fn liquid_corners(state: BlockState, n: &impl Neighbors) -> [f32; 4] {
    let water = state.is_water();
    let same = |s: BlockState| s.is_liquid() && s.is_water() == water;
    let height = |dx: i32, dz: i32| -> Option<f32> {
        let s = n.get(dx, 0, dz);
        if same(n.get(dx, 1, dz)) {
            return Some(1.0);
        }
        if same(s) {
            let level = s.meta();
            return Some(if level >= 8 { 1.0 - 1.0 / 9.0 } else { 1.0 - (level as f32 + 1.0) / 9.0 });
        }
        None
    };
    let corner = |cells: [(i32, i32); 4]| -> f32 {
        let mut sum = 0.0;
        let mut count = 0.0;
        for (dx, dz) in cells {
            match height(dx, dz) {
                Some(1.0) => return 1.0,
                Some(h) => {
                    sum += h;
                    count += 1.0;
                }
                None => {
                    if !n.get(dx, 0, dz).props().solid {
                        count += 1.0;
                    }
                }
            }
        }
        if count == 0.0 { 1.0 - 1.0 / 9.0 } else { sum / count }
    };
    [
        corner([(0, 0), (-1, 0), (0, -1), (-1, -1)]),
        corner([(0, 0), (1, 0), (0, -1), (1, -1)]),
        corner([(0, 0), (1, 0), (0, 1), (1, 1)]),
        corner([(0, 0), (-1, 0), (0, 1), (-1, 1)]),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Sword,
    Shears,
}

/// Tool kind and mining speed of a held item id (1.8 numeric item ids).
pub fn tool_of_item(item: i16) -> Option<(ToolKind, f32, u8)> {
    // (kind, speed, harvest level)
    Some(match item {
        270 => (ToolKind::Pickaxe, 2.0, 0),
        274 => (ToolKind::Pickaxe, 4.0, 1),
        257 => (ToolKind::Pickaxe, 6.0, 2),
        278 => (ToolKind::Pickaxe, 8.0, 3),
        285 => (ToolKind::Pickaxe, 12.0, 0),
        271 => (ToolKind::Axe, 2.0, 0),
        275 => (ToolKind::Axe, 4.0, 1),
        258 => (ToolKind::Axe, 6.0, 2),
        279 => (ToolKind::Axe, 8.0, 3),
        286 => (ToolKind::Axe, 12.0, 0),
        269 => (ToolKind::Shovel, 2.0, 0),
        273 => (ToolKind::Shovel, 4.0, 1),
        256 => (ToolKind::Shovel, 6.0, 2),
        277 => (ToolKind::Shovel, 8.0, 3),
        284 => (ToolKind::Shovel, 12.0, 0),
        268 | 272 | 267 | 276 | 283 => (ToolKind::Sword, 1.5, 0),
        359 => (ToolKind::Shears, 1.5, 0),
        _ => return None,
    })
}

/// Pickaxe harvest level some ores need (0 wood … 3 diamond).
fn harvest_level(id: u16) -> u8 {
    match id {
        49 => 3,
        14 | 56 | 57 | 73 | 74 | 129 | 133 => 2,
        15 | 21 | 22 | 42 => 1,
        _ => 0,
    }
}

/// Ticks to break `state` with the held item under normal conditions,
/// following 1.8's dig-speed formula (no enchantments or potion effects).
/// `None` for unbreakable blocks.
pub fn break_ticks(state: BlockState, held_item: i16, on_ground: bool, in_water: bool) -> Option<u32> {
    let p = state.props();
    if p.hardness < 0.0 {
        return None;
    }
    if p.hardness == 0.0 {
        return Some(0);
    }
    let tool = tool_of_item(held_item);
    let effective = match (tool, p.tool) {
        (Some((ToolKind::Pickaxe, ..)), Tool::Pickaxe)
        | (Some((ToolKind::Axe, ..)), Tool::Axe)
        | (Some((ToolKind::Shovel, ..)), Tool::Shovel) => true,
        (Some((ToolKind::Shears, ..)), _) => matches!(state.id(), 18 | 161 | 35 | 30 | 106),
        (Some((ToolKind::Sword, ..)), _) => state.id() == 30,
        _ => false,
    };
    let mut speed = if effective { tool.map_or(1.0, |t| t.1) } else { 1.0 };
    if state.id() == 30 && matches!(tool, Some((ToolKind::Sword | ToolKind::Shears, ..))) {
        speed = 15.0;
    }
    if in_water {
        speed /= 5.0;
    }
    if !on_ground {
        speed /= 5.0;
    }
    let can_harvest = !p.needs_tool
        || matches!(tool, Some((ToolKind::Pickaxe, _, level)) if p.tool == Tool::Pickaxe && level >= harvest_level(state.id()));
    let per_tick = speed / p.hardness / if can_harvest { 30.0 } else { 100.0 };
    Some((1.0 / per_tick).ceil() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Around(std::collections::HashMap<(i32, i32, i32), BlockState>);
    impl Neighbors for Around {
        fn get(&self, dx: i32, dy: i32, dz: i32) -> BlockState {
            self.0.get(&(dx, dy, dz)).copied().unwrap_or(BlockState::AIR)
        }
    }

    #[test]
    fn table_covers_all_1_8_ids() {
        assert_eq!(TABLE.len(), 198);
        assert_eq!(props(1).name, "stone");
        assert_eq!(props(54).name, "chest");
        assert_eq!(props(95).name, "stained_glass");
        assert_eq!(props(160).name, "stained_glass_pane");
        assert_eq!(props(175).name, "double_plant");
        assert_eq!(props(197).name, "wooden_door");
        assert_eq!(props(500).name, "unknown");
        // Spot-check ids around the end of the 1.8 set.
        assert_eq!(props(183).name, "fence_gate");
        assert_eq!(props(188).name, "fence");
        assert_eq!(props(193).name, "wooden_door");
    }

    #[test]
    fn textures_follow_metadata() {
        assert_eq!(texture(BlockState::new(1, 1), Face::Up).0, "stone_granite");
        assert_eq!(texture(BlockState::new(2, 0), Face::Up), ("grass_top", Tint::Grass));
        assert_eq!(texture(BlockState::new(17, 2), Face::Up).0, "log_birch_top");
        // Log lying along X shows its end on the east face.
        assert_eq!(texture(BlockState::new(17, 4 | 1), Face::East).0, "log_spruce_top");
        assert_eq!(texture(BlockState::new(17, 4 | 1), Face::Up).0, "log_spruce");
        assert_eq!(texture(BlockState::new(35, 14), Face::North).0, "wool_colored_red");
        assert_eq!(texture(BlockState::new(61, 3), Face::South).0, "furnace_front_off");
        assert_eq!(texture(BlockState::new(86, 2), Face::North).0, "pumpkin_face_off");
        assert_eq!(texture(BlockState::new(99, 5), Face::Up).0, "mushroom_block_skin_brown");
        assert_eq!(texture(BlockState::new(99, 5), Face::North).0, "mushroom_block_inside");
        assert_eq!(texture(BlockState::new(44, 1), Face::Up).0, "sandstone_top");
        assert_eq!(texture(BlockState::new(141, 7), Face::North).0, "carrots_stage_3");
    }

    #[test]
    fn connected_shapes_follow_neighbours() {
        let fence = BlockState::new(85, 0);
        let mut around = Around(Default::default());
        assert_eq!(boxes(fence, &around).len(), 1);
        around.0.insert((1, 0, 0), BlockState::new(1, 0));
        around.0.insert((0, 0, -1), fence);
        assert_eq!(boxes(fence, &around).len(), 5);
        let c = collision(fence, &around);
        assert!(c.iter().all(|b| b.max.y == 1.5));
        // An isolated pane shows all four arms, as in 1.8.
        assert_eq!(boxes(BlockState::new(102, 0), &Around(Default::default())).len(), 5);
    }

    #[test]
    fn slabs_stairs_and_layers() {
        let none = Around(Default::default());
        assert_eq!(boxes(BlockState::new(44, 0), &none), [Aabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)]);
        assert_eq!(boxes(BlockState::new(44, 8), &none)[0].min.y, 0.5);
        let stairs = boxes(BlockState::new(53, 0), &none);
        assert_eq!(stairs[1], Aabb::new(0.5, 0.5, 0.0, 1.0, 1.0, 1.0));
        assert!(collision(BlockState::new(78, 0), &none).is_empty());
        assert_eq!(collision(BlockState::new(78, 3), &none)[0].max.y, 0.375);
        assert!(collision(BlockState::new(31, 1), &none).is_empty());
    }

    #[test]
    fn break_times_match_vanilla() {
        // Dirt by hand: 0.75 s; stone by hand: 7.5 s; stone with a wooden pickaxe: 1.15 s.
        assert_eq!(break_ticks(BlockState::new(3, 0), 0, true, false), Some(15));
        assert_eq!(break_ticks(BlockState::new(1, 0), 0, true, false), Some(150));
        assert_eq!(break_ticks(BlockState::new(1, 0), 270, true, false), Some(23));
        // Iron ore needs a stone pickaxe to harvest.
        assert_eq!(break_ticks(BlockState::new(15, 0), 270, true, false), Some(150));
        assert_eq!(break_ticks(BlockState::new(15, 0), 274, true, false), Some(23));
        assert_eq!(break_ticks(BlockState::new(7, 0), 278, true, false), None);
        assert_eq!(break_ticks(BlockState::new(31, 1), 0, true, false), Some(0));
        // Mining while airborne is five times slower.
        assert_eq!(break_ticks(BlockState::new(3, 0), 0, false, false), Some(75));
    }
}
