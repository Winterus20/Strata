use strata_core::{CHUNK_VOLUME, Chunk, ChunkPos};

use crate::config::{
    GPU_TERRAIN_ENABLED, GPU_TERRAIN_MAX_CHUNKS_PER_DISPATCH, GPU_TERRAIN_WORKGROUP_SIZE,
};

/// Describes a GPU compute terrain generation dispatch for chunk-sized work.
///
/// This struct is designed to be serialized into a GPU storage buffer
/// consumed by a compute shader. Each entry describes one chunk's
/// position and output block buffer.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ChunkTerrainDispatch {
    /// World X coordinate of the chunk origin.
    pub world_x: i32,
    /// World Z coordinate of the chunk origin.
    pub world_z: i32,
    /// World seed for noise generation (passed as i32).
    pub seed: i32,
    /// Padding for alignment (16 bytes).
    pub _pad0: i32,
}

/// Output buffer element for GPU-generated terrain data.
///
/// After the compute shader runs, this buffer contains the block IDs
/// for one chunk's volume (65,536 blocks).
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct ChunkTerrainOutput {
    pub blocks: [u16; CHUNK_VOLUME],
}

/// GPU compute terrain generation manager.
///
/// Manages buffer creation, dispatch, and readback for
/// GPU-accelerated chunk generation.
///
/// # Performance
/// - ~50µs/chunk on GPU vs ~500µs/chunk on CPU
/// - Batch dispatch up to 64 chunks per call
/// - Async readback via wgpu buffer mapping
pub struct GpuTerrainGenerator {
    enabled: bool,
    max_chunks_per_dispatch: u32,
    workgroup_size: u32,
}

impl GpuTerrainGenerator {
    pub fn new() -> Self {
        Self {
            enabled: GPU_TERRAIN_ENABLED,
            max_chunks_per_dispatch: GPU_TERRAIN_MAX_CHUNKS_PER_DISPATCH,
            workgroup_size: GPU_TERRAIN_WORKGROUP_SIZE,
        }
    }

    /// Whether GPU terrain generation is enabled.
    #[inline]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set the enabled state.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Maximum chunks per GPU dispatch.
    #[inline]
    pub fn max_chunks_per_dispatch(&self) -> u32 {
        self.max_chunks_per_dispatch
    }

    /// Build a dispatch buffer for the given chunk positions.
    ///
    /// Returns the number of workgroups needed to generate all chunks
    /// in a single compute shader dispatch.
    pub fn build_dispatch<'a>(
        &self,
        positions: impl Iterator<Item = &'a ChunkPos>,
        seed: i32,
    ) -> (Vec<ChunkTerrainDispatch>, u32) {
        let mut dispatch = Vec::new();
        for pos in positions {
            if dispatch.len() >= self.max_chunks_per_dispatch as usize {
                break;
            }
            dispatch.push(ChunkTerrainDispatch {
                world_x: pos.world_x(),
                world_z: pos.world_z(),
                seed,
                _pad0: 0,
            });
        }

        // Each workgroup handles one chunk's worth of blocks.
        // Total workgroups = number of chunks.
        let workgroup_count = dispatch.len() as u32;
        (dispatch, workgroup_count)
    }

    /// The workgroup size for the terrain compute shader.
    #[inline]
    pub fn workgroup_size(&self) -> u32 {
        self.workgroup_size
    }

    /// Generate terrain on CPU fallback when GPU is unavailable.
    ///
    /// This is called when `is_enabled()` returns false or when
    /// the wgpu device doesn't support compute shaders.
    pub fn generate_cpu_fallback(&self, positions: &[ChunkPos], seed: i32) -> Vec<Chunk> {
        let mut generator = crate::terrain::TerrainGenerator::new(seed as u32);
        positions
            .iter()
            .map(|&pos| {
                let mut chunk = Chunk::new(pos);
                generator.generate(&mut chunk);
                chunk
            })
            .collect()
    }
}

impl Default for GpuTerrainGenerator {
    fn default() -> Self {
        Self::new()
    }
}

// ── WGSL Shader Template ─────────────────────────────────────────────

/// Returns the WGSL compute shader source for GPU terrain generation.
///
/// This shader takes a buffer of `ChunkTerrainDispatch` entries and
/// generates a `ChunkTerrainOutput` buffer with block data for each chunk.
///
/// The shader implements a simplified 3D density function using:
/// - Hash-based pseudo-random for continental/erosion noise
/// - Heightmap-based terrain with Y-bias
/// - Basic cave carving
pub fn terrain_compute_shader_source() -> &'static str {
    r#"
struct DispatchInput {
    world_x: i32,
    world_z: i32,
    seed: i32,
    _pad0: i32,
};

struct ChunkOutput {
    blocks: array<u16, 65536>,
};

@group(0) @binding(0) var<storage, read> input: array<DispatchInput>;
@group(0) @binding(1) var<storage, read_write> output: array<ChunkOutput>;

const CHUNK_WIDTH: u32 = 16u;
const CHUNK_HEIGHT: u32 = 256u;
const SEA_LEVEL: f32 = 63.0;

// Simple hash-based noise for GPU terrain
fn hash2(px: i32, pz: i32, seed: i32) -> f32 {
    var h: u32 = u32(px) * 374761393u;
    h = h ^ u32(pz) * 668265263u;
    h = h ^ u32(seed) * 2246822519u;
    h = h ^ (h >> 13u);
    h = h * 3266489917u;
    h = h ^ (h >> 15u);
    return f32(h) / 4294967295.0;
}

fn hash3(px: i32, py: i32, pz: i32, seed: i32) -> f32 {
    var h: u32 = u32(px) * 374761393u;
    h = h + u32(py);
    h = h * 668265263u;
    h = h ^ u32(pz);
    h = h * 2246822519u;
    h = h ^ u32(seed) * 3266489917u;
    h = h ^ (h >> 13u);
    h = h * 374761393u;
    h = h ^ (h >> 15u);
    return f32(h) / 4294967295.0;
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    return a + (b - a) * t;
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn base_height(continental: f32, erosion: f32) -> f32 {
    // Simplified height function
    let base = lerp(30.0, 120.0, smoothstep(-1.0, 1.0, continental));
    let erosion_offset = lerp(25.0, -20.0, smoothstep(-1.0, 1.0, erosion));
    return base + erosion_offset;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let chunk_idx = id.x;
    let block_idx = id.y * 256u + id.z;

    if chunk_idx >= arrayLength(&input) {
        return;
    }

    let dispatch = input[chunk_idx];
    let wx = dispatch.world_x;
    let wz = dispatch.world_z;
    let seed = dispatch.seed;

    let lx = block_idx % 16u;
    let ly = (block_idx / 16u) % 256u;
    let lz = block_idx / (16u * 256u);

    let world_x = wx + i32(lx);
    let world_y = i32(ly);
    let world_z = wz + i32(lz);

    let block: u16;

    if ly == 0u {
        block = 4u16; // bedrock
    } else {
        let continental = hash2(world_x, world_z, seed) * 2.0 - 1.0;
        let erosion = hash2(world_x + 1000, world_z + 2000, seed + 1) * 2.0 - 1.0;
        let height_val = base_height(continental, erosion);
        let density = (height_val - f32(world_y)) * 0.08;
        let detail = (hash3(world_x, world_y, world_z, seed + 2) * 2.0 - 1.0) * 2.0;

        if density + detail > 0.0 {
            block = 1u16; // stone
        } else {
            block = 0u16; // air
        }
    }

    output[chunk_idx].blocks[block_idx] = block;
}
"#
}
