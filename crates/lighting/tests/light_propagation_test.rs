#[cfg(test)]
mod tests {
    use strata_core::{BlockId, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk, ChunkPos};
    use strata_lighting::propagate::propagate_all;

    fn emission_table() -> Vec<u8> {
        let mut t = vec![0u8; 256];
        t[5] = 15; // BlockId(5) emits light
        t
    }

    #[test]
    fn test_sky_light_top_down() {
        let pos = ChunkPos(glam::IVec2::new(0, 0));
        let mut chunk = Chunk::new(pos);
        let light_emission = emission_table();

        // Fill bottom 10 layers with stone
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                for y in 0..10 {
                    chunk.set_block(x, y, z, BlockId::STONE);
                }
            }
        }

        // Verify heightmap
        let col = Chunk::column_index(0, 0);
        assert_eq!(chunk.heightmap_top[col], 9, "heightmap top should be y=9");

        propagate_all(&mut chunk, &light_emission);

        // Top of chunk should have max sky light
        let top_idx = Chunk::index(0, CHUNK_HEIGHT - 1, 0);
        assert_eq!(
            chunk.light.get_sky(top_idx),
            15,
            "top should have full sky light"
        );

        // Below stone (y=0) should have 0 sky light
        let bottom_idx = Chunk::index(0, 0, 0);
        assert_eq!(
            chunk.light.get_sky(bottom_idx),
            0,
            "deep underground should have 0 sky light"
        );

        // Stone surface (y=9) should have 0 sky light
        let surface_idx = Chunk::index(0, 9, 0);
        assert_eq!(
            chunk.light.get_sky(surface_idx),
            0,
            "stone surface should be dark"
        );
    }

    #[test]
    fn test_block_light_emission() {
        let pos = ChunkPos(glam::IVec2::new(0, 0));
        let mut chunk = Chunk::new(pos);
        let light_emission = emission_table();

        // Place a light-emitting block at center
        chunk.set_block(8, 50, 8, BlockId(5));

        propagate_all(&mut chunk, &light_emission);

        // Source should have max block light
        let source_idx = Chunk::index(8, 50, 8);
        assert_eq!(
            chunk.light.get_block(source_idx),
            15,
            "source should have block light 15"
        );

        // Immediately adjacent should have some light
        let nearby_idx = Chunk::index(8, 49, 8);
        assert!(
            chunk.light.get_block(nearby_idx) > 0,
            "adjacent block should receive light"
        );
    }
}
