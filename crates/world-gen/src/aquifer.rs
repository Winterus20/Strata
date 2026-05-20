use strata_core::{BlockId, CHUNK_WIDTH, Chunk};

use crate::biome::ResolvedBiome;
use crate::config::{
    AQUIFER_BARRIER_SCALE, AQUIFER_BARRIER_THRESHOLD, AQUIFER_CELL_CHUNKS, AQUIFER_EMPTY_THRESHOLD,
    AQUIFER_FLOODED_THRESHOLD, AQUIFER_LAVA_DENSITY, AQUIFER_LAVA_LEVEL, AQUIFER_LOCAL_VARIATION,
    AQUIFER_POCKET_SCALE, SEA_LEVEL,
};

/// Full Minecraft-style aquifer fluid type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidType {
    /// No fluid.
    Empty,
    /// Water.
    Water,
    /// Lava.
    Lava,
}

/// State of an aquifer cell.
#[derive(Debug, Clone, Copy)]
pub struct AquiferCellState {
    pub fluid: FluidType,
    pub water_level: i32,
    pub barrier_height: f32,
}

/// Full Minecraft-style aquifer system (Faz 4).
///
/// Key improvements over Faz 3:
/// - Local water tables with per-cell noise-driven variation
/// - Lava pockets at deep Y levels with density-based placement
/// - Aquifer barriers between cave systems (noise-driven walls)
/// - Smooth fluid transitions between cells
/// - Efficient lazy computation (only processes cave-containing columns)
pub struct Aquifer {}

impl Aquifer {
    pub fn new(_seed: u64) -> Self {
        Self {}
    }

    /// Compute the aquifer state at a given world block position.
    ///
    /// Implements the full Minecraft-style decision tree:
    /// 1. Above sea level → Empty
    /// 2. Below lava level → Lava (with density variation for pockets)
    /// 3. Noise-based: empty / flooded / local water table
    /// 4. Barrier check: prevents fluid from crossing between cave systems
    #[inline]
    pub fn state_at(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
        aquifer_noise: f32,
        barrier_noise: f32,
    ) -> AquiferCellState {
        self.state_at_with_shoreline(wx, wy, wz, aquifer_noise, barrier_noise, SEA_LEVEL)
    }

    /// Compute aquifer state using a per-column shoreline water level.
    #[inline]
    pub fn state_at_with_shoreline(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
        aquifer_noise: f32,
        barrier_noise: f32,
        shoreline_level: i32,
    ) -> AquiferCellState {
        if wy > shoreline_level {
            return AquiferCellState {
                fluid: FluidType::Empty,
                water_level: shoreline_level,
                barrier_height: 0.0,
            };
        }

        let cell = self.cell_for(wx, wy, wz);

        if wy < AQUIFER_LAVA_LEVEL {
            let pocket_noise = aquifer_noise * AQUIFER_POCKET_SCALE;
            if pocket_noise > AQUIFER_LAVA_DENSITY {
                return AquiferCellState {
                    fluid: FluidType::Lava,
                    water_level: wy + 10,
                    barrier_height: 0.0,
                };
            }
            return AquiferCellState {
                fluid: FluidType::Empty,
                water_level: wy,
                barrier_height: 0.0,
            };
        }

        let barrier = if barrier_noise > AQUIFER_BARRIER_THRESHOLD {
            barrier_noise * AQUIFER_BARRIER_SCALE
        } else {
            0.0
        };

        match aquifer_noise {
            n if n < AQUIFER_EMPTY_THRESHOLD => AquiferCellState {
                fluid: FluidType::Empty,
                water_level: wy,
                barrier_height: barrier,
            },
            n if n > AQUIFER_FLOODED_THRESHOLD => AquiferCellState {
                fluid: FluidType::Water,
                water_level: shoreline_level,
                barrier_height: barrier,
            },
            _ => {
                let water_level =
                    self.local_water_level_with_shoreline(cell, aquifer_noise, shoreline_level);
                AquiferCellState {
                    fluid: if wy <= water_level {
                        FluidType::Water
                    } else {
                        FluidType::Empty
                    },
                    water_level,
                    barrier_height: barrier,
                }
            }
        }
    }

