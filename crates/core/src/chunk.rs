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
/// Number of horizontal border faces (neighbors).
pub const BORDER_FACE_COUNT: usize = 4;
/// Size of one border face slice (16Ãƒâ€”256).
pub const BORDER_SLICE_SIZE: usize = CHUNK_WIDTH * CHUNK_HEIGHT;
/// Total border block storage.
pub const BORDER_TOTAL: usize = BORDER_FACE_COUNT * BORDER_SLICE_SIZE;

/// Face index constants for [`Chunk::border_blocks`].
pub mod border_face {
    /// Neighbor in the -X direction.
    pub const NEG_X: usize = 0;
    /// Neighbor in the +X direction.
    pub const POS_X: usize = 1;
    /// Neighbor in the -Z direction.
    pub const NEG_Z: usize = 2;
    /// Neighbor in the +Z direction.
    pub const POS_Z: usize = 3;
}

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
    /// 1-block-thick border slices from neighbor chunks (NEG_X, POS_X, NEG_Z, POS_Z).
    /// Each slice is [`BORDER_SLICE_SIZE`] 16Ãƒâ€”256 blocks indexed as `u + y * 16`
    /// where `u` is the local coordinate along the face (z for X faces, x for Z faces).
    pub border_blocks: Vec<u16>,
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
            border_blocks: vec![0u16; BORDER_TOTAL],
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

    /// Returns the block id at a border position.
    ///
    /// `face` is one of [`border_face::NEG_X`], [`border_face::POS_X`],
    /// [`border_face::NEG_Z`], [`border_face::POS_Z`].
    /// `u` is the coordinate along the face: `z` for X faces, `x` for Z faces.
    #[inline]
    pub fn get_border_block(&self, face: usize, u: usize, y: usize) -> BlockId {
        debug_assert!(face < BORDER_FACE_COUNT && u < CHUNK_WIDTH && y < CHUNK_HEIGHT);
        BlockId(self.border_blocks[face * BORDER_SLICE_SIZE + u + y * CHUNK_WIDTH])
    }

    /// Sets a block id in the border slice.
    #[inline]
    pub fn set_border_block(&mut self, face: usize, u: usize, y: usize, id: BlockId) {
        debug_assert!(face < BORDER_FACE_COUNT && u < CHUNK_WIDTH && y < CHUNK_HEIGHT);
        let idx = face * BORDER_SLICE_SIZE + u + y * CHUNK_WIDTH;
        self.border_blocks[idx] = id.0;
    }

    /// Copies one block-layer border slice from another chunk.
    ///
    /// `face` is the face of `self` to fill.
    /// `other` is the neighbor chunk.
    /// `other_face` is the face of `other` to read from (0:NEG_X..POS_Z).
    /// The slice is 1-block thick, so we copy the first or last column of `other`.
    pub fn copy_border_from(&mut self, face: usize, other: &Chunk) {
        match face {
            border_face::NEG_X => {
                // Our -X border = other's blocks at x=15
                for z in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let id = other.get_block(CHUNK_WIDTH - 1, y, z);
                        self.set_border_block(face, z, y, id);
                    }
                }
            }
            border_face::POS_X => {
                // Our +X border = other's blocks at x=0
                for z in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let id = other.get_block(0, y, z);
                        self.set_border_block(face, z, y, id);
                    }
                }
            }
            border_face::NEG_Z => {
                // Our -Z border = other's blocks at z=15
                for x in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let id = other.get_block(x, y, CHUNK_DEPTH - 1);
                        self.set_border_block(face, x, y, id);
                    }
                }
            }
            border_face::POS_Z => {
                // Our +Z border = other's blocks at z=0
                for x in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let id = other.get_block(x, y, 0);
                        self.set_border_block(face, x, y, id);
                    }
                }
            }
            _ => unreachable!(),
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
