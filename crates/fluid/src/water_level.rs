use strata_core::{BlockId, Chunk, CHUNK_DEPTH, CHUNK_HEIGHT, CHUNK_VOLUME, CHUNK_WIDTH};

/// Water level at a single block position (0–15).
///
/// `0` = no water, `15` = full source block.
/// Levels 1–14 represent flowing water with decreasing height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WaterLevel(pub u8);

impl WaterLevel {
    /// No water.
    pub const EMPTY: Self = Self(0);
    /// Full source block (infinite water).
    pub const SOURCE: Self = Self(15);
    /// Maximum water level value.
    pub const MAX: u8 = 15;

    /// Returns `true` if this position has no water.
    #[inline(always)]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` if this is a source block (level 15).
    #[inline(always)]
    pub fn is_source(self) -> bool {
        self.0 == Self::MAX
    }

    /// Returns the water level as a u8 (0–15).
    #[inline(always)]
    pub fn level(self) -> u8 {
        self.0
    }

    /// Creates a `WaterLevel` from a raw u8 value, clamped to 0–15.
    #[inline(always)]
    pub fn from_raw(level: u8) -> Self {
        Self(level.min(Self::MAX))
    }
}

/// Per-chunk water level data.
///
/// Stored as a flat `Vec<u8>` parallel to the chunk's block array.
/// Index formula matches `Chunk::index`: `x + z * 16 + y * 256`.
///
/// Memory: 65536 bytes per chunk (~64 KB).
#[derive(Clone)]
pub struct ChunkWaterLevels {
    /// Flat array of water levels, one per block position.
    pub levels: Vec<u8>,
    /// Whether any water level changed since last tick.
    pub dirty: bool,
    /// Number of non-zero water entries (for early-exit optimization).
    pub water_count: usize,
}

impl ChunkWaterLevels {
    /// Creates an empty water level array (all zeros).
    pub fn new() -> Self {
        Self {
            levels: vec![0u8; CHUNK_VOLUME],
            dirty: false,
            water_count: 0,
        }
    }

    /// Initializes water levels from existing water blocks in a chunk.
    ///
    /// This should be called after world generation or chunk loading
    /// to set up the initial water state.
    pub fn init_from_chunk(chunk: &Chunk) -> Self {
        let mut water = Self::new();
        for idx in 0..CHUNK_VOLUME {
            if chunk.blocks[idx] == BlockId::WATER.0 {
                water.levels[idx] = WaterLevel::MAX;
                water.water_count += 1;
            }
        }
        water
    }

    /// Returns the water level at local coordinates.
    #[inline(always)]
    pub fn get(&self, x: usize, y: usize, z: usize) -> WaterLevel {
        debug_assert!(x < CHUNK_WIDTH && y < CHUNK_HEIGHT && z < CHUNK_DEPTH);
        let idx = x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_DEPTH;
        WaterLevel(self.levels[idx])
    }

    /// Sets the water level at local coordinates.
    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, z: usize, level: WaterLevel) {
        debug_assert!(x < CHUNK_WIDTH && y < CHUNK_HEIGHT && z < CHUNK_DEPTH);
        let idx = x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_DEPTH;
        let old = self.levels[idx];
        self.levels[idx] = level.0;
        if old == 0 && level.0 > 0 {
            self.water_count += 1;
        } else if old > 0 && level.0 == 0 {
            self.water_count -= 1;
        }
        if old != level.0 {
            self.dirty = true;
        }
    }

    /// Sets the water level at a flat index.
    #[inline(always)]
    pub fn set_at(&mut self, idx: usize, level: WaterLevel) {
        debug_assert!(idx < CHUNK_VOLUME);
        let old = self.levels[idx];
        self.levels[idx] = level.0;
        if old == 0 && level.0 > 0 {
            self.water_count += 1;
        } else if old > 0 && level.0 == 0 {
            self.water_count -= 1;
        }
        if old != level.0 {
            self.dirty = true;
        }
    }

    /// Returns the water level at a flat index.
    #[inline(always)]
    pub fn get_at(&self, idx: usize) -> WaterLevel {
        debug_assert!(idx < CHUNK_VOLUME);
        WaterLevel(self.levels[idx])
    }

    /// Returns `true` if this chunk has any water.
    #[inline(always)]
    pub fn has_water(&self) -> bool {
        self.water_count > 0
    }

    /// Clears the dirty flag. Call after processing changes.
    #[inline(always)]
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Returns the flat array as a mutable slice.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.levels
    }

    /// Returns the flat array as a shared slice.
    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        &self.levels
    }
}

impl Default for ChunkWaterLevels {
    fn default() -> Self {
        Self::new()
    }
}
