use criterion::{black_box, Criterion, criterion_group, criterion_main};
use strata_core::chunk::{Chunk, ChunkPos, CHUNK_VOLUME};
use strata_network::{ChunkSnapshot, compress_chunk, decompress_chunk};
use glam::IVec2;

fn benchmark_chunk_compression(c: &mut Criterion) {
    let mut chunk = Chunk::new(ChunkPos(IVec2::new(0, 0)));
    for i in 0..CHUNK_VOLUME {
        chunk.blocks[i] = (i % 5) as u16;
    }
    let snapshot = ChunkSnapshot::from_chunk(&chunk);

    let mut group = c.benchmark_group("chunk_sync");
    group.bench_function("compress_chunk", |b| {
        b.iter(|| compress_chunk(black_box(&snapshot)));
    });

    let compressed = compress_chunk(&snapshot).unwrap();
    group.bench_function("decompress_chunk", |b| {
        b.iter(|| decompress_chunk(black_box(&compressed)));
    });
    group.finish();
}

criterion_group!(benches, benchmark_chunk_compression);
criterion_main!(benches);
