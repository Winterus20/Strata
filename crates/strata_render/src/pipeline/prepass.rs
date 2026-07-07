//! GPU depth pre-pass: vertex-pulling greedy quads into the 64-bit visibility
//! buffer (M4c, plan 10 §1-§2).
//!
//! The pass draws every opaque quad as two triangles. The vertex shader pulls
//! the quad out of a storage buffer (no vertex buffers), expands it using its
//! face normal, and transforms it by `proj * view`. The fragment shader writes
//! a single `VisBufEntry` per covered pixel via `atomicMax`, so the nearest
//! fragment (reversed-Z) wins. All control flow is branchless (`select` /
//! arithmetic); no divergent `if` on the quad/pixel path.

use std::sync::Arc;

use wgpu::*;

/// Branchless WGSL for the depth pre-pass.
pub const PREPASS_WGSL: &str = r#"
struct CameraView {
  eye: vec4<f32>,
  view: mat4x4<f32>,
  proj: mat4x4<f32>,
  width: u32,
  height: u32,
  _pad0: u32,
  _pad1: u32,
};

// Mirror of PackedQuadGpu: two little-endian u32 = 8 bytes.
struct PackedQuad {
  data: array<u32, 2>,
};

@group(0) @binding(0) var<uniform> cam: CameraView;
@group(0) @binding(1) var<storage, read> quads: array<PackedQuad>;
@group(0) @binding(2) var<storage, read_write> visbuf: array<atomic<u64>>;

struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) @interpolate(flat) voxel_pos: u32,
  @location(1) @interpolate(flat) normal: u32,
  @location(2) @interpolate(flat) sector_id: u32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32,
           @builtin(instance_index) ii: u32) -> VOut {
  let q = quads[ii].data;
  let geom = q[0];
  let x = geom & 0x1Fu;
  let y = (geom >> 5u) & 0x1Fu;
  let z = (geom >> 10u) & 0x1Fu;
  let w = (geom >> 15u) & 0x3Fu;
  let h = (geom >> 21u) & 0x3Fu;
  let face = (geom >> 27u) & 0x7u;

  // Two triangles over the quad's 4 corners (no divergent branching).
  let corners = array<vec2<u32>, 6>(
    vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(0u, 1u),
    vec2<u32>(0u, 1u), vec2<u32>(1u, 0u), vec2<u32>(1u, 1u)
  );
  let c = corners[vi];
  let du = c.x;
  let dv = c.y;

  let axis = face / 2u;
  let uaxis = (axis + 1u) % 3u;
  let vaxis = (axis + 2u) % 3u;

  var p = vec3<f32>(f32(x), f32(y), f32(z));
  p[uaxis] = p[uaxis] + f32(du * w);
  p[vaxis] = p[vaxis] + f32(dv * h);

  let clip = cam.proj * cam.view * vec4<f32>(p, 1.0);

  var out: VOut;
  out.pos = clip;
  // 15-bit local voxel position packed into the low 24 bits.
  out.voxel_pos = x | (y << 5u) | (z << 10u);
  out.normal = face;
  out.sector_id = 0u;
  return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
  // WebGPU NDC depth: position.z in [0,1], 0 = near, 1 = far. Reversed-Z so a
  // nearer fragment yields a larger magnitude and wins the atomicMax.
  let rev = 1.0 - in.pos.z;
  let depth = u32(rev * 16777215.0); // 2^24 - 1, saturated by construction.

  let entry = (u64(in.voxel_pos) & u64(0xFFFFFFu))
            | ((u64(in.sector_id) & u64(0xFFFu)) << 24u)
            | ((u64(in.normal) & u64(0x7u)) << 37u)
            | ((u64(depth) & u64(0xFFFFFFu)) << 40u);

  var pix = u32(floor(in.pos.y) * f32(cam.width) + floor(in.pos.x));
  pix = min(pix, cam.width * cam.height - 1u);
  atomicMax(&visbuf[pix], entry);

  return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#;

/// Color attachment format for the (discarded) pre-pass color target.
pub const PREPASS_COLOR_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Bind group layout for the pre-pass: uniform camera + read-only quad SSBO +
/// read-write atomic visbuf SSBO.
pub fn prepass_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("strata_prepass_bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Build the depth pre-pass render pipeline. `color_format` is the dummy color
/// target that absorbs the fragment output (the real result lands in `visbuf`).
pub fn prepass_pipeline(
    device: &Device,
    layout: &BindGroupLayout,
    color_format: TextureFormat,
) -> RenderPipeline {
    let module = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("strata_prepass_shader"),
        source: ShaderSource::Wgsl(PREPASS_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("strata_prepass_layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("strata_prepass_pipeline"),
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
                format: color_format,
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

/// Recreate a storage buffer (used for the quad SSBO) sized for `quad_count`
/// [`PackedQuadGpu`] entries, returning the new buffer.
pub fn make_quad_buffer(device: &Device, quad_count: usize) -> Arc<Buffer> {
    let size = (quad_count.max(1) * std::mem::size_of::<crate::pipeline::PackedQuadGpu>()) as u64;
    Arc::new(device.create_buffer(&BufferDescriptor {
        label: Some("strata_prepass_quads"),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }))
}
