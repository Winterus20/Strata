use crate::frustum::Frustum;
use std::collections::HashMap;
use strata_core::ChunkPos;
use strata_meshing::MeshData;
use wgpu::{Buffer, Device, RenderPass};

/// GPU-side mesh data for a single chunk.
pub struct ChunkGpuMesh {
    pub vertex_buffer: Buffer,
    pub index_buffer: Buffer,
    pub index_count: u32,
}

/// Manages GPU mesh buffers for chunks and frustum culling.
pub struct ChunkRenderer {
    pub mesh_buffers: HashMap<ChunkPos, ChunkGpuMesh>,
    visible_chunks: Vec<ChunkPos>,
}

impl ChunkRenderer {
    pub fn new() -> Self {
        Self {
            mesh_buffers: HashMap::new(),
            visible_chunks: Vec::new(),
        }
    }

    /// Upload mesh data to GPU buffers.
    pub fn upload_mesh(&mut self, device: &Device, pos: ChunkPos, mesh: &MeshData) {
        use wgpu::util::DeviceExt;

        if mesh.is_empty() {
            tracing::warn!("Empty mesh for chunk {:?}, removing", pos);
            self.mesh_buffers.remove(&pos);
            return;
        }

        tracing::debug!(
            "Uploading chunk {:?}: {} verts, {} indices",
            pos, mesh.vertex_count, mesh.index_count
        );

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Vertex Buffer {:?}", pos)),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("Index Buffer {:?}", pos)),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        self.mesh_buffers.insert(
            pos,
            ChunkGpuMesh {
                vertex_buffer,
                index_buffer,
                index_count: mesh.index_count as u32,
            },
        );
    }

    /// Remove a chunk mesh from GPU.
    pub fn remove_mesh(&mut self, pos: ChunkPos) {
        self.mesh_buffers.remove(&pos);
    }

    /// Filter visible chunks using frustum culling.
    pub fn cull(&mut self, frustum: &Frustum, chunk_positions: &[ChunkPos]) {
        self.visible_chunks.clear();
        for pos in chunk_positions {
            if !self.mesh_buffers.contains_key(pos) {
                continue;
            }
            let wx = pos.0.x as f32 * 16.0;
            let wz = pos.0.y as f32 * 16.0;
            if frustum.test_chunk(wx, wz) {
                self.visible_chunks.push(*pos);
            }
        }
        tracing::trace!(
            "Cull: {} total, {} visible",
            self.mesh_buffers.len(),
            self.visible_chunks.len()
        );
    }

    /// Draw all visible chunks.
    pub fn render(&self, render_pass: &mut RenderPass) {
        for pos in &self.visible_chunks {
            if let Some(gpu_mesh) = self.mesh_buffers.get(pos) {
                render_pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(gpu_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..gpu_mesh.index_count, 0, 0..1);
            }
        }
    }

    pub fn visible_count(&self) -> usize {
        self.visible_chunks.len()
    }

    pub fn total_count(&self) -> usize {
        self.mesh_buffers.len()
    }
}

impl Default for ChunkRenderer {
    fn default() -> Self {
        Self::new()
    }
}
