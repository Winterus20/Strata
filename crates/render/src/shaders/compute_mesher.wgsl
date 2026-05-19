// compute_mesher.wgsl — GPU per-face meshing
//
// Each thread handles one voxel, checks 6 faces, and atomically writes
// vertex/index data for visible faces. Output is compatible with the
// Vertex struct used by the render pipeline.

const CHUNK_VOLUME: u32 = 65536u;
const CHUNK_WIDTH: u32 = 16u;
const CHUNK_HEIGHT: u32 = 256u;
const STRIDE: u32 = 10u; // 10 × u32 per Vertex

struct VoxelData {
    blocks: array<u32>,
};

struct VertexPacked {
    data: array<u32>,
};

struct IndexData {
    data: array<u32>,
};

struct Counters {
    vertex_count: atomic<u32>,
    index_count: atomic<u32>,
};

struct ChunkOffset {
    ox: f32,
    oz: f32,
};

@group(0) @binding(0) var<storage, read> voxel_input: VoxelData;
@group(0) @binding(1) var<storage, read_write> vertex_output: VertexPacked;
@group(0) @binding(2) var<storage, read_write> index_output: IndexData;
@group(0) @binding(3) var<storage, read_write> counters: Counters;
@group(0) @binding(4) var<uniform> chunk_offset: ChunkOffset;

fn voxel_index(x: u32, y: u32, z: u32) -> u32 {
    return x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_WIDTH;
}

fn get_block(idx: u32) -> u32 {
    return voxel_input.blocks[idx];
}

fn is_air(b: u32) -> bool {
    return b == 0u;
}

fn neighbor_block(x: u32, y: u32, z: u32, axis: u32, dir: i32) -> u32 {
    var nx = i32(x);
    var ny = i32(y);
    var nz = i32(z);
    if axis == 0u { nx = nx + dir; }
    else if axis == 1u { ny = ny + dir; }
    else { nz = nz + dir; }
    if nx < 0 || nx >= 16 || ny < 0 || ny >= 256 || nz < 0 || nz >= 16 {
        return 0u;
    }
    return get_block(voxel_index(u32(nx), u32(ny), u32(nz)));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let voxel_idx = id.x;
    if voxel_idx >= CHUNK_VOLUME { return; }

    let block = get_block(voxel_idx);
    if is_air(block) { return; }

    let x = voxel_idx % CHUNK_WIDTH;
    let yz = voxel_idx / CHUNK_WIDTH;
    let z = yz % CHUNK_WIDTH;
    let y = yz / CHUNK_WIDTH;

    if is_air(neighbor_block(x, y, z, 0u, -1)) { write_quad(x, y, z, 0u, -1, block); }
    if is_air(neighbor_block(x, y, z, 0u, 1))  { write_quad(x, y, z, 0u, 1, block); }
    if is_air(neighbor_block(x, y, z, 1u, -1)) { write_quad(x, y, z, 1u, -1, block); }
    if is_air(neighbor_block(x, y, z, 1u, 1))  { write_quad(x, y, z, 1u, 1, block); }
    if is_air(neighbor_block(x, y, z, 2u, -1)) { write_quad(x, y, z, 2u, -1, block); }
    if is_air(neighbor_block(x, y, z, 2u, 1))  { write_quad(x, y, z, 2u, 1, block); }
}

fn face_id(axis: u32, dir: i32) -> u32 {
    if axis == 0u { return select(1u, 2u, dir == 1); }
    if axis == 1u { return select(3u, 4u, dir == 1); }
    return select(5u, 6u, dir == 1);
}

fn get_texture_layer(block_id: u32, face_id: u32) -> u32 {
    if block_id == 1u {
        return 0u; // STONE
    }
    if block_id == 2u {
        return 1u; // DIRT
    }
    if block_id == 3u { // GRASS
        if face_id == 3u { // NegativeY (bottom)
            return 1u; // dirt.png
        }
        if face_id == 4u { // PositiveY (top)
            return 2u; // grass_top.png
        }
        return 8u; // grass_side.png
    }
    if block_id == 4u {
        return 3u; // BEDROCK
    }
    return 0u;
}

