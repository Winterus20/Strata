use divan::{black_box, Divan};
use strata_core::{Chunk, ChunkPos};
use strata_lighting::propagate_all;
use glam::IVec2;

#[divan::bench]
fn bench_light_propagation(bencher: divan::Bencher) {
    // 1. Create a typical chunk with some terrain and light obstacles
    let pos = ChunkPos(IVec2::new(0, 0));
    let mut chunk = Chunk::new(pos);
    
    // Fill bottom 64 layers with stone blocks to create terrain
    for y in 0..64 {
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, y, z, strata_core::BlockId(1)); // stone
            }
        }
    }
    
    // Set a block light source
    chunk.set_block(8, 65, 8, strata_core::BlockId(4)); // light emitting block (glowstone)
    
    // Prepare a mock light emission table (glowstone at id 4 emits light level 15)
    let mut light_emission = vec![0u8; 256];
    light_emission[4] = 15;

    bencher.bench_local(|| {
        let mut test_chunk = chunk.clone();
        propagate_all(&mut test_chunk, black_box(&light_emission));
        black_box(test_chunk);
    });
}

fn main() {
    Divan::from_args().main();
}
