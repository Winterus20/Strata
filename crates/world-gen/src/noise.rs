use fastnoise2::generator::prelude::*;

/// Noise generator for terrain height and biome selection.
pub struct TerrainNoise {
    height_node: fastnoise2::SafeNode,
    biome_node: fastnoise2::SafeNode,
    seed: i32,
}

impl TerrainNoise {
    /// Creates a new noise generator with the given seed.
    pub fn new(seed: u32) -> Self {
        let height_node = supersimplex().fbm(0.5, 0.0, 4, 2.0).build().0;
        let biome_node = value().fbm(0.5, 0.0, 3, 2.0).build().0;

        Self {
            height_node,
            biome_node,
            seed: seed as i32,
        }
    }

    /// Returns the terrain height at the given world-space `(x, z)` coordinate.
    pub fn height(&self, x: i32, z: i32) -> f32 {
        let nx = x as f32 * 0.01;
        let nz = z as f32 * 0.01;
        let noise = self.height_node.gen_single_2d(nx, nz, self.seed);
        (noise + 1.0) * 80.0 + 20.0
    }

    /// Returns the biome index (0–3) at the given world-space `(x, z)` coordinate.
    pub fn biome(&self, x: i32, z: i32) -> u8 {
        let nx = x as f32 * 0.005;
        let nz = z as f32 * 0.005;
        let noise = self.biome_node.gen_single_2d(nx, nz, self.seed);
        if noise < -0.3 {
            0
        } else if noise < 0.0 {
            1
        } else if noise < 0.3 {
            2
        } else {
            3
        }
    }
}
