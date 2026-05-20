use strata_core::{BlockId, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

use crate::aquifer::Aquifer;
use crate::biome::{BiomeRegistry, NoiseParams};
use crate::carver::Carver;
use crate::config::{
    CAVE_Y_MAX, CONTINENTAL_SPLINE, DETAIL_AMPLITUDE, DOMAIN_WARP_AMPLITUDE, DOMAIN_WARP_ENABLED,
    EROSION_SPLINE, HEIGHT_BIAS_MULTIPLIER, HEIGHTMAP_PADDING, SEA_LEVEL, WEIRDNESS_SPLINE,
};
use crate::noise::TerrainNoise;
use crate::structure::{PoissonTreePlacer, StructurePlacer, default_ore_veins, place_ores};
use crate::surface::default_surface_rules;

/// Faz 4 terrain generator — full Minecraft-style world generation.
///
/// Key improvements over Faz 3:
/// - Full aquifer system with local water tables, lava pockets, barriers
/// - Data-driven biome system (serializable from JSON/TOML)
/// - Biome-specific structures (villages, dungeons, ruins, huts, ice spikes)
/// - Multi-resolution LOD noise support
/// - GPU compute terrain generation infrastructure
pub struct TerrainGenerator {
    noise: TerrainNoise,
    biomes: BiomeRegistry,
    tree_placer: PoissonTreePlacer,
    structure_placer: StructurePlacer,
    ore_veins: Vec<crate::structure::OreVein>,
    carver: Carver,
    aquifer: Aquifer,
}

impl TerrainGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            noise: TerrainNoise::new(seed),
            biomes: BiomeRegistry::new(),
            tree_placer: PoissonTreePlacer::new(seed as u64),
            structure_placer: StructurePlacer::new(seed as u64),
            ore_veins: default_ore_veins(),
            carver: Carver::new(seed as u64),
            aquifer: Aquifer::new(seed as u64),
        }
    }

    pub fn noise(&self) -> &TerrainNoise {
        &self.noise
    }

    pub fn biomes(&self) -> &BiomeRegistry {
        &self.biomes
    }

    pub fn carver(&self) -> &Carver {
        &self.carver
    }

    pub fn aquifer(&self) -> &Aquifer {
        &self.aquifer
    }

    pub fn height_at(&self, x: i32, z: i32) -> f32 {
        base_height_from_continental(self.noise.continental(x, z))
            + erosion_offset(self.noise.erosion(x, z))
            + weirdness_offset(self.noise.weirdness(x, z))
    }

    /// Smooth biome parameters using a 5x5 Gaussian-weighted neighborhood.
    ///
    /// This prevents abrupt biome transitions at chunk boundaries by
    /// averaging noise parameters over a local region. Each column's
    /// continentalness, erosion, weirdness, temperature, and humidity
    /// values are smoothed independently.
    ///
    /// Uses direct noise sampling (not the pre-computed grid) for columns
    /// outside the current chunk, ensuring deterministic results regardless
    /// of chunk generation order.
    fn smooth_biome_params(
        out: &mut [f32; 5 * 256],
        grid: &[f32; 5 * 256],
        wx: i32,
        wz: i32,
        noise: &crate::noise::TerrainNoise,
    ) {
        let stride = CHUNK_WIDTH;
        let radius = 2i32;

        // Gaussian weights for 5x5 kernel (sigma=1.0)
        const W: [[f32; 5]; 5] = [
            [0.0037, 0.0150, 0.0232, 0.0150, 0.0037],
            [0.0150, 0.0608, 0.0938, 0.0608, 0.0150],
            [0.0232, 0.0938, 0.1447, 0.0938, 0.0232],
            [0.0150, 0.0608, 0.0938, 0.0608, 0.0150],
            [0.0037, 0.0150, 0.0232, 0.0150, 0.0037],
        ];

        for cz in 0..stride {
            for cx in 0..stride {
                let col = cx + cz * stride;
                let mut sums = [0.0f32; 5];
                let mut weight_total = 0.0f32;

                for dz in -radius..=radius {
                    for dx in -radius..=radius {
                        let wi = (dz + radius) as usize;
                        let wj = (dx + radius) as usize;
                        let weight = W[wi][wj];

                        let local_x = cx as i32 + dx;
                        let local_z = cz as i32 + dz;
                        let nx = wx + local_x;
                        let nz = wz + local_z;

                        // Use grid for in-chunk, direct sample for neighbors
                        if local_x >= 0
                            && local_x < stride as i32
                            && local_z >= 0
                            && local_z < stride as i32
                        {
                            let ncol = local_x as usize + local_z as usize * stride;
                            for p in 0..5 {
                                sums[p] += grid[p * stride * stride + ncol] * weight;
                            }
                        } else {
                            sums[0] += noise.continental(nx, nz) * weight;
                            sums[1] += noise.erosion(nx, nz) * weight;
                            sums[2] += noise.weirdness(nx, nz) * weight;
                            sums[3] += noise.temperature(nx, nz) * weight;
                            sums[4] += noise.humidity(nx, nz) * weight;
                        }
                        weight_total += weight;
                    }
                }

                // Normalize
                let inv = 1.0 / weight_total;
                for p in 0..5 {
                    out[p * stride * stride + col] = sums[p] * inv;
                }
            }
        }
    }

    /// Compute local shoreline water level for each column.
    ///
    /// For columns below sea level, the water level is set to the minimum
    /// terrain height of adjacent land columns (clamped to SEA_LEVEL max).
    /// This ensures water always connects naturally to the shoreline.
    fn compute_shoreline_water_levels(heightmap: &[u16; 256]) -> [i32; 256] {
        let mut water_levels = [SEA_LEVEL; 256];
        let sea_u16 = SEA_LEVEL as u16;

        for col in 0..256 {
            let col_x = col % CHUNK_WIDTH;
            let col_z = col / CHUNK_WIDTH;
            let top = heightmap[col];

            if top >= sea_u16 {
                water_levels[col] = top as i32;
                continue;
            }

            let mut min_land_height = SEA_LEVEL;
            let mut found_land = false;

            for dz in -1..=1i32 {
                for dx in -1..=1i32 {
                    if dx == 0 && dz == 0 {
                        continue;
                    }
                    let nx = col_x as i32 + dx;
                    let nz = col_z as i32 + dz;
                    if nx >= 0 && nx < CHUNK_WIDTH as i32 && nz >= 0 && nz < CHUNK_WIDTH as i32 {
                        let ncol = Chunk::column_index(nx as usize, nz as usize);
                        let ntop = heightmap[ncol];
                        if ntop >= sea_u16 {
                            if (ntop as i32) < min_land_height {
                                min_land_height = ntop as i32;
                            }
                            found_land = true;
                        }
                    }
                }
            }

            if found_land {
                water_levels[col] = min_land_height;
            } else {
                water_levels[col] = SEA_LEVEL;
            }
        }

        water_levels
    }

    /// Full Faz 4 chunk generation pipeline.
    ///
    /// Pipeline:
    ///  1. Batch biome noise grids (SIMD)
    ///  2. Smooth biome parameters (5x5 Gaussian) for natural transitions
    ///  3. Column biomes + heightmap
    ///  4. Heightmap bounding
    ///  5. 3D density (optionally domain-warped)
    ///  6. Fill terrain blocks
    ///  7. Carve caves (cheese + spaghetti + noodle)
    ///  8. Carve traditional carvers (ravines)
    ///  9. Apply surface rules (Faz 4 data-driven engine)
    /// 10. Apply full aquifer (water/lava/barriers)
    /// 11. Place trees + ores
    /// 12. Place biome-specific structures (Faz 4)
    pub fn generate(&mut self, chunk: &mut Chunk) {
        let wx = chunk.position.world_x();
        let wz = chunk.position.world_z();

        // ── Step 1: Batch biome noise grids ────────────────────────
        let stride = 256;
        let mut biome_grid = [0.0f32; 5 * 256];
        self.noise.biome_params_grid(&mut biome_grid, wx, wz);

        // ── Step 2: Smooth biome parameters (5x5 Gaussian) ─────────
        let mut smoothed = [0.0f32; 5 * 256];
        Self::smooth_biome_params(&mut smoothed, &biome_grid, wx, wz, &self.noise);

        let continental_vals = &smoothed[..stride];
        let erosion_vals = &smoothed[stride..2 * stride];
        let weirdness_vals = &smoothed[2 * stride..3 * stride];
        let temperature_vals = &smoothed[3 * stride..4 * stride];
        let humidity_vals = &smoothed[4 * stride..5 * stride];

        // ── Step 2: Column biomes + heightmap ──────────────────────
        let mut biome_ids = [0u16; 256];
        let mut base_heightmap = [0usize; 256];

        for col in 0..256 {
            let params = NoiseParams::from_grid(
                continental_vals,
                erosion_vals,
                weirdness_vals,
                temperature_vals,
                humidity_vals,
                col,
            );
            let biome = self.biomes.select(&params);
            biome_ids[col] = biome.id;

            let base_h = (base_height_from_continental(params.continentalness)
                + erosion_offset(params.erosion)
                + weirdness_offset(params.weirdness))
            .round() as i32;
            base_heightmap[col] = base_h.clamp(1, CHUNK_HEIGHT as i32 - 1) as usize;
        }

        // ── Step 3: Heightmap bounding ─────────────────────────────
        let min_col_y_iter = base_heightmap.iter().min().copied().unwrap_or(0);
        let max_col_y_iter = base_heightmap.iter().max().copied().unwrap_or(0);
        let y_start = min_col_y_iter.saturating_sub(HEIGHTMAP_PADDING).max(1);
        let y_end = (max_col_y_iter + HEIGHTMAP_PADDING).min(CHUNK_HEIGHT - 1);

        // ── Step 4: 3D density computation ─────────────────────────
        let y_range_size = y_end - y_start + 1;
        let mut density_buffer = vec![0.0f32; 256 * y_range_size];

        if DOMAIN_WARP_ENABLED {
            let mut warp_x_vals = [0.0f32; 256];
            let mut warp_z_vals = [0.0f32; 256];
            self.noise.warp_grid(
                &mut warp_x_vals,
                &mut warp_z_vals,
                wx,
                wz,
                DOMAIN_WARP_AMPLITUDE,
            );

            for col in 0..256 {
                let base_h = base_heightmap[col] as f32;
                let wx_warped = warp_x_vals[col];
                let wz_warped = warp_z_vals[col];

                for (y_offset, wy) in (y_start..=y_end).enumerate() {
                    let wy_f = wy as f32;
                    let height_bias = (base_h - wy_f) * HEIGHT_BIAS_MULTIPLIER;
                    let detail =
                        self.noise.detail_3d(wx_warped, wy_f, wz_warped) * DETAIL_AMPLITUDE;
                    density_buffer[col * y_range_size + y_offset] = height_bias + detail;
                }
            }
        } else {
            for col in 0..256 {
                let col_x = col % CHUNK_WIDTH;
                let col_z = col / CHUNK_WIDTH;
                let base_h = base_heightmap[col] as f32;
                let wx_f = (wx + col_x as i32) as f32;
                let wz_f = (wz + col_z as i32) as f32;

                for (y_offset, wy) in (y_start..=y_end).enumerate() {
                    let wy_f = wy as f32;
                    let height_bias = (base_h - wy_f) * HEIGHT_BIAS_MULTIPLIER;
                    let detail = self.noise.detail_3d(wx_f, wy_f, wz_f) * DETAIL_AMPLITUDE;
                    density_buffer[col * y_range_size + y_offset] = height_bias + detail;
                }
            }
        }

        // ── Step 5: Fill terrain blocks ────────────────────────────
        for col in 0..256 {
            let col_x = col % CHUNK_WIDTH;
            let col_z = col / CHUNK_WIDTH;
            let base_h = base_heightmap[col];

            for y in 0..CHUNK_HEIGHT {
                let idx = Chunk::index(col_x, y, col_z);

                let block = if y == 0 {
                    BlockId::BEDROCK
                } else if y < y_start || y > y_end {
                    if y <= base_h {
                        BlockId::STONE
                    } else {
                        BlockId::AIR
                    }
                } else {
                    let y_offset = y - y_start;
                    let density = density_buffer[col * y_range_size + y_offset];
                    if density > 0.0 {
                        BlockId::STONE
                    } else {
                        BlockId::AIR
                    }
                };

                chunk.blocks[idx] = block.0;
            }

            let mut top = 0u16;
            for y in (1..CHUNK_HEIGHT).rev() {
                let idx = Chunk::index(col_x, y, col_z);
                if chunk.blocks[idx] != BlockId::AIR.0 {
                    top = y as u16;
                    break;
                }
            }
            chunk.heightmap_top[col] = top;

            let mut bottom = 0u16;
            for y in 1..CHUNK_HEIGHT {
                let idx = Chunk::index(col_x, y, col_z);
                if chunk.blocks[idx] != BlockId::AIR.0 {
                    bottom = y as u16;
                    break;
                }
            }
            chunk.heightmap_bottom[col] = bottom;
        }

        // ── Step 6: Carve caves ────────────────────────────────────
        self.carve_caves_in_chunk(chunk, wx, wz, y_start, y_end, &biome_ids);

        // ── Step 7: Carve traditional carvers ──────────────────────
        let dominant_biome_id = {
            let mut freq = [0u16; 256];
            for &id in &biome_ids {
                freq[id as usize] += 1;
            }
            freq.iter()
                .enumerate()
                .max_by_key(|&(_, &c)| c)
                .map(|(i, _)| i as u16)
                .unwrap_or(0)
        };
        let resolved_biomes = self.biomes.resolve_all();
        let dominant_biome = &resolved_biomes[dominant_biome_id as usize];
        self.carver.carve(chunk, dominant_biome, SEA_LEVEL);

        // ── Step 8: Compute shoreline water levels ──────────────────
        let shoreline_levels = Self::compute_shoreline_water_levels(&chunk.heightmap_top);

        // ── Step 9: Apply surface rules ────────────────────────────
        let rules = default_surface_rules();
        for (col, &biome_id) in biome_ids.iter().enumerate() {
            let col_x = col % CHUNK_WIDTH;
            let col_z = col / CHUNK_WIDTH;
            let biome = &resolved_biomes[biome_id as usize];

            let top = chunk.heightmap_top[col] as usize;
            if top == 0 {
                continue;
            }

            let stone_depth = (top as i32 - biome.filler_depth as i32).max(0) as usize;

            for y in 0..=top {
                let idx = Chunk::index(col_x, y, col_z);

                let block = if y == 0 {
                    BlockId::BEDROCK
                } else if y > top {
                    BlockId::AIR
                } else if y < stone_depth {
                    BlockId::STONE
                } else if y == top {
                    rules
                        .evaluate(chunk, col_x, y, col_z, biome)
                        .unwrap_or(biome.top_block)
                } else {
                    biome.filler_block
                };

                chunk.blocks[idx] = block.0;
            }

            if let Some(ocean) = biome.ocean_block
                && top < shoreline_levels[col] as usize
            {
                let water_level = shoreline_levels[col] as usize;
                for y in (top + 1)..=water_level {
                    let idx = Chunk::index(col_x, y, col_z);
                    if chunk.blocks[idx] == BlockId::AIR.0 {
                        chunk.blocks[idx] = ocean.0;
                    }
                }
            }
        }

        // ── Step 10: Apply full aquifer (Faz 4) ─────────────────────
        let carve_y_start_aq = y_start.max(1);
        let carve_y_end_aq = y_end.min(CAVE_Y_MAX as usize);
        if carve_y_start_aq < carve_y_end_aq {
            let y_count = carve_y_end_aq - carve_y_start_aq + 1;
            let aquifer_volume = CHUNK_WIDTH * y_count * CHUNK_WIDTH;

            let mut aquifer_vals = vec![0.0f32; aquifer_volume];
            let mut barrier_vals = vec![0.0f32; aquifer_volume];

            self.noise.aquifer_grid(
                &mut aquifer_vals,
                wx as f32,
                carve_y_start_aq as f32,
                wz as f32,
                CHUNK_WIDTH as i32,
                y_count as i32,
                CHUNK_WIDTH as i32,
            );
            self.noise.aquifer_barrier_grid(
                &mut barrier_vals,
                wx as f32,
                carve_y_start_aq as f32,
                wz as f32,
                CHUNK_WIDTH as i32,
                y_count as i32,
                CHUNK_WIDTH as i32,
            );

            self.aquifer.fill_chunk_with_shoreline(
                chunk,
                &aquifer_vals,
                &barrier_vals,
                &resolved_biomes[dominant_biome_id as usize],
                carve_y_start_aq,
                carve_y_end_aq,
                SEA_LEVEL,
                &shoreline_levels,
            );
        }

        // ── Step 11: Place trees and ores ──────────────────────────
        self.tree_placer.place_trees(chunk, dominant_biome);
        place_ores(chunk, &self.ore_veins);

        // ── Step 12: Place biome-specific structures (Faz 4) ───────
        self.structure_placer
            .place_structures(chunk, dominant_biome);

        chunk.dirty = false;
        chunk.light_dirty = true;
    }

    /// Carve caves (cheese + spaghetti + noodle) using batch noise grids.
    fn carve_caves_in_chunk(
        &self,
        chunk: &mut Chunk,
        wx: i32,
        wz: i32,
        y_start: usize,
        y_end: usize,
        biome_ids: &[u16],
    ) {
        let carve_y_start = y_start.max(1);
        let carve_y_end = y_end.min(CAVE_Y_MAX as usize);
        if carve_y_start >= carve_y_end {
            return;
        }

        let y_count = (carve_y_end - carve_y_start + 1) as i32;
        let cave_volume = CHUNK_WIDTH * y_count as usize * CHUNK_WIDTH;

        let mut cave_vals = vec![0.0f32; cave_volume];
        let mut spaghetti_vals = vec![0.0f32; cave_volume];
        let mut noodle_vals = vec![0.0f32; cave_volume];

        self.noise.cave_grid(
            &mut cave_vals,
            wx as f32,
            carve_y_start as f32,
            wz as f32,
            CHUNK_WIDTH as i32,
            y_count,
            CHUNK_WIDTH as i32,
        );
        self.noise.spaghetti_grid(
            &mut spaghetti_vals,
            wx as f32,
            carve_y_start as f32,
            wz as f32,
            CHUNK_WIDTH as i32,
            y_count,
            CHUNK_WIDTH as i32,
        );
        self.noise.noodle_grid(
            &mut noodle_vals,
            wx as f32,
            carve_y_start as f32,
            wz as f32,
            CHUNK_WIDTH as i32,
            y_count,
            CHUNK_WIDTH as i32,
        );

        let sea_level = SEA_LEVEL;
        let resolved_biomes = self.biomes.resolve_all();

        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let biome_id = biome_ids[col];
                let biome = &resolved_biomes[biome_id as usize];

                if biome.cave_density <= 0.0 {
                    continue;
                }

                let top = chunk.heightmap_top[col] as usize;
                if top < 2 {
                    continue;
                }

                let col_y_max = top.min((sea_level - 1) as usize).min(carve_y_end);

                for y in carve_y_start..=col_y_max {
                    let idx = Chunk::index(x, y, z);
                    if chunk.blocks[idx] == BlockId::AIR.0 {
                        continue;
                    }

                    let noise_idx =
                        x + (y - carve_y_start) * CHUNK_WIDTH + z * CHUNK_WIDTH * y_count as usize;
                    if noise_idx >= cave_vals.len() {
                        continue;
                    }

                    let cheese_val = cave_vals[noise_idx];
                    let spaghetti_val = spaghetti_vals[noise_idx];
                    let noodle_val = if noise_idx < noodle_vals.len() {
                        noodle_vals[noise_idx]
                    } else {
                        0.0
                    };

                    let density = crate::cave::cave_system_density(
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

        // Recompute heightmaps after carving
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let mut top = 0u16;
                for y in (1..CHUNK_HEIGHT).rev() {
                    let idx = Chunk::index(x, y, z);
                    if chunk.blocks[idx] != BlockId::AIR.0 {
                        top = y as u16;
                        break;
                    }
                }
                chunk.heightmap_top[col] = top;

                let mut bottom = 0u16;
                for y in 1..CHUNK_HEIGHT {
                    let idx = Chunk::index(x, y, z);
                    if chunk.blocks[idx] != BlockId::AIR.0 {
                        bottom = y as u16;
                        break;
                    }
                }
                chunk.heightmap_bottom[col] = bottom;
            }
        }
    }
}

// ── Spline functions ─────────────────────────────────────────────────

#[inline(always)]
fn base_height_from_continental(continental: f32) -> f32 {
    lerp_spline(continental, &CONTINENTAL_SPLINE)
}

#[inline(always)]
fn erosion_offset(erosion: f32) -> f32 {
    lerp_spline(erosion, &EROSION_SPLINE)
}

#[inline(always)]
fn weirdness_offset(weirdness: f32) -> f32 {
    lerp_spline(weirdness, &WEIRDNESS_SPLINE)
}

#[inline(always)]
fn lerp_spline(value: f32, points: &[(f32, f32)]) -> f32 {
    debug_assert!(!points.is_empty());
    if value <= points[0].0 {
        return points[0].1;
    }
    if value >= points[points.len() - 1].0 {
        return points[points.len() - 1].1;
    }
    for i in 0..points.len() - 1 {
        let (x0, y0) = points[i];
        let (x1, y1) = points[i + 1];
        if value >= x0 && value <= x1 {
            let t = if (x1 - x0).abs() < f32::EPSILON {
                0.0
            } else {
                (value - x0) / (x1 - x0)
            };
            return y0 + t * (y1 - y0);
        }
    }
    points[points.len() - 1].1
}
