# Phase 2 Implementation Plan — Strata (Faz 2: Render & Işıklandırma)

**Süre:** 4 Hafta (Hafta 5-8)
**Hedef:** Full render pipeline, BFS light propagation, frustum culling, GPU compute shader meshing, Texture2DArray rendering, lazy loading, debug overlay

---

## Özet: Faz 1 → Faz 2 Geçişi

Faz 1'de mevcut olan altyapı:
- `core` crate: Block registry, Chunk (Vec<u16>), World coordinates ✅
- `ecs` crate: Bevy ECS components (Position, ChunkPosition, Player, BlockBreakEvent) ✅
- `world-gen` crate: fastnoise2 FBM terrain generation ✅
- `meshing` crate: `Mesher` trait + `MeshData`, `ClassicGreedyMesher` (CPU) ✅
- `storage` crate: Binary+zstd format, RegionManager, LRU cache ✅
- `physics` crate: bevy_rapier plugin, AABB, collision, raycast ✅
- `bin/client`: winit 0.30 + wgpu minimal pipeline, camera, input, block break/place ✅

Faz 2'de yapılacaklar:
- `render` crate → wgpu pipeline'ı crate'e taşı + Texture2DArray + frustum culling
- `lighting` crate → BFS light propagation (yeni crate)
- `meshing` crate → GPU compute shader meshing ekle (`gpu_compute.rs` + `compute_mesher.wgsl`)
- `bin/client` → debug overlay, lazy loading, dirty-flag throttling

---

## Hafta 5: `render` Crate — Full Render Pipeline

### Gün 1-2: Workspace + Crate Yapısı

#### 5.1.1. Root Cargo.toml Güncellemesi

```toml
# Cargo.toml — workspace members'e ekle
members = [
    "crates/core",
    "crates/ecs",
    "crates/world-gen",
    "crates/meshing",
    "crates/storage",
    "crates/physics",
    "crates/render",      # YENİ
    "crates/lighting",    # YENİ
    "bin/client",
]
```

#### 5.1.2. `render` Crate Dizin Yapısı

```
crates/render/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public API
    ├── engine.rs           # wgpu engine initialization (mevcut RenderState'ten ayrıştır)
    ├── pipeline.rs         # Render pipeline setup + shader module management
    ├── camera.rs           # Camera + uniform buffer (bin/client/camera.rs'den taşı)
    ├── frustum.rs          # Frustum culling implementation
    ├── chunk_renderer.rs   # Chunk mesh upload + batch draw
    ├── texture_manager.rs  # Texture2DArray loading + bind group
    └── shaders/
        ├── chunk.wgsl      # Ana vertex/fragment shader
        ├── chunk_textured.wgsl  # Texture2DArray variant
        └── compute_mesher.wgsl  # Faz 2 GPU compute shader
```

#### 5.1.3. `crates/render/Cargo.toml`

```toml
[package]
name = "strata-render"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
strata-meshing = { path = "../meshing" }
strata-ecs = { path = "../ecs" }
glam.workspace = true
wgpu.workspace = true
winit.workspace = true
tracing.workspace = true
anyhow.workspace = true
bytemuck = { version = "1", features = ["derive"] }
image = "0.25"  # Texture loading
```

**Not:** `winit` workspace dependency'i mevcut. `image` crate'i Texture2DArray yükleme için eklendi.

---

### Gün 2-3: Texture Manager — Texture2DArray

#### 5.2.1. `texture_manager.rs` — Block Texture Atlas

```rust
use wgpu::{Device, Queue, Texture, TextureView, Sampler, BindGroup, BindGroupLayout};
use image::GenericImageView;
use strata_core::BlockRegistry;
use std::collections::HashMap;

/// Manages Texture2DArray for block textures
/// 
/// Her block tipi için 6 yüz texture'ı (top, bottom, front, back, left, right)
/// Texture2DArray'de layer olarak saklanır.
/// Block.texture_index → array layer index
pub struct TextureManager {
    pub texture: Texture,
    pub texture_view: TextureView,
    pub sampler: Sampler,
    pub bind_group: BindGroup,
    pub bind_group_layout: BindGroupLayout,
    texture_count: u32,
    texture_size: u32,  // 16x16 (pixel)
}

impl TextureManager {
    /// Load all block textures from assets/textures/
    /// Her block için: {block_name}_top.png, _{block_name}_bottom.png, etc.
    /// Varsayılan olmayan yüzler için fallback: {block_name}.png (all faces)
    pub async fn new(device: &Device, queue: &Queue, registry: &BlockRegistry) -> Self {
        // 1. Assets/textures dizinini tara
        // 2. Her block için texture yükle (veya placeholder generate et)
        // 3. Texture2DArray oluştur (layer = block count × 6)
        // 4. Sampler + BindGroup oluştur
        // 5. Her block'a texture_index ata (layer offset)
        todo!("Implement texture atlas loading")
    }

    /// Get bind group layout for shader binding
    pub fn create_bind_group_layout(device: &Device) -> BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Array Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }
}
```

**Texture2DArray Vertex Attribute Güncellemesi:**
```rust
// Vertex yapısına texture_layer_index eklenmeyecek — texture_id zaten u16
// Shader'da: texture_id & 0x00FF = layer_index, texture_id >> 8 = face_index
// Bu sayede ek vertex attribute'a gerek kalmaz
```

---

### Gün 3-4: Camera Taşıma + Uniform Buffer Refactor

#### 5.3.1. `camera.rs` — Render Crate'ine Taşı

```rust
use glam::{Mat4, Vec3};

pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(aspect: f32) -> Self { /* ... */ }
    pub fn view_matrix(&self) -> Mat4 { /* ... */ }
    pub fn projection_matrix(&self) -> Mat4 { /* ... */ }
    
    /// ViewProjection matrix for shader uniform
    pub fn view_projection_matrix(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }
    
    /// Frustum corners in world space (for culling)
    pub fn frustum_corners(&self) -> [Vec3; 8] {
        // Extract frustum corners from VP matrix inverse
        // Used by Frustum::update()
        todo!("Extract frustum corners from view-projection matrix")
    }
}
```

