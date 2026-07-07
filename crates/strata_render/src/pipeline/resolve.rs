//! Color-resolve pass (M4d, plan 10 §1-§2).
//!
//! A fullscreen-triangle pass that runs *after* the depth pre-pass. The fragment
//! shader reads the 64-bit visibility buffer (`visbuf`) per pixel:
//! * `entry == 0` (cleared sentinel) -> sky gradient (branchless mix of horizon /
//!   zenith colors by screen-space y).
//! * otherwise -> decode the face normal, apply constant-sun Lambert lighting with
//!   face-based albedo (top brightest, bottom darkest, sides tinted), ACES tonemap,
//!   and write the HDR color into the offscreen `rgba16float` target.
//!
//! All control flow is branchless (`select` / `min` / arithmetic / array index);
//! no divergent `if` on the pixel path.

use bytemuck::{Pod, Zeroable};

use wgpu::*;

/// Default sky horizon color (linear-ish HDR, before tonemap).
pub const HORIZON: [f32; 4] = [0.55, 0.70, 0.95, 1.0];
/// Default sky zenith color.
pub const ZENITH: [f32; 4] = [0.18, 0.42, 0.92, 1.0];

/// Uniform fed to the resolve fragment shader.
///
/// Offsets (std140-friendly, 16-byte aligned):
/// * `0`  — `width`  : `u32`
/// * `4`  — `height` : `u32`
/// * `8`  — `_pad`   : `u32` x2
/// * `16` — `horizon`: `vec4<f32>`
/// * `32` — `zenith` : `vec4<f32>`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ResolveParams {
    pub width: u32,
    pub height: u32,
    pub _pad: [u32; 2],
    pub horizon: [f32; 4],
    pub zenith: [f32; 4],
}

impl ResolveParams {
    /// Build the params for a framebuffer of `width`×`height`.
    #[inline]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            _pad: [0; 2],
            horizon: HORIZON,
            zenith: ZENITH,
        }
    }
}

/// Branchless WGSL for the color-resolve pass.
pub const RESOLVE_WGSL: &str = r#"
struct ResolveParams {
  width: u32,
  height: u32,
  _pad0: u32,
  _pad1: u32,
  horizon: vec4<f32>,
  zenith: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: ResolveParams;
@group(0) @binding(1) var<storage, read> visbuf: array<u64>;

struct VOut {
  @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
  // Fullscreen triangle (covers the whole clip volume, no vertex buffer).
  var p = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 3.0, -1.0),
    vec2<f32>(-1.0,  3.0)
  );
  var out: VOut;
  out.pos = vec4<f32>(p[vi], 0.0, 1.0);
  return out;
}

// Approximate filmic ACES tonemap (Narkowicz fit), branchless.
fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51;
  let b = 0.03;
  let c = 2.43;
  let d = 0.59;
  let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
  let width = params.width;
  let height = params.height;
  let px = min(u32(in.pos.x), width - 1u);
  let py = min(u32(in.pos.y), height - 1u);
  let pix = py * width + px;
  let entry = visbuf[pix];

  // Sky gradient by screen-space y (branchless, normalized t in [0,1]).
  let t = clamp(in.pos.y / f32(height), 0.0, 1.0);
  let sky = mix(params.horizon.rgb, params.zenith.rgb, t);

  // Empty == cleared sentinel (0). select(false, true, cond).
  let is_empty = select(0.0, 1.0, entry == u64(0));

  // Decode face normal (3 bits at [37:39]).
  let n_idx = u32((entry >> u32(37)) & u64(0x7));
  var normals = array<vec3<f32>, 6>(
    vec3<f32>( 1.0,  0.0,  0.0),
    vec3<f32>(-1.0,  0.0,  0.0),
    vec3<f32>( 0.0,  1.0,  0.0),
    vec3<f32>( 0.0, -1.0,  0.0),
    vec3<f32>( 0.0,  0.0,  1.0),
    vec3<f32>( 0.0,  0.0, -1.0)
  );
  let n = normals[n_idx];

  // Constant sun + ambient Lambert term.
  let sun = normalize(vec3<f32>(0.4, 0.85, 0.3));
  let ambient = 0.28;
  let lambert = max(dot(n, sun), 0.0);

  // Face-based albedo: top brightest, bottom darkest, sides tinted by axis.
  let up = max(n.y, 0.0);
  let down = max(-n.y, 0.0);
  let side = 1.0 - up - down;
  let ax = abs(n.x);
  let az = abs(n.z);
  let side_tint = mix(vec3<f32>(0.95, 0.74, 0.58),
                      vec3<f32>(0.58, 0.80, 0.95),
                      az / (ax + az + 1e-3));
  let albedo = vec3<f32>(0.55) * up + vec3<f32>(0.30) * down + side_tint * side;

  let lit = albedo * (ambient + (1.0 - ambient) * lambert);
  let color = aces(lit);

  // Branchless blend: geometry where present, sky elsewhere.
  let rgb = mix(color, sky, is_empty);
  return vec4<f32>(rgb, 1.0);
}
"#;

/// Bind group layout: uniform params + read-only visbuf storage.
pub fn resolve_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("strata_resolve_bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Build the color-resolve render pipeline (fullscreen triangle -> `rgba16float`).
pub fn resolve_pipeline(device: &Device, layout: &BindGroupLayout) -> RenderPipeline {
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("strata_resolve_shader"),
        source: ShaderSource::Wgsl(RESOLVE_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("strata_resolve_layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("strata_resolve_pipeline"),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format: TextureFormat::Rgba16Float,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}