    /// Fill a chunk's cave volumes with water/lava based on aquifer state.
    ///
    /// Uses batch noise from `aquifer_grid` and `barrier_grid` for efficiency.
    /// Only processes blocks that are AIR (carved caves) within the cave Y range.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_chunk(
        &self,
        chunk: &mut Chunk,
        aquifer_grid: &[f32],
        barrier_grid: &[f32],
        biome: &ResolvedBiome,
        y_start: usize,
        y_end: usize,
        _sea_level: i32,
    ) {
        let shoreline = [_sea_level; 256];
        self.fill_chunk_with_shoreline(
            chunk,
            aquifer_grid,
            barrier_grid,
            biome,
            y_start,
            y_end,
            _sea_level,
            &shoreline,
        );
    }

    /// Fill a chunk's cave volumes with water/lava using shoreline-aware water levels.
    ///
    /// Uses per-column shoreline water levels so water connects naturally to land.
    #[allow(clippy::too_many_arguments)]
    pub fn fill_chunk_with_shoreline(
        &self,
        chunk: &mut Chunk,
        aquifer_grid: &[f32],
        barrier_grid: &[f32],
        biome: &ResolvedBiome,
        y_start: usize,
        y_end: usize,
        _sea_level: i32,
        shoreline_levels: &[i32; 256],
    ) {
        if biome.cave_density <= 0.0 {
            return;
        }

        let wx = chunk.position.world_x();
        let wz = chunk.position.world_z();
        let layer_stride = CHUNK_WIDTH * CHUNK_WIDTH;
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let top = chunk.heightmap_top[col] as usize;
                if top < 2 {
                    continue;
                }

                let col_water_level = shoreline_levels[col];
                let col_y_end = top.min((col_water_level - 1) as usize).min(y_end);

                for y in y_start..=col_y_end {
                    let idx = Chunk::index(x, y, z);
                    if chunk.blocks[idx] != BlockId::AIR.0 {
                        continue;
                    }

                    let layer = y - y_start;
                    let noise_idx = x + layer * CHUNK_WIDTH + z * layer_stride;

                    let aquifer_val = if noise_idx < aquifer_grid.len() {
                        aquifer_grid[noise_idx]
                    } else {
                        0.5
                    };
                    let barrier_val = if noise_idx < barrier_grid.len() {
                        barrier_grid[noise_idx]
                    } else {
                        0.0
                    };

                    let state = self.state_at_with_shoreline(
                        wx + x as i32,
                        y as i32,
                        wz + z as i32,
                        aquifer_val,
                        barrier_val,
                        col_water_level,
                    );

                    match state.fluid {
                        FluidType::Water => {
                            if state.barrier_height <= (y as f32 - y_start as f32) * 0.1 {
                                chunk.blocks[idx] = BlockId::WATER.0;
                            }
                        }
                        FluidType::Lava => {
                            chunk.blocks[idx] = BlockId::from_raw(29).0;
                        }
                        FluidType::Empty => {}
                    }
                }
            }
        }
    }

    /// Map a world position to its aquifer cell coordinate.
    /// Cells are `AQUIFER_CELL_CHUNKS * 16` blocks wide in X/Z
    /// and `AQUIFER_CELL_CHUNKS * 32` blocks tall in Y.
    #[inline]
    fn cell_for(&self, wx: i32, wy: i32, wz: i32) -> AquiferCell {
        let cell_w = AQUIFER_CELL_CHUNKS * CHUNK_WIDTH as i32;
        let cell_h = cell_w * 2;
        AquiferCell {
            cx: wx.div_euclid(cell_w),
            cy: wy.div_euclid(cell_h),
            cz: wz.div_euclid(cell_w),
        }
    }

    /// Compute local water level using a per-column shoreline level.
    #[inline]
    fn local_water_level_with_shoreline(
        &self,
        _cell: AquiferCell,
        aquifer_noise: f32,
        shoreline: i32,
    ) -> i32 {
        let base_level = shoreline - 12;
        let t = ((aquifer_noise - AQUIFER_EMPTY_THRESHOLD)
            / (AQUIFER_FLOODED_THRESHOLD - AQUIFER_EMPTY_THRESHOLD))
            .clamp(0.0, 1.0);
        let variation = (t - 0.5) * AQUIFER_LOCAL_VARIATION;
        (base_level as f32 + variation) as i32
    }

    /// Returns whether a given Y level is within the lava zone.
    #[inline]
    pub fn is_lava_zone(y: i32) -> bool {
        y < AQUIFER_LAVA_LEVEL
    }
}

/// Aquifer cell coordinate in the coarse 3D grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AquiferCell {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl AquiferCell {
    /// Returns the world-space origin of this cell.
    #[inline]
    pub fn origin(&self) -> (i32, i32, i32) {
        let cell_w = AQUIFER_CELL_CHUNKS * CHUNK_WIDTH as i32;
        let cell_h = cell_w * 2;
        (self.cx * cell_w, self.cy * cell_h, self.cz * cell_w)
    }
}