#### 5.3.2. `pipeline.rs` — Shader Pipeline Yönetimi

```rust
use wgpu::{Device, RenderPipeline, ShaderModule, PipelineLayout, BindGroupLayout};
use strata_meshing::Vertex;

pub struct RenderPipelineManager {
    pub chunk_pipeline: RenderPipeline,
    pub chunk_layout: PipelineLayout,
    pub uniform_bind_group_layout: BindGroupLayout,
}

impl RenderPipelineManager {
    pub fn new(device: &Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Chunk Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/chunk_textured.wgsl").into()
            ),
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Uniform BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let texture_bind_group_layout = TextureManager::create_bind_group_layout(device);
        
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Chunk Pipeline Layout"),
            bind_group_layouts: &[
                Some(&uniform_bind_group_layout),
                Some(&texture_bind_group_layout),  // BindGroup(0) = VP matrix, BindGroup(1) = textures
            ],
            immediate_size: 0,
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Chunk Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,  // position
                        1 => Float32x3,  // normal
                        2 => Float32x2,  // uv
                        3 => Float32,    // ao
                        4 => Uint32,     // texture_id (packed: layer|face)
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            chunk_pipeline: render_pipeline,
            chunk_layout: pipeline_layout,
            uniform_bind_group_layout,
        }
    }
}
```

---

### Gün 4-5: Frustum Culling

#### 5.4.1. `frustum.rs` — View Frustum Culling

```rust
use glam::{Mat4, Vec3, Vec4};

/// 6-plane view frustum for chunk visibility testing
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    planes: [Vec4; 6],  // Left, Right, Bottom, Top, Near, Far
}

impl Frustum {
    /// Extract frustum planes from view-projection matrix
    /// Planes are in Hessian Normal Form: plane.x * x + plane.y * y + plane.z * z + plane.w = 0
    pub fn from_view_projection(vp: Mat4) -> Self {
        let rows = vp.to_cols_array_2d();
        
        // Extract planes from VP matrix rows
        // Left:   row3 + row0
        // Right:  row3 - row0
        // Bottom: row3 + row1
        // Top:    row3 - row1
        // Near:   row3 + row2
        // Far:    row3 - row2
        fn plane_from_rows(r0: [f32; 4], r1: [f32; 4]) -> Vec4 {
            Vec4::new(
                r1[0] + r0[0],
                r1[1] + r0[1],
                r1[2] + r0[2],
                r1[3] + r0[3],
            )
        }
        
        let left   = plane_from_rows(rows[0], rows[3]);
        let right  = plane_from_rows([-rows[0][0], -rows[0][1], -rows[0][2], -rows[0][3]], rows[3]);
        let bottom = plane_from_rows(rows[1], rows[3]);
        let top    = plane_from_rows([-rows[1][0], -rows[1][1], -rows[1][2], -rows[1][3]], rows[3]);
        let near   = plane_from_rows(rows[2], rows[3]);
        let far    = plane_from_rows([-rows[2][0], -rows[2][1], -rows[2][2], -rows[2][3]], rows[3]);
        
        // Normalize planes
        let normalize = |p: Vec4| -> Vec4 {
            let len = p.truncate().length();
            if len > 0.0 { p / len } else { p }
        };
        
        Self {
            planes: [
                normalize(left), normalize(right),
                normalize(bottom), normalize(top),
                normalize(near), normalize(far),
            ],
        }
    }
    
    /// Test AABB against all 6 planes
    /// Returns true if the AABB is partially or fully inside the frustum
    pub fn test_aabb(&self, center: Vec3, half_extents: Vec3) -> bool {
        for &plane in &self.planes {
            // Compute signed distance of AABB center from plane
            let dist = plane.x * center.x + plane.y * center.y + plane.z * center.z + plane.w;
            
            // Compute projection radius of AABB onto plane normal
            let radius = half_extents.x * plane.x.abs() 
                       + half_extents.y * plane.y.abs() 
                       + half_extents.z * plane.z.abs();
            
            // If distance > radius, AABB is entirely outside this plane
            if dist > radius {
                return false;
            }
        }
        true // AABB intersects frustum
    }
    
    /// Test chunk visibility using its position and bounds
    pub fn test_chunk(&self, chunk_world_x: f32, chunk_world_z: f32) -> bool {
        let center = Vec3::new(
            chunk_world_x + 8.0,  // CHUNK_WIDTH/2
            128.0,                 // CHUNK_HEIGHT/2
            chunk_world_z + 8.0,  // CHUNK_DEPTH/2
        );
        let half_extents = Vec3::new(8.0, 128.0, 8.0);
        self.test_aabb(center, half_extents)
    }
}
```

**Culling Stratejisi:**
- Her frame'de frustum yeniden hesaplanır (kamera değiştiyse)
- Tüm loaded chunk'lar test edilir
- Sadece frustum içindeki chunk'lar render edilir
- Culling maliyeti: <1µs/1000 chunk (6 plane dot product)

---

### Gün 5: Chunk Renderer — Batch Rendering

#### 5.5.1. `chunk_renderer.rs`

```rust
use std::collections::HashMap;
use strata_core::ChunkPos;
use strata_meshing::MeshData;
use wgpu::{Device, Queue, Buffer, RenderPass};

pub struct ChunkGpuMesh {
    pub vertex_buffer: Buffer,
    pub index_buffer: Buffer,
    pub index_count: u32,
}

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

    /// Upload mesh data to GPU
    pub fn upload_mesh(&mut self, device: &Device, pos: ChunkPos, mesh: &MeshData) {
        if mesh.is_empty() {
            self.mesh_buffers.remove(&pos);
            return;
        }
        // wgpu buffer oluşturma (mevcut render.rs'deki upload_mesh mantığı)
        todo!("Buffer creation from MeshData")
    }

    /// Remove mesh from GPU
    pub fn remove_mesh(&mut self, pos: ChunkPos) {
        self.mesh_buffers.remove(&pos);
    }

    /// Filter visible chunks using frustum
    pub fn cull(&mut self, frustum: &crate::frustum::Frustum, chunk_positions: &[ChunkPos]) {
        self.visible_chunks.clear();
        for pos in chunk_positions {
            let world_x = pos.world_x() as f32;
            let world_z = pos.world_z() as f32;
            if frustum.test_chunk(world_x, world_z) {
                self.visible_chunks.push(*pos);
            }
        }
    }

    /// Draw all visible chunks
    pub fn render(&self, render_pass: &mut RenderPass) {
        for pos in &self.visible_chunks {
            if let Some(gpu_mesh) = self.mesh_buffers.get(pos) {
                render_pass.set_vertex_buffer(0, gpu_mesh.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    gpu_mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
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
```

