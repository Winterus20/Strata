use crate::chunk::{CHUNK_DEPTH, CHUNK_HEIGHT, CHUNK_WIDTH, ChunkPos};
use glam::IVec3;

/// World-space block coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos(pub IVec3);

impl BlockPos {
    /// Converts this world position into a `(ChunkPos, local_x, local_y, local_z)` tuple.
    /// Returns `None` if `y` is out of the valid range `0..CHUNK_HEIGHT`.
    #[inline]
    pub fn to_chunk_local(self) -> Option<(ChunkPos, usize, usize, usize)> {
        if self.0.y < 0 || self.0.y >= CHUNK_HEIGHT as i32 {
            return None;
        }
        let chunk_x = self.0.x.div_euclid(CHUNK_WIDTH as i32);
        let chunk_z = self.0.z.div_euclid(CHUNK_DEPTH as i32);
        let local_x = self.0.x.rem_euclid(CHUNK_WIDTH as i32) as usize;
        let local_y = self.0.y as usize;
        let local_z = self.0.z.rem_euclid(CHUNK_DEPTH as i32) as usize;
        Some((
            ChunkPos(glam::IVec2::new(chunk_x, chunk_z)),
            local_x,
            local_y,
            local_z,
        ))
    }
}