fn write_quad(x: u32, y: u32, z: u32, axis: u32, dir: i32, block_id: u32) {
    let ox = chunk_offset.ox;
    let oz = chunk_offset.oz;
    let fy = f32(y);

    var p0: vec3<f32>;
    var p1: vec3<f32>;
    var p2: vec3<f32>;
    var p3: vec3<f32>;
    var n: vec3<f32>;
    var u0: f32; var v0: f32;
    var u1: f32; var v1: f32;
    var u2: f32; var v2: f32;
    var u3: f32; var v3: f32;

    if axis == 0u {
        let fx = select(ox + f32(x), ox + f32(x + 1u), dir == 1);
        n = vec3<f32>(f32(dir), 0.0, 0.0);
        p0 = vec3<f32>(fx, fy, oz + f32(z));
        p1 = vec3<f32>(fx, fy + 1.0, oz + f32(z));
        p2 = vec3<f32>(fx, fy + 1.0, oz + f32(z + 1u));
        p3 = vec3<f32>(fx, fy, oz + f32(z + 1u));
        u0 = 0.0; v0 = 0.0;
        u1 = 0.0; v1 = 1.0;
        u2 = 1.0; v2 = 1.0;
        u3 = 1.0; v3 = 0.0;
    } else if axis == 1u {
        let fy_face = select(fy, fy + 1.0, dir == 1);
        n = vec3<f32>(0.0, f32(dir), 0.0);
        p0 = vec3<f32>(ox + f32(x), fy_face, oz + f32(z));
        p1 = vec3<f32>(ox + f32(x + 1u), fy_face, oz + f32(z));
        p2 = vec3<f32>(ox + f32(x + 1u), fy_face, oz + f32(z + 1u));
        p3 = vec3<f32>(ox + f32(x), fy_face, oz + f32(z + 1u));
        u0 = 0.0; v0 = 0.0;
        u1 = 1.0; v1 = 0.0;
        u2 = 1.0; v2 = 1.0;
        u3 = 0.0; v3 = 1.0;
    } else {
        let fz = select(oz + f32(z), oz + f32(z + 1u), dir == 1);
        n = vec3<f32>(0.0, 0.0, f32(dir));
        p0 = vec3<f32>(ox + f32(x), fy, fz);
        p1 = vec3<f32>(ox + f32(x), fy + 1.0, fz);
        p2 = vec3<f32>(ox + f32(x + 1u), fy + 1.0, fz);
        p3 = vec3<f32>(ox + f32(x + 1u), fy, fz);
        u0 = 0.0; v0 = 0.0;
        u1 = 0.0; v1 = 1.0;
        u2 = 1.0; v2 = 1.0;
        u3 = 1.0; v3 = 0.0;
    }

    let fid = face_id(axis, dir);
    let layer = get_texture_layer(block_id, fid);

    let base_vert = atomicAdd(&counters.vertex_count, 4u);

    let off0 = base_vert * STRIDE;
    vertex_output.data[off0 + 0u] = bitcast<u32>(p0.x);
    vertex_output.data[off0 + 1u] = bitcast<u32>(p0.y);
    vertex_output.data[off0 + 2u] = bitcast<u32>(p0.z);
    vertex_output.data[off0 + 3u] = bitcast<u32>(n.x);
    vertex_output.data[off0 + 4u] = bitcast<u32>(n.y);
    vertex_output.data[off0 + 5u] = bitcast<u32>(n.z);
    vertex_output.data[off0 + 6u] = bitcast<u32>(u0);
    vertex_output.data[off0 + 7u] = bitcast<u32>(v0);
    vertex_output.data[off0 + 8u] = bitcast<u32>(1.0);
    vertex_output.data[off0 + 9u] = layer;

    let off1 = (base_vert + 1u) * STRIDE;
    vertex_output.data[off1 + 0u] = bitcast<u32>(p1.x);
    vertex_output.data[off1 + 1u] = bitcast<u32>(p1.y);
    vertex_output.data[off1 + 2u] = bitcast<u32>(p1.z);
    vertex_output.data[off1 + 3u] = bitcast<u32>(n.x);
    vertex_output.data[off1 + 4u] = bitcast<u32>(n.y);
    vertex_output.data[off1 + 5u] = bitcast<u32>(n.z);
    vertex_output.data[off1 + 6u] = bitcast<u32>(u1);
    vertex_output.data[off1 + 7u] = bitcast<u32>(v1);
    vertex_output.data[off1 + 8u] = bitcast<u32>(1.0);
    vertex_output.data[off1 + 9u] = layer;

    let off2 = (base_vert + 2u) * STRIDE;
    vertex_output.data[off2 + 0u] = bitcast<u32>(p2.x);
    vertex_output.data[off2 + 1u] = bitcast<u32>(p2.y);
    vertex_output.data[off2 + 2u] = bitcast<u32>(p2.z);
    vertex_output.data[off2 + 3u] = bitcast<u32>(n.x);
    vertex_output.data[off2 + 4u] = bitcast<u32>(n.y);
    vertex_output.data[off2 + 5u] = bitcast<u32>(n.z);
    vertex_output.data[off2 + 6u] = bitcast<u32>(u2);
    vertex_output.data[off2 + 7u] = bitcast<u32>(v2);
    vertex_output.data[off2 + 8u] = bitcast<u32>(1.0);
    vertex_output.data[off2 + 9u] = layer;

    let off3 = (base_vert + 3u) * STRIDE;
    vertex_output.data[off3 + 0u] = bitcast<u32>(p3.x);
    vertex_output.data[off3 + 1u] = bitcast<u32>(p3.y);
    vertex_output.data[off3 + 2u] = bitcast<u32>(p3.z);
    vertex_output.data[off3 + 3u] = bitcast<u32>(n.x);
    vertex_output.data[off3 + 4u] = bitcast<u32>(n.y);
    vertex_output.data[off3 + 5u] = bitcast<u32>(n.z);
    vertex_output.data[off3 + 6u] = bitcast<u32>(u3);
    vertex_output.data[off3 + 7u] = bitcast<u32>(v3);
    vertex_output.data[off3 + 8u] = bitcast<u32>(1.0);
    vertex_output.data[off3 + 9u] = layer;

    let base_idx = atomicAdd(&counters.index_count, 6u);

    // Y-axis (axis=1): vertex order in XZ plane (X right, Z down)
    // is CW when CCW is needed from above → reverse winding.
    if axis == 1u {
        if dir == 1 {
            index_output.data[base_idx + 0u] = base_vert;
            index_output.data[base_idx + 1u] = base_vert + 2u;
            index_output.data[base_idx + 2u] = base_vert + 1u;
            index_output.data[base_idx + 3u] = base_vert;
            index_output.data[base_idx + 4u] = base_vert + 3u;
            index_output.data[base_idx + 5u] = base_vert + 2u;
        } else {
            index_output.data[base_idx + 0u] = base_vert;
            index_output.data[base_idx + 1u] = base_vert + 1u;
            index_output.data[base_idx + 2u] = base_vert + 2u;
            index_output.data[base_idx + 3u] = base_vert;
            index_output.data[base_idx + 4u] = base_vert + 2u;
            index_output.data[base_idx + 5u] = base_vert + 3u;
        }
    } else {
        if dir == 1 {
            index_output.data[base_idx + 0u] = base_vert;
            index_output.data[base_idx + 1u] = base_vert + 1u;
            index_output.data[base_idx + 2u] = base_vert + 2u;
            index_output.data[base_idx + 3u] = base_vert;
            index_output.data[base_idx + 4u] = base_vert + 2u;
            index_output.data[base_idx + 5u] = base_vert + 3u;
        } else {
            index_output.data[base_idx + 0u] = base_vert;
            index_output.data[base_idx + 1u] = base_vert + 2u;
            index_output.data[base_idx + 2u] = base_vert + 1u;
            index_output.data[base_idx + 3u] = base_vert;
            index_output.data[base_idx + 4u] = base_vert + 3u;
            index_output.data[base_idx + 5u] = base_vert + 2u;
        }
    }
}
