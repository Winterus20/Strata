use crate::terrain::TerrainGenerator;
use std::collections::VecDeque;
use strata_core::{Chunk, ChunkPos};

/// Queue-based chunk generator that produces chunks on demand.
pub struct ChunkGenerator {
    generator: TerrainGenerator,
    queue: VecDeque<ChunkPos>,
    chunks_per_frame: u8,
}

impl ChunkGenerator {
    /// Creates a new generator with the given world seed.
    pub fn new(seed: u32) -> Self {
        Self {
            generator: TerrainGenerator::new(seed),
            queue: VecDeque::new(),
            chunks_per_frame: 2,
        }
    }

    /// Enqueues a chunk position for generation (skips duplicates).
    pub fn request_chunk(&mut self, pos: ChunkPos) {
        if !self.queue.contains(&pos) {
            self.queue.push_back(pos);
        }
    }

    /// Processes up to `chunks_per_frame` queued requests and returns the generated chunks.
    pub fn process(&mut self) -> Vec<Chunk> {
        let mut results = Vec::new();
        let limit = self.chunks_per_frame.min(self.queue.len() as u8);

        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                let mut chunk = Chunk::new(pos);
                self.generator.generate(&mut chunk);
                results.push(chunk);
            }
        }

        results
    }
}