---

### Gün 5-6: `engine.rs` — Wgpu Engine (RenderState Refactor)

```rust
use std::sync::Arc;
use wgpu::{Device, Queue, Surface, SurfaceConfiguration};
use winit::window::Window;
use crate::pipeline::RenderPipelineManager;
use crate::camera::Camera;
use crate::chunk_renderer::ChunkRenderer;
use crate::texture_manager::TextureManager;
use crate::frustum::Frustum;
use strata_core::BlockRegistry;

pub struct RenderEngine {
    pub surface: Surface<'static>,
    pub device: Device,
    pub queue: Queue,
    pub config: SurfaceConfiguration,
    pub pipeline_manager: RenderPipelineManager,
    pub texture_manager: TextureManager,
    pub chunk_renderer: ChunkRenderer,
    pub camera: Camera,
    pub frustum: Frustum,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    pub depth_texture: wgpu::TextureView,
}

impl RenderEngine {
    pub async fn new(
        window: Arc<Window>,
        registry: &BlockRegistry,
    ) -> anyhow::Result<Self> {
        // 1. Instance + Surface + Adapter + Device
        // 2. Surface configuration
        // 3. Depth texture
        // 4. Pipeline manager
        // 5. Texture manager
        // 6. Uniform buffer + bind group
        // 7. Camera + Frustum
        // 8. Chunk renderer
        todo!("Full engine initialization")
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_texture = Self::create_depth_texture(&self.device, &self.config);
        self.camera.aspect = width as f32 / height as f32;
    }

    pub fn update_camera(&mut self) {
        self.frustum = Frustum::from_view_projection(self.camera.view_projection_matrix());
        // Update uniform buffer
        let vp = self.camera.view_projection_matrix();
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&vp.to_cols_array()),
        );
    }

    pub fn render_frame(&mut self) {
        // 1. Get surface texture
        // 2. Begin render pass (color + depth)
        // 3. Set pipeline + bind groups
        // 4. chunk_renderer::render()
        // 5. End pass + submit
        todo!("Full render frame")
    }
}
```

---

### Gün 6-7: Textured WGSL Shader

#### 5.6.1. `shaders/chunk_textured.wgsl`

```wgsl
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) ao: f32,
    @location(4) packed_id: u32,  // texture_id packed: lower 16 bits = layer, upper 16 bits = face
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) ao: f32,
    @location(3) layer_index: u32,
};

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@group(1) @binding(0)
var texture_array: texture_2d_array<f32>;

@group(1) @binding(1)
var texture_sampler: sampler;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_pos = view_proj * vec4<f32>(input.position, 1.0);
    output.normal = input.normal;
    output.uv = input.uv;
    output.ao = input.ao;
    output.layer_index = input.packed_id & 0xFFu;  // Lower 8 bits = texture layer
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Sample texture from Texture2DArray
    let tex_color = textureSample(texture_array, texture_sampler, input.uv, f32(input.layer_index));
    
    // Basic diffuse lighting
    let light_dir = normalize(vec3<f32>(0.3, 1.0, 0.5));
    let diffuse = max(dot(normalize(input.normal), light_dir), 0.0);
    let ambient = 0.3;
    
    let final_color = tex_color.rgb * (ambient + diffuse * 0.7) * input.ao;
    return vec4<f32>(final_color, tex_color.a);
}
```

#### 5.6.2. **Faz 1 Shader'ı Koru** → `shaders/chunk.wgsl` (fallback, textured variant henüz yoksa)

---

### Gün 7: Doğrulama

```bash
# Render crate test
cargo test -p strata-render

# Tüm workspace
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo fmt
```

**Milestone 4:** Render pipeline crate'e taşındı, Texture2DArray hazır, frustum culling çalışıyor.

---

## Hafta 6: `lighting` Crate + Light Propagation

### Gün 8-9: `lighting` Crate Yapısı

#### 6.1.1. `crates/lighting/Cargo.toml`

```toml
[package]
name = "strata-lighting"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
strata-ecs = { path = "../ecs" }
bevy_ecs.workspace = true
bevy_app.workspace = true
glam.workspace = true
tracing.workspace = true
```

#### 6.1.2. Dizin Yapısı

```
crates/lighting/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public API + LightPlugin
    ├── sunlight.rs         # Sky light propagation (BFS)
    ├── block_light.rs      # Block light propagation (torch, lava, etc.)
    └── propagate.rs        # BFS flood-fill algorithm
```

---

### Gün 9-10: Light Data Structure

#### 6.2.1. Sunlight + Blocklight Arrays

Her chunk kendi light data'sını tutar. `Chunk` yapısına eklenir:

```rust
// crates/core/src/chunk.rs — LightData field ekle
use crate::light::LightData;

pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: Vec<u16>,
    pub light: LightData,              // YENİ
    pub heightmap_top: [u16; 256],
    pub heightmap_bottom: [u16; 256],
    pub dirty: bool,
}
```

