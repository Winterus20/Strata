use bevy_ecs::observer::On;
use bevy_ecs::prelude::*;
use hashbrown::HashMap;
use strata_core::{BlockId, Chunk, ChunkPos};
use tracing::debug;

use crate::components::interaction::{BlockBreakEvent, BlockPlaceEvent};

#[derive(Resource)]
pub struct ChunkStorage {
    pub chunks: HashMap<ChunkPos, Chunk>,
}

impl ChunkStorage {
    pub fn new() -> Self {
        Self {
            chunks: HashMap::new(),
        }
    }

    pub fn get_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn get_chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }

    pub fn insert_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.chunks.insert(pos, chunk);
    }

    pub fn remove_chunk(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }
}

impl Default for ChunkStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub fn on_block_break(trigger: On<BlockBreakEvent>, mut chunk_storage: ResMut<ChunkStorage>) {
    let block_pos = trigger.event().0;
    let Some((chunk_pos, lx, ly, lz)) = block_pos.to_chunk_local() else {
        return;
    };

    if let Some(chunk) = chunk_storage.get_chunk_mut(chunk_pos) {
        let old_block = chunk.get_block(lx, ly, lz);
        if old_block.is_air() {
            return;
        }

        chunk.set_block(lx, ly, lz, BlockId::AIR);
        debug!(
            "Block broken at {:?} (was {:?}), chunk {:?} marked dirty",
            block_pos, old_block, chunk_pos
        );
    }

    mark_neighbor_chunks(&mut chunk_storage, chunk_pos, lx, lz);
}

pub fn on_block_place(trigger: On<BlockPlaceEvent>, mut chunk_storage: ResMut<ChunkStorage>) {
    let block_pos = trigger.event().position;
    let block_id = BlockId(trigger.event().block_id);
    let Some((chunk_pos, lx, ly, lz)) = block_pos.to_chunk_local() else {
        return;
    };

    if let Some(chunk) = chunk_storage.get_chunk_mut(chunk_pos) {
        if !chunk.get_block(lx, ly, lz).is_air() {
            return;
        }

        chunk.set_block(lx, ly, lz, block_id);
        debug!(
            "Block placed at {:?} (id={:?}), chunk {:?} marked dirty",
            block_pos, block_id, chunk_pos
        );
    }

    mark_neighbor_chunks(&mut chunk_storage, chunk_pos, lx, lz);
}

fn mark_neighbor_chunks(
    chunk_storage: &mut ChunkStorage,
    chunk_pos: ChunkPos,
    lx: usize,
    lz: usize,
) {
    if lx == 0 {
        let neighbor = ChunkPos(glam::IVec2::new(chunk_pos.0.x - 1, chunk_pos.0.y));
        if let Some(c) = chunk_storage.get_chunk_mut(neighbor) {
            c.dirty = true;
            c.light_dirty = true;
        }
    }
    if lx == strata_core::chunk::CHUNK_WIDTH - 1 {
        let neighbor = ChunkPos(glam::IVec2::new(chunk_pos.0.x + 1, chunk_pos.0.y));
        if let Some(c) = chunk_storage.get_chunk_mut(neighbor) {
            c.dirty = true;
            c.light_dirty = true;
        }
    }
    if lz == 0 {
        let neighbor = ChunkPos(glam::IVec2::new(chunk_pos.0.x, chunk_pos.0.y - 1));
        if let Some(c) = chunk_storage.get_chunk_mut(neighbor) {
            c.dirty = true;
            c.light_dirty = true;
        }
    }
    if lz == strata_core::chunk::CHUNK_DEPTH - 1 {
        let neighbor = ChunkPos(glam::IVec2::new(chunk_pos.0.x, chunk_pos.0.y + 1));
        if let Some(c) = chunk_storage.get_chunk_mut(neighbor) {
            c.dirty = true;
            c.light_dirty = true;
        }
    }

    if let Some(c) = chunk_storage.get_chunk_mut(chunk_pos) {
        c.dirty = true;
    }
}
