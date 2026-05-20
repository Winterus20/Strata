use std::collections::HashMap;

use strata_core::{BlockId, CHUNK_DEPTH, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

use crate::biome::{ResolvedBiome, TreeType};
use crate::config::{
    DUNGEON_SPACING, MAX_STRUCTURES_PER_CHUNK, RUIN_SPACING, SWAMP_HUT_SPACING, VILLAGE_SPACING,
};

// ── Ore Vein Definitions ─────────────────────────────────────────────

/// An ore vein definition matching Minecraft-style distribution.
#[derive(Debug, Clone)]
pub struct OreVein {
    pub block: BlockId,
    pub filler_block: BlockId,
    pub min_y: i32,
    pub max_y: i32,
    pub density: f32,
    pub size: f32,
    pub noise_frequency: f32,
}

/// Returns the default ore vein set for Faz 2.
pub fn default_ore_veins() -> Vec<OreVein> {
    vec![
        OreVein {
            block: BlockId::from_raw(11),
            filler_block: BlockId::STONE,
            min_y: 0,
            max_y: 130,
            density: 0.2,
            size: 8.0,
            noise_frequency: 0.02,
        },
        OreVein {
            block: BlockId::from_raw(12),
            filler_block: BlockId::STONE,
            min_y: -24,
            max_y: 72,
            density: 0.15,
            size: 5.0,
            noise_frequency: 0.025,
        },
        OreVein {
            block: BlockId::from_raw(13),
            filler_block: BlockId::STONE,
            min_y: -16,
            max_y: 112,
            density: 0.12,
            size: 6.0,
            noise_frequency: 0.03,
        },
        OreVein {
            block: BlockId::from_raw(14),
            filler_block: BlockId::STONE,
            min_y: -64,
            max_y: 30,
            density: 0.08,
            size: 4.0,
            noise_frequency: 0.04,
        },
        OreVein {
            block: BlockId::from_raw(15),
            filler_block: BlockId::STONE,
            min_y: -64,
            max_y: 15,
            density: 0.1,
            size: 3.0,
            noise_frequency: 0.035,
        },
        OreVein {
            block: BlockId::from_raw(16),
            filler_block: BlockId::STONE,
            min_y: -64,
            max_y: 16,
            density: 0.03,
            size: 2.0,
            noise_frequency: 0.05,
        },
        OreVein {
            block: BlockId::from_raw(17),
            filler_block: BlockId::STONE,
            min_y: -16,
            max_y: 32,
            density: 0.01,
            size: 1.0,
            noise_frequency: 0.06,
        },
    ]
}

/// Places ore veins in a chunk using 3D noise.
#[allow(clippy::too_many_arguments)]
pub fn place_ores(chunk: &mut Chunk, veins: &[OreVein]) {
    let wx = chunk.position.world_x();
    let wz = chunk.position.world_z();

    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let hx = wx + x as i32;
            let hz = wz + z as i32;

            for y in 0..CHUNK_HEIGHT {
                let idx = Chunk::index(x, y, z);
                if chunk.blocks[idx] != BlockId::STONE.0 {
                    continue;
                }

                for vein in veins {
                    if y < vein.min_y as usize || y > vein.max_y as usize {
                        continue;
                    }

                    let density = ore_noise(hx, y as i32, hz, vein.noise_frequency);
                    if density > vein.density {
                        chunk.blocks[idx] = vein.block.0;
                        break;
                    }
                }
            }
        }
    }
}

/// Simple deterministic 3D noise for ore placement.
#[inline(always)]
fn ore_noise(x: i32, y: i32, z: i32, _freq: f32) -> f32 {
    let val = hash_3d(x, y, z) as f32 / u32::MAX as f32;
    let val2 = hash_3d(x + 100, y + 200, z + 300) as f32 / u32::MAX as f32;
    (val + val2) * 0.5
}

#[inline(always)]
fn hash_3d(x: i32, y: i32, z: i32) -> u32 {
    let mut h = x.wrapping_mul(374761393) as u64;
    h = h.wrapping_add(y as u64);
    h = h.wrapping_mul(668265263);
    h ^= h >> 15;
    h = h.wrapping_add(z as u64);
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    h as u32
}

