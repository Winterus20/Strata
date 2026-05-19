use crate::block::BlockId;
use crate::light::LightData;
use glam::IVec2;

/// Width of a chunk along the X axis (blocks).
pub const CHUNK_WIDTH: usize = 16;
/// Height of a chunk along the Y axis (blocks).
pub const CHUNK_HEIGHT: usize = 256;
/// Depth of a chunk along the Z axis (blocks).
pub const CHUNK_DEPTH: usize = 16;
/// Total number of blocks in a chunk (`WIDTH * HEIGHT * DEPTH`).
pub const CHUNK_VOLUME: usize = CHUNK_WIDTH * CHUNK_HEIGHT * CHUNK_DEPTH;

/// Chunk-grid coordinate (measured in chunks, not blocks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos(pub IVec2);

impl ChunkPos {
    /// Converts world-space block coordinates to the containing chunk position.
    #[inline]
    pub fn from_world(x: i32, z: i32) -> Self {
        Self(IVec2::new(
            x.div_euclid(CHUNK_WIDTH as i32),
            z.div_euclid(CHUNK_DEPTH as i32),
        ))
    }

    /// Returns the world-space X coordinate of this chunk's origin.
    #[inline]
    pub fn world_x(&self) -> i32 {
        self.0.x * CHUNK_WIDTH as i32
    }

    /// Returns the world-space Z coordinate of this chunk's origin.
    #[inline]
    pub fn world_z(&self) -> i32 {
        self.0.y * CHUNK_DEPTH as i32
    }
}

/// A 16Ãƒâ€”256Ãƒâ€”16 column of blocks stored as a flat `Vec<u16>`.
///
/// Index formula: `x + z * 16 + y * 256`.
#[derive(Clone)]
pub struct Chunk {
    /// Position of this chunk in chunk-grid coordinates.
    pub position: ChunkPos,
    /// Flat block-id array (`CHUNK_VOLUME` elements).
    pub blocks: Vec<u16>,
    /// Per-column highest non-air Y (0 if column is empty).
    pub heightmap_top: [u16; 256],
    /// Per-column lowest non-air Y (0 if column is empty).
    pub heightmap_bottom: [u16; 256],
    /// Whether this chunk has been modified since the last mesh build.
    pub dirty: bool,
    /// Whether this chunk needs light propagation.
    pub light_dirty: bool,
    /// Per-chunk light data (sky + block).
    pub light: LightData,
}

impl Chunk {
    /// Creates an empty (all-air) chunk at the given position.
    pub fn new(position: ChunkPos) -> Self {
        Self {
            position,
            blocks: vec![0u16; CHUNK_VOLUME],
            heightmap_top: [0u16; 256],
            heightmap_bottom: [0u16; 256],
            dirty: false,
            light_dirty: true,
            light: LightData::new(),
        }
    }

    /// Computes the flat-array index for the block at `(x, y, z)`.
    #[inline(always)]
    pub fn index(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_WIDTH && y < CHUNK_HEIGHT && z < CHUNK_DEPTH);
        x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_DEPTH
    }

    /// Computes the column index for `(x, z)` in the 16Ãƒâ€”16 heightmap.
    #[inline(always)]
    pub fn column_index(x: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_WIDTH && z < CHUNK_DEPTH);
        x + z * CHUNK_WIDTH
    }

    /// Returns the block id at the given local coordinates.
    #[inline]
    pub fn get_block(&self, x: usize, y: usize, z: usize) -> BlockId {
        BlockId(self.blocks[Self::index(x, y, z)])
    }

    /// Sets the block at the given local coordinates and updates the heightmap.
    #[inline]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        let idx = Self::index(x, y, z);
        self.blocks[idx] = id.0;
        self.update_heightmap(x, z);
        self.dirty = true;
        self.light_dirty = true;
    }

    /// Fully recomputes the heightmap for the column at `(x, z)`.
    fn update_heightmap(&mut self, x: usize, z: usize) {
        let col = Self::column_index(x, z);

        // Find the highest non-air block.
        let mut top = 0u16;
        let mut found_top = false;
        for y in (0..CHUNK_HEIGHT).rev() {
            if !BlockId(self.blocks[Self::index(x, y, z)]).is_air() {
                top = y as u16;
                found_top = true;
                break;
            }
        }
        self.heightmap_top[col] = if found_top { top } else { 0 };

        // Find the lowest non-air block.
        let mut bottom = 0u16;
        let mut found_bottom = false;
        for y in 0..CHUNK_HEIGHT {
            if !BlockId(self.blocks[Self::index(x, y, z)]).is_air() {
                bottom = y as u16;
                found_bottom = true;
                break;
            }
        }
        self.heightmap_bottom[col] = if found_bottom { bottom } else { 0 };
    }

    /// Recomputes `heightmap_top` and `heightmap_bottom` for every column.
    ///
    /// Call this after bulk-loading block data (e.g. deserialization) where
    /// `set_block` was not used.
    pub fn rebuild_all_heightmaps(&mut self) {
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_DEPTH {
                self.update_heightmap(x, z);
            }
        }
    }

    /// Returns `true` if every block in this chunk is air.
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|&b| b == 0)
    }

    /// Returns the block data as a shared slice.
    #[inline]
    pub fn as_slice(&self) -> &[u16] {
        &self.blocks
    }

    /// Returns the block data as a mutable slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u16] {
        &mut self.blocks
    }
}