```rust
// crates/lighting/src/lib.rs — LightData struct
use bevy_ecs::prelude::*;

/// Per-chunk light data
/// 2 bytes per voxel: 4 bits sky + 4 bits block = 128 KB/chunk
pub struct LightData {
    pub sky_light: Box<[u8; 65536]>,    // 4 bits packed per voxel, 2 voxels per byte
    pub block_light: Box<[u8; 65536]>,  // 4 bits packed per voxel
}

impl LightData {
    pub fn new() -> Self {
        Self {
            sky_light: Box::new([15u8; 65536]),   // Varsayılan: max sky light
            block_light: Box::new([0u8; 65536]),  // Varsayılan: no block light
        }
    }

    #[inline]
    pub fn get_sky(&self, index: usize) -> u8 {
        if index & 1 == 0 {
            self.sky_light[index >> 1] & 0x0F
        } else {
            self.sky_light[index >> 1] >> 4
        }
    }

    #[inline]
    pub fn set_sky(&mut self, index: usize, value: u8) {
        let value = value.min(15);
        if index & 1 == 0 {
            let byte = self.sky_light[index >> 1];
            self.sky_light[index >> 1] = (byte & 0xF0) | value;
        } else {
            let byte = self.sky_light[index >> 1];
            self.sky_light[index >> 1] = (byte & 0x0F) | (value << 4);
        }
    }

    #[inline]
    pub fn get_block(&self, index: usize) -> u8 {
        if index & 1 == 0 {
            self.block_light[index >> 1] & 0x0F
        } else {
            self.block_light[index >> 1] >> 4
        }
    }

    #[inline]
    pub fn set_block(&mut self, index: usize, value: u8) {
        let value = value.min(15);
        if index & 1 == 0 {
            let byte = self.block_light[index >> 1];
            self.block_light[index >> 1] = (byte & 0xF0) | value;
        } else {
            let byte = self.block_light[index >> 1];
            self.block_light[index >> 1] = (byte & 0x0F) | (value << 4);
        }
    }
}
```

**Neden bit-packed:** 2 byte/voxel → 128 KB/chunk yerine 1 byte/voxel → 64 KB/chunk. 100 chunk'ta 6.4 MB tasarruf.

---

### Gün 10-11: Sunlight Propagation (BFS)

#### 6.3.1. `sunlight.rs`

```rust
use strata_core::{Chunk, CHUNK_HEIGHT, CHUNK_WIDTH};
use crate::LightData;

/// Sky light propagation using BFS from top
/// Algoritma (Minecraft-style):
/// 1. Transparent olmayan bloklar ışığı geçirmez (opacity = 15 - light level)
/// 2. Sky light top-down: her column'da en üstteki non-transparent block'a kadar 15
/// 3. BFS: her komşuya current - 1
/// 4. Semi-transparent bloklar (yaprak, su) ışığı azaltır (light - opacity)

const MAX_LIGHT: u8 = 15;

pub struct SunlightPropagator;

impl SunlightPropagator {
    /// Initialize sunlight for a newly generated chunk
    pub fn init(chunk: &Chunk, light: &mut LightData) {
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let top = chunk.heightmap_top[col] as usize;
                
                // Above heightmap = full sky light
                for y in (top + 1)..CHUNK_HEIGHT {
                    let idx = Chunk::index(x, y, z);
                    light.set_sky(idx, MAX_LIGHT);
                }
                
                // Below heightmap = propagate through transparent blocks
                if top > 0 {
                    Self::propagate_column(chunk, light, x, z, top);
                }
            }
        }
    }
    
    /// Propagate sky light down through a single column
    fn propagate_column(chunk: &Chunk, light: &mut LightData, x: usize, z: usize, start_y: usize) {
        let mut current_light = MAX_LIGHT;
        for y in (0..=start_y).rev() {
            let idx = Chunk::index(x, y, z);
            let block = chunk.get_block(x, y, z);
            
            if block.is_air() {
                light.set_sky(idx, current_light);
                current_light = current_light.saturating_sub(1);
            } else {
                // Non-air block: check transparency
                // TODO: Use BlockRegistry for transparency info
                light.set_sky(idx, 0);
                if current_light > 0 {
                    current_light = current_light.saturating_sub(1);
                }
            }
            
            if current_light == 0 { break; }
        }
    }
    
    /// BFS propagation after block change
    /// 1. Clear affected area
    /// 2. Queue light sources (sky-exposed positions)
    /// 3. Flood fill BFS
    pub fn propagate_bfs(chunk: &mut Chunk, light: &mut LightData) {
        let mut queue = Vec::new();
        let mut visited = vec![false; 65536];
        
        // Enqueue all sky-exposed positions (top of chunk + transparent neighbors)
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let top = chunk.heightmap_top[col] as usize;
                let y = CHUNK_HEIGHT - 1;
                let idx = Chunk::index(x, y, z);
                queue.push((x, y, z, MAX_LIGHT));
                visited[idx] = true;
            }
        }
        
        // BFS flood fill
        while let Some((x, y, z, level)) = queue.pop() {
            let idx = Chunk::index(x, y, z);
            light.set_sky(idx, level);
            
            if level == 0 { continue; }
            
            // Neighbors (6 directions)
            for (dx, dy, dz) in &[(0, -1, 0), (0, 1, 0), (-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let nz = z as i32 + dz;
                
                if nx < 0 || nx >= CHUNK_WIDTH as i32 || ny < 0 || ny >= CHUNK_HEIGHT as i32 || nz < 0 || nz >= CHUNK_WIDTH as i32 {
                    continue;
                }
                
                let nidx = Chunk::index(nx as usize, ny as usize, nz as usize);
                if visited[nidx] { continue; }
                
                let neighbor_block = chunk.get_block(nx as usize, ny as usize, nz as usize);
                let opacity = if neighbor_block.is_air() { 1 } else { 15 }; // TODO: real opacity
                
                if level > opacity {
                    let new_level = level - opacity;
                    queue.push((nx as usize, ny as usize, nz as usize, new_level));
                    visited[nidx] = true;
                }
            }
        }
    }
}
```

---

### Gün 11-12: Block Light Propagation

#### 6.4.1. `block_light.rs`

