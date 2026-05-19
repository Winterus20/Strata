@group(0) @binding(0) var<storage, read_write> block_data: array<u32>;   // Voxel Block IDs (quantized)
@group(0) @binding(1) var<storage, read_write> light_data: array<u32>;   // Packed Light Levels (4-bit sky, 4-bit block)

struct ComputeParams {
    chunk_width: u32,  // 16
    chunk_height: u32, // 256
    chunk_depth: u32,  // 16
    iteration: u32,
}
@group(0) @binding(2) var<uniform> params: ComputeParams;

fn get_index(x: u32, y: u32, z: u32) -> u32 {
    return x + (z * params.chunk_width) + (y * params.chunk_width * params.chunk_depth);
}

// Unpack 4-bit Sky and Block light levels from packed u32 array
fn unpack_light(index: u32) -> vec2<u32> {
    let byte_index = index / 2u;
    let packed_byte = light_data[byte_index];
    let is_odd = (index & 1u) == 1u;
    
    var val: u32 = 0u;
    if (is_odd) {
        val = packed_byte >> 4u;
    } else {
        val = packed_byte & 0x0Fu;
    }
    
    // Low 4 bits = block light, High 4 bits = sky light
    let block = val & 0x0Fu;
    let sky = (val >> 4u) & 0x0Fu;
    return vec2<u32>(sky, block);
}

fn pack_light(index: u32, sky: u32, block: u32) {
    let byte_index = index / 2u;
    let is_odd = (index & 1u) == 1u;
    let light_val = (block & 0x0Fu) | ((sky & 0x0Fu) << 4u);
    
    // double buffering is typically used to avoid races, but here we write directly or double-buffer
    if (is_odd) {
        light_data[byte_index] = (light_data[byte_index] & 0x0Fu) | (light_val << 4u);
    } else {
        light_data[byte_index] = (light_data[byte_index] & 0xF0u) | light_val;
    }
}

@compute @workgroup_size(4, 8, 4)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let x = global_id.x;
    let y = global_id.y;
    let z = global_id.z;
    
    if (x >= params.chunk_width || y >= params.chunk_height || z >= params.chunk_depth) {
        return;
    }
    
    let idx = get_index(x, y, z);
    
    // Current light values
    let current_light = unpack_light(idx);
    var max_sky = current_light.x;
    var max_block = current_light.y;
    
    // Check 6 neighbors (Cellular Automata)
    let dirs = array<vec3<i32>, 6>(
        vec3<i32>(-1, 0, 0), vec3<i32>(1, 0, 0),
        vec3<i32>(0, -1, 0), vec3<i32>(0, 1, 0),
        vec3<i32>(0, 0, -1), vec3<i32>(0, 0, 1)
    );
    
    for (var i = 0u; i < 6u; i = i + 1u) {
        let nx = i32(x) + dirs[i].x;
        let ny = i32(y) + dirs[i].y;
        let nz = i32(z) + dirs[i].z;
        
        if (nx >= 0 && nx < i32(params.chunk_width) &&
            ny >= 0 && ny < i32(params.chunk_height) &&
            nz >= 0 && nz < i32(params.chunk_depth)) {
            
            let n_idx = get_index(u32(nx), u32(ny), u32(nz));
            let neighbor_light = unpack_light(n_idx);
            
            // Attenuation = 1
            if (neighbor_light.x > 0u) {
                max_sky = max(max_sky, neighbor_light.x - 1u);
            }
            if (neighbor_light.y > 0u) {
                max_block = max(max_block, neighbor_light.y - 1u);
            }
        }
    }
    
    // Update if light propagated
    if (max_sky > current_light.x || max_block > current_light.y) {
        pack_light(idx, max_sky, max_block);
    }
}
