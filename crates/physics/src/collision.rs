use glam::IVec3;
use strata_core::{BlockPos, Chunk};

/// Returns `true` if the block at `(x, y, z)` is not air.
pub fn is_block_solid(chunk: &Chunk, x: usize, y: usize, z: usize) -> bool {
    let block = chunk.get_block(x, y, z);
    !block.is_air()
}

/// Steps along a ray in small increments and returns the first solid block hit.
pub fn voxel_raycast(
    chunk: &Chunk,
    origin: glam::Vec3,
    direction: glam::Vec3,
    max_dist: f32,
) -> Option<BlockPos> {
    let dir = direction.normalize();
    let mut pos = origin;
    let step = 0.1;
    let steps = (max_dist / step) as usize;

    for _ in 0..steps {
        pos += dir * step;
        let bx = pos.x.floor() as i32;
        let by = pos.y.floor() as i32;
        let bz = pos.z.floor() as i32;

        let Some((chunk_pos, lx, ly, lz)) = BlockPos(IVec3::new(bx, by, bz)).to_chunk_local() else {
            continue;
        };
        if chunk_pos == chunk.position && !chunk.get_block(lx, ly, lz).is_air() {
            return Some(BlockPos(IVec3::new(bx, by, bz)));
        }
    }

    None
}
