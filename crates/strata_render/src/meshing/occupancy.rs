//! Heap-free per-sector occupancy bitmask for the meshing hot path (plan 06 / 39).
//!
//! One bit per voxel of the 32³ sector (32_768 bits == `[u64; 512]`). Built once
//! per `mesh_sector` call from the live `XBrickMap`, it gives O(1) branchless
//! solid/air lookups for the greedy merge and AO passes without touching the
//! `GlobalBrickPool` on the inner loop. Out-of-range (neighbor sector) voxels are
//! sampled separately from the neighbor `XBrickMap`s.

use strata_core::prelude::*;

/// Number of voxels per sector axis.
pub const SECTOR_DIM: u32 = 32;
/// Bitmask words covering the 32³ sector (32_768 bits).
const WORDS: usize = (SECTOR_DIM * SECTOR_DIM * SECTOR_DIM / 64) as usize; // 512

/// Stack-allocated solid/air bitmask over the 32³ sector.
///
/// No heap allocation occurs in `mesh_sector` beyond building this struct; it is
/// created once and cleared/reused by the caller's scratch if desired.
#[derive(Clone)]
pub struct OccupancyScratch {
    mask: [u64; WORDS],
}

impl Default for OccupancyScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl OccupancyScratch {
    #[inline]
    pub fn new() -> Self {
        OccupancyScratch {
            mask: [0u64; WORDS],
        }
    }

    /// Flatten a local voxel coordinate to its bit index (range 0..32768).
    ///
    /// Layout `x*1024 + z*32 + y` keeps each axis in `[0,32)` and packs tightly.
    /// NOTE: This is **X-major** order, different from [`snap_index`] in greedy.rs
    /// which uses Z-major (`x + y*32 + z*1024`). Both are self-consistent.
    #[inline]
    pub fn bit_index(x: u32, y: u32, z: u32) -> usize {
        ((x << 10) | (z << 5) | y) as usize
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, z: u32) {
        let i = Self::bit_index(x, y, z);
        self.mask[i >> 6] |= 1u64 << (i & 63);
    }

    #[inline]
    pub fn clear_bit(&mut self, x: u32, y: u32, z: u32) {
        let i = Self::bit_index(x, y, z);
        self.mask[i >> 6] &= !(1u64 << (i & 63));
    }

    /// True if the voxel at `(x,y,z)` is occupied (solid or transparent block).
    #[inline]
    pub fn is_occupied(&self, x: u32, y: u32, z: u32) -> bool {
        let i = Self::bit_index(x, y, z);
        (self.mask[i >> 6] >> (i & 63)) & 1 != 0
    }

    pub fn clear(&mut self) {
        self.mask.fill(0);
    }
}

/// Populate `scratch` from a sector's `GlobalBrickPool` occupancy (no block ids).
#[inline]
pub fn fill_scratch(scratch: &mut OccupancyScratch, sector: &XBrickMap, pool: &GlobalBrickPool) {
    scratch.clear();
    for x in 0..SECTOR_DIM {
        for y in 0..SECTOR_DIM {
            for z in 0..SECTOR_DIM {
                if sector.is_occupied(pool, VoxelCoord::new(x, y, z)) {
                    scratch.set(x, y, z);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_index_unique_and_in_range() {
        let mut seen = [0u64; WORDS];
        for x in 0..SECTOR_DIM {
            for y in 0..SECTOR_DIM {
                for z in 0..SECTOR_DIM {
                    let i = OccupancyScratch::bit_index(x, y, z);
                    assert!(i < WORDS * 64, "bit index {i} out of range");
                    assert_eq!(
                        seen[i >> 6] & (1u64 << (i & 63)),
                        0,
                        "collision at {x},{y},{z}"
                    );
                    seen[i >> 6] |= 1u64 << (i & 63);
                }
            }
        }
    }

    #[test]
    fn set_and_query_round_trip() {
        let mut s = OccupancyScratch::new();
        assert!(!s.is_occupied(5, 6, 7));
        s.set(5, 6, 7);
        assert!(s.is_occupied(5, 6, 7));
        s.clear_bit(5, 6, 7);
        assert!(!s.is_occupied(5, 6, 7));
    }
}
