use strata_core::{BlockId, CHUNK_DEPTH, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

/// Places trees on the surface of a generated chunk using seed-based RNG.
pub struct TreePlacer {
    seed: u32,
}

impl TreePlacer {
    pub fn new(seed: u32) -> Self {
        Self { seed }
    }

    /// Places trees on the given chunk. Must be called after terrain generation.
    pub fn place_trees(&self, chunk: &mut Chunk) {
        let cx = chunk.position.0.x;
        let cz = chunk.position.0.y;

        // Keep 2-block margin from chunk edges so leaves don't clip at boundaries
        for lx in 2..CHUNK_WIDTH - 2 {
            for lz in 2..CHUNK_DEPTH - 2 {
                let wy = self.surface_height(chunk, lx, lz);
                if wy == 0 {
                    continue;
                }

                let wx = chunk.position.world_x() + lx as i32;
                let wz = chunk.position.world_z() + lz as i32;

                if self.should_place_tree(cx, cz, lx, lz, wx, wz) {
                    self.make_oak(chunk, lx, wy, lz);
                }
            }
        }
    }

    /// Returns the Y of the topmost solid (grass/dirt) block at the given column, or 0 if none.
    fn surface_height(&self, chunk: &Chunk, lx: usize, lz: usize) -> usize {
        for y in (1..CHUNK_HEIGHT).rev() {
            let block = chunk.get_block(lx, y, lz);
            if block == BlockId::GRASS || block == BlockId::DIRT {
                return y;
            }
        }
        0
    }

    /// Deterministic RNG from a seed + coordinates, returns 0..1 as a bool with given probability.
    fn should_place_tree(&self, cx: i32, cz: i32, lx: usize, lz: usize, _wx: i32, _wz: i32) -> bool {
        let hash = self.hash(self.seed as i64, cx as i64, cz as i64, lx as i64, lz as i64);
        // ~5% tree density
        hash.rem_euclid(100) == 0
    }

    /// Simple PCG-style hash for deterministic placement.
    fn hash(&self, seed: i64, cx: i64, cz: i64, lx: i64, lz: i64) -> i64 {
        let mut h = seed;
        h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h ^= cx;
        h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h ^= cz;
        h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h ^= lx;
        h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h ^= lz;
        h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        h
    }

    /// Places a simple oak tree: 5-tall trunk + leaf blob.
    fn make_oak(&self, chunk: &mut Chunk, lx: usize, ground_y: usize, lz: usize) {
        let trunk_height = 5usize;

        // Need room for trunk + leaves above
        if ground_y + trunk_height + 2 >= CHUNK_HEIGHT {
            return;
        }

        // Trunk
        for dy in 1..=trunk_height {
            chunk.set_block(lx, ground_y + dy, lz, BlockId::WOOD);
        }

        let leaf_start = ground_y + trunk_height - 1;

        // Leaf blob: 5x5 horizontal, 3 layers tall, centered on trunk
        for dy in 0..3 {
            let radius: i32 = if dy == 1 { 2 } else { 1 };
            let y = leaf_start + dy;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let bx = lx as i32 + dx;
                    let bz = lz as i32 + dz;

                    // Skip trunk position itself (keep wood) and corners for natural look
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
}
