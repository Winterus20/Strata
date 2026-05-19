use crate::cache::ChunkCache;
use crate::region::RegionManager;
use std::sync::Arc;
use strata_core::{Chunk, ChunkPos};
use tokio::sync::mpsc;

/// Asynchronous chunk loader backed by a region manager and an in-memory cache.
pub struct AsyncChunkLoader {
    cache: ChunkCache,
    region: Arc<RegionManager>,
    rx: mpsc::Receiver<Chunk>,
    tx: mpsc::Sender<Chunk>,
}

impl AsyncChunkLoader {
    /// Creates a new loader with the given region manager and cache capacity.
    pub fn new(region: RegionManager, cache_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self {
            cache: ChunkCache::new(cache_size),
            region: Arc::new(region),
            rx,
            tx,
        }
    }

    /// Spawns an async task to load the chunk at `pos` from disk.
    pub fn request_load(&self, pos: ChunkPos) {
        let tx = self.tx.clone();
        let region = Arc::clone(&self.region);

        tokio::spawn(async move {
            if let Ok(Some(chunk)) = region.load_chunk(pos) {
                let _ = tx.send(chunk).await;
            }
        });
    }

    /// Drains all chunks that have been loaded since the last call.
    pub fn drain_loaded(&mut self) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        while let Ok(chunk) = self.rx.try_recv() {
            chunks.push(chunk);
        }
        chunks
    }

    /// Returns a cached chunk reference, if present.
    pub fn get_cached(&self, pos: &ChunkPos) -> Option<&Chunk> {
        self.cache.get(pos)
    }

    /// Inserts a chunk into the in-memory cache.
    pub fn cache_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.cache.insert(pos, chunk);
    }
}
