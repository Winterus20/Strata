use glam::Vec3;
use strata_core::{BlockPos, Chunk};

/// Casts a ray within a single chunk and returns the first solid block hit.
pub fn raycast_chunk(
    chunk: &Chunk,
    origin: Vec3,
    direction: Vec3,
    max_dist: f32,
) -> Option<BlockPos> {
    crate::collision::voxel_raycast(chunk, origin, direction, max_dist)
}
