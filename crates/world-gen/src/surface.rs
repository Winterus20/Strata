use strata_core::{BlockId, CHUNK_WIDTH, Chunk};

use crate::biome::ResolvedBiome;
use crate::config::SEA_LEVEL;

/// Surface rule condition.
#[derive(Debug, Clone)]
pub enum SurfaceCondition {
    Biome(u16),
    AboveSeaLevel,
    BelowSeaLevel,
    Steepness(f32),
    And(Box<SurfaceCondition>, Box<SurfaceCondition>),
    Or(Box<SurfaceCondition>, Box<SurfaceCondition>),
    Not(Box<SurfaceCondition>),
    Always,
}

/// A surface rule that determines what block to place at a given position.
#[derive(Debug, Clone)]
pub enum SurfaceRule {
    /// Fixed block for a specific condition.
    Block {
        condition: SurfaceCondition,
        block: BlockId,
    },
    /// Height-map based block (min_y, max_y).
    HeightMap {
        min_y: i32,
        max_y: i32,
        block: BlockId,
    },
    /// Conditional branch.
    If {
        condition: SurfaceCondition,
        then: Box<SurfaceRule>,
        r#else: Box<SurfaceRule>,
    },
    /// First matching rule in sequence.
    Sequence(Vec<SurfaceRule>),
    /// Fill entire column from sea level to bottom with block.
    SeaFill { above: BlockId, below: BlockId },
    /// Always matches (passthrough).
    Always,
}

impl SurfaceCondition {
    fn evaluate(&self, chunk: &Chunk, x: usize, y: usize, z: usize, biome: &ResolvedBiome) -> bool {
        match self {
            SurfaceCondition::Biome(id) => biome.id == *id,
            SurfaceCondition::AboveSeaLevel => y >= SEA_LEVEL as usize,
            SurfaceCondition::BelowSeaLevel => y < SEA_LEVEL as usize,
            SurfaceCondition::Steepness(threshold) => {
                // Estimate local steepness from heightmap differences
                let col = Chunk::column_index(x, z);
                let h = chunk.heightmap_top[col] as i32;
                let mut max_grad = 0i32;
                for dx in [-1i32, 1i32] {
                    for dz in [-1i32, 1i32] {
                        let nx = x as i32 + dx;
                        let nz = z as i32 + dz;
                        if nx >= 0 && nx < CHUNK_WIDTH as i32 && nz >= 0 && nz < CHUNK_WIDTH as i32
                        {
                            let ncol = Chunk::column_index(nx as usize, nz as usize);
                            let nh = chunk.heightmap_top[ncol] as i32;
                            let grad = (h - nh).abs();
                            if grad > max_grad {
                                max_grad = grad;
                            }
                        }
                    }
                }
                (max_grad as f32) >= *threshold
            }
            SurfaceCondition::And(a, b) => {
                a.evaluate(chunk, x, y, z, biome) && b.evaluate(chunk, x, y, z, biome)
            }
            SurfaceCondition::Or(a, b) => {
                a.evaluate(chunk, x, y, z, biome) || b.evaluate(chunk, x, y, z, biome)
            }
            SurfaceCondition::Not(a) => !a.evaluate(chunk, x, y, z, biome),
            SurfaceCondition::Always => true,
        }
    }
}

impl SurfaceRule {
    /// Evaluate this rule at (x, y, z) and return the block to place,
    /// or None if the rule doesn't apply.
    pub fn evaluate(
        &self,
        chunk: &Chunk,
        x: usize,
        y: usize,
        z: usize,
        biome: &ResolvedBiome,
    ) -> Option<BlockId> {
        match self {
            SurfaceRule::Block { condition, block } => {
                if condition.evaluate(chunk, x, y, z, biome) {
                    Some(*block)
                } else {
                    None
                }
            }
            SurfaceRule::HeightMap {
                min_y,
                max_y,
                block,
            } => {
                if (y as i32) >= *min_y && (y as i32) <= *max_y {
                    Some(*block)
                } else {
                    None
                }
            }
            SurfaceRule::If {
                condition,
                then,
                r#else,
            } => {
                if condition.evaluate(chunk, x, y, z, biome) {
                    then.evaluate(chunk, x, y, z, biome)
                } else {
                    r#else.evaluate(chunk, x, y, z, biome)
                }
            }
            SurfaceRule::Sequence(rules) => {
                for rule in rules {
                    if let Some(block) = rule.evaluate(chunk, x, y, z, biome) {
                        return Some(block);
                    }
                }
                None
            }
            SurfaceRule::SeaFill { above, below } => {
                if y >= SEA_LEVEL as usize {
                    Some(*above)
                } else {
                    Some(*below)
                }
            }
            SurfaceRule::Always => None,
        }
    }
}

