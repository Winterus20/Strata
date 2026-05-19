use crate::noise::TerrainNoise;
use strata_core::{BlockId, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

/// Procedural terrain generator using noise-based heightmap.
pub struct TerrainGenerator {
    noise: TerrainNoise,
}

impl TerrainGenerator {
    /// Creates a new terrain generator with the given world seed.
    pub fn new(seed: u32) -> Self {
        Self {
            noise: TerrainNoise::new(seed),
        }
    }

    /// Returns the terrain height at the given world-space `(x, z)` coordinate.
    pub fn height_at(&self, x: i32, z: i32) -> f32 {
        self.noise.height(x, z)
    }

    /// Fills the given chunk with terrain blocks based on noise.
    pub fn generate(&self, chunk: &mut Chunk) {
        let world_x = chunk.position.world_x();
        let world_z = chunk.position.world_z();

        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let wx = world_x + x as i32;
                let wz = world_z + z as i32;
                let height = self.noise.height(wx, wz) as usize;
                let stone_top = height.saturating_sub(4);

                for y in 0..CHUNK_HEIGHT {
                    let block = if y == 0 {
                        BlockId::BEDROCK
                    } else if y < stone_top {
                        BlockId::STONE
                    } else if y < height {
                        BlockId::DIRT
                    } else if y == height {
                        BlockId::GRASS
                    } else {
                        BlockId::AIR
                    };
                    chunk.set_block(x, y, z, block);
                }
            }
        }
        chunk.dirty = false;
    }
}
