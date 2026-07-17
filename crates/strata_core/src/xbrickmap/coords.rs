//! Voxel coordinate math for a 32³ sector (plan 05 §3 / 06 §1).
//!
//! All decomposition uses shifts and masks only (`>> 3`, `>> 1`, `& 7`, `& 1`)
//! — never integer division — so the hot path stays branchless and O(1).

/// Sector edge length in voxels.
pub const SECTOR_DIM: u32 = 32;
/// Brick edge length in voxels (8³ = 512 voxels per brick).
pub const BRICK_DIM: u32 = 8;
/// Sub-brick edge length in voxels (2³ = 8 voxels per sub-brick).
pub const SUBBRICK_DIM: u32 = 2;

/// A voxel coordinate local to a sector, each axis in `0..SECTOR_DIM`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoxelCoord {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

impl VoxelCoord {
    #[inline]
    pub fn new(x: u32, y: u32, z: u32) -> Self {
        // Sector-local coords must be 0..SECTOR_DIM. An out-of-range axis makes
        // `brick_index()` exceed 63, so `sector_mask >> bi` shifts by >= 64 —
        // a debug panic ("shift with overflow") or a silently masked, wrong
        // result in release. Callers sampling neighbours must wrap into 0..31
        // before constructing a `VoxelCoord` (see meshing's `sample_block`).
        debug_assert!(
            x < SECTOR_DIM && y < SECTOR_DIM && z < SECTOR_DIM,
            "VoxelCoord out of range: ({x}, {y}, {z}) not in 0..{SECTOR_DIM}"
        );
        VoxelCoord { x, y, z }
    }

    /// Brick index within the sector (0..64), matching plan 06 WGSL layout.
    #[inline]
    pub fn brick_index(&self) -> usize {
        let bx = self.x >> 3;
        let by = self.y >> 3;
        let bz = self.z >> 3;
        (bx + bz * 4 + by * 16) as usize
    }

    /// Local voxel within the brick (0..8 per axis).
    #[inline]
    fn local(&self) -> (u32, u32, u32) {
        (self.x & 7, self.y & 7, self.z & 7)
    }

    /// Sub-brick index within the brick (0..64).
    #[inline]
    pub fn sub_index(&self) -> usize {
        let (vx, vy, vz) = self.local();
        let sx = vx >> 1;
        let sy = vy >> 1;
        let sz = vz >> 1;
        (sx + sz * 4 + sy * 16) as usize
    }

    /// Bit position of the voxel within its 2³ sub-brick (0..8).
    #[inline]
    pub fn voxel_bit(&self) -> usize {
        let (vx, vy, vz) = self.local();
        let lx = vx & 1;
        let ly = vy & 1;
        let lz = vz & 1;
        (lx + lz * 2 + ly * 4) as usize
    }
}
