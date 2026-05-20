use strata_core::{BlockId, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

use crate::biome::ResolvedBiome;
use crate::config::{
    CARVER_RAVINE_MAX_LENGTH, CARVER_RAVINE_MAX_WIDTH, CARVER_RAVINE_MIN_LENGTH,
    CARVER_RAVINE_MIN_WIDTH, CARVER_RAVINE_PROBABILITY,
};

/// A traditional carver that cuts geometric shapes (ravines, canyons)
/// through terrain. Unlike noise-based caves, carvers use explicit
/// geometric primitives for controlled, structured voids.
pub struct Carver {
    seed: u64,
}

impl Carver {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    /// Carve all applicable carvers into the given chunk.
    ///
    /// Currently supports:
    /// - Ravines (long, narrow canyons with variable depth)
    pub fn carve(&self, chunk: &mut Chunk, biome: &ResolvedBiome, _sea_level: i32) {
        if biome.cave_density <= 0.0 {
            return;
        }

        let cx = chunk.position.0.x;
        let cz = chunk.position.0.y;

        self.carve_ravine(chunk, cx, cz);
    }

    /// Attempt to carve a ravine through this chunk.
    ///
    /// A ravine is defined by a line segment in world space. The chunk
    /// tests intersection with the ravine's bounding cylinder.
    fn carve_ravine(&self, chunk: &mut Chunk, cx: i32, cz: i32) {
        let chunk_seed = self.chunk_seed(cx, cz);
        let rng_val = (simple_hash(chunk_seed) % 10000) as f32 / 10000.0;

        // Only some chunks start a ravine
        if rng_val > CARVER_RAVINE_PROBABILITY {
            return;
        }

        // Ravine parameters — deterministic from chunk seed
        let angle = (simple_hash(chunk_seed ^ 0x1111) % 628) as f32 / 100.0; // 0..2pi
        let length = CARVER_RAVINE_MIN_LENGTH
            + (simple_hash(chunk_seed ^ 0x2222) % 1000) as f32 / 1000.0
                * (CARVER_RAVINE_MAX_LENGTH - CARVER_RAVINE_MIN_LENGTH);
        let width = CARVER_RAVINE_MIN_WIDTH
            + (simple_hash(chunk_seed ^ 0x3333) % 1000) as f32 / 1000.0
                * (CARVER_RAVINE_MAX_WIDTH - CARVER_RAVINE_MIN_WIDTH);
        let depth = 20.0 + (simple_hash(chunk_seed ^ 0x4444) % 1000) as f32 / 1000.0 * 60.0; // 20..80 blocks

        // Ravine start position (center of this chunk or offset)
        let start_x = (cx * CHUNK_WIDTH as i32)
            + (simple_hash(chunk_seed ^ 0x5555) as i32 % CHUNK_WIDTH as i32);
        let start_z = (cz * CHUNK_WIDTH as i32)
            + (simple_hash(chunk_seed ^ 0x6666) as i32 % CHUNK_WIDTH as i32);
        let start_y = 40.0 + (simple_hash(chunk_seed ^ 0x7777) % 1000) as f32 / 1000.0 * 80.0; // 40..120

        let end_x = start_x + (angle.cos() * length) as i32;
        let end_z = start_z + (angle.sin() * length) as i32;

        let chunk_wx = chunk.position.world_x();
        let chunk_wz = chunk.position.world_z();

        // Check if this ravine segment passes through the chunk
        let (near_x, near_z) = closest_point_on_segment(
            chunk_wx as f32,
            chunk_wz as f32,
            start_x as f32,
            start_z as f32,
            end_x as f32,
            end_z as f32,
        );
        let dist_to_chunk =
            (((near_x - chunk_wx as f32).powi(2) + (near_z - chunk_wz as f32).powi(2)).sqrt()
                - CHUNK_WIDTH as f32 * 0.707)
                .max(0.0);

        let carve_radius = width * 0.5 + CHUNK_WIDTH as f32 * 0.707;
        if dist_to_chunk > carve_radius {
            return;
        }

        let half_width = (width * 0.5) as i32;

        // Carve the ravine through the chunk
        for lx in 0..CHUNK_WIDTH {
            for lz in 0..CHUNK_WIDTH {
                let wx = chunk_wx + lx as i32;
                let wz = chunk_wz + lz as i32;

                // Distance from ravine segment
                let (px, pz) = closest_point_on_segment(
                    wx as f32,
                    wz as f32,
                    start_x as f32,
                    start_z as f32,
                    end_x as f32,
                    end_z as f32,
                );
                let dist = ((wx as f32 - px).powi(2) + (wz as f32 - pz).powi(2)).sqrt();

                if dist > half_width as f32 {
                    continue;
                }

                // V-shaped cross section: deeper in center, shallower at edges
                let normalized_dist = dist / half_width.max(1) as f32;
                let depth_at_point = (depth * (1.0 - normalized_dist * normalized_dist)) as i32;

                let bottom_y = (start_y as i32 - depth_at_point).max(1);
                let top_y = start_y as i32;

                for y in bottom_y..top_y.min(CHUNK_HEIGHT as i32 - 1) {
                    if y < 0 || y >= CHUNK_HEIGHT as i32 {
                        continue;
                    }
                    let idx = Chunk::index(lx, y as usize, lz);
                    let block = chunk.blocks[idx];
                    if block != BlockId::AIR.0 && block != BlockId::WATER.0 {
                        chunk.blocks[idx] = BlockId::AIR.0;
                    }
                }
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
}

/// Find the closest point on segment AB to point P.
#[inline(always)]
fn closest_point_on_segment(px: f32, pz: f32, ax: f32, az: f32, bx: f32, bz: f32) -> (f32, f32) {
    let abx = bx - ax;
    let abz = bz - az;
    let len_sq = abx * abx + abz * abz;
    if len_sq < f32::EPSILON {
        return (ax, az);
    }
    let t = ((px - ax) * abx + (pz - az) * abz) / len_sq;
    let t = t.clamp(0.0, 1.0);
    (ax + t * abx, az + t * abz)
}

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
