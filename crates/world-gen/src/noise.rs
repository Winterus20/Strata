use fastnoise2::generator::prelude::*;

use crate::config::{LOD_BASE_FREQ, LOD_FREQ_MULTIPLIER, LOD_LEVELS};

/// 14-node noise graph for Faz 4 world generation.
///
/// Extends Faz 3 with:
/// - Multi-resolution noise for LOD system (4 levels)
/// - Barrier noise for full aquifer system
/// - GPU-friendly batch generation methods
pub struct TerrainNoise {
    // Core terrain nodes
    continental: fastnoise2::SafeNode,
    erosion: fastnoise2::SafeNode,
    weirdness: fastnoise2::SafeNode,
    detail: fastnoise2::SafeNode,
    temperature: fastnoise2::SafeNode,
    humidity: fastnoise2::SafeNode,

    // Cave nodes
    cave: fastnoise2::SafeNode,
    spaghetti: fastnoise2::SafeNode,
    noodle: fastnoise2::SafeNode,

    // Domain warp nodes
    warp_x: fastnoise2::SafeNode,
    warp_z: fastnoise2::SafeNode,

    // Aquifer nodes (Faz 4)
    aquifer: fastnoise2::SafeNode,
    aquifer_barrier: fastnoise2::SafeNode,

    // Multi-resolution LOD noise (Faz 4)
    lod_detail: [fastnoise2::SafeNode; LOD_LEVELS],

    seed: i32,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        let seed = seed as i32;

