use divan::{black_box, Divan};
use strata_core::{Chunk, ChunkPos};
use strata_storage::FjallChunkStore;
use glam::IVec2;
use std::path::PathBuf;

fn get_temp_db_path() -> PathBuf {
    let pid = std::process::id();
    let unique_name = format!("strata_bench_db_{}", pid);
    std::env::temp_dir().join(unique_name)
}

#[divan::bench]
fn bench_save_chunk(bencher: divan::Bencher) {
    let path = get_temp_db_path();
    let store = FjallChunkStore::new(&path).unwrap();
    
    // Create a dummy chunk to save
    let pos = ChunkPos(IVec2::new(0, 0));
    let mut chunk = Chunk::new(pos);
    // Add some representative block data
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 10, z, strata_core::BlockId(1)); // stone
            chunk.set_block(x, 9, z, strata_core::BlockId(2));  // dirt
            chunk.set_block(x, 8, z, strata_core::BlockId(3));  // grass
        }
    }

    bencher.bench_local(|| {
        let _ = store.save_chunk(black_box(&chunk));
    });

    // Cleanup
    let _ = std::fs::remove_dir_all(&path);
}

#[divan::bench]
fn bench_load_chunk(bencher: divan::Bencher) {
    let path = get_temp_db_path();
    let store = FjallChunkStore::new(&path).unwrap();
    
    let pos = ChunkPos(IVec2::new(0, 0));
    let mut chunk = Chunk::new(pos);
    for x in 0..16 {
        for z in 0..16 {
            chunk.set_block(x, 10, z, strata_core::BlockId(1));
        }
    }
    store.save_chunk(&chunk).unwrap();
    store.persist().unwrap();

    bencher.bench_local(|| {
        let loaded = store.load_chunk(black_box(pos)).unwrap();
        black_box(loaded);
    });

    // Cleanup
    let _ = std::fs::remove_dir_all(&path);
}

fn main() {
    Divan::from_args().main();
}
