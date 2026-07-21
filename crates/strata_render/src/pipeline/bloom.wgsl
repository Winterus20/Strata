// Bloom pass WGSL (M10c, plan 10 §11.3).
//
// One shader module, one entry per stage. Bind groups:
//
//   Bright extract  : @0 hdr_src, @1 bright_sampler, @2 bright_params
//   Blur H          : @0 blur_src,  @1 blur_sampler
//   Blur V          : @0 blur_src,  @1 blur_sampler
//   Composite       : @0 composite_params, @1 composite_sampler,
//                     @2..@(2+mip_count-1) mip textures (one per mip)
//
// All math is branchless. The Gaussian weights are constants; the 9 taps are
// unrolled into a fixed-size loop the WGSL compiler can constant-fold.

struct BloomParams {
  threshold: f32,
  intensity: f32,
  mip_count: u32,
  _pad: u32,
};

// NOTE: every binding below is declared at a module-unique @binding index. Naga
// requires all global resource variables in one shader module to have distinct
// (group, binding) pairs, even though each pipeline only uses a subset. The host
// bind-group layouts in `bloom.rs` mirror these exact indices per entry point.
@group(0) @binding(2) var<uniform> bright_params: BloomParams;
@group(0) @binding(5) var<uniform> composite_params: BloomParams;

struct VertexOutput {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VertexOutput {
  var p = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 3.0, -1.0),
    vec2<f32>(-1.0,  3.0)
  );
  var out: VertexOutput;
  out.pos = vec4<f32>(p[vi], 0.0, 1.0);
  out.uv = vec2<f32>((p[vi].x + 1.0) * 0.5, 1.0 - (p[vi].y + 1.0) * 0.5);
  return out;
}

// ---- bright extract ----
@group(0) @binding(0) var hdr_src: texture_2d<f32>;
@group(0) @binding(1) var bright_sampler: sampler;

@fragment
fn fs_bright(in: VertexOutput) -> @location(0) vec4<f32> {
  let src = textureSample(hdr_src, bright_sampler, in.uv).rgb;
  let bright = max(vec3<f32>(0.0), src - vec3<f32>(bright_params.threshold));
  return vec4<f32>(bright, 1.0);
}

// ---- separable blur (H + V) ----
@group(0) @binding(3) var blur_src: texture_2d<f32>;
@group(0) @binding(4) var blur_sampler: sampler;

// 9-tap separable Gaussian, sigma ~ 2.0. Hand-unrolled for branchless codegen
// and constant-folded by the WGSL compiler.
const BLUR_OFFSETS: array<f32, 9> = array<f32, 9>(
  -4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0
);
const BLUR_WEIGHTS: array<f32, 9> = array<f32, 9>(
  0.016, 0.054, 0.121, 0.194, 0.227, 0.194, 0.121, 0.054, 0.016
);

@fragment
fn fs_blur_h(in: VertexOutput) -> @location(0) vec4<f32> {
  let dims = vec2<f32>(textureDimensions(blur_src));
  let step = vec2<f32>(1.0 / dims.x, 0.0);
  return blur9(in.uv, step);
}

@fragment
fn fs_blur_v(in: VertexOutput) -> @location(0) vec4<f32> {
  let dims = vec2<f32>(textureDimensions(blur_src));
  let step = vec2<f32>(0.0, 1.0 / dims.y);
  return blur9(in.uv, step);
}

fn blur9(uv: vec2<f32>, step: vec2<f32>) -> vec4<f32> {
  var acc: vec3<f32> = vec3<f32>(0.0);
  for (var i: i32 = 0; i < 9; i = i + 1) {
    let o = step * BLUR_OFFSETS[i];
    let s = textureSample(blur_src, blur_sampler, uv + o).rgb;
    acc = acc + s * BLUR_WEIGHTS[i];
  }
  return vec4<f32>(acc, 1.0);
}

// ---- composite (additive bloom into the HDR target) ----
@group(0) @binding(6) var composite_sampler: sampler;
@group(0) @binding(7) var composite_tex0: texture_2d<f32>;
@group(0) @binding(8) var composite_tex1: texture_2d<f32>;
@group(0) @binding(9) var composite_tex2: texture_2d<f32>;
@group(0) @binding(10) var composite_tex3: texture_2d<f32>;
@group(0) @binding(11) var composite_tex4: texture_2d<f32>;

@fragment
fn fs_composite_5(in: VertexOutput) -> @location(0) vec4<f32> {
  // Each mip is weighted half its parent so the largest mip fades into a wide
  // halo without dominating. The host multiplies the final sum by
  // `composite_params.intensity` so the user can dial bloom from a single knob.
  let w0 = textureSample(composite_tex0, composite_sampler, in.uv).rgb;
  let w1 = textureSample(composite_tex1, composite_sampler, in.uv).rgb;
  let w2 = textureSample(composite_tex2, composite_sampler, in.uv).rgb;
  let w3 = textureSample(composite_tex3, composite_sampler, in.uv).rgb;
  let w4 = textureSample(composite_tex4, composite_sampler, in.uv).rgb;
  let bloom = w0
            + w1 * 0.5
            + w2 * 0.25
            + w3 * 0.125
            + w4 * 0.0625;
  return vec4<f32>(bloom * composite_params.intensity, 1.0);
}

// ---- present blit (M10b: ACES tonemap + sRGB encode; samples the HDR target
// that the bloom composite wrote into) ----
@group(0) @binding(12) var present_src: texture_2d<f32>;

// Approximate filmic ACES tonemap (Narkowicz fit), branchless.
fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51;
  let b = 0.03;
  let c = 2.43;
  let d = 0.59;
  let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Linear -> sRGB transfer, branchless.
fn linear_to_srgb(x: vec3<f32>) -> vec3<f32> {
  let cutoff = vec3<f32>(0.0031308);
  let lo = x * 12.92;
  let hi = 1.055 * pow(x, vec3<f32>(0.41666)) - 0.055;
  return select(hi, lo, x < cutoff);
}

@fragment
fn fs_present(in: VertexOutput) -> @location(0) vec4<f32> {
  let coord = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
  let hdr = textureLoad(present_src, coord, 0).rgb;
  let mapped = aces(hdr);
  let srgb = linear_to_srgb(mapped);
  return vec4<f32>(srgb, 1.0);
}