```rust
use strata_core::{Chunk, CHUNK_VOLUME};
use crate::LightData;

/// Block light propagation (torch, lava, glowstone)
pub struct BlockLightPropagator;

impl BlockLightPropagator {
    /// Initialize block light from emitting blocks
    pub fn init(chunk: &Chunk, light: &mut LightData) {
        // 1. Find all light-emitting blocks (light_emission > 0)
        let mut sources = Vec::new();
        for idx in 0..CHUNK_VOLUME {
            let block_id = chunk.blocks[idx];
            if block_id != 0 {  // TODO: check BlockProperties.light_emission
                sources.push(idx);
            }
        }
        
        // 2. BFS from each source
        Self::propagate_bfs(chunk, light, &sources);
    }
    
    /// BFS flood fill from light sources
    pub fn propagate_bfs(chunk: &Chunk, light: &mut LightData, sources: &[usize]) {
        // Priority queue: higher light = higher priority
        let mut queue = std::collections::BinaryHeap::new();
        let mut visited = vec![false; CHUNK_VOLUME];
        
        for &idx in sources {
            let block_id = chunk.blocks[idx];
            let emission = if block_id != 0 { 15u8 } else { 0u8 }; // TODO: from BlockRegistry
            if emission > 0 {
                queue.push(std::cmp::Reverse((255u8 - emission, idx)));
                visited[idx] = true;
            }
        }
        
        while let Some(std::cmp::Reverse((_, idx))) = queue.pop() {
            let level = if let Some(&std::cmp::Reverse((_, src_idx))) = queue.iter().find(|r| r.0.1 == idx) {
                255u8 - src_idx as u8
            } else { 0 };
            // Actually need proper BFS — simplified pseudocode
            todo!("Full BFS implementation with BinaryHeap")
        }
    }
    
    /// Re-initialize after block change
    pub fn on_block_change(chunk: &mut Chunk, light: &mut LightData, idx: usize, old_block: u16, new_block: u16) {
        // 1. Clear light at position
        // 2. If old block was light source, propagate removal
        // 3. If new block is light source, propagate addition
        todo!("Incremental light update after block change")
    }
}
```

---

### Gün 12: Light Propagation Engine — `propagate.rs`

#### 6.5.1. Ana Propagasyon Algoritması

```rust
use strata_core::{Chunk, CHUNK_VOLUME};
use crate::{LightData, sunlight::SunlightPropagator, block_light::BlockLightPropagator};

/// Complete light propagation for a chunk
pub fn propagate_all(chunk: &mut Chunk) {
    let mut light = LightData::new();
    
    // Sky light
    SunlightPropagator::init(chunk, &mut light);
    SunlightPropagator::propagate_bfs(chunk, &mut light);
    
    // Block light
    BlockLightPropagator::init(chunk, &mut light);
    
    chunk.light = light;
}

/// Incremental update after block change
pub fn on_block_change(chunk: &mut Chunk, idx: usize, old_block: u16, new_block: u16) {
    // Re-propagate affected region only
    SunlightPropagator::propagate_bfs(chunk, &mut chunk.light);
    BlockLightPropagator::on_block_change(chunk, &mut chunk.light, idx, old_block, new_block);
}
```

#### 6.5.2. `lib.rs`

```rust
pub mod sunlight;
pub mod block_light;
pub mod propagate;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

pub struct LightPlugin;

impl Plugin for LightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, lighting_system);
    }
}

fn lighting_system() {
    // TODO: Query dirty chunks, propagate light
}
```

---

### Gün 12-13: Light Integration Test

```bash
# Generate chunk → propagate light → verify
cargo test -p strata-lighting
```

**Milestone 5:** Light propagation çalışıyor: sky light (top-down BFS) + block light (BFS flood fill).

---

## Hafta 7: GPU Compute Shader Meshing + Lazy Loading

### Gün 13-14: GPU Compute Shader — `gpu_compute.rs`

#### 7.1.1. `crates/meshing/src/gpu_compute.rs`

```rust
use strata_core::Chunk;
use crate::mesher::{MeshData, Mesher};
use wgpu::{Device, Queue, ComputePipeline, BindGroup, Buffer};

/// GPU Compute Shader meshing (Faz 2)
/// 
/// Binary greedy'nin bitwise operasyonları GPU'da doğal olarak paralel:
/// - Her thread bir face grubunu işler
/// - VRAM'de ayrı buffer'lar
/// - multi_draw_indexed_indirect ile tek draw call
///
/// Performans hedefi: <50µs/chunk
pub struct GpuComputeMesher {
    device: Device,
    queue: Queue,
    compute_pipeline: ComputePipeline,
    readback_buffer: Buffer,
    // Indirect draw buffers for multi_draw
    indirect_buffer: Buffer,
}

impl GpuComputeMesher {
    pub fn new(device: Device, queue: Queue) -> Self {
        // 1. Load compute shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Compute Mesher"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../render/src/shaders/compute_mesher.wgsl").into()
            ),
        });
        
        // 2. Create compute pipeline
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Compute Mesher Layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        
        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Compute Mesher Pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        });
        
        // 3. Create output buffer (max vertices per chunk)
        let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesher Output Buffer"),
            size: 1024 * 1024,  // 1MB per chunk
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        
        // 4. Indirect buffer for multi_draw
        let indirect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Indirect Draw Buffer"),
            size: std::mem::size_of::<wgpu::util::DrawIndexedIndirectArgs>() as u64 * 1024,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        Self { device, queue, compute_pipeline, readback_buffer, indirect_buffer }
    }
    
    /// Dispatch compute shader for a chunk
    pub fn dispatch_chunk(&self, chunk: &Chunk) {
        // 1. Upload chunk voxel data to GPU storage buffer
        // 2. Dispatch compute shader (CHUNK_VOLUME / 64 threads)
        // 3. Each thread: process 64 voxels, generate quads
        // 4. Write to output buffer via atomic counters
        // 5. Generate indirect draw args
        todo!("GPU compute dispatch implementation")
    }
    
    /// Read back mesh data from GPU
    pub fn readback_mesh(&self) -> MeshData {
        // 1. Copy output buffer to staging buffer
        // 2. Map staging buffer
        // 3. Parse vertex/index data
        todo!("GPU readback implementation")
    }
}

impl Mesher for GpuComputeMesher {
    fn generate_mesh(&self, chunk: &Chunk) -> MeshData {
        self.dispatch_chunk(chunk);
        self.readback_mesh()
    }
    
    fn name(&self) -> &str {
        "gpu_compute"
    }
}
```

