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
  inv_view_proj: mat4x4<f32>,
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
// Per-quad world origin (sector_coord * 32) so every sector's local 0..31 quad
// geometry is placed at its real world position. `.xyz` used; `.w` is padding.
@group(0) @binding(3) var<storage, read> origins: array<vec4<f32>>;
// Reserved remapping slot (identity today). Draw paths use
// `draw(0..6, base..base+count)` so `instance_index` is already the global
// SSBO / lightmap slot — see `out.quad_id = ii` below. Kept live so the bind
// group layout stays stable for optional non-identity uploads.
@group(0) @binding(4) var<storage, read> quad_ids: array<u32>;
// M11 visbuf v2: per-quad sector id (0..N-1 over the meshes in the AOI). Lets
// the resolve shader disambiguate pixels when multiple sectors' quads are
// rasterized into the same visbuf. Allocated parallel to `quads`; the same
// instance index is reused.
@group(0) @binding(5) var<storage, read> sector_ids: array<u32>;

struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) @interpolate(flat) voxel_pos: u32,
  @location(1) @interpolate(flat) normal: u32,
  @location(2) @interpolate(flat) block_id: u32,
  // All four corner AO values, packed 2 bits each (c0[0:2] c1[2:4] c2[4:6] c3[6:8]).
  // The fragment shader pulls the corner matching this vertex's (du, dv) and
  // writes the 4-corner byte into the visbuf so resolve can bi-linearly
  // interpolate across the quad surface (Exile / Andre Blunt Quad Interpolation).
  @location(3) @interpolate(flat) ao_corners: u32,
  @location(4) @interpolate(flat) quad_id: u32,
  @location(5) @interpolate(flat) sector_id: u32,
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

  let block_id = q[1] & 0xFFu;
  let ao_byte = (q[1] >> 8u) & 0xFFu;
  let c0 = ao_byte & 0x3u;
  let c1 = (ao_byte >> 2u) & 0x3u;
  let c2 = (ao_byte >> 4u) & 0x3u;
  let c3 = (ao_byte >> 6u) & 0x3u;
  let flags = (q[1] >> 24u) & 0xFFu;
  // 0fps.net anisotropy fix, branchless: CPU decided once per quad whether the
  // diagonal goes (c0,c1,c2)->(c0,c1,c3) (flip=1) or (c0,c1,c3)->(c1,c2,c3)
  // (flip=0). `select` on a single bit keeps the vertex shader divergent-free.
  let flip = (flags & 0x1u) == 1u;

  // Index tables for the 6 triangle vertices; same six positions in both halves
  // because the 0fps rule is `a00+a11 > a01+a10`. The two triangles are:
  //   !flip:  T0=(0,0)(1,0)(0,1)  T1=(0,1)(1,0)(1,1)   (corners c0, c1, c2 / c2, c1, c3)
  //    flip:  T0=(0,0)(1,0)(1,1)  T1=(0,0)(1,1)(0,1)   (corners c0, c1, c3 / c0, c3, c2)
  // Using `select` with a single bit keeps the shader branchless — no divergent
  // wavefronts. The per-vertex (du, dv) of the chosen triangle is `corners[vi]`.
  //
  // **M10a.3 AO corner index mirror**: the `ao` field of `VOut` is now the full
  // 4-corner byte (c0|c1|c2|c3 packed as 2 bits each), not a single selected
  // value. The fragment shader writes the whole byte to the visbuf, and the
  // resolve shader does the bi-linear interpolation. We must therefore
  // compute the (du, dv) BEFORE consuming `ao` so the resolve shader can
  // match the same per-vertex assignment.
  let corners_no_flip = array<vec2<u32>, 6>(
    vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(0u, 1u),
    vec2<u32>(0u, 1u), vec2<u32>(1u, 0u), vec2<u32>(1u, 1u)
  );
  let corners_flip = array<vec2<u32>, 6>(
    vec2<u32>(0u, 0u), vec2<u32>(1u, 0u), vec2<u32>(1u, 1u),
    vec2<u32>(0u, 0u), vec2<u32>(1u, 1u), vec2<u32>(0u, 1u)
  );
  let c = select(corners_no_flip[vi], corners_flip[vi], flip);
  let du = c.x;
  let dv = c.y;  let axis = face / 2u;
  let uaxis = (axis + 1u) % 3u;
  let vaxis = (axis + 2u) % 3u;

  var p = vec3<f32>(f32(x), f32(y), f32(z));
  // The CPU packs the owning voxel (0..31) to keep the 5-bit position field from
  // overflowing at the sector boundary (a +d face plane would be 32). Advance +d
  // faces (even face index) by one voxel here, in float space, to reach the true
  // plane. `-d` faces (odd index) stay at the owning voxel.
  p[axis] = p[axis] + f32(1u - (face & 1u));
  // Distance-aware UV-plane expand: adjacent greedy quads / sector T-junctions
  // leave sub-pixel gaps under perspective. Uncovered visbuf pixels stay at the
  // cleared sentinel → resolve draws sky/fog, which reads as a black grid that
  // thickens with distance. 0.0015 was far too small once a voxel is ~1px.
  // Near: ~1cm seals seams; far: up to ~5cm so coverage still spans a pixel.
  // Expand stays in-plane (no normal push) to avoid z-fight with coplanar faces.
  var p_est = p;
  p_est[uaxis] = p_est[uaxis] + 0.5 * f32(w);
  p_est[vaxis] = p_est[vaxis] + 0.5 * f32(h);
  p_est = p_est + origins[ii].xyz;
  let dist = length(p_est - cam.eye.xyz);
  let expand = 0.01 + 0.04 * clamp(dist * (1.0 / 160.0), 0.0, 1.0);
  p[uaxis] = p[uaxis] + f32(du * w) + (f32(du) * 2.0 - 1.0) * expand;
  p[vaxis] = p[vaxis] + f32(dv * h) + (f32(dv) * 2.0 - 1.0) * expand;
  // Translate sector-local coords into world space by the per-quad origin.
  p = p + origins[ii].xyz;

  let clip = cam.proj * cam.view * vec4<f32>(p, 1.0);

  var out: VOut;
  out.pos = clip;
  // 15-bit local voxel position (5+5+5) for visbuf v5.
  out.voxel_pos = x | (y << 5u) | (z << 10u);
  out.normal = face;
  out.block_id = block_id & 0xFu;
  out.ao_corners = ao_byte;
  // Global SSBO slot == lightmap index when draws use first_instance=base.
  // Keep quad_ids / sector_ids bindings live (layout + optional remapping).
  let _qid_keep = arrayLength(&quad_ids);
  let _sid_keep = arrayLength(&sector_ids);
  out.quad_id = ii;
  out.sector_id = sector_ids[ii] & 0xFu;
  return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
  // WebGPU NDC depth: position.z in [0,1], 0 = near, 1 = far. Reversed-Z so a
  // nearer fragment yields a larger magnitude and wins the atomicMax.
  // M12 visbuf v5: 13-bit reversed-Z depth; 21-bit quad_id (up to 2M SSBO slots).
  let rev = 1.0 - in.pos.z;
  let depth = u32(rev * 8191.0);

  // Visbuf layout (v5):
  //   bit[0:15]   voxel_pos  (15b),
  //   bit[15:19]  block_id   (4b),
  //   bit[19:27]  ao_corners (8b = 4 corners x 2b),
  //   bit[27:48]  quad_id    (21b, global SSBO / lightmap slot),
  //   bit[48:51]  normal     (3b),
  //   bit[51:64]  depth      (13b, reversed-Z).
  // sector_id stays on the interpolant for binding liveness but is not packed;
  // those bits extend quad_id past the old 64K truncation.
  let _sid_alive = in.sector_id;
  let entry = (u64(in.voxel_pos) & u64(0x7FFFu))
            | ((u64(in.block_id) & u64(0xFu)) << 15u)
            | ((u64(in.ao_corners) & u64(0xFFu)) << 19u)
            | ((u64(in.quad_id) & u64(0x1FFFFFu)) << 27u)
            | ((u64(in.normal) & u64(0x7u)) << 48u)
            | ((u64(depth) & u64(0x1FFFu)) << 51u);

  var pix = u32(floor(in.pos.y) * f32(cam.width) + floor(in.pos.x));
  pix = min(pix, cam.width * cam.height - 1u);
  atomicMax(&visbuf[pix], entry);

  return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}
