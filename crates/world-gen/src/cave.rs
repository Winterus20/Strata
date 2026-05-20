use strata_core::{BlockId, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

use crate::biome::ResolvedBiome;
use crate::config::CHEESE_CAVE_THRESHOLD;
use crate::noise::TerrainNoise;

/// Compute fused cave density from cheese + spaghetti + noodle noise.
///
/// Returns a value in [0, 1] where higher = more likely to be a cave.
/// The biome's `cave_density` and `cave_modifier` scale the result.
#[inline(always)]
pub fn cave_system_density(
    cheese_val: f32,
    spaghetti_val: f32,
    noodle_val: f32,
    cave_density: f32,
    cave_modifier: f32,
) -> f32 {
    // Cheese caves: threshold test
    let cheese_mask = smoothstep(
        CHEESE_CAVE_THRESHOLD - 0.1,
        CHEESE_CAVE_THRESHOLD + 0.1,
        cheese_val,
    );

    // Spaghetti caves: thin band around zero-crossing
    let spaghetti_abs = spaghetti_val.abs();
    let spaghetti_dist = 1.0 - (spaghetti_abs - 0.4).abs() * 10.0;
    let spaghetti_mask = smoothstep(0.2, 0.8, spaghetti_dist);

    // Noodle caves: even thinner band, higher frequency
    let noodle_abs = noodle_val.abs();
    let noodle_dist = 1.0 - (noodle_abs - 0.425).abs() * 20.0;
    let noodle_mask = smoothstep(0.3, 0.7, noodle_dist);

    // Fuse: max of all three cave types
    let fused = cheese_mask
        .max(spaghetti_mask * cave_modifier)
        .max(noodle_mask * cave_modifier * 0.5);
    fused * cave_density
}

/// Apply the 3-cave-type system (cheese + spaghetti + noodle) to a chunk
/// using pre-computed noise grids. Only processes columns with terrain.
pub fn carve_caves_faz3(
    chunk: &mut Chunk,
    cave_grid: &[f32],
    spaghetti_grid: &[f32],
    noodle_grid: &[f32],
    biome: &ResolvedBiome,
    sea_level: i32,
) {
    if biome.cave_density <= 0.0 {
        return;
    }

    let y_max = (sea_level - 1).max(1) as usize;

    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let col = Chunk::column_index(x, z);
            let top = chunk.heightmap_top[col] as usize;
            if top < 2 {
                continue;
            }

            let col_y_max = top.min(y_max);

            for y in 1..col_y_max {
                let idx = Chunk::index(x, y, z);
                if chunk.blocks[idx] == BlockId::AIR.0 {
                    continue;
                }

                let noise_idx = x + (y - 1) * CHUNK_WIDTH + z * CHUNK_WIDTH * y_max;
                let cheese_val = cave_grid[noise_idx];
                let spaghetti_val = if noise_idx < spaghetti_grid.len() {
                    spaghetti_grid[noise_idx]
                } else {
                    0.0
                };
                let noodle_val = if noise_idx < noodle_grid.len() {
                    noodle_grid[noise_idx]
                } else {
                    0.0
                };

                let density = cave_system_density(
                    cheese_val,
                    spaghetti_val,
                    noodle_val,
                    biome.cave_density,
                    biome.cave_modifier,
                );

                if density > 0.5 {
                    chunk.blocks[idx] = BlockId::AIR.0;
                }
            }
        }
    }

    rebuild_heightmaps_after_carve(chunk);
}

/// Legacy carve_caves (Faz 2 compatible) — cheese + spaghetti only.
pub fn carve_caves(
    chunk: &mut Chunk,
    cave_grid: &[f32],
    spaghetti_grid: &[f32],
    biome: &ResolvedBiome,
    sea_level: i32,
) {
    if biome.cave_density <= 0.0 {
        return;
    }

    let y_max = (sea_level - 1).max(1) as usize;

    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let col = Chunk::column_index(x, z);
            let top = chunk.heightmap_top[col] as usize;
            if top < 2 {
                continue;
            }

            let col_y_max = top.min(y_max);

            for y in 1..col_y_max {
                let idx = Chunk::index(x, y, z);
                if chunk.blocks[idx] == BlockId::AIR.0 {
                    continue;
                }

                let noise_idx = x + (y - 1) * CHUNK_WIDTH + z * CHUNK_WIDTH * y_max;
                let cheese_val = cave_grid[noise_idx];
                let spaghetti_val = if noise_idx < spaghetti_grid.len() {
                    spaghetti_grid[noise_idx]
                } else {
                    0.0
                };

                let density = cave_system_density(
                    cheese_val,
                    spaghetti_val,
                    0.0,
                    biome.cave_density,
                    biome.cave_modifier,
                );

                if density > 0.5 {
                    chunk.blocks[idx] = BlockId::AIR.0;
                }
            }
        }
    }

    rebuild_heightmaps_after_carve(chunk);
}

/// Recompute heightmaps for all columns after cave carving.
fn rebuild_heightmaps_after_carve(chunk: &mut Chunk) {
    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let col = Chunk::column_index(x, z);
            let mut top = 0u16;
            for y in (1..CHUNK_HEIGHT).rev() {
                if chunk.blocks[Chunk::index(x, y, z)] != BlockId::AIR.0 {
                    top = y as u16;
                    break;
                }
            }
            chunk.heightmap_top[col] = top;

            let mut bottom = 0u16;
            for y in 1..CHUNK_HEIGHT {
                if chunk.blocks[Chunk::index(x, y, z)] != BlockId::AIR.0 {
                    bottom = y as u16;
                    break;
                }
            }
            chunk.heightmap_bottom[col] = bottom;
        }
    }
}

/// Legacy single-type cave carving (Faz 1 compatible).
pub fn carve_cheese_caves(
    chunk: &mut Chunk,
    noise: &TerrainNoise,
    biome: &ResolvedBiome,
    sea_level: i32,
    threshold: f32,
) {
    if biome.cave_density <= 0.0 {
        return;
    }

    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let col = Chunk::column_index(x, z);
            let top = chunk.heightmap_top[col] as usize;
            if top < 2 {
                continue;
            }

            let wx = chunk.position.world_x() + x as i32;
            let wz = chunk.position.world_z() + z as i32;

            let y_max = top.min((sea_level - 1) as usize);
            for y in 1..y_max {
                let block = chunk.get_block(x, y, z);
                if block == BlockId::AIR {
                    continue;
                }

                let cave_val = noise.cave_3d(wx as f32 * 0.01, y as f32 * 0.01, wz as f32 * 0.01);
                if cave_val > threshold * biome.cave_density {
                    chunk.set_block(x, y, z, BlockId::AIR);
                }
            }
        }
    }
}

#[inline(always)]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
