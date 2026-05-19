#[cfg(test)]
mod dhat_tests {
    use strata_core::{Chunk, ChunkPos};
    use strata_storage::FjallChunkStore;
    use glam::IVec2;
    use tempfile::tempdir;

    #[global_allocator]
    static ALLOC: dhat::Alloc = dhat::Alloc;

    #[test]
    fn profile_memory_allocations() {
        // Prevent DHAT profiler from writing file when not running under actual profiling test
        // (so it runs fine during standard `cargo test`)
        let _profiler = dhat::Profiler::builder().testing().build();
        
        let dir = tempdir().unwrap();
        let store = FjallChunkStore::new(dir.path()).unwrap();

        // Simulate 50 chunk saves and loads to profile compaction and write buffers
        for x in 0..5 {
            for z in 0..5 {
                let pos = ChunkPos(IVec2::new(x, z));
                let mut chunk = Chunk::new(pos);
                chunk.set_block(0, 0, 0, strata_core::BlockId(5));
                store.save_chunk(&chunk).unwrap();
                
                let loaded = store.load_chunk(pos).unwrap().unwrap();
                assert_eq!(loaded.get_block(0, 0, 0).0, 5);
            }
        }

        store.persist().unwrap();

        let stats = dhat::HeapStats::get();
        println!("DHAT Storage Profile Stats:");
        println!("  Peak memory allocated: {} bytes", stats.max_bytes);
        println!("  Total allocations: {}", stats.total_blocks);
    }
}
