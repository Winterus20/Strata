#[cfg(test)]
mod tests {
    use crate::chunk::{Chunk, ChunkPos, CHUNK_WIDTH, CHUNK_DEPTH};
    use crate::block::BlockId;
    use glam::IVec2;

    #[test]
    fn test_chunk_flat_indexing() {
        let x = 3;
        let y = 120;
        let z = 14;
        let expected_idx = x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_DEPTH;
        assert_eq!(Chunk::index(x, y, z), expected_idx);
    }

    #[test]
    fn test_chunk_heightmap_updates() {
        let mut chunk = Chunk::new(ChunkPos(IVec2::new(0, 0)));
        let col = Chunk::column_index(5, 5);

        // Initial air columns must have 0 heights
        assert_eq!(chunk.heightmap_top[col], 0);
        assert_eq!(chunk.heightmap_bottom[col], 0);

        // Set an active block at height 50
        chunk.set_block(5, 50, 5, BlockId(1));
        assert_eq!(chunk.heightmap_top[col], 50);
        assert_eq!(chunk.heightmap_bottom[col], 50);

        // Set another block at height 100
        chunk.set_block(5, 100, 5, BlockId(2));
        assert_eq!(chunk.heightmap_top[col], 100);
        assert_eq!(chunk.heightmap_bottom[col], 50); // Bottom should remain 50
    }
}
