use divan::{Divan, black_box};
use glam::IVec2;
use strata_core::{Chunk, ChunkPos};
use strata_world_gen::TerrainGenerator;

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

#[divan::bench]
fn bench_carver_noise(bencher: divan::Bencher) {
    let generator = TerrainGenerator::new(42);

    bencher.bench_local(|| {
        let noise = generator.noise();
        let mut vals = vec![0.0f32; 4096];
        noise.cave_grid(&mut vals, 0.0, 0.0, 0.0, 16, 16, 16);
        noise.spaghetti_grid(&mut vals, 0.0, 0.0, 0.0, 16, 16, 16);
        noise.noodle_grid(&mut vals, 0.0, 0.0, 0.0, 16, 16, 16);
        noise.aquifer_grid(&mut vals, 0.0, 0.0, 0.0, 16, 16, 16);
        black_box(vals);
    });
}

#[divan::bench]
fn bench_domain_warp(bencher: divan::Bencher) {
    let generator = TerrainGenerator::new(42);

    bencher.bench_local(|| {
        let noise = generator.noise();
        let mut out_x = [0.0f32; 256];
        let mut out_z = [0.0f32; 256];
        noise.warp_grid(&mut out_x, &mut out_z, 10, -5, 80.0);
        black_box((out_x, out_z));
    });
}

#[divan::bench]
fn bench_cave_system_density(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut sum = 0.0f32;
        for _ in 0..4096 {
            sum += strata_world_gen::cave_system_density(0.5, 0.3, 0.42, 0.8, 1.0);
        }
        black_box(sum);
    });
}

fn main() {
    Divan::from_args().main();
}