// ── Biome-specific Structure Types ────────────────────────────────────

/// Types of biome-specific structures (Faz 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StructureType {
    Village,
    DesertVillage,
    SavannaVillage,
    TaigaVillage,
    Dungeon,
    DesertWell,
    Ruin,
    SwampHut,
    IceSpike,
    CactusPatch,
}

/// A structure instance placed in the world.
#[derive(Debug, Clone)]
pub struct StructureInstance {
    pub structure_type: StructureType,
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub local_x: usize,
    pub local_z: usize,
    pub ground_y: usize,
}

/// Biome-specific structure placer (Faz 4).
///
/// Places villages, dungeons, ruins, wells, and swamp huts
/// using deterministic seed-based placement with biome filtering.
pub struct StructurePlacer {
    seed: u64,
    structure_cache: HashMap<(i32, i32), Vec<StructureInstance>>,
}

impl StructurePlacer {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            structure_cache: HashMap::new(),
        }
    }

    /// Place all biome-specific structures in a chunk.
    pub fn place_structures(&mut self, chunk: &mut Chunk, biome: &ResolvedBiome) {
        let cx = chunk.position.0.x;
        let cz = chunk.position.0.y;

        let structures = self.structures_for_chunk(cx, cz, biome);
        for struct_inst in &structures {
            self.place_structure(chunk, struct_inst, biome);
        }
    }

    /// Determine which structures should be placed in this chunk.
    fn structures_for_chunk(
        &self,
        cx: i32,
        cz: i32,
        biome: &ResolvedBiome,
    ) -> Vec<StructureInstance> {
        let key = (cx, cz);
        if let Some(cached) = self.structure_cache.get(&key) {
            return cached.clone();
        }

        let mut structures = Vec::new();
        let biome_id = biome.id;

        // Check each structure type
        if let Some(s) = self.check_village(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_dungeon(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_ruin(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_swamp_hut(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_desert_well(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_ice_spike(cx, cz, biome_id) {
            structures.push(s);
        }
        if let Some(s) = self.check_cactus_patch(cx, cz, biome_id) {
            structures.push(s);
        }

        structures.truncate(MAX_STRUCTURES_PER_CHUNK);
        structures
    }

    fn place_structure(&self, chunk: &mut Chunk, s: &StructureInstance, _biome: &ResolvedBiome) {
        match s.structure_type {
            StructureType::Village => {
                self.build_house(chunk, s, BlockId::WOOD, BlockId::from_raw(5))
            }
            StructureType::DesertVillage => {
                self.build_house(chunk, s, BlockId::SAND, BlockId::from_raw(7))
            }
            StructureType::SavannaVillage => {
                self.build_house(chunk, s, BlockId::from_raw(24), BlockId::from_raw(5))
            }
            StructureType::TaigaVillage => {
                self.build_house(chunk, s, BlockId::from_raw(20), BlockId::from_raw(5))
            }
            StructureType::Dungeon => self.build_dungeon(chunk, s),
            StructureType::DesertWell => self.build_desert_well(chunk, s),
            StructureType::Ruin => self.build_ruin(chunk, s),
            StructureType::SwampHut => self.build_swamp_hut(chunk, s),
            StructureType::IceSpike => self.build_ice_spike(chunk, s),
            StructureType::CactusPatch => self.build_cactus_patch(chunk, s),
        }
    }

    // ── Structure Checks ────────────────────────────────────────────

    fn check_village(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 5 && biome_id != 6 && biome_id != 10 && biome_id != 11 && biome_id != 12 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x1000);
        if !seed.is_multiple_of(VILLAGE_SPACING as u64) {
            return None;
        }

        let village_type = match biome_id {
            10 => StructureType::DesertVillage,
            11 => StructureType::SavannaVillage,
            12 => StructureType::TaigaVillage,
            _ => StructureType::Village,
        };

        let local_x = ((seed >> 16) % 10) as usize + 3;
        let local_z = ((seed >> 24) % 10) as usize + 3;

        Some(StructureInstance {
            structure_type: village_type,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_dungeon(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        // Dungeons spawn in most biomes underground
        if biome_id >= 10 || biome_id == 0 || biome_id == 1 || biome_id == 2 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x2000);
        if !seed.is_multiple_of(DUNGEON_SPACING as u64) {
            return None;
        }

        let local_x = ((seed >> 16) % 8) as usize + 4;
        let local_z = ((seed >> 24) % 8) as usize + 4;

        Some(StructureInstance {
            structure_type: StructureType::Dungeon,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_ruin(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 3 && biome_id != 10 && biome_id != 11 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x3000);
        if !seed.is_multiple_of(RUIN_SPACING as u64) {
            return None;
        }

        let local_x = ((seed >> 16) % 12) as usize + 2;
        let local_z = ((seed >> 24) % 12) as usize + 2;

        Some(StructureInstance {
            structure_type: StructureType::Ruin,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_swamp_hut(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 18 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x4000);
        if !seed.is_multiple_of(SWAMP_HUT_SPACING as u64) {
            return None;
        }

        let local_x = ((seed >> 16) % 8) as usize + 4;
        let local_z = ((seed >> 24) % 8) as usize + 4;

        Some(StructureInstance {
            structure_type: StructureType::SwampHut,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_desert_well(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 10 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x5000);
        if !seed.is_multiple_of(8) {
            return None;
        }

        let local_x = ((seed >> 16) % 10) as usize + 3;
        let local_z = ((seed >> 24) % 10) as usize + 3;

        Some(StructureInstance {
            structure_type: StructureType::DesertWell,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_ice_spike(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 19 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x6000);
        if !seed.is_multiple_of(4) {
            return None;
        }

        let local_x = ((seed >> 16) % 14) as usize + 1;
        let local_z = ((seed >> 24) % 14) as usize + 1;

        Some(StructureInstance {
            structure_type: StructureType::IceSpike,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    fn check_cactus_patch(&self, cx: i32, cz: i32, biome_id: u16) -> Option<StructureInstance> {
        if biome_id != 10 {
            return None;
        }

        let seed = self.structure_seed(cx, cz, 0x7000);
        if !seed.is_multiple_of(3) {
            return None;
        }

        let local_x = ((seed >> 16) % 12) as usize + 2;
        let local_z = ((seed >> 24) % 12) as usize + 2;

        Some(StructureInstance {
            structure_type: StructureType::CactusPatch,
            chunk_x: cx,
            chunk_z: cz,
            local_x,
            local_z,
            ground_y: 0,
        })
    }

    // ── Structure Builders ──────────────────────────────────────────

    fn build_house(
        &self,
        chunk: &mut Chunk,
        s: &StructureInstance,
        wall_block: BlockId,
        roof_block: BlockId,
    ) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 6).contains(&ground_y) {
            return;
        }

        let w = 5usize;
        let h = 3usize;
        let d = 4usize;

        // Clear interior
        for dx in 0..w {
            for dz in 0..d {
                for dy in 1..=h {
                    let bx = s.local_x + dx;
                    let bz = s.local_z + dz;
                    let by = ground_y + dy;
                    if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && by < CHUNK_HEIGHT {
                        // Interior is air
                        if dx > 0 && dx < w - 1 && dz > 0 && dz < d - 1 {
                            chunk.set_block(bx, by, bz, BlockId::AIR);
                        }
                    }
                }
            }
        }

        // Walls
        for dx in 0..w {
            for dz in 0..d {
                for dy in 0..=h {
                    let bx = s.local_x + dx;
                    let bz = s.local_z + dz;
                    let by = ground_y + dy;

                    if bx >= CHUNK_WIDTH || bz >= CHUNK_DEPTH || by >= CHUNK_HEIGHT {
                        continue;
                    }

                    let is_wall = dx == 0 || dx == w - 1 || dz == 0 || dz == d - 1;
                    let is_roof = dy == h;

                    if is_roof {
                        chunk.set_block(bx, by, bz, roof_block);
                    } else if is_wall {
                        // Door opening on front face
                        if dz == 0 && dx >= w / 2 - 1 && dx <= w / 2 + 1 && dy < 2 {
                            chunk.set_block(bx, by, bz, BlockId::AIR);
                        } else {
                            chunk.set_block(bx, by, bz, wall_block);
                        }
                    }
                }
            }
        }

        // Floor
        for dx in 0..w {
            for dz in 0..d {
                let bx = s.local_x + dx;
                let bz = s.local_z + dz;
                if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && ground_y < CHUNK_HEIGHT {
                    chunk.set_block(bx, ground_y, bz, wall_block);
                }
            }
        }
    }

    fn build_dungeon(&self, chunk: &mut Chunk, s: &StructureInstance) {
        // Find underground position
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if ground_y < 10 {
            return;
        }

        let room_y = (ground_y / 2).max(10).min(ground_y - 5);
        let room_w = 7usize;
        let room_h = 3usize;
        let room_d = 7usize;

        let seed = self.structure_seed(s.chunk_x, s.chunk_z, 0x8000);

        // Carve room
        for dx in 0..room_w {
            for dz in 0..room_d {
                for dy in 0..room_h {
                    let bx = s.local_x + dx + 1;
                    let bz = s.local_z + dz + 1;
                    let by = room_y + dy;

                    if bx >= CHUNK_WIDTH || bz >= CHUNK_DEPTH || by >= CHUNK_HEIGHT {
                        continue;
                    }

                    let is_edge = dx == 0 || dx == room_w - 1 || dz == 0 || dz == room_d - 1;
                    let is_floor = dy == 0;
                    let is_ceiling = dy == room_h - 1;

                    if is_edge || is_floor || is_ceiling {
                        // Cobblestone walls / floor / ceiling
                        chunk.set_block(bx, by, bz, BlockId::STONE);
                    } else {
                        chunk.set_block(bx, by, bz, BlockId::AIR);
                    }
                }
            }
        }

        // Entrance tunnel (use i32 to avoid usize underflow)
        let tunnel_len = (ground_y as i32 - room_y as i32 + 5).max(1) as usize;
        for dy in 0..tunnel_len {
            let bx = s.local_x + room_w / 2 + 1;
            let bz = s.local_z + room_d / 2 + 1;
            let by = ground_y as i32 - dy as i32;
            if by < 0 || by as usize >= CHUNK_HEIGHT {
                continue;
            }
            let by = by as usize;

            if bx >= CHUNK_WIDTH || bz >= CHUNK_DEPTH {
                continue;
            }

            chunk.set_block(bx, by, bz, BlockId::AIR);
            if by > 0 {
                chunk.set_block(bx, by - 1, bz, BlockId::AIR);
            }
        }

        // Spawner (marker — use mossy cobblestone as placeholder)
        let spawner_x = s.local_x + room_w / 2 + 1;
        let spawner_z = s.local_z + room_d / 2 + 1;
        let spawner_y = room_y + 1;
        if spawner_x < CHUNK_WIDTH && spawner_z < CHUNK_DEPTH && spawner_y < CHUNK_HEIGHT {
            chunk.set_block(spawner_x, spawner_y, spawner_z, BlockId::from_raw(30));
        }

        // Chests (2-3 per dungeon)
        let num_chests = 2 + ((seed >> 40) % 2) as usize;
        for i in 0..num_chests {
            let cx = s.local_x + 2 + ((seed >> (i * 8)) % (room_w - 4) as u64) as usize;
            let cz = s.local_z + 2 + ((seed >> (i * 8 + 4)) % (room_d - 4) as u64) as usize;
            let cy = room_y + 1;
            if cx < CHUNK_WIDTH && cz < CHUNK_DEPTH && cy < CHUNK_HEIGHT {
                chunk.set_block(cx, cy, cz, BlockId::from_raw(31));
            }
        }
    }

    fn build_desert_well(&self, chunk: &mut Chunk, s: &StructureInstance) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 3).contains(&ground_y) {
            return;
        }

        // Well walls (3x3 circle, 2 blocks high)
        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                let dist = dx.abs().max(dz.abs());
                if dist != 2 && dist != 1 {
                    continue;
                }

                let bx = s.local_x as i32 + dx;
                let bz = s.local_z as i32 + dz;
                if bx < 0 || bx >= CHUNK_WIDTH as i32 || bz < 0 || bz >= CHUNK_DEPTH as i32 {
                    continue;
                }

                for dy in 0..2 {
                    let by = ground_y + dy;
                    if by < CHUNK_HEIGHT {
                        chunk.set_block(bx as usize, by, bz as usize, BlockId::SAND);
                    }
                }
            }
        }

        // Water in center
        let cx = s.local_x;
        let cz = s.local_z;
        if cx < CHUNK_WIDTH && cz < CHUNK_DEPTH && ground_y < CHUNK_HEIGHT {
            chunk.set_block(cx, ground_y, cz, BlockId::WATER);
        }
    }

    fn build_ruin(&self, chunk: &mut Chunk, s: &StructureInstance) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 3).contains(&ground_y) {
            return;
        }

        let seed = self.structure_seed(s.chunk_x, s.chunk_z, 0x9000);

        for dx in -2i32..=2 {
            for dz in -2i32..=2 {
                let is_wall = dx == -2 || dx == 2 || dz == -2 || dz == 2;
                if !is_wall {
                    continue;
                }

                let bx = s.local_x as i32 + dx;
                let bz = s.local_z as i32 + dz;
                if bx < 0 || bx >= CHUNK_WIDTH as i32 || bz < 0 || bz >= CHUNK_DEPTH as i32 {
                    continue;
                }

                let wall_seed = seed ^ ((dx as u64) << 8) ^ (dz as u64);
                if wall_seed.is_multiple_of(3) {
                    continue;
                }

                let wall_height = 1 + (wall_seed % 2) as usize;
                for dy in 0..wall_height {
                    let by = ground_y + dy;
                    if by < CHUNK_HEIGHT {
                        chunk.set_block(bx as usize, by, bz as usize, BlockId::STONE);
                    }
                }
            }
        }

        for dx in -1i32..=1 {
            for dz in -1i32..=1 {
                let bx = s.local_x as i32 + dx;
                let bz = s.local_z as i32 + dz;
                if bx >= 0
                    && bx < CHUNK_WIDTH as i32
                    && bz >= 0
                    && bz < CHUNK_DEPTH as i32
                    && ground_y < CHUNK_HEIGHT
                {
                    chunk.set_block(bx as usize, ground_y, bz as usize, BlockId::STONE);
                }
            }
        }
    }

    fn build_swamp_hut(&self, chunk: &mut Chunk, s: &StructureInstance) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 5).contains(&ground_y) {
            return;
        }

        // Stilts (4 pillars)
        let stilts = [(0, 0), (3, 0), (0, 3), (3, 3)];
        for &(dx, dz) in &stilts {
            let bx = s.local_x + dx;
            let bz = s.local_z + dz;

            for dy in 1..=3 {
                let by = ground_y + dy;
                if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && by < CHUNK_HEIGHT {
                    chunk.set_block(bx, by, bz, BlockId::WOOD);
                }
            }
        }

        // Floor
        let floor_y = ground_y + 3;
        for dx in 0..4 {
            for dz in 0..4 {
                let bx = s.local_x + dx;
                let bz = s.local_z + dz;
                if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && floor_y < CHUNK_HEIGHT {
                    chunk.set_block(bx, floor_y, bz, BlockId::WOOD);
                }
            }
        }

        // Walls
        for dx in 0..4 {
            for dz in 0..4 {
                let is_wall = dx == 0 || dx == 3 || dz == 0 || dz == 3;
                if !is_wall {
                    continue;
                }

                let by = floor_y + 1;
                let bx = s.local_x + dx;
                let bz = s.local_z + dz;
                if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && by < CHUNK_HEIGHT {
                    chunk.set_block(bx, by, bz, BlockId::WOOD);
                }
            }
        }

        // Roof
        let roof_y = floor_y + 2;
        for dx in 0..4 {
            for dz in 0..4 {
                let bx = s.local_x + dx;
                let bz = s.local_z + dz;
                if bx < CHUNK_WIDTH && bz < CHUNK_DEPTH && roof_y < CHUNK_HEIGHT {
                    chunk.set_block(bx, roof_y, bz, BlockId::LEAVES);
                }
            }
        }
    }

    fn build_ice_spike(&self, chunk: &mut Chunk, s: &StructureInstance) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 15).contains(&ground_y) {
            return;
        }

        let seed = self.structure_seed(s.chunk_x, s.chunk_z, 0xA000);
        let height = 5 + (seed % 10) as usize;
        let base_radius = 2usize;

        // Build spike from bottom up
        for dy in 0..height {
            let radius = if dy < 2 {
                base_radius
            } else {
                let t = dy as f32 / height as f32;
                (base_radius as f32 * (1.0 - t)).ceil() as usize
            };

            for dx in -(radius as i32)..=(radius as i32) {
                for dz in -(radius as i32)..=(radius as i32) {
                    let dist = ((dx * dx + dz * dz) as f32).sqrt();
                    if dist > radius as f32 {
                        continue;
                    }

                    let bx = s.local_x as i32 + dx;
                    let bz = s.local_z as i32 + dz;
                    let by = ground_y + dy;

                    if bx >= 0
                        && bx < CHUNK_WIDTH as i32
                        && bz >= 0
                        && bz < CHUNK_DEPTH as i32
                        && by < CHUNK_HEIGHT
                    {
                        chunk.set_block(bx as usize, by, bz as usize, BlockId::from_raw(32));
                    }
                }
            }
        }
    }

    fn build_cactus_patch(&self, chunk: &mut Chunk, s: &StructureInstance) {
        let ground_y = self.find_ground(chunk, s.local_x, s.local_z);
        if !(2..CHUNK_HEIGHT - 3).contains(&ground_y) {
            return;
        }

        let seed = self.structure_seed(s.chunk_x, s.chunk_z, 0xB000);
        let count = 2 + (seed % 3) as usize;

        for i in 0..count {
            let ox = ((seed >> (i * 8)) % 5) as usize;
            let oz = ((seed >> (i * 8 + 4)) % 5) as usize;
            let bx = s.local_x + ox;
            let bz = s.local_z + oz;

            if bx >= CHUNK_WIDTH || bz >= CHUNK_DEPTH {
                continue;
            }

            let cactus_ground = self.find_ground(chunk, bx, bz);
            if !(2..CHUNK_HEIGHT - 3).contains(&cactus_ground) {
                continue;
            }

            let cactus_height = 2 + ((seed >> (i * 4)) % 2) as usize;
            for dy in 1..=cactus_height {
                let by = cactus_ground + dy;
                if by < CHUNK_HEIGHT {
                    chunk.set_block(bx, by, bz, BlockId::from_raw(28));
                }
            }
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────

    fn find_ground(&self, chunk: &Chunk, lx: usize, lz: usize) -> usize {
        let col = Chunk::column_index(lx, lz);
        chunk.heightmap_top[col] as usize
    }

    fn structure_seed(&self, cx: i32, cz: i32, base: u64) -> u64 {
        let mut h = self.seed;
        h ^= base;
        h ^= (cx as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h ^= (cz as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h
    }
}

// ── Poisson Disk Tree Placer ─────────────────────────────────────────

/// Poisson disk tree placer for natural tree distribution.
pub struct PoissonTreePlacer {
    seed: u64,
    min_radius: f32,
    max_radius: f32,
}

impl PoissonTreePlacer {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            min_radius: 4.0,
            max_radius: 8.0,
        }
    }

    pub fn place_trees(&self, chunk: &mut Chunk, biome: &ResolvedBiome) {
        if biome.tree_density <= 0.0 || biome.tree_type == TreeType::None {
            return;
        }

        let cx = chunk.position.0.x;
        let cz = chunk.position.0.y;
        let chunk_seed = self.chunk_seed(cx, cz);

        let radius = self.min_radius + (biome.tree_density * (self.max_radius - self.min_radius));
        let cell_size = (radius * 0.707) as usize;
        let cell_size = cell_size.max(2);

        let grid_cols = CHUNK_WIDTH / cell_size;
        let grid_rows = CHUNK_DEPTH / cell_size;

        for gx in 0..grid_cols {
            for gz in 0..grid_rows {
                let cell_seed = chunk_seed ^ ((gx as u64) << 16) ^ (gz as u64);
                let pos_hash = simple_hash(cell_seed);

                let lx = gx * cell_size + (pos_hash & 0xF) as usize;
                let lz = gz * cell_size + ((pos_hash >> 4) & 0xF) as usize;

                if !(2..CHUNK_WIDTH - 2).contains(&lx) || !(2..CHUNK_DEPTH - 2).contains(&lz) {
                    continue;
                }

                let density_hash = simple_hash(cell_seed ^ 0xABCD);
                let density_val = (density_hash % 1000) as f32 / 1000.0;
                if density_val > biome.tree_density {
                    continue;
                }

                let wy = self.surface_height(chunk, lx, lz);
                if wy == 0 || wy >= CHUNK_HEIGHT - 10 {
                    continue;
                }

                self.make_tree(chunk, lx, wy, lz, biome.tree_type);
            }
        }
    }

    fn chunk_seed(&self, cx: i32, cz: i32) -> u64 {
        let mut h = self.seed;
        h ^= (cx as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h ^= (cz as u64).wrapping_mul(0x9E3779B97F4A7C15);
        h = h.wrapping_mul(0xBF58476D1CE4E5B9);
        h
    }

    fn surface_height(&self, chunk: &Chunk, lx: usize, lz: usize) -> usize {
        for y in (1..CHUNK_HEIGHT).rev() {
            let block = chunk.get_block(lx, y, lz);
            if block == BlockId::GRASS || block == BlockId::DIRT || block == BlockId::SNOW {
                return y;
            }
        }
        0
    }

    fn make_tree(
        &self,
        chunk: &mut Chunk,
        lx: usize,
        ground_y: usize,
        lz: usize,
        tree_type: TreeType,
    ) {
        match tree_type {
            TreeType::Oak => self.make_oak(chunk, lx, ground_y, lz),
            TreeType::Birch => self.make_birch(chunk, lx, ground_y, lz),
            TreeType::Pine => self.make_pine(chunk, lx, ground_y, lz),
            TreeType::Jungle => self.make_jungle(chunk, lx, ground_y, lz),
            TreeType::Acacia => self.make_acacia(chunk, lx, ground_y, lz),
            TreeType::DarkOak => self.make_dark_oak(chunk, lx, ground_y, lz),
            TreeType::Cactus => self.make_cactus(chunk, lx, ground_y, lz),
            TreeType::None => {}
        }
    }

    fn make_oak(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 5usize;
        if ground_y + trunk_height + 2 >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::WOOD);
        }

        let leaf_start = ground_y + trunk_height - 1;
        for dy in 0..3 {
            let radius: i32 = if dy == 1 { 2 } else { 1 };
            let y = leaf_start + dy;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;

                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    if dx.abs() == radius && dz.abs() == radius && dy != 1 {
                        continue;
                    }

                    if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                        let block = chunk.get_block(bx as usize, y, bz as usize);
                        if block.is_air() || block == BlockId::LEAVES {
                            chunk.set_block(bx as usize, y, bz as usize, BlockId::LEAVES);
                        }
                    }
                }
            }
        }
    }

    fn make_birch(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 4usize;
        if ground_y + trunk_height + 2 >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(18));
        }

        let leaf_start = ground_y + trunk_height - 1;
        for dy in 0..3 {
            let radius: i32 = if dy == 1 { 2 } else { 1 };
            let y = leaf_start + dy;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;

                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    if dx.abs() == radius && dz.abs() == radius && dy != 1 {
                        continue;
                    }

                    if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                        let block = chunk.get_block(bx as usize, y, bz as usize);
                        if block.is_air() || block == BlockId::LEAVES {
                            chunk.set_block(bx as usize, y, bz as usize, BlockId::from_raw(19));
                        }
                    }
                }
            }
        }
    }

    fn make_pine(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 7usize;
        if ground_y + trunk_height + 1 >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(20));
        }

        for dy in 0i32..4 {
            let radius = (3 - dy).max(1);
            let y = ground_y + trunk_height - 3 + dy as usize;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() == radius && dz.abs() == radius {
                        continue;
                    }
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;
                    if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                        let block = chunk.get_block(bx as usize, y, bz as usize);
                        if block.is_air() || block == BlockId::LEAVES {
                            chunk.set_block(bx as usize, y, bz as usize, BlockId::from_raw(21));
                        }
                    }
                }
            }
        }
    }

    fn make_jungle(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 8usize;
        if ground_y + trunk_height + 3 >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(22));
        }

        let leaf_start = ground_y + trunk_height - 2;
        for dy in 0..4 {
            let radius: i32 = if dy == 1 || dy == 2 { 3 } else { 2 };
            let y = leaf_start + dy;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let dist = dx.abs().max(dz.abs());
                    if dist == radius && dy != 1 && dy != 2 {
                        continue;
                    }
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;
                    if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                        let block = chunk.get_block(bx as usize, y, bz as usize);
                        if block.is_air() || block == BlockId::LEAVES {
                            chunk.set_block(bx as usize, y, bz as usize, BlockId::from_raw(23));
                        }
                    }
                }
            }
        }
    }

    fn make_acacia(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 3usize;
        if ground_y + trunk_height + 2 >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(24));
        }

        let y = ground_y + trunk_height + 1;
        for dx in -2..=2 {
            for dz in -2..=2 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let bx = lx as i32 + dx;
                let bz = lz as i32 + dz;
                if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                    let block = chunk.get_block(bx as usize, y, bz as usize);
                    if block.is_air() || block == BlockId::LEAVES {
                        chunk.set_block(bx as usize, y, bz as usize, BlockId::from_raw(25));
                    }
                }
            }
        }
    }

    fn make_dark_oak(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 6usize;
        if ground_y + trunk_height + 2 >= CHUNK_HEIGHT
            || lx >= CHUNK_WIDTH - 1
            || lz >= CHUNK_DEPTH - 1
        {
            return;
        }

        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(26));
            chunk.set_block(lx + 1, ground_y + dy, lz, BlockId::from_raw(26));
            chunk.set_block(lx, ground_y + dy, lz + 1, BlockId::from_raw(26));
            chunk.set_block(lx + 1, ground_y + dy, lz + 1, BlockId::from_raw(26));
        }

        let leaf_start = ground_y + trunk_height - 2;
        for dy in 0..4 {
            let radius: i32 = if dy == 1 || dy == 2 { 3 } else { 2 };
            let y = leaf_start + dy;
            for dx in -(radius + 1)..=(radius + 1) {
                for dz in -(radius + 1)..=(radius + 1) {
                    let dist = dx.abs().max(dz.abs());
                    if dist > radius + 1 || (dist == radius + 1 && dy != 1 && dy != 2) {
                        continue;
                    }
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;
                    if bx >= 0 && bx < CHUNK_WIDTH as i32 && bz >= 0 && bz < CHUNK_DEPTH as i32 {
                        let block = chunk.get_block(bx as usize, y, bz as usize);
                        if block.is_air() || block == BlockId::LEAVES {
                            chunk.set_block(bx as usize, y, bz as usize, BlockId::from_raw(27));
                        }
                    }
                }
            }
        }
    }

    fn make_cactus(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let height = 3usize;
        if ground_y + height >= CHUNK_HEIGHT {
            return;
        }

        for dy in 1..=height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::from_raw(28));
        }
    }
}

// ── Utility ──────────────────────────────────────────────────────────

#[inline(always)]
fn simple_hash(seed: u64) -> u64 {
    let mut h = seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51AFD7ED558CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CEB9FE1A85EC53);
    h ^= h >> 33;
    h
}