"#;

/// Color attachment format for the (discarded) pre-pass color target.
pub const PREPASS_COLOR_FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

/// Bind group layout for the pre-pass: uniform camera + read-only quad SSBO +
/// read-write atomic visbuf SSBO + per-quad world origin + per-quad id +
/// per-quad sector id (M11).
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
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 4,
                visibility: ShaderStages::VERTEX,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 5,
                visibility: ShaderStages::VERTEX,
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
        // COPY_SRC is required so `Renderer::ensure_quad_capacity` can copy the
        // old buffer into a larger one when the slot allocator grows the SSBO
        // (streaming fragmentation bumps `next_base` past capacity). Without it
        // wgpu rejects the grow copy with a validation error.
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }))
}

/// Recreate the per-quad world-origin SSBO sized for `quad_count` `vec4<f32>`
/// entries (16 bytes each, `.xyz` = sector world offset, `.w` = padding).
pub fn make_origins_buffer(device: &Device, quad_count: usize) -> Arc<Buffer> {
    let size = (quad_count.max(1) * std::mem::size_of::<[f32; 4]>()) as u64;
    Arc::new(device.create_buffer(&BufferDescriptor {
        label: Some("strata_prepass_origins"),
        size,
        // COPY_SRC: same reason as the quad buffer — the grow path copies the
        // old origins buffer into the enlarged one.
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }))
}

/// Recreate the per-quad id SSBO sized for `quad_count` `u32` entries. Each
/// entry is the sector-local quad index (0..N-1) so the resolve shader can
/// index into the per-sector lightmap (M10a.4).
pub fn make_quad_ids_buffer(device: &Device, quad_count: usize) -> Arc<Buffer> {
    let size = (quad_count.max(1) * std::mem::size_of::<u32>()) as u64;
    Arc::new(device.create_buffer(&BufferDescriptor {
        label: Some("strata_prepass_quad_ids"),
        size,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }))
}

/// Recreate the per-quad sector-id SSBO sized for `quad_count` `u32` entries
/// (M11). Each entry is the sector index in the AOI that the quad belongs to
/// (0..N-1 over the `meshes` slice), parallel to the quad / origins / quad_ids
/// SSBOs and indexed by the same `instance_index`. WGSL masks to 4 bits.
pub fn make_sector_ids_buffer(device: &Device, quad_count: usize) -> Arc<Buffer> {
    let size = (quad_count.max(1) * std::mem::size_of::<u32>()) as u64;
    Arc::new(device.create_buffer(&BufferDescriptor {
        label: Some("strata_prepass_sector_ids"),
        size,
        // COPY_SRC: same reason as the quad buffer — the grow path copies the
        // old sector_ids buffer into the enlarged one.
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    }))
}