**Not:** GPU compute meshing tam implementasyonu wgpu'nun `features::INDIRECT_FIRST_INSTANCE` ve `multi_draw_indexed_indirect` desteğine bağlı. Şu an taslak olarak ekleniyor, detaylı implementasyon wgpu feature flag doğrulaması sonrası yapılacak.

---

### Gün 14: Compute Shader WGSL — `compute_mesher.wgsl`

```wgsl
// compute_mesher.wgsl — GPU binary greedy meshing
//
// Her workgroup: 64 threads (1 warp/wavefront)
// Her thread: 1 voxel face
// 
// Output: compacted vertex buffer + indirect draw args

struct VoxelData {
    blocks: array<u16>,  // 65536 u16 flat array
};

struct VertexOutput {
    // Interleaved vertex data packed as u32
    data: array<u32>,
};

struct IndirectDraw {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
};

@group(0) @binding(0) var<storage, read> voxel_input: VoxelData;
@group(0) @binding(1) var<storage, read_write> vertex_output: VertexOutput;
@group(0) @binding(2) var<storage, read_write> indirect_draw: IndirectDraw;
@group(0) @binding(3) var<storage, read_write> atomic_counter: atomic<u32>;

// Binary greedy mask generation
// Her face direction için 2D bit mask
// Greedy merge: bitwise AND + popcount
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    // Her thread bir voxel face'ini işler
    // 6 face × 65536 voxels = 393216 thread
    let thread_id = id.x;
    let face_dir = thread_id / 65536u;  // 0..5
    let voxel_idx = thread_id % 65536u; // 0..65535
    
    // Binary greedy: 
    // 1. Compute face mask
    // 2. Greedy merge using bitwise operations
    // 3. Atomic counter for vertex output
    // 4. Write vertex data
    
    todo!("Full compute shader implementation")
}
```

---

### Gün 14-15: Lazy Loading + Dirty-Flag Throttling

#### 7.3.1. `bin/client/src/lazy_loader.rs` (WorldManager'a entegre)

```rust
use std::collections::VecDeque;
use strata_core::{Chunk, ChunkPos};
use crate::world::WorldManager;

/// Frame-throttled chunk loading
/// Her N frame'de 1-2 chunk yükle
/// Öncelik: oyuncuya en yakın chunk'lar
pub struct LazyChunkLoader {
    queue: VecDeque<ChunkPos>,
    chunks_per_frame: u8,
    frame_counter: u32,
    load_interval: u32,
}

impl LazyChunkLoader {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            chunks_per_frame: 2,
            frame_counter: 0,
            load_interval: 3, // Her 3 frame'de bir
        }
    }
    
    /// Request chunk to load (called when player moves)
    pub fn request_chunks(&mut self, positions: &[ChunkPos]) {
        for pos in positions {
            if !self.queue.contains(pos) {
                self.queue.push_back(*pos);
            }
        }
    }
    
    /// Process pending chunks — call every frame
    pub fn process(&mut self, world: &mut WorldManager) -> Vec<ChunkPos> {
        self.frame_counter += 1;
        if self.frame_counter % self.load_interval != 0 {
            return Vec::new();
        }
        
        let mut loaded = Vec::new();
        let limit = self.chunks_per_frame.min(self.queue.len() as u8);
        
        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                world.get_or_generate(pos);
                loaded.push(pos);
            }
        }
        
        loaded
    }
    
    /// Priority sort: nearest chunks first
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
```

#### 7.3.2. Dirty-Flag Throttling

```rust
// WorldManager'a ekle:
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
    
    pub fn mark_dirty(&mut self, pos: ChunkPos) {
        if !self.dirty_chunks.contains(&pos) {
            self.dirty_chunks.push(pos);
        }
    }
    
    /// Process up to N dirty chunks per frame
    pub fn process(&mut self, world: &mut WorldManager) -> Vec<ChunkPos> {
        let limit = self.max_rebuild_per_frame.min(self.dirty_chunks.len() as u8);
        let mut rebuilt = Vec::new();
        
        for pos in self.dirty_chunks.drain(..limit as usize) {
            world.rebuild_mesh(pos);
            rebuilt.push(pos);
        }
        
        // Keep remaining
        self.dirty_chunks.retain(|p| !rebuilt.contains(p));
        
        rebuilt
    }
}
```

---

### Gün 15: Chunk Loading Pipeline (Player Movement)

WorldManager'da oyuncu hareketine göre chunk yükleme mantığı:

```rust
// WorldManager'a ekle:
impl WorldManager {
    /// Calculate which chunks should be loaded based on player position
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
    
    /// Unload chunks that are too far from player
    pub fn unload_distant_chunks(&mut self, player_pos: ChunkPos, render_distance: u32) {
        let rd = render_distance as i32 + 2; // +2 for buffer
        self.chunks.retain(|pos, _| {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            dx.abs() <= rd && dz.abs() <= rd
        });
        self.meshes.retain(|pos, _| {
            let dx = pos.0.x - player_pos.0.x;
            let dz = pos.0.y - player_pos.0.y;
            dx.abs() <= rd && dz.abs() <= rd
        });
    }
}
```

---

### Gün 15-16: Doğrulama

```bash
cargo build --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

**Milestone 6:** GPU compute shader draft'ı hazır, lazy loading + dirty-flag throttling çalışıyor.

---

## Hafta 8: Debug Overlay + Entegrasyon + Final

### Gün 16-17: Debug Overlay

#### 8.1.1. FPS + Chunk Count + Memory Display

```rust
// bin/client/src/debug_overlay.rs
pub struct DebugOverlay {
    pub fps: f32,
    pub chunk_count: usize,
    pub visible_chunks: usize,
    pub meshing_time: Duration,
    pub memory_usage: usize,     // MB
    pub render_time: Duration,
    pub frame_time: Duration,
}

impl DebugOverlay {
    pub fn new() -> Self { /* ... */ }
    
