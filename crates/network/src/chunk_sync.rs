use serde::{Deserialize, Serialize};
use strata_core::chunk::{Chunk, ChunkPos};
use strata_core::light::LightData;
use glam::IVec2;
use std::collections::{HashSet, VecDeque};

/// Network-friendly snapshot of a chunk (serde-compatible).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkSnapshot {
    pub x: i32,
    pub z: i32,
    pub blocks: Vec<u16>,
    pub heightmap_top: Vec<u16>,
    pub heightmap_bottom: Vec<u16>,
    pub sky_light: Vec<u8>,
    pub block_light: Vec<u8>,
}

impl ChunkSnapshot {
    pub fn from_chunk(chunk: &Chunk) -> Self {
        Self {
            x: chunk.position.0.x,
            z: chunk.position.0.y,
            blocks: chunk.blocks.clone(),
            heightmap_top: chunk.heightmap_top.to_vec(),
            heightmap_bottom: chunk.heightmap_bottom.to_vec(),
            sky_light: chunk.light.sky_light.to_vec(),
            block_light: chunk.light.block_light.to_vec(),
        }
    }

    pub fn into_chunk(self) -> Chunk {
        let mut chunk = Chunk::new(ChunkPos(IVec2::new(self.x, self.z)));
        chunk.blocks = self.blocks;

        if self.heightmap_top.len() == 256 {
            let mut arr = [0u16; 256];
            arr.copy_from_slice(&self.heightmap_top);
            chunk.heightmap_top = arr;
        }
        if self.heightmap_bottom.len() == 256 {
            let mut arr = [0u16; 256];
            arr.copy_from_slice(&self.heightmap_bottom);
            chunk.heightmap_bottom = arr;
        }

        let mut sky = [0u8; 32768];
        let mut block = [0u8; 32768];
        let copy_len = self.sky_light.len().min(32768);
        sky[..copy_len].copy_from_slice(&self.sky_light[..copy_len]);
        let copy_len = self.block_light.len().min(32768);
        block[..copy_len].copy_from_slice(&self.block_light[..copy_len]);

        chunk.light = LightData::from_raw(sky, block);
        chunk.dirty = false;
        chunk.light_dirty = false;
        chunk
    }
}

pub fn compress_chunk(snapshot: &ChunkSnapshot) -> Result<Vec<u8>, std::io::Error> {
    let bytes = postcard::to_allocvec(snapshot).map_err(|e| {
        std::io::Error::other(e)
    })?;
    zstd::encode_all(bytes.as_slice(), 3)
}

pub fn decompress_chunk(data: &[u8]) -> Result<ChunkSnapshot, std::io::Error> {
    let decompressed = zstd::decode_all(data)?;
    let snapshot: ChunkSnapshot = postcard::from_bytes(&decompressed).map_err(|e| {
        std::io::Error::other(e)
    })?;
    Ok(snapshot)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkRequestPacket {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkResponsePacket {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub compressed_data: Vec<u8>,
}

pub struct ChunkRequestManager {
    pub requested: HashSet<IVec2>,
    pub queue: VecDeque<IVec2>,
    pub chunks_per_frame: u8,
    pub view_distance: u8,
}

impl ChunkRequestManager {
    pub fn new(view_distance: u8) -> Self {
        Self {
            requested: HashSet::new(),
            queue: VecDeque::new(),
            chunks_per_frame: 2,
            view_distance,
        }
    }

    pub fn update_view(&mut self, player_chunk: IVec2) {
        let radius = self.view_distance as i32;
        let mut candidates = Vec::new();

        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let pos = IVec2::new(player_chunk.x + dx, player_chunk.y + dz);
                if !self.requested.contains(&pos) {
                    candidates.push((pos, dx.abs() + dz.abs()));
                }
            }
        }

        candidates.sort_by_key(|&(_, dist)| dist);
        for (pos, _) in candidates {
            self.queue.push_back(pos);
        }
    }

    pub fn poll_requests(&mut self) -> Vec<ChunkRequestPacket> {
        let count = self.chunks_per_frame.min(self.queue.len() as u8) as usize;
        (0..count)
            .filter_map(|_| self.queue.pop_front())
            .map(|pos| ChunkRequestPacket {
                chunk_x: pos.x,
                chunk_z: pos.y,
            })
            .collect()
    }

    pub fn mark_received(&mut self, x: i32, z: i32) {
        self.requested.insert(IVec2::new(x, z));
    }
}
