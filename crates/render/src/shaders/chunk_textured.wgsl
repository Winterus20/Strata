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
    output.layer_index = input.packed_id & 0xFFu;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let tex_color = textureSample(texture_array, texture_sampler, input.uv, i32(input.layer_index));
    
    let light_dir = normalize(vec3<f32>(0.3, 1.0, 0.5));
    let diffuse = max(dot(normalize(input.normal), light_dir), 0.0);
    let ambient = 0.3;
    let final_color = tex_color.rgb * (ambient + diffuse * 0.7) * input.ao;
    return vec4<f32>(final_color, tex_color.a);
}