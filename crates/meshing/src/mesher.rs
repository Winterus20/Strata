use glam::Vec3;

/// GPU-ready vertex with position, normal, UVs, ambient occlusion, and texture id.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub ao: f32,
    pub texture_id: u16,
    pub _padding: u16,
}

/// Axis-aligned bounding box for frustum culling.
#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub min: Vec3,
    pub max: Vec3,
}

/// Output of a mesher: vertices, indices, and a bounding box.
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub vertex_count: usize,
    pub index_count: usize,
    pub bounds: BoundingBox,
}

impl MeshData {
    /// Creates an empty mesh with zero vertices.
    pub fn empty() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            vertex_count: 0,
            index_count: 0,
            bounds: BoundingBox {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
        }
    }

    /// Returns `true` if this mesh has no vertices.
    pub fn is_empty(&self) -> bool {
        self.vertex_count == 0
    }
}

/// Algorithm-agnostic meshing interface.
///
/// Implementations receive a [`Chunk`](strata_core::Chunk) and produce a [`MeshData`].
pub trait Mesher: Send + Sync {
    /// Generates a mesh for the given chunk.
    fn generate_mesh(&self, chunk: &strata_core::Chunk) -> MeshData;
    /// Returns the human-readable name of this mesher.
    fn name(&self) -> &str;
}
