//! GPU Compute Shader meshing — wgpu compute pipeline
//!
//! Dispatches a compute shader that generates mesh data (vertices + indices)
//! for a chunk, then reads back the results via staging buffers.
//! Per-face generation (no greedy merge) — targets <50µs/chunk.

use crate::mesher::{BoundingBox, MeshData, Mesher, Vertex};
use strata_core::{CHUNK_VOLUME, Chunk};
use wgpu::Buffer;

/// GPU compute mesher that produces mesh data via a WGSL compute shader.
pub struct GpuComputeMesher {
    device: wgpu::Device,
    queue: wgpu::Queue,
    compute_pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,

    /// Storage buffer for voxel input (65536 × u16).
    voxel_buffer: Buffer,
    /// Storage buffer for vertex output.
    vertex_buffer: Buffer,
    /// Storage buffer for index output.
    index_buffer: Buffer,
    /// Storage buffer for atomic counters (vertex_count + index_count).
    counter_buffer: Buffer,
    /// Uniform buffer for chunk offset (ox: f32, oz: f32).
    offset_buffer: Buffer,

    /// Staging buffer for vertex readback.
    staging_vertex: Buffer,
    /// Staging buffer for index readback.
    staging_index: Buffer,
    /// Staging buffer for counter readback.
    staging_counter: Buffer,
}

impl GpuComputeMesher {
    const MAX_VERT_U32: u64 = 524288;
    const MAX_IDX_U32: u64 = 786432;
    const STRIDE_U32: u64 = 10;

