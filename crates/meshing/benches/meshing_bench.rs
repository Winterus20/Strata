use criterion::{Criterion, criterion_group, criterion_main};
use strata_core::{Chunk, ChunkPos};
use strata_meshing::{ClassicGreedyMesher, Mesher};
use strata_world_gen::TerrainGenerator;

fn bench_classic_greedy(c: &mut Criterion) {
    let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(0, 0)));
    let terrain_gen = TerrainGenerator::new(42);
    terrain_gen.generate(&mut chunk);

    let mesher = ClassicGreedyMesher;

    c.bench_function("classic_greedy_meshing", |b| {
        b.iter(|| {
            let _mesh = mesher.generate_mesh(&chunk);
        });
    });
}

fn bench_gpu_compute(_c: &mut Criterion) {
    // GPU mesher requires wgpu device/queue - skip for now
}

criterion_group!(benches, bench_classic_greedy, bench_gpu_compute);
criterion_main!(benches);
