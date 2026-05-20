use crate::chunk_gen_worker::ChunkGenWorker;
use hashbrown::HashSet;
use std::collections::VecDeque;
use strata_core::ChunkPos;

/// Frame-throttled chunk loading with distance-based prioritization.
///
/// Submits chunk generation requests to a background worker pool,
/// keeping the main thread completely free from terrain gen and disk I/O.
pub struct LazyChunkLoader {
    queue: VecDeque<ChunkPos>,
    /// O(1) lookup to avoid duplicate entries in the queue.
    queued_set: HashSet<ChunkPos>,
    /// Tracks chunks that have been submitted to the worker but not yet returned.
    in_flight: HashSet<ChunkPos>,
    chunks_per_frame: u8,
    frame_counter: u32,
    load_interval: u32,
}

impl LazyChunkLoader {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            queued_set: HashSet::new(),
            in_flight: HashSet::new(),
            chunks_per_frame: 4,
            frame_counter: 0,
            load_interval: 1,
        }
    }

    /// Request chunks to load (called when player moves).
    pub fn request_chunks(&mut self, positions: &[ChunkPos]) {
        for pos in positions {
            // Skip if already queued or already being generated
            if !self.queued_set.contains(pos) && !self.in_flight.contains(pos) {
                self.queued_set.insert(*pos);
                self.queue.push_back(*pos);
            }
        }
    }

    /// Process pending chunks — submits to background worker.
    /// Returns the number of newly submitted requests.
    pub fn process(&mut self, worker: &mut ChunkGenWorker) -> usize {
        self.frame_counter += 1;
        if !self.frame_counter.is_multiple_of(self.load_interval) {
            return 0;
        }

        let limit = self.chunks_per_frame.min(self.queue.len() as u8);
        let mut submitted = 0;

        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                self.queued_set.remove(&pos);
                self.in_flight.insert(pos);
                worker.submit(pos);
                submitted += 1;
            }
        }

        submitted
    }

    /// Mark a chunk as no longer in-flight (called when result is received).
    pub fn mark_completed(&mut self, pos: ChunkPos) {
        self.in_flight.remove(&pos);
    }

    /// Sort queue so nearest chunks to player are loaded first.
    /// Only call when the player's chunk actually changes.
    pub fn prioritize(&mut self, player_chunk: ChunkPos) {
        let mut vec: Vec<ChunkPos> = self.queue.drain(..).collect();
        vec.sort_by_key(|pos| {
            let dx = pos.0.x - player_chunk.0.x;
            let dz = pos.0.y - player_chunk.0.y;
            (dx * dx + dz * dz) as u32
        });
        self.queue.extend(vec);
    }
}

impl Default for LazyChunkLoader {
    fn default() -> Self {
        Self::new()
    }
}
