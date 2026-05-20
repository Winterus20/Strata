struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) ao: f32,
    @location(4) packed_id: u32,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) ao: f32,
    @location(3) layer_index: u32,
    @location(4) world_pos: vec3<f32>,
};

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    fog_color_density: vec4<f32>,
    camera_pos: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> frame: FrameUniforms;

@group(1) @binding(0)
var texture_array: texture_2d_array<f32>;

@group(1) @binding(1)
var texture_sampler: sampler;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_pos = frame.view_proj * vec4<f32>(input.position, 1.0);
    output.normal = input.normal;
    output.uv = input.uv;
    output.ao = input.ao;
    output.layer_index = input.packed_id & 0xFFu;
    output.world_pos = input.position;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(texture_array, texture_sampler, input.uv, i32(input.layer_index));
    
    let light_dir = normalize(vec3<f32>(0.3, 1.0, 0.5));
    let diffuse = max(dot(normalize(input.normal), light_dir), 0.0);
    let ambient = 0.3;
    let lit_color = tex_color.rgb * (ambient + diffuse * 0.7) * input.ao;

    // Distance-based fog — only below the camera (underwater objects)
    var frag_color: vec3<f32> = lit_color;
    if (input.world_pos.y < frame.camera_pos.y) {
        let dist = distance(input.world_pos, frame.camera_pos.xyz);
        let fog_factor = exp(-frame.fog_color_density.w * frame.fog_color_density.w * dist * dist);
        frag_color = mix(frame.fog_color_density.rgb, lit_color, fog_factor);
    }

    // Water blocks (layer 7) get partial transparency
    var alpha: f32 = tex_color.a;
    if (input.layer_index == 7u) {
        alpha = 0.65;
    }
    return vec4<f32>(frag_color, alpha);
}