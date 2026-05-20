use glam::{IVec3, Vec3};
use hashbrown::{HashMap, HashSet};
use std::collections::VecDeque;
use strata_core::{
    border_face, BlockId, BlockPos, Chunk, ChunkPos, BORDER_SLICE_SIZE, CHUNK_DEPTH, CHUNK_HEIGHT,
    CHUNK_WIDTH,
};
use strata_lighting::propagate_all;
use strata_meshing::{ChunkMeshBuilder, ClassicGreedyMesher, GpuComputeMesher, MeshData};
use strata_storage::RegionManager;
use strata_world_gen::TerrainGenerator;

use crate::mesh_worker::MeshWorker;

pub struct WorldManager {
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub meshes: HashMap<ChunkPos, MeshData>,
    mesh_worker: Option<MeshWorker>,
    region: RegionManager,
    terrain_gen: TerrainGenerator,
    light_emission: Vec<u8>,
    /// Cached chunk keys to avoid re-collecting every frame.
    pub chunk_keys: Vec<ChunkPos>,
    chunk_keys_dirty: bool,
    /// Pending light propagation queue (throttled per frame).
    light_dirty_queue: VecDeque<ChunkPos>,
    light_dirty_set: HashSet<ChunkPos>,
}

impl WorldManager {
    pub fn new(seed: u32) -> Self {
        Self {
            chunks: HashMap::new(),
            meshes: HashMap::new(),
            mesh_worker: None,
            region: RegionManager::new("world_data"),
            terrain_gen: TerrainGenerator::new(seed),
            light_emission: vec![0u8; 256],
            chunk_keys: Vec::new(),
            chunk_keys_dirty: false,
            light_dirty_queue: VecDeque::new(),
            light_dirty_set: HashSet::new(),
        }
    }

    /// Returns cached chunk keys, rebuilding only if dirty.
    pub fn get_chunk_keys(&mut self) -> &[ChunkPos] {
        if self.chunk_keys_dirty {
            self.chunk_keys.clear();
            self.chunk_keys.extend(self.chunks.keys().copied());
            self.chunk_keys_dirty = false;
        }
        &self.chunk_keys
    }

    /// Initialize background mesh worker.
    pub fn init_mesh_worker(&mut self) {
        self.mesh_worker = Some(MeshWorker::new(ChunkMeshBuilder::new(ClassicGreedyMesher)));
    }

    /// Switch to GPU compute meshing (disables background worker, uses sync).
    #[allow(dead_code)]
    pub fn init_gpu_mesher(&mut self, device: wgpu::Device, queue: wgpu::Queue) {
        self.mesh_worker = None;
        self.mesh_worker = Some(MeshWorker::new(ChunkMeshBuilder::new(GpuComputeMesher::new(device, queue))));
    }

    /// Poll for completed mesh results from background thread.
    pub fn poll_completed_meshes(&self) -> Vec<(ChunkPos, MeshData)> {
        if let Some(worker) = &self.mesh_worker {
            worker.poll()
        } else {
            Vec::new()
        }
    }

    /// Set the light emission level for a block type.
    #[allow(dead_code)]
    pub fn set_light_emission(&mut self, block_id: u16, level: u8) {
        if block_id as usize >= self.light_emission.len() {
            self.light_emission.resize(block_id as usize + 1, 0);
        }
        self.light_emission[block_id as usize] = level.min(15);
    }

