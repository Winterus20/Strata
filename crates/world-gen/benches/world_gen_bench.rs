use divan::{black_box, Divan};
use strata_core::{Chunk, ChunkPos};
use strata_world_gen::TerrainGenerator;
use glam::IVec2;

#[divan::bench]
fn bench_terrain_generation(bencher: divan::Bencher) {
    let generator = TerrainGenerator::new(42);
    let chunk_pos = ChunkPos(IVec2::new(10, -5));

    bencher.bench_local(|| {
        let mut chunk = Chunk::new(chunk_pos);
        generator.generate(black_box(&mut chunk));
        black_box(chunk);
    });
}

#[divan::bench]
fn bench_terrain_noise_only(bencher: divan::Bencher) {
    let generator = TerrainGenerator::new(42);

    bencher.bench_local(|| {
        let mut sum = 0.0;
        for x in 0..16 {
            for z in 0..16 {
                sum += generator.height_at(black_box(x), black_box(z));
            }
        }
        black_box(sum);
    });
}

fn main() {
    Divan::from_args().main();
}
