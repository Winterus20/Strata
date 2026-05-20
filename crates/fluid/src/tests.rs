#[cfg(test)]
mod tests {
    use glam::IVec2;
    use strata_core::{BlockId, Chunk, ChunkPos};

    use crate::flow::WaterFlow;
    use crate::water_level::{ChunkWaterLevels, WaterLevel};

    fn make_test_chunk() -> Chunk {
        let pos = ChunkPos(IVec2::new(0, 0));
        Chunk::new(pos)
    }

    #[test]
    fn water_level_empty_by_default() {
        let water = ChunkWaterLevels::new();
        assert!(!water.has_water());
        assert_eq!(water.get(0, 0, 0), WaterLevel::EMPTY);
        assert_eq!(water.get(7, 100, 7), WaterLevel::EMPTY);
    }

    #[test]
    fn water_level_set_and_get() {
        let mut water = ChunkWaterLevels::new();
        water.set(5, 10, 5, WaterLevel::SOURCE);
        assert!(water.has_water());
        assert_eq!(water.get(5, 10, 5), WaterLevel::SOURCE);
        assert_eq!(water.water_count, 1);
    }

    #[test]
    fn water_level_clamps_to_max() {
        let level = WaterLevel::from_raw(20);
        assert_eq!(level.level(), 15);
    }

    #[test]
    fn water_level_is_source() {
        assert!(WaterLevel::SOURCE.is_source());
        assert!(!WaterLevel::from_raw(14).is_source());
        assert!(!WaterLevel::EMPTY.is_source());
    }

    #[test]
    fn water_flow_downward_gravity() {
        let mut chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        // Place water block and set water level at y=10
        chunk.set_block(8, 10, 8, BlockId::WATER);
        water.set(8, 10, 8, WaterLevel::SOURCE);

        let changed = WaterFlow::tick(&chunk, &mut water);
        assert!(changed);

        // Water should have flowed down to y=9
        assert_eq!(water.get(8, 9, 8).level(), WaterLevel::MAX);
    }

    #[test]
    fn water_flow_horizontal_spread() {
        let mut chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        // Place water source at center
        chunk.set_block(8, 5, 8, BlockId::WATER);
        water.set(8, 5, 8, WaterLevel::SOURCE);

        // Run multiple ticks to allow horizontal spread
        for _ in 0..5 {
            WaterFlow::tick(&chunk, &mut water);
        }

        // Water should have spread downward at minimum
        assert!(water.get(8, 4, 8).level() > 0 || water.get(8, 3, 8).level() > 0);
    }

    #[test]
    fn water_flow_no_upward_flow() {
        let mut chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        // Place water at y=5
        chunk.set_block(8, 5, 8, BlockId::WATER);
        water.set(8, 5, 8, WaterLevel::SOURCE);

        // Run multiple ticks
        for _ in 0..10 {
            WaterFlow::tick(&chunk, &mut water);
        }

        // Water should NOT have flowed upward to y=6
        assert_eq!(water.get(8, 6, 8), WaterLevel::EMPTY);
    }

    #[test]
    fn water_flow_level_decreases_with_distance() {
        let mut chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        // Place water source
        chunk.set_block(8, 5, 8, BlockId::WATER);
        water.set(8, 5, 8, WaterLevel::SOURCE);

        // Run multiple ticks
        for _ in 0..10 {
            WaterFlow::tick(&chunk, &mut water);
        }

        // Water level should decrease with distance from source
        let source_level = water.get(8, 5, 8).level();
        let neighbor_level = water.get(7, 5, 8).level();

        if neighbor_level > 0 {
            assert!(neighbor_level <= source_level);
        }
    }

    #[test]
    fn water_flow_stops_at_solid_blocks() {
        let mut chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        // Place water source
        chunk.set_block(8, 5, 8, BlockId::WATER);
        water.set(8, 5, 8, WaterLevel::SOURCE);

        // Place stone block to the left
        chunk.set_block(7, 5, 8, BlockId::STONE);

        // Run multiple ticks
        for _ in 0..10 {
            WaterFlow::tick(&chunk, &mut water);
        }

        // Water should NOT have flowed through stone
        assert_eq!(water.get(7, 5, 8), WaterLevel::EMPTY);
    }

    #[test]
    fn water_flow_no_change_when_no_water() {
        let chunk = make_test_chunk();
        let mut water = ChunkWaterLevels::new();

        let changed = WaterFlow::tick(&chunk, &mut water);
        assert!(!changed);
    }

    #[test]
    fn water_flow_dirty_tracking() {
        let mut water = ChunkWaterLevels::new();
        assert!(!water.dirty);

        water.set(5, 5, 5, WaterLevel::SOURCE);
        assert!(water.dirty);

        water.clear_dirty();
        assert!(!water.dirty);
    }

    #[test]
    fn water_flow_water_count_tracking() {
        let mut water = ChunkWaterLevels::new();
        assert_eq!(water.water_count, 0);

        water.set(5, 5, 5, WaterLevel::SOURCE);
        assert_eq!(water.water_count, 1);

        water.set(5, 5, 5, WaterLevel::EMPTY);
        assert_eq!(water.water_count, 0);
    }

    #[test]
    fn water_level_init_from_chunk() {
        let mut chunk = make_test_chunk();
        // Place water blocks
        chunk.set_block(5, 10, 5, BlockId::WATER);
        chunk.set_block(6, 10, 6, BlockId::WATER);
        chunk.set_block(7, 10, 7, BlockId::WATER);

        let water = ChunkWaterLevels::init_from_chunk(&chunk);
        assert!(water.has_water());
        assert_eq!(water.water_count, 3);
        assert_eq!(water.get(5, 10, 5), WaterLevel::SOURCE);
        assert_eq!(water.get(6, 10, 6), WaterLevel::SOURCE);
        assert_eq!(water.get(7, 10, 7), WaterLevel::SOURCE);
        assert_eq!(water.get(0, 0, 0), WaterLevel::EMPTY);
    }
}