        let continental = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 4, 2.0)
            .build()
            .0;
        let erosion = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 4, 2.0)
            .build()
            .0;
        let weirdness = supersimplex()
            .with_feature_scale(1.0)
            .ridged(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let detail = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let temperature = value()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let humidity = value()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let cave = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let spaghetti = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let noodle = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 3, 2.0)
            .build()
            .0;
        let warp_x = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 2, 2.0)
            .build()
            .0;
        let warp_z = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 2, 2.0)
            .build()
            .0;
        let aquifer = supersimplex()
            .with_feature_scale(1.0)
            .fbm(0.5, 0.0, 2, 2.0)
            .build()
            .0;
        let aquifer_barrier = supersimplex()
            .with_feature_scale(1.0)
            .ridged(0.5, 0.0, 2, 2.0)
            .build()
            .0;

        // Multi-resolution LOD noise (4 levels, decreasing frequency)
        let lod_detail = [
            supersimplex()
                .with_feature_scale(1.0)
                .fbm(0.5, 0.0, 3, 2.0)
                .build()
                .0,
            supersimplex()
                .with_feature_scale(1.0)
                .fbm(0.5, 0.0, 2, 2.0)
                .build()
                .0,
            supersimplex()
                .with_feature_scale(1.0)
                .fbm(0.5, 0.0, 2, 2.0)
                .build()
                .0,
            supersimplex()
                .with_feature_scale(1.0)
                .fbm(0.5, 0.0, 1, 2.0)
                .build()
                .0,
        ];

        Self {
            continental,
            erosion,
            weirdness,
            detail,
            temperature,
            humidity,
            cave,
            spaghetti,
            noodle,
            warp_x,
            warp_z,
            aquifer,
            aquifer_barrier,
            lod_detail,
            seed,
        }
    }

    // ── 2D single-point helpers ─────────────────────────────────────

    #[inline]
    pub fn continental(&self, x: i32, z: i32) -> f32 {
        self.continental
            .gen_single_2d(x as f32 * 0.0015, z as f32 * 0.0015, self.seed)
    }

    #[inline]
    pub fn erosion(&self, x: i32, z: i32) -> f32 {
        self.erosion
            .gen_single_2d(x as f32 * 0.002, z as f32 * 0.002, self.seed)
    }

    #[inline]
    pub fn weirdness(&self, x: i32, z: i32) -> f32 {
        self.weirdness
            .gen_single_2d(x as f32 * 0.0025, z as f32 * 0.0025, self.seed)
    }

    #[inline]
    pub fn temperature(&self, x: i32, z: i32) -> f32 {
        self.temperature
            .gen_single_2d(x as f32 * 0.003, z as f32 * 0.003, self.seed)
    }

    #[inline]
    pub fn humidity(&self, x: i32, z: i32) -> f32 {
        self.humidity
            .gen_single_2d(x as f32 * 0.003, z as f32 * 0.003, self.seed)
    }

    // ── 3D single-point helpers ─────────────────────────────────────

    #[inline]
    pub fn detail_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.detail
            .gen_single_3d(x * 0.016, y * 0.016, z * 0.016, self.seed)
    }

    #[inline]
    pub fn cave_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.cave.gen_single_3d(x, y, z, self.seed)
    }

    #[inline]
    pub fn spaghetti_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.spaghetti.gen_single_3d(x, y, z, self.seed)
    }

    #[inline]
    pub fn noodle_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.noodle.gen_single_3d(x, y, z, self.seed)
    }

    #[inline]
    pub fn domain_warp(&self, x: f32, z: f32, amplitude: f32) -> (f32, f32) {
        let ox = self.warp_x.gen_single_2d(x * 0.002, z * 0.002, self.seed) * amplitude;
        let oz = self.warp_z.gen_single_2d(x * 0.002, z * 0.002, self.seed) * amplitude;
        (x + ox, z + oz)
    }

    #[inline]
    pub fn aquifer_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.aquifer.gen_single_3d(x, y, z, self.seed)
    }

    #[inline]
    pub fn aquifer_barrier_3d(&self, x: f32, y: f32, z: f32) -> f32 {
        self.aquifer_barrier.gen_single_3d(x, y, z, self.seed)
    }

    // ── Multi-resolution LOD noise (Faz 4) ──────────────────────────

    /// Get 3D detail noise at a specific LOD level.
    /// Level 0 = highest detail, level N = coarser.
    #[inline]
    pub fn lod_detail_3d(&self, level: usize, x: f32, y: f32, z: f32) -> f32 {
        if level >= LOD_LEVELS {
            return 0.0;
        }
        let freq = LOD_BASE_FREQ * LOD_FREQ_MULTIPLIER.powi(level as i32);
        self.lod_detail[level].gen_single_3d(x * freq, y * freq, z * freq, self.seed)
    }

    /// Generate a downsampled 3D noise grid for a chunk at a given LOD level.
    /// Output size: `stride^3` where stride = 16 / 2^level.
    pub fn lod_grid(&self, out: &mut [f32], level: usize, wx: f32, wy: f32, wz: f32, size: i32) {
        if level >= LOD_LEVELS || out.len() < (size * size * size) as usize {
            return;
        }
        let freq = LOD_BASE_FREQ * LOD_FREQ_MULTIPLIER.powi(level as i32);
        let step = 1i32 << level;
        let gen_size = size * step;
        self.lod_detail[level].gen_uniform_grid_3d(
            out,
            wx * freq,
            wy * freq,
            wz * freq,
            gen_size,
            gen_size,
            gen_size,
            freq,
            freq,
            freq,
            self.seed,
        );
    }

    // ── SIMD 2D batch helpers ───────────────────────────────────────

    pub fn continental_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        self.continental.gen_uniform_grid_2d(
            out,
            wx as f32 * 0.0015,
            wz as f32 * 0.0015,
            16,
            16,
            0.0015,
            0.0015,
            self.seed,
        );
    }

    pub fn erosion_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        self.erosion.gen_uniform_grid_2d(
            out,
            wx as f32 * 0.002,
            wz as f32 * 0.002,
            16,
            16,
            0.002,
            0.002,
            self.seed,
        );
    }

    pub fn weirdness_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        self.weirdness.gen_uniform_grid_2d(
            out,
            wx as f32 * 0.0025,
            wz as f32 * 0.0025,
            16,
            16,
            0.0025,
            0.0025,
            self.seed,
        );
    }

    pub fn temperature_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        self.temperature.gen_uniform_grid_2d(
            out,
            wx as f32 * 0.003,
            wz as f32 * 0.003,
            16,
            16,
            0.003,
            0.003,
            self.seed,
        );
    }

    pub fn humidity_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        self.humidity.gen_uniform_grid_2d(
            out,
            wx as f32 * 0.003,
            wz as f32 * 0.003,
            16,
            16,
            0.003,
            0.003,
            self.seed,
        );
    }

    // ── SIMD 3D batch helpers ───────────────────────────────────────

    #[allow(clippy::too_many_arguments)]
    pub fn cave_grid(
        &self,
        out: &mut [f32],
        wx: f32,
        wy: f32,
        wz: f32,
        x_count: i32,
        y_count: i32,
        z_count: i32,
    ) {
        self.cave.gen_uniform_grid_3d(
            out,
            wx * 0.01,
            wy * 0.01,
            wz * 0.01,
            x_count,
            y_count,
            z_count,
            0.01,
            0.01,
            0.01,
            self.seed,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spaghetti_grid(
        &self,
        out: &mut [f32],
        wx: f32,
        wy: f32,
        wz: f32,
        x_count: i32,
        y_count: i32,
        z_count: i32,
    ) {
        self.spaghetti.gen_uniform_grid_3d(
            out,
            wx * 0.008,
            wy * 0.008,
            wz * 0.008,
            x_count,
            y_count,
            z_count,
            0.008,
            0.008,
            0.008,
            self.seed,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn noodle_grid(
        &self,
        out: &mut [f32],
        wx: f32,
        wy: f32,
        wz: f32,
        x_count: i32,
        y_count: i32,
        z_count: i32,
    ) {
        self.noodle.gen_uniform_grid_3d(
            out,
            wx * 0.015,
            wy * 0.015,
            wz * 0.015,
            x_count,
            y_count,
            z_count,
            0.015,
            0.015,
            0.015,
            self.seed,
        );
    }

    pub fn warp_grid(
        &self,
        out_x: &mut [f32],
        out_z: &mut [f32],
        wx: i32,
        wz: i32,
        amplitude: f32,
    ) {
        let start_x = wx as f32 * 0.002;
        let start_z = wz as f32 * 0.002;
        self.warp_x
            .gen_uniform_grid_2d(out_x, start_x, start_z, 16, 16, 0.002, 0.002, self.seed);
        self.warp_z
            .gen_uniform_grid_2d(out_z, start_x, start_z, 16, 16, 0.002, 0.002, self.seed);
        for v in out_x.iter_mut() {
            *v = *v * amplitude + wx as f32;
        }
        for v in out_z.iter_mut() {
            *v = *v * amplitude + wz as f32;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn aquifer_grid(
        &self,
        out: &mut [f32],
        wx: f32,
        wy: f32,
        wz: f32,
        x_count: i32,
        y_count: i32,
        z_count: i32,
    ) {
        self.aquifer.gen_uniform_grid_3d(
            out,
            wx * 0.005,
            wy * 0.005,
            wz * 0.005,
            x_count,
            y_count,
            z_count,
            0.005,
            0.005,
            0.005,
            self.seed,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn aquifer_barrier_grid(
        &self,
        out: &mut [f32],
        wx: f32,
        wy: f32,
        wz: f32,
        x_count: i32,
        y_count: i32,
        z_count: i32,
    ) {
        self.aquifer_barrier.gen_uniform_grid_3d(
            out,
            wx * 0.003,
            wy * 0.003,
            wz * 0.003,
            x_count,
            y_count,
            z_count,
            0.003,
            0.003,
            0.003,
            self.seed,
        );
    }

    /// Batch-generate all 5 biome parameters for a 16x16 column grid.
    pub fn biome_params_grid(&self, out: &mut [f32], wx: i32, wz: i32) {
        let stride = 256;
        self.continental_grid(&mut out[..stride], wx, wz);
        self.erosion_grid(&mut out[stride..2 * stride], wx, wz);
        self.weirdness_grid(&mut out[2 * stride..3 * stride], wx, wz);
        self.temperature_grid(&mut out[3 * stride..4 * stride], wx, wz);
        self.humidity_grid(&mut out[4 * stride..5 * stride], wx, wz);
    }

    pub fn seed(&self) -> i32 {
        self.seed
    }
}
