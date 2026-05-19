use crate::world::WorldManager;
use strata_core::ChunkPos;

/// Manages dirty chunk flags and throttles mesh rebuilds per frame.
pub struct DirtyChunkManager {
    dirty_chunks: Vec<ChunkPos>,
    max_rebuild_per_frame: u8,
}

impl DirtyChunkManager {
    pub fn new() -> Self {
        Self {
            dirty_chunks: Vec::new(),
            max_rebuild_per_frame: 4,
        }
    }

    /// Mark a chunk as dirty (needs mesh rebuild).
    pub fn mark_dirty(&mut self, pos: ChunkPos) {
        if !self.dirty_chunks.contains(&pos) {
            self.dirty_chunks.push(pos);
        }
    }

    /// Process up to N dirty chunks per frame.
    /// Returns the list of rebuilt chunk positions.
    pub fn process(&mut self, world: &mut WorldManager) -> Vec<ChunkPos> {
        let limit = self
            .max_rebuild_per_frame
            .min(self.dirty_chunks.len() as u8);
        let mut rebuilt = Vec::new();

        for pos in self.dirty_chunks.drain(..limit as usize) {
            world.rebuild_mesh(pos);
            rebuilt.push(pos);
        }

        rebuilt
    }
}

impl Default for DirtyChunkManager {
    fn default() -> Self {
        Self::new()
    }
}