/// Build the default surface rule sequence for Faz 2.
pub fn default_surface_rules() -> SurfaceRule {
    SurfaceRule::Sequence(vec![
        // Ocean biomes: fill with water below sea level
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(0), // deep_ocean
            block: BlockId::GRAVEL,
        },
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(1), // ocean
            block: BlockId::SAND,
        },
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(2), // warm_ocean
            block: BlockId::SAND,
        },
        // Beach: sand down to sea level
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(3), // beach
            block: BlockId::SAND,
        },
        // Snowy beach
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(4), // snowy_beach
            block: BlockId::SNOW,
        },
        // Desert: sand
        SurfaceRule::Block {
            condition: SurfaceCondition::Biome(10), // desert
            block: BlockId::SAND,
        },
        // Swamp: water fill
        SurfaceRule::If {
            condition: SurfaceCondition::Biome(18), // swamp
            then: Box::new(SurfaceRule::SeaFill {
                above: BlockId::GRASS,
                below: BlockId::WATER,
            }),
            r#else: Box::new(SurfaceRule::Always),
        },
        // Default: pass through to biome-based handling
        SurfaceRule::Always,
    ])
}

/// Apply the surface rules engine to a chunk.
pub fn apply_surface_rules(
    chunk: &mut Chunk,
    biome: &ResolvedBiome,
    rules: &SurfaceRule,
    sea_level: i32,
) {
    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_WIDTH {
            let col = Chunk::column_index(x, z);
            let top = chunk.heightmap_top[col] as usize;
            if top == 0 {
                continue;
            }

            let stone_depth = (top as i32 - biome.filler_depth as i32).max(0) as usize;

            for y in 0..=top {
                let idx = Chunk::index(x, y, z);

                let block = if y == 0 {
                    BlockId::BEDROCK
                } else if y > top {
                    BlockId::AIR
                } else if y < stone_depth {
                    BlockId::STONE
                } else if y == top {
                    // Surface block: check rule engine first, fall back to biome top
                    rules
                        .evaluate(chunk, x, y, z, biome)
                        .unwrap_or(biome.top_block)
                } else {
                    // Sub-surface filler
                    biome.filler_block
                };

                chunk.blocks[idx] = block.0;
            }

            // Ocean/water fill below sea level
            if let Some(ocean) = biome.ocean_block
                && top < sea_level as usize
            {
                for y in (top + 1)..=sea_level as usize {
                    let idx = Chunk::index(x, y, z);
                    if chunk.blocks[idx] == BlockId::AIR.0 {
                        chunk.blocks[idx] = ocean.0;
                    }
                }
            }
        }
    }
}

/// Legacy function kept for API compatibility.
pub fn apply_surface(chunk: &mut Chunk, biome: &ResolvedBiome, sea_level: i32) {
    let rules = default_surface_rules();
    apply_surface_rules(chunk, biome, &rules, sea_level);
}

/// Apply biome surface blocks to a single column.
pub fn apply_surface_column(
    chunk: &mut Chunk,
    x: usize,
    z: usize,
    biome: &ResolvedBiome,
    sea_level: i32,
) {
    let col = Chunk::column_index(x, z);
    let top_y = chunk.heightmap_top[col] as usize;
    if top_y == 0 {
        return;
    }

    let depth = biome.filler_depth as usize;
    let filler_start = top_y.saturating_sub(depth);

    for y in filler_start..top_y {
        let block = chunk.get_block(x, y, z);
        if !block.is_air() {
            chunk.set_block(x, y, z, biome.filler_block);
        }
    }
    if !biome.top_block.is_air() {
        chunk.set_block(x, top_y, z, biome.top_block);
    }

    if let Some(ocean) = biome.ocean_block
        && top_y < sea_level as usize
    {
        for y in (top_y + 1)..sea_level as usize {
            let b = chunk.get_block(x, y, z);
            if b.is_air() {
                chunk.set_block(x, y, z, ocean);
            }
        }
    }
}
