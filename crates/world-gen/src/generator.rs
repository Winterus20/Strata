use std::collections::{HashSet, VecDeque};

use rayon::prelude::*;
use strata_core::{Chunk, ChunkPos};

use crate::config::{DEFAULT_LOAD_DISTANCE, DEFAULT_VIEW_DISTANCE};
use crate::noise_cache::NoiseCache;
use crate::terrain::TerrainGenerator;

/// Rayon-based parallel chunk generator with heightmap bounding
/// optimization and load management.
pub struct ChunkGenerator {
    generator: TerrainGenerator,
    queue: VecDeque<ChunkPos>,
    chunks_per_tick: u8,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            generator: TerrainGenerator::new(seed),
            queue: VecDeque::new(),
            chunks_per_tick: 4,
        }
    }

    pub fn generator(&self) -> &TerrainGenerator {
        &self.generator
    }

    pub fn enqueue(&mut self, pos: ChunkPos) {
        if !self.queue.contains(&pos) {
            self.queue.push_back(pos);
        }
    }

    /// Process up to `chunks_per_tick` queued requests sequentially.
    pub fn process(&mut self) -> Vec<Chunk> {
        let limit = self.chunks_per_tick.min(self.queue.len() as u8);
        let mut results = Vec::with_capacity(limit as usize);

        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                let mut chunk = Chunk::new(pos);
                self.generator.generate(&mut chunk);
                results.push(chunk);
            }
        }

        results
    }

    /// Process all pending chunks in parallel using rayon.
    /// Each thread creates its own TerrainGenerator clone.
    pub fn process_all_parallel(&mut self) -> Vec<Chunk> {
        let positions: Vec<ChunkPos> = self.queue.drain(..).collect();
        if positions.is_empty() {
            return Vec::new();
        }

        let seed = self.generator.noise().seed();

        positions
            .par_iter()
            .map(|&pos| {
                let mut chunk = Chunk::new(pos);
                let mut local_gen = TerrainGenerator::new(seed as u32);
                local_gen.generate(&mut chunk);
                chunk
            })
            .collect()
    }

    pub fn set_chunks_per_tick(&mut self, n: u8) {
        self.chunks_per_tick = n;
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }
}

/// Priority-sorted chunk load manager with frame throttling and noise cache.
///
/// Implements the pipeline from `world_gen_plan.md §7`:
/// 1. Player position → view distance → chunk pos list
/// 2. Cache hit/miss check (noise cache + chunk cache)
/// 3. Queue generation (prioritized by distance)
/// 4. Batch generate (rayon parallel)
/// 5. Return generated chunks
pub struct ChunkLoadManager {
    queue: VecDeque<ChunkPos>,
    loading: HashSet<ChunkPos>,
    chunks_per_tick: u8,
    _max_concurrent: u8,
    view_distance: u32,
    load_distance: u32,
    tick_counter: u32,
    noise_cache: NoiseCache,
}

impl ChunkLoadManager {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            loading: HashSet::new(),
            chunks_per_tick: 4,
            _max_concurrent: 8,
            view_distance: DEFAULT_VIEW_DISTANCE,
            load_distance: DEFAULT_LOAD_DISTANCE,
            tick_counter: 0,
            noise_cache: NoiseCache::new(),
        }
    }

    /// Calculate which chunks should be loaded based on player position.
    pub fn required_chunks(&self, player_pos: ChunkPos) -> Vec<ChunkPos> {
        let rd = self.load_distance as i32;
        let mut required = Vec::with_capacity((rd * 2 + 1) as usize * (rd * 2 + 1) as usize);

        for x in (player_pos.0.x - rd)..=(player_pos.0.x + rd) {
            for z in (player_pos.0.y - rd)..=(player_pos.0.y + rd) {
                let pos = ChunkPos(glam::IVec2::new(x, z));
                required.push(pos);
            }
        }

        required
    }

    /// Request chunks for loading (skips duplicates and already-loading).
    pub fn request_chunks(&mut self, positions: &[ChunkPos], existing: &HashSet<ChunkPos>) {
        for &pos in positions {
            if !self.loading.contains(&pos) && !existing.contains(&pos) {
                self.loading.insert(pos);
                self.queue.push_back(pos);
            }
        }
    }

    /// Prioritize the queue by distance from player (nearest first).
    pub fn prioritize(&mut self, player_pos: ChunkPos) {
        let mut vec: Vec<ChunkPos> = self.queue.drain(..).collect();
        vec.sort_by_key(|pos| {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            (dx * dx + dz * dz) as u32
        });
        self.queue.extend(vec);
    }

    /// Process up to `chunks_per_tick` pending chunks sequentially.
    pub fn process(&mut self, seed: u32) -> Vec<Chunk> {
        self.tick_counter += 1;

        let limit = self.chunks_per_tick.min(self.queue.len() as u8);
        let mut results = Vec::with_capacity(limit as usize);

        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                self.loading.remove(&pos);
                let mut chunk = Chunk::new(pos);
                let mut local_gen = TerrainGenerator::new(seed);
                local_gen.generate(&mut chunk);
                results.push(chunk);
            }
        }

        results
    }

    /// Process chunks in parallel using rayon.
    pub fn process_parallel(&mut self, seed: u32) -> Vec<Chunk> {
        self.tick_counter += 1;

        let positions: Vec<ChunkPos> = self.queue.drain(..).collect();
        if positions.is_empty() {
            return Vec::new();
        }

        // Remove from loading set
        for &pos in &positions {
            self.loading.remove(&pos);
        }

        positions
            .par_iter()
            .map(|&pos| {
                let mut chunk = Chunk::new(pos);
                let mut local_gen = TerrainGenerator::new(seed);
                local_gen.generate(&mut chunk);
                chunk
            })
            .collect()
    }

    /// Mark chunks for unloading (outside load distance).
    pub fn chunks_to_unload(
        &self,
        player_pos: ChunkPos,
        existing: &HashSet<ChunkPos>,
    ) -> Vec<ChunkPos> {
        let rd = (self.view_distance + 2) as i32;
        let mut to_unload = Vec::new();

        for &pos in existing {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            if dx.abs() > rd || dz.abs() > rd {
                to_unload.push(pos);
            }
        }

        to_unload
    }

    pub fn noise_cache(&self) -> &NoiseCache {
        &self.noise_cache
    }

    pub fn noise_cache_mut(&mut self) -> &mut NoiseCache {
        &mut self.noise_cache
    }

    pub fn set_view_distance(&mut self, dist: u32) {
        self.view_distance = dist;
        self.load_distance = dist + 2;
    }

    pub fn set_chunks_per_tick(&mut self, n: u8) {
        self.chunks_per_tick = n;
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn loading_count(&self) -> usize {
        self.loading.len()
    }
}

impl Default for ChunkLoadManager {
    fn default() -> Self {
        Self::new()
    }
}