    /// Render overlay text using glyphon
    pub fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        // glyphon text rendering
        // 1. FPS
        // 2. Chunks: loaded / visible
        // 3. Mesh time: X.XXms
        // 4. Render time: X.XXms
        // 5. Memory: X MB
        // 6. Position: X, Y, Z
        todo!("glyphon debug overlay")
    }
}
```

**glyphon Entegrasyonu:**
```toml
# bin/client/Cargo.toml — ekle
glyphon = "0.12"
cosmic-text = "0.18"
```

**Not:** glyphon entegrasyonu kompleks (Atlas, SwashCache, TextRenderer). Minimal implementasyon için önce basit bir FPS counter yazı, sonra glyphon'u ekle.

#### 8.1.2. Minimal FPS Counter (glyphon öncesi)

```rust
// bin/client/src/render.rs — title bar FPS (mevcut)
// Halihazırda window.set_title() ile FPS gösteriliyor
```

---

### Gün 17-18: Chunk Loading After Player Movement

#### 8.2.1. Main Loop Entegrasyonu

```rust
// bin/client/src/main.rs — App::about_to_wait() güncellemesi

fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
    let now = Instant::now();
    let dt = now.duration_since(self.last_frame_time).as_secs_f32();
    self.last_frame_time = now;
    
    // 1. Update camera
    self.camera.update(&self.input, dt);
    self.input.update();
    
    // 2. Update world (lazy loading)
    let player_chunk = ChunkPos::from_world(
        self.camera.position.x as i32,
        self.camera.position.z as i32,
    );
    
    // Request new chunks
    let required = self.world.get_required_chunks(player_chunk, 4);
    self.lazy_loader.request_chunks(&required);
    self.lazy_loader.prioritize(player_chunk);
    let loaded = self.lazy_loader.process(&mut self.world);
    
    // Upload new meshes to GPU
    if let Some(render) = &mut self.render {
        for pos in loaded {
            if let Some(mesh) = self.world.get_mesh(pos) {
                render.chunk_renderer.upload_mesh(&render.device, pos, mesh);
            }
        }
    }
    
    // Unload distant chunks
    self.world.unload_distant_chunks(player_chunk, 4);
    
    // 3. Process dirty chunks (block changes)
    let rebuilt = self.dirty_manager.process(&mut self.world);
    // Upload rebuilt meshes
    // ...
    
    // 4. Update render
    if let Some(render) = &mut self.render {
        render.camera = self.camera.clone();
        render.update_camera();
        
        // Frustum culling
        let all_chunks: Vec<_> = self.world.chunks.keys().copied().collect();
        render.chunk_renderer.cull(&render.frustum, &all_chunks);
        
        render.render_frame();
    }
    
    // 5. FPS update
    // ...
}
```

---

### Gün 18-19: Heightmap Optimizasyonu

Mevcut heightmap (Faz 1'de implemente edildi) şu amaçlarla kullanılır:

| Kullanım | Açıklama |
|----------|----------|
| **Light propagation** | Sky light: top-down, heightmap'ten başla |
| **Mesh generation** | Sadece heightmap altındaki blokları işle (boş chunk'ları atla) |
| **Frustum culling** | Chunk AABB'yi heightmap'a göre daralt |
| **Empty chunk skip** | `is_empty()` → mesh oluşturma, render etme |

```rust
// Mesher optimizasyonu: sadece dolu bölgeleri işle
impl ClassicGreedyMesher {
    pub fn generate_mesh_optimized(&self, chunk: &Chunk) -> MeshData {
        if chunk.is_empty() {
            return MeshData::empty();
        }
        
        // Heightmap kullan: min_y = heightmap_bottom.min(), max_y = heightmap_top.max()
        let min_y = chunk.heightmap_bottom.iter().filter(|&&h| h > 0).min().copied().unwrap_or(0) as usize;
        let max_y = chunk.heightmap_top.iter().max().copied().unwrap_or(0) as usize;
        
        if min_y > max_y || max_y == 0 {
            return MeshData::empty();
        }
        
        // Sadece [min_y..=max_y] aralığında mesh oluştur
        // Bu, boş chunk'ları ve tamamen dolu chunk'ları hızlıca atlar
        todo!("Heightmap-optimized meshing")
    }
}
```

---

### Gün 19-20: Client Binary Refactor

#### 8.4.1. Yeni `bin/client/src/main.rs` Yapısı

```rust
mod camera;         // Basit wrapper (render crate'teki Camera'ı kullanır)
mod input;          // InputState (mevcut)
mod world;          // WorldManager (mevcut, genişletilmiş)
mod debug_overlay;  // YENİ

use strata_render::{RenderEngine, Camera, Frustum};
use strata_lighting::LightPlugin;
use strata_ecs::EcsPlugin;

struct App {
    render: Option<RenderEngine>,
    world: WorldManager,
    input: InputState,
    lazy_loader: LazyChunkLoader,
    dirty_manager: DirtyChunkManager,
    debug: DebugOverlay,
    // ...
}
```

---

### Gün 20-21: Test + Lint + Benchmark

#### 8.5.1. Performance Benchmarks

```rust
// crates/meshing/benches/meshing_bench.rs — güncelle
use criterion::{criterion_group, criterion_main, Criterion};
use strata_meshing::{ClassicGreedyMesher, Mesher};
use strata_core::{Chunk, ChunkPos};
use strata_world_gen::TerrainGenerator;

fn bench_classic_greedy(c: &mut Criterion) {
    let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(0, 0)));
    let gen = TerrainGenerator::new(42);
    gen.generate(&mut chunk);
    
    let mesher = ClassicGreedyMesher;
    
    c.bench_function("classic_greedy_meshing", |b| {
        b.iter(|| {
            let _mesh = mesher.generate_mesh(&chunk);
        });
    });
}

criterion_group!(benches, bench_classic_greedy);
criterion_main!(benches);
```

```bash
# Benchmark
cargo bench -p strata-meshing

# Hedef: <500µs/chunk (classic greedy)
# Hedef: <50µs/chunk (GPU compute — Faz 2 sonu)
```

#### 8.5.2. Light Propagation Test

```rust
// crates/lighting/tests/light_propagation_test.rs
#[cfg(test)]
mod tests {
    use strata_core::{Chunk, ChunkPos, BlockId};
    use strata_lighting::propagate::propagate_all;
    
