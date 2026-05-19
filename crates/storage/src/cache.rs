use hashbrown::HashMap;
use std::collections::VecDeque;
use strata_core::{Chunk, ChunkPos};

/// Fixed-capacity LRU-like cache for recently accessed chunks.
///
/// When full, the oldest inserted chunk is evicted.
pub struct ChunkCache {
    cache: HashMap<ChunkPos, Chunk>,
    order: VecDeque<ChunkPos>,
    max_size: usize,
}

impl ChunkCache {
    /// Creates a new cache with the given maximum capacity.
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(max_size),
            order: VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    /// Returns a reference to the chunk at `pos`, if cached.
    pub fn get(&self, pos: &ChunkPos) -> Option<&Chunk> {
        self.cache.get(pos)
    }

    /// Inserts a chunk, evicting the oldest entry if at capacity.
    pub fn insert(&mut self, pos: ChunkPos, chunk: Chunk) {
        if self.cache.len() >= self.max_size
            && let Some(oldest) = self.order.pop_front()
        {
            self.cache.remove(&oldest);
        }
        self.cache.insert(pos, chunk);
        self.order.push_back(pos);
    }

    /// Removes and returns the chunk at `pos`, if present.
    pub fn remove(&mut self, pos: &ChunkPos) -> Option<Chunk> {
        self.order.retain(|p| p != pos);
        self.cache.remove(pos)
    }

    /// Returns the number of cached chunks.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Returns `true` if `pos` is in the cache.
    pub fn contains(&self, pos: &ChunkPos) -> bool {
        self.cache.contains_key(pos)
    }
}