    /// Creates a new GPU compute mesher with the given device and queue.
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Mesher"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../render/src/shaders/compute_mesher.wgsl").into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Compute Mesher BGL"),
            entries: &[
                // 0: voxel_input (storage, read)
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 1: vertex_output (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 2: index_output (storage, read_write)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3: counters (storage, read_write) — vertex_count + index_count atomics
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 4: chunk_offset (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compute Mesher Pipeline Layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Compute Mesher Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let voxel_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Voxel Input"),
            size: (CHUNK_VOLUME * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Vertex Output"),
            size: Self::MAX_VERT_U32 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Index Output"),
            size: Self::MAX_IDX_U32 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Counters"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let offset_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Chunk Offset"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let staging_vertex = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Staging Vertex"),
            size: Self::MAX_VERT_U32 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let staging_index = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Staging Index"),
            size: Self::MAX_IDX_U32 * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let staging_counter = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Staging Counter"),
            size: 16,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            device,
            queue,
            compute_pipeline,
            bind_group_layout,
            voxel_buffer,
            vertex_buffer,
            index_buffer,
            counter_buffer,
            offset_buffer,
            staging_vertex,
            staging_index,
            staging_counter,
        }
    }
}

impl Mesher for GpuComputeMesher {
    fn generate_mesh(&self, chunk: &Chunk) -> MeshData {
        // 1. Reset counters to 0
        self.queue.write_buffer(&self.counter_buffer, 0, &[0u8; 16]);

        // 2. Upload chunk voxel data (u16 → u32 conversion for WGSL compatibility)
        let voxel_u32: Vec<u32> = chunk.as_slice().iter().map(|&v| v as u32).collect();
        self.queue
            .write_buffer(&self.voxel_buffer, 0, bytemuck::cast_slice(&voxel_u32));

        // 3. Upload chunk offset
        let offset_data: [f32; 2] = [
            chunk.position.world_x() as f32,
            chunk.position.world_z() as f32,
        ];
        self.queue
            .write_buffer(&self.offset_buffer, 0, bytemuck::cast_slice(&offset_data));

        // 4. Create bind group
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Compute Mesher BG"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.voxel_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.vertex_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.index_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.counter_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.offset_buffer.as_entire_binding(),
                },
            ],
        });

        // 5. Dispatch compute
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Compute Mesher Encoder"),
            });

        {
            let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Compute Mesher Pass"),
                timestamp_writes: None,
            });
            cpass.set_pipeline(&self.compute_pipeline);
            cpass.set_bind_group(0, Some(&bind_group), &[]);
            cpass.dispatch_workgroups(1024, 1, 1);
        }

        // 6. Copy results to staging
        encoder.copy_buffer_to_buffer(&self.counter_buffer, 0, &self.staging_counter, 0, 16);
        encoder.copy_buffer_to_buffer(
            &self.vertex_buffer,
            0,
            &self.staging_vertex,
            0,
            Self::MAX_VERT_U32 * 4,
        );
        encoder.copy_buffer_to_buffer(
            &self.index_buffer,
            0,
            &self.staging_index,
            0,
            Self::MAX_IDX_U32 * 4,
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // 7. Read back counters
        let cs = self.staging_counter.slice(..);
        cs.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::wgt::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let cv = cs.get_mapped_range();
        let vertex_count = u32::from_ne_bytes([cv[0], cv[1], cv[2], cv[3]]);
        let index_count = u32::from_ne_bytes([cv[4], cv[5], cv[6], cv[7]]);
        drop(cv);
        self.staging_counter.unmap();

        if vertex_count == 0 || index_count == 0 {
            return MeshData::empty();
        }

        let actual_verts = vertex_count as usize;
        let actual_indices = index_count as usize;

        // 8. Read back vertices
        let vert_bytes =
            (actual_verts * Self::STRIDE_U32 as usize * 4).min((Self::MAX_VERT_U32 * 4) as usize);
        let vs = self.staging_vertex.slice(..vert_bytes as u64);
        vs.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::wgt::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let vv = vs.get_mapped_range();

        let mut vertices = Vec::with_capacity(actual_verts);
        for i in 0..actual_verts {
            let base = i * Self::STRIDE_U32 as usize * 4;
            if base + 40 > vv.len() {
                break;
            }
            let pos_x = f32::from_ne_bytes([vv[base], vv[base + 1], vv[base + 2], vv[base + 3]]);
            let pos_y =
                f32::from_ne_bytes([vv[base + 4], vv[base + 5], vv[base + 6], vv[base + 7]]);
            let pos_z =
                f32::from_ne_bytes([vv[base + 8], vv[base + 9], vv[base + 10], vv[base + 11]]);
            let n_x =
                f32::from_ne_bytes([vv[base + 12], vv[base + 13], vv[base + 14], vv[base + 15]]);
            let n_y =
                f32::from_ne_bytes([vv[base + 16], vv[base + 17], vv[base + 18], vv[base + 19]]);
            let n_z =
                f32::from_ne_bytes([vv[base + 20], vv[base + 21], vv[base + 22], vv[base + 23]]);
            let u =
                f32::from_ne_bytes([vv[base + 24], vv[base + 25], vv[base + 26], vv[base + 27]]);
            let v =
                f32::from_ne_bytes([vv[base + 28], vv[base + 29], vv[base + 30], vv[base + 31]]);
            let _ao =
                f32::from_ne_bytes([vv[base + 32], vv[base + 33], vv[base + 34], vv[base + 35]]);
            let tex_id = u16::from_ne_bytes([vv[base + 36], vv[base + 37]]);

            vertices.push(Vertex {
                position: [pos_x, pos_y, pos_z],
                normal: [n_x, n_y, n_z],
                uv: [u, v],
                ao: _ao,
                texture_id: tex_id,
                _padding: 0,
            });
        }
        drop(vv);
        self.staging_vertex.unmap();

        // 9. Read back indices
        let idx_bytes = (actual_indices * 4).min((Self::MAX_IDX_U32 * 4) as usize);
        let is = self.staging_index.slice(..idx_bytes as u64);
        is.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::wgt::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        let iv = is.get_mapped_range();

        let mut indices = Vec::with_capacity(actual_indices);
        for i in 0..actual_indices {
            let base = i * 4;
            if base + 4 > iv.len() {
                break;
            }
            let idx = u32::from_ne_bytes([iv[base], iv[base + 1], iv[base + 2], iv[base + 3]]);
            indices.push(idx);
        }
        drop(iv);
        self.staging_index.unmap();

        let bounds = BoundingBox {
            min: glam::Vec3::new(
                chunk.position.world_x() as f32,
                0.0,
                chunk.position.world_z() as f32,
            ),
            max: glam::Vec3::new(
                (chunk.position.world_x() + 16) as f32,
                256.0,
                (chunk.position.world_z() + 16) as f32,
            ),
        };

        MeshData {
            vertex_count: vertices.len(),
            index_count: indices.len(),
            vertices,
            indices,
            bounds,
        }
    }

    fn name(&self) -> &str {
        "gpu_compute"
    }
}
