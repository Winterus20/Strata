use std::collections::HashMap;

use crate::config::{NOISE_CACHE_MAX_REGIONS, NOISE_CACHE_REGION_SIZE};

/// A key identifying a cached noise region.
///
/// Regions are aligned to `NOISE_CACHE_REGION_SIZE`-chunk boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoiseRegionKey {
    pub rx: i32,
    pub ry: i32,
    pub rz: i32,
}

/// Cached noise data for a 3×3 chunk region.
///
/// The cache stores pre-computed noise values so that overlapping
/// regions between adjacent chunks don't require redundant noise
/// generation. The region spans `(NOISE_CACHE_REGION_SIZE * 16)` blocks
/// in X and Z, and the full chunk height in Y.
pub struct CachedRegion {
    pub key: NoiseRegionKey,
    pub data: Vec<f32>,
}

/// LRU-evicted cache for 3×3 chunk noise regions.
///
/// Reduces noise calls by ~3× when generating adjacent chunks,
/// since overlapping border regions are reused.
pub struct NoiseCache {
    regions: HashMap<NoiseRegionKey, CachedRegion>,
    access_order: Vec<NoiseRegionKey>,
    max_regions: usize,
    region_size_blocks: i32,
}

impl NoiseCache {
    pub fn new() -> Self {
        Self {
            regions: HashMap::new(),
            access_order: Vec::new(),
            max_regions: NOISE_CACHE_MAX_REGIONS,
            region_size_blocks: NOISE_CACHE_REGION_SIZE * 16,
        }
    }

    /// Compute the region key for a chunk position.
    #[inline]
    pub fn region_for_chunk(&self, cx: i32, cz: i32) -> NoiseRegionKey {
        let region_blocks = self.region_size_blocks;
        let rx = (cx * 16).div_euclid(region_blocks);
        let rz = (cz * 16).div_euclid(region_blocks);
        NoiseRegionKey { rx, ry: 0, rz }
    }

    /// Get the world-space origin of a cached region.
    #[inline]
    pub fn region_origin(&self, key: &NoiseRegionKey) -> (i32, i32) {
        let region_blocks = self.region_size_blocks;
        (key.rx * region_blocks, key.rz * region_blocks)
    }

    /// Compute the local offset within a cached region for a given chunk.
    #[inline]
    pub fn chunk_offset_in_region(&self, cx: i32, cz: i32, key: &NoiseRegionKey) -> (usize, usize) {
        let (rx0, rz0) = self.region_origin(key);
        let region_blocks = self.region_size_blocks as usize;
        let lx = ((cx * 16) - rx0) as usize;
        let lz = ((cz * 16) - rz0) as usize;
        (
            lx.clamp(0, region_blocks - 1),
            lz.clamp(0, region_blocks - 1),
        )
    }

    /// Returns the size of a cached region in blocks.
    #[inline]
    pub fn region_block_size(&self) -> i32 {
        self.region_size_blocks
    }

    /// Look up a region by key. Returns `None` if not cached.
    pub fn get(&mut self, key: &NoiseRegionKey) -> Option<&CachedRegion> {
        if self.regions.contains_key(key) {
            // Move to front (LRU promotion)
            if let Some(pos) = self.access_order.iter().position(|k| k == key) {
                self.access_order.remove(pos);
            }
            self.access_order.push(*key);
            self.regions.get(key)
        } else {
            None
        }
    }

    /// Insert a region into the cache, evicting LRU if necessary.
    pub fn insert(&mut self, key: NoiseRegionKey, data: Vec<f32>) {
        if self.regions.len() >= self.max_regions {
            // Evict LRU
            if let Some(lru_key) = self.access_order.first().copied() {
                self.regions.remove(&lru_key);
                self.access_order.remove(0);
            }
        }
        self.access_order.push(key);
        self.regions.insert(key, CachedRegion { key, data });
    }

    /// Check if a region is cached without promoting it.
    pub fn contains(&self, key: &NoiseRegionKey) -> bool {
        self.regions.contains_key(key)
    }

    /// Clear all cached regions.
    pub fn clear(&mut self) {
        self.regions.clear();
        self.access_order.clear();
    }

    /// Current number of cached regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

impl Default for NoiseCache {
    fn default() -> Self {
        Self::new()
    }
}