    #[allow(dead_code)]
    pub fn get_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn get_or_generate(&mut self, pos: ChunkPos) -> &Chunk {
        if !self.chunks.contains_key(&pos) {
            if let Ok(Some(chunk)) = self.region.load_chunk(pos) {
                self.chunks.insert(pos, chunk);
            } else {
                let mut chunk = Chunk::new(pos);
                self.terrain_gen.generate(&mut chunk);
                self.chunks.insert(pos, chunk);
                let _ = self.region.save_chunk(&self.chunks[&pos]);
            }

            // Fill border slices from existing neighbors
            self.fill_borders(pos);
            // Also update neighbors' borders and remesh them
            for neighbor_pos in &[
                ChunkPos(glam::IVec2::new(pos.0.x - 1, pos.0.y)),
                ChunkPos(glam::IVec2::new(pos.0.x + 1, pos.0.y)),
                ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y - 1)),
                ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y + 1)),
            ] {
                if self.chunks.contains_key(neighbor_pos) {
                    self.fill_borders(*neighbor_pos);
                    self.rebuild_mesh(*neighbor_pos);
                }
            }

            self.chunk_keys_dirty = true;

            // Register light-dirty status
            if let Some(chunk) = self.chunks.get(&pos) {
                if chunk.light_dirty && self.light_dirty_set.insert(pos) {
                    self.light_dirty_queue.push_back(pos);
                }
            }

            // Submit mesh generation to background thread
            if let Some(worker) = &self.mesh_worker {
                if let Some(chunk) = self.chunks.get(&pos) {
                    let chunk_clone = chunk.clone();
                    worker.submit(pos, chunk_clone);
                }
            } else {
                // Fallback: build synchronously if no worker
                let chunk = self.chunks.get(&pos).unwrap();
                let mesh = ChunkMeshBuilder::new(ClassicGreedyMesher).build(chunk);
                self.meshes.insert(pos, mesh);
            }
        }
        self.chunks.get(&pos).unwrap()
    }

    /// Insert a chunk that was generated by the background worker.
    /// Does NOT trigger mesh generation (mesh is provided by the worker).
    pub fn insert_generated_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        if !self.chunks.contains_key(&pos) {
            let light_dirty = chunk.light_dirty;
            self.chunks.insert(pos, chunk);
            // Fill border slices from existing neighbors
            self.fill_borders(pos);
            // Update neighbors' borders and trigger remesh
            for neighbor_pos in &[
                ChunkPos(glam::IVec2::new(pos.0.x - 1, pos.0.y)),
                ChunkPos(glam::IVec2::new(pos.0.x + 1, pos.0.y)),
                ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y - 1)),
                ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y + 1)),
            ] {
                if self.chunks.contains_key(neighbor_pos) {
                    self.fill_borders(*neighbor_pos);
                    self.rebuild_mesh(*neighbor_pos);
                }
            }
            if light_dirty && self.light_dirty_set.insert(pos) {
                self.light_dirty_queue.push_back(pos);
            }
            self.chunk_keys_dirty = true;
        }
    }

    pub fn get_required_chunks(&self, player_pos: ChunkPos, render_distance: u32) -> Vec<ChunkPos> {
        let mut required = Vec::new();
        let rd = render_distance as i32;

        for x in (player_pos.0.x - rd)..=(player_pos.0.x + rd) {
            for z in (player_pos.0.y - rd)..=(player_pos.0.y + rd) {
                let pos = ChunkPos(glam::IVec2::new(x, z));
                if !self.chunks.contains_key(&pos) {
                    required.push(pos);
                }
            }
        }

        required
    }

    /// Unload chunks that are too far from the player.
    /// Returns the list of unloaded chunk positions for GPU buffer cleanup.
    pub fn unload_distant_chunks(&mut self, player_pos: ChunkPos, render_distance: u32) -> Vec<ChunkPos> {
        let rd = render_distance as i32 + 2;
        let mut removed = Vec::new();
        self.chunks.retain(|pos, _| {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            let keep = dx.abs() <= rd && dz.abs() <= rd;
            if !keep {
                removed.push(*pos);
            }
            keep
        });
        self.meshes.retain(|pos, _| {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            dx.abs() <= rd && dz.abs() <= rd
        });
        if !removed.is_empty() {
            self.chunk_keys_dirty = true;
        }
        removed
    }

    /// Propagate light for light-dirty chunks (throttled: max 4 per frame).
    pub fn propagate_light(&mut self) {
        // Process up to 4 chunks this frame
        let limit = self.light_dirty_queue.len().min(4);
        for _ in 0..limit {
            if let Some(pos) = self.light_dirty_queue.pop_front() {
                self.light_dirty_set.remove(&pos);
                if let Some(chunk) = self.chunks.get_mut(&pos)
                    && chunk.light_dirty
                {
                    propagate_all(chunk, &self.light_emission);
                    chunk.light_dirty = false;
                }
            }
        }
    }

    /// Rebuild mesh for a specific chunk (e.g., after block change).
    pub fn rebuild_mesh(&mut self, pos: ChunkPos) {
        if let Some(chunk) = self.chunks.get(&pos)
            && let Some(worker) = &self.mesh_worker
        {
            let chunk_clone = chunk.clone();
            worker.submit(pos, chunk_clone);
        }
    }

    pub fn break_block(&mut self, pos: BlockPos) -> Option<BlockId> {
        let (chunk_pos, lx, ly, lz) = pos.to_chunk_local()?;
        let chunk = self.chunks.get_mut(&chunk_pos)?;
        let old = chunk.get_block(lx, ly, lz);
        chunk.set_block(lx, ly, lz, BlockId::AIR);
        if chunk.light_dirty && self.light_dirty_set.insert(chunk_pos) {
            self.light_dirty_queue.push_back(chunk_pos);
        }
        self.chunk_keys_dirty = true;
        Some(old)
    }

    pub fn place_block(&mut self, pos: BlockPos, block: BlockId) {
        let Some((chunk_pos, lx, ly, lz)) = pos.to_chunk_local() else {
            return;
        };
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            chunk.set_block(lx, ly, lz, block);
            if chunk.light_dirty && self.light_dirty_set.insert(chunk_pos) {
                self.light_dirty_queue.push_back(chunk_pos);
            }
            self.chunk_keys_dirty = true;
        }
    }

    pub fn get_mesh(&self, pos: ChunkPos) -> Option<&MeshData> {
        self.meshes.get(&pos)
    }

    /// Returns `true` if the block at the given world position is water.
    pub fn is_water_at(&self, x: i32, y: i32, z: i32) -> bool {
        if y < 0 || y >= 256 {
            return false;
        }
        let Some((chunk_pos, lx, ly, lz)) = BlockPos(IVec3::new(x, y, z)).to_chunk_local() else {
            return false;
        };
        self.chunks
            .get(&chunk_pos)
            .map_or(false, |chunk| chunk.get_block(lx, ly, lz) == BlockId::WATER)
    }

    pub fn terrain_height_at(&self, x: i32, z: i32) -> f32 {
        self.terrain_gen.height_at(x, z)
    }

        /// Fill the border slices of `pos` from its loaded neighbors.
    pub fn fill_borders(&mut self, pos: ChunkPos) {
        // Collect neighbor positions first (avoids borrowing conflicts)
        let neighbor_positions = [
            ChunkPos(glam::IVec2::new(pos.0.x - 1, pos.0.y)),
            ChunkPos(glam::IVec2::new(pos.0.x + 1, pos.0.y)),
            ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y - 1)),
            ChunkPos(glam::IVec2::new(pos.0.x, pos.0.y + 1)),
        ];
        let faces = [
            border_face::NEG_X,
            border_face::POS_X,
            border_face::NEG_Z,
            border_face::POS_Z,
        ];

        for i in 0..4 {
            let neighbor_pos = neighbor_positions[i];
            let face = faces[i];
            // Copy neighbor's edge into a temporary vec to avoid borrow conflicts
            let border_slice = {
                let neighbor = self.chunks.get(&neighbor_pos);
                match neighbor {
                    Some(n) => {
                        let mut slice = vec![0u16; BORDER_SLICE_SIZE];
                        match face {
                            border_face::NEG_X | border_face::POS_X => {
                                let src_x = if face == border_face::NEG_X { CHUNK_WIDTH - 1 } else { 0 };
                                for z in 0..CHUNK_WIDTH {
                                    for y in 0..CHUNK_HEIGHT {
                                        slice[z + y * CHUNK_WIDTH] = n.get_block(src_x, y, z).0;
                                    }
                                }
                            }
                            _ => {
                                let src_z = if face == border_face::NEG_Z { CHUNK_DEPTH - 1 } else { 0 };
                                for x in 0..CHUNK_WIDTH {
                                    for y in 0..CHUNK_HEIGHT {
                                        slice[x + y * CHUNK_WIDTH] = n.get_block(x, y, src_z).0;
                                    }
                                }
                            }
                        }
                        slice
                    }
                    None => continue,
                }
            };

            // Now write the border data into the target chunk
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                for u in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let idx = face * BORDER_SLICE_SIZE + u + y * CHUNK_WIDTH;
                        chunk.border_blocks[idx] = border_slice[u + y * CHUNK_WIDTH];
                    }
                }
            }
        }
    }

    /// Check if an AABB overlaps any solid block in loaded chunks.
    /// Liquid blocks (water) are excluded — the player can pass through them.
    pub fn is_colliding(&self, aabb_min: Vec3, aabb_max: Vec3) -> bool {
        let min_x = aabb_min.x.floor() as i32;
        let min_y = aabb_min.y.floor() as i32;
        let min_z = aabb_min.z.floor() as i32;
        let max_x = aabb_max.x.floor() as i32;
        let max_y = aabb_max.y.floor() as i32;
        let max_z = aabb_max.z.floor() as i32;

        for bx in min_x..=max_x {
            for by in min_y..=max_y {
                if by < 0 || by >= 256 {
                    continue;
                }
                for bz in min_z..=max_z {
                    let world_pos = BlockPos(IVec3::new(bx, by, bz));
                    let Some((chunk_pos, lx, ly, lz)) = world_pos.to_chunk_local() else {
                        continue;
                    };
                    if let Some(chunk) = self.chunks.get(&chunk_pos) {
                        let block = chunk.get_block(lx, ly, lz);
                        if !block.is_air() && !block.is_liquid()
                            && aabb_min.x < (bx + 1) as f32
                            && aabb_max.x > bx as f32
                            && aabb_min.y < (by + 1) as f32
                            && aabb_max.y > by as f32
                            && aabb_min.z < (bz + 1) as f32
                            && aabb_max.z > bz as f32
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Raycast returning `(block_pos, face_normal)` where `face_normal`
    /// is the direction from the neighboring air block toward the hit block.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_dist: f32) -> Option<(BlockPos, IVec3)> {
        let dir = direction.normalize();
        let step = 0.1;
        let steps = (max_dist / step) as usize;
        let mut pos = origin;
        let mut prev_bpos: Option<IVec3> = None;

        for _ in 0..steps {
            pos += dir * step;
            let bx = pos.x.floor() as i32;
            let by = pos.y.floor() as i32;
            let bz = pos.z.floor() as i32;

            if !(0..256).contains(&by) {
                prev_bpos = Some(IVec3::new(bx, by, bz));
                continue;
            }

            let world_pos = BlockPos(IVec3::new(bx, by, bz));
            let Some((chunk_pos, lx, ly, lz)) = world_pos.to_chunk_local() else {
                prev_bpos = Some(IVec3::new(bx, by, bz));
                continue;
            };

            if let Some(chunk) = self.chunks.get(&chunk_pos) {
                let block = chunk.get_block(lx, ly, lz);
                if !block.is_air() && !block.is_liquid() {
                    let normal = match prev_bpos {
                        Some(p) => IVec3::new(bx - p.x, by - p.y, bz - p.z).signum(),
                        None => -dir.as_ivec3().signum(),
                    };
                    return Some((world_pos, normal));
                }
            }

            prev_bpos = Some(IVec3::new(bx, by, bz));
        }

        None
    }

}