    #[test]
    fn test_sky_light_top_down() {
        let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(0, 0)));
        // Fill bottom half with stone
        for x in 0..16 {
            for z in 0..16 {
                for y in 0..64 {
                    chunk.set_block(x, y, z, BlockId::STONE);
                }
            }
        }
        
        propagate_all(&mut chunk);
        
        // Top should have max sky light
        let top_idx = Chunk::index(0, 255, 0);
        assert_eq!(chunk.light.get_sky(top_idx), 15);
        
        // Below stone should have 0
        let bottom_idx = Chunk::index(0, 0, 0);
        assert_eq!(chunk.light.get_sky(bottom_idx), 0);
    }
    
    #[test]
    fn test_block_light_emission() {
        let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(0, 0)));
        // Place a torch-like block at center
        chunk.set_block(8, 50, 8, BlockId(5)); // hypothetical torch
        
        propagate_all(&mut chunk);
        
        // Position 1 block away should have some light
        let nearby_idx = Chunk::index(8, 49, 8);
        assert!(chunk.light.get_block(nearby_idx) > 0);
    }
}
```

```bash
cargo test -p strata-lighting
```

---

### Gün 21-22: Final Integration + Polish

#### 8.6.1. Tüm Sistemleri Birleştir

```bash
# Clean build
cargo clean
cargo build --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt

# Test
cargo test --workspace

# Run
cargo run -p strata-client
```

#### 8.6.2. Texture Asset Hazırlığı

```
assets/textures/
├── stone.png          # 16x16 placeholder (tüm yüzler aynı)
├── dirt.png
├── grass_top.png
├── grass_side.png
├── grass_bottom.png
└── bedrock.png
```

**Placeholder texture oluşturma scripti (isteğe bağlı):**
```bash
# PowerShell ile 16x16 renkli kareler oluştur
# Veya elle PNG hazırla
```

---

### Gün 22: Doğrulama + Teslim

#### 8.7.1. Faz 2 Teslim Kriterleri

| # | Kriter | Durum |
|---|--------|-------|
| 1 | `cargo build --workspace` başarılı | |
| 2 | `cargo clippy --workspace -- -D warnings` temiz | |
| 3 | `cargo test --workspace` geçer | |
| 4 | **`render` crate**: wgpu engine, pipeline, camera, chunk renderer | |
| 5 | **Texture2DArray**: block textures GPU'da, shader'da örnekleniyor | |
| 6 | **Frustum culling**: görünür chunk'lar filtreleniyor | |
| 7 | **`lighting` crate**: sky + block light propagation (BFS) | |
| 8 | **Light data**: chunk'ta 64 KB/chunk, 4-bit packed | |
| 9 | **GPU compute mesher**: draft implementasyon (compute_mesher.wgsl) | |
| 10 | **Lazy loading**: frame-throttled chunk yükleme (oyuncu hareketine göre) | |
| 11 | **Dirty-flag throttling**: frame başına max N chunk rebuild | |
| 12 | **Heightmap optimizasyonu**: boş chunk atlama, mesh aralığı daraltma | |
| 13 | **Debug overlay**: FPS, chunk count, visible count | |
| 14 | **Block texture rendering**: her blok tipi farklı texture | |

---

## Bağımlılık Grafiği

```
Hafta 5: render crate
  ├── Hafta 6: lighting crate (bağımlı: chunk + block registry)
  │     └── Hafta 7: lazy loading (bağımlı: lighting + render)
  │           └── Hafta 8: debug overlay + entegrasyon
  └── Hafta 7: GPU compute mesher (bağımlı: render crate + wgpu)
        └── Hafta 8: batch rendering + multi_draw
```

**Kritik Yol:** render crate → lighting crate → entegrasyon

---

## Riskler ve Mitigasyon

| Risk | Olasılık | Etki | Mitigasyon |
|------|----------|------|------------|
| wgpu Texture2DArray feature flag olgunluğu | Düşük | Orta | Fallback: her block tipi için ayrı texture, shader'da `texture_2d` kullan |
| BFS light propagation CPU maliyeti (full chunk ~150µs) | Orta | Orta | Incremental update: sadece değişen bölgeyi yeniden hesapla |
| GPU compute shader binary greedy karmaşıklığı | Yüksek | Yüksek | Önce basit face-generation (greedy olmadan), Faz 2+ greedy merge |
| glyphon text rendering entegrasyonu | Orta | Düşük | Fallback: window title bar'da FPS göster (mevcut) |
| Frustum culling chunk sınırlarında yanlış pozitif | Düşük | Düşük | %5-10 extra draw call kabul edilebilir |
| Lazy loading + dirty throttling frame spike | Düşük | Orta | Async wgpu buffer upload, main thread'i bloklama |

---

## Performans Hedefleri (Faz 2)

| Metrik | Hedef | Not |
|--------|-------|-----|
| FPS (100 chunk) | 60+ | Frustum culling + lazy loading ile |
| Frustum culling süresi | <1µs/1000 chunk | 6 plane dot product |
| Light propagation (full chunk) | <150µs/chunk | BFS flood fill |
| Light propagation (incremental) | <30µs/chunk | Sadece dirty bölge |
| GPU compute meshing | <50µs/chunk | Bitwise parallelism |
| Texture2DArray bind | ~0µs (once) | Pipeline sabit |
| Lazy loading throughput | 2 chunk/frame | 3 frame interval |
| Dirty chunk rebuild | 4 chunk/frame | Max rebuild limit |
| Bellek (render buffers) | <500MB/100 chunk | Vertex + index buffers |

---

## Faz 3 Hazırlığı

Faz 2 tamamlandığında şu altyapılar Faz 3 için hazır olmalı:

1. **`render` crate** → Player controller + entity rendering için kullanılabilir
2. **`lighting` crate** → Block interaction feedback (ışık değişimi)
3. **Frustum culling** → Entity visibility + LOD seçimi
4. **Chunk lazy loading** → Entity chunk tracking
5. **GPU compute mesher** → Dynamic mesh update (blok kırma/yerleştirme)
6. **Texture2DArray** → Entity texture atlas (mob, item)