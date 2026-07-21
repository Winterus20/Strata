//! Sector generation: density-function terrain, biomes, caves, trees (plan 11).
//!
//! `generate_sector` fills a 32³ [`XBrickMap`] purely from world coordinates and
//! the constant [`WORLD_SEED`]; there is no per-sector mutable state, so the same
//! `SectorCoord` always produces identical voxels (column caching falls out for
//! free — the function is a deterministic map). All live voxel data is written
//! through `set_block` into the shared [`GlobalBrickPool`] (heap-fragmentation
//! ban: no per-sector `Vec`).

use std::sync::Arc;

use strata_core::prelude::*;

use crate::biome::{Biome, biome_at};
use crate::noise::{fbm2, fbm3};
use crate::rng::{Pcg32, WORLD_SEED, hash64};

/// Base terrain height (world Y) before biome amplitude is applied.
const BASE_HEIGHT: i32 = 28;
/// Water fills any air column at or below this world Y.
const SEA_LEVEL: i32 = 30;

const HEIGHT_FREQ: f32 = 0.01;
const TERRAIN_FREQ: f32 = 0.05;
const CAVE_FREQ: f32 = 0.08;
const CAVE_THRESHOLD: f32 = 0.72;

const HEIGHT_SALT: u64 = 0x3A3A_3A3A_3A3A_3A3A;
const TERRAIN_SALT: u64 = 0x4B4B_4B4B_4B4B_4B4B;
const CAVE_SALT: u64 = 0x5C5C_5C5C_5C5C_5C5C;
const TREE_SALT: u64 = 0x6D6D_6D6D_6D6D_6D6D;

/// Resolved block ids used during generation (looked up once per call).
pub struct WorldBlocks {
    pub stone: BlockId,
    pub dirt: BlockId,
    pub grass: BlockId,
    pub sand: BlockId,
    pub ice: BlockId,
    pub water: BlockId,
    pub log: BlockId,
    pub leaf: BlockId,
}

impl WorldBlocks {
    pub fn from_registry(reg: &BlockRegistry) -> Self {
        let by_name = |n: &str, fallback: u16| reg.id_by_name(n).unwrap_or(BlockId(fallback));
        WorldBlocks {
            stone: by_name("stone", 1),
            dirt: by_name("dirt", 2),
            grass: by_name("grass", 3),
            sand: by_name("sand", 6),
            ice: by_name("ice", 14),
            water: by_name("water", 7),
            log: by_name("log", 8),
            leaf: by_name("leaf", 5),
        }
    }
}

/// Surface height (world Y) for a `(x, z)` column, given its biome.
pub fn height_at(x: i32, z: i32, biome: Biome) -> i32 {
    let n = fbm2(
        x as f32 * HEIGHT_FREQ,
        z as f32 * HEIGHT_FREQ,
        WORLD_SEED ^ HEIGHT_SALT,
    ) - 0.5; // [-0.5, 0.5]
    (BASE_HEIGHT as f32 + n * biome.amplitude() as f32).round() as i32
}

/// World-Y a player should stand on for column `(x, z)`: the greater of the
/// terrain surface and sea level, so a spawn never lands inside water or below
/// ground. The topmost solid/water block occupies this Y, so feet rest at
/// `surface_y(..) + 1`. Deterministic (no sector generation needed), so the
/// client can place the player before the world streams in.
pub fn surface_y(x: i32, z: i32) -> i32 {
    height_at(x, z, biome_at(x, z)).max(SEA_LEVEL)
}

/// Density at a world point. `> 0 => solid`. Combines the terrain function
/// (`height - y` plus a low-frequency 3D wobble) with a 3D cave isosurface that
/// carves air where its value exceeds [`CAVE_THRESHOLD`].
pub fn density(x: f32, y: f32, z: f32, biome: Biome) -> f32 {
    let h = height_at(x as i32, z as i32, biome) as f32;
    let wob = (fbm3(
        x * TERRAIN_FREQ,
        y * TERRAIN_FREQ,
        z * TERRAIN_FREQ,
        WORLD_SEED ^ TERRAIN_SALT,
    ) - 0.5)
        * 3.0;
    let d = (h - y) + wob;
    if d <= 0.0 {
        return d;
    }
    let cave = fbm3(
        x * CAVE_FREQ,
        y * CAVE_FREQ,
        z * CAVE_FREQ,
        WORLD_SEED ^ CAVE_SALT,
    );
    if cave > CAVE_THRESHOLD {
        return -1.0;
    }
    d
}

/// Decide the block at a world coordinate using the precomputed column `biome`
/// and surface height `h` (AIR sentinel = 0 for empty). Byte-identical to the
/// old per-voxel path (same biome/height, just cached once per column).
fn select_block_cached(x: i32, y: i32, z: i32, wb: &WorldBlocks, biome: Biome, h: i32) -> BlockId {
    let air = BlockId::AIR;
    // Bedrock: the very bottom world layer is always solid.
    if y == 0 {
        return wb.stone;
    }
    let wob = (fbm3(
        x as f32 * TERRAIN_FREQ,
        y as f32 * TERRAIN_FREQ,
        z as f32 * TERRAIN_FREQ,
        WORLD_SEED ^ TERRAIN_SALT,
    ) - 0.5)
        * 3.0;
    let terrain_d = (h as f32 - y as f32) + wob;
    if terrain_d <= 0.0 {
        // Above the surface: fill oceans up to sea level, else air.
        if y <= SEA_LEVEL {
            return wb.water;
        }
        return air;
    }

    let cave = fbm3(
        x as f32 * CAVE_FREQ,
        y as f32 * CAVE_FREQ,
        z as f32 * CAVE_FREQ,
        WORLD_SEED ^ CAVE_SALT,
    );
    if cave > CAVE_THRESHOLD {
        return air;
    }

    let depth = h - y; // >= 0 for solid voxels (wobble can make it slightly < 0)
    match biome {
        Biome::Desert => {
            if depth < 4 {
                wb.sand
            } else {
                wb.stone
            }
        }
        Biome::Snow => {
            if depth <= 0 {
                wb.ice
            } else if depth <= 3 {
                wb.dirt
            } else {
                wb.stone
            }
        }
        Biome::Plains | Biome::Hills => {
            if depth <= 0 {
                wb.grass
            } else if depth <= 3 {
                wb.dirt
            } else {
                wb.stone
            }
        }
    }
}

/// Per-column RNG for structure placement (trees). Chunk-independent: derived
/// purely from the column coordinates.
fn col_rng(x: i32, z: i32) -> Pcg32 {
    let mixed = (x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((z as u64).wrapping_mul(0x85EB_CA77_C2B2_AE63));
    Pcg32::new(hash64(mixed, WORLD_SEED ^ TREE_SALT), 0xA3C1)
}

/// Place tree templates. Trees are only written when they fit entirely inside
/// the 32³ sector, so generation never touches a neighbouring sector.
fn place_trees(
    coord: SectorCoord,
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    wb: &WorldBlocks,
) {
    let ox = coord.0 * 32;
    let oy = coord.1 * 32;
    let oz = coord.2 * 32;

    for lx in 0..32i32 {
        for lz in 0..32i32 {
            let wx = ox + lx;
            let wz = oz + lz;
            let biome = biome_at(wx, wz);
            if biome != Biome::Plains && biome != Biome::Hills {
                continue;
            }
            let h = height_at(wx, wz, biome);
            if h <= SEA_LEVEL + 1 {
                continue; // need dry land above water
            }
            let local_surface = h - oy;
            if !((1..=31).contains(&local_surface)) {
                continue;
            }

            let mut rng = col_rng(wx, wz);
            if rng.next_u32() % 100 >= 3 {
                continue; // ~3% of eligible columns
            }
            let tree_h = 4 + (rng.next_u32() % 3) as i32; // 4..6
            let leaf_top = local_surface + tree_h + 1;
            if leaf_top > 31 {
                continue; // must fit within the sector
            }

            // Trunk.
            for dy in 0..tree_h {
                let y = (local_surface + dy) as u32;
                map.set_block(
                    pool,
                    palette,
                    VoxelCoord::new(lx as u32, y, lz as u32),
                    wb.log,
                )
                .expect("worldgen trunk set_block");
            }

            // Leaf blob centred on the trunk top.
            let cy = local_surface + tree_h;
            for dx in -2..=2i32 {
                for dy in -2..=2i32 {
                    for dz in -2..=2i32 {
                        if dx.abs() == 2 && dy.abs() == 2 && dz.abs() == 2 {
                            continue; // round the corners
                        }
                        let lx2 = lx + dx;
                        let ly2 = cy + dy;
                        let lz2 = lz + dz;
                        if !(0..=31).contains(&lx2)
                            || !(0..=31).contains(&ly2)
                            || !(0..=31).contains(&lz2)
                        {
                            continue;
                        }
                        let c = VoxelCoord::new(lx2 as u32, ly2 as u32, lz2 as u32);
                        if map.is_occupied(pool, c) {
                            continue; // never overwrite existing voxels
                        }
                        map.set_block(pool, palette, c, wb.leaf)
                            .expect("worldgen leaf set_block");
                    }
                }
            }
        }
    }
}

/// Generate a fully-populated 32³ sector into `pool`/`palette` and return its
/// [`XBrickMap`]. Pure function of `coord` + seed: re-calling with the same
/// arguments is byte-identical (column caching is automatic).
pub fn generate_sector(
    coord: SectorCoord,
    reg: &BlockRegistry,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
) -> XBrickMap {
    let wb = WorldBlocks::from_registry(reg);
    let mut map = XBrickMap::new(coord);
    let ox = coord.0 * 32;
    let oy = coord.1 * 32;
    let oz = coord.2 * 32;

    // Column cache (plan 11 §4.2.1): biome + surface height depend only on
    // `(x, z)`, so compute them once per column (32×32) instead of per voxel
    // (32×32×32). This drops the biome/height fbm2 evaluations ~32×.
    let mut col_biome = [[Biome::Plains; 32]; 32];
    let mut col_height = [[0i32; 32]; 32];
    for lx in 0..32usize {
        for lz in 0..32usize {
            let wx = ox + lx as i32;
            let wz = oz + lz as i32;
            let biome = biome_at(wx, wz);
            col_biome[lx][lz] = biome;
            col_height[lx][lz] = height_at(wx, wz, biome);
        }
    }

    for lx in 0..32u32 {
        for ly in 0..32u32 {
            for lz in 0..32u32 {
                let wx = ox + lx as i32;
                let wy = oy + ly as i32;
                let wz = oz + lz as i32;
                let biome = col_biome[lx as usize][lz as usize];
                let h = col_height[lx as usize][lz as usize];
                let b = select_block_cached(wx, wy, wz, &wb, biome, h);
                if b != BlockId::AIR {
                    map.set_block(pool, palette, VoxelCoord::new(lx, ly, lz), b)
                        .expect("worldgen terrain set_block");
                }
            }
        }
    }

    place_trees(coord, &mut map, pool, palette, &wb);
    map
}

/// Generate a sector and pack it into a shareable [`CompressedChunkData`].
/// Used by `WorldGenPlugin` (async off-thread) and the round-trip test.
pub fn generate_compressed(coord: SectorCoord, reg: &BlockRegistry) -> Arc<CompressedChunkData> {
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let map = generate_sector(coord, reg, &mut pool, &mut palette);
    Arc::new(
        map.pack(&pool, &palette)
            .expect("worldgen pack must succeed"),
    )
}

/// Generate a sector, returning both the map and its palette (shares `pool`).
/// Convenience for callers that already own the global pool (e.g. the ECS plugin).
pub fn generate_sector_in(
    coord: SectorCoord,
    reg: &BlockRegistry,
    pool: &mut GlobalBrickPool,
) -> (XBrickMap, SectorPalette) {
    let mut palette = SectorPalette::new();
    let map = generate_sector(coord, reg, pool, &mut palette);
    (map, palette)
}
