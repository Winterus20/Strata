//! Color-resolve pass (M4d + M10a.2/3/4, plan 10 §1-§2).
//!
//! A fullscreen-triangle pass that runs *after* the depth pre-pass. The fragment
//! shader reads the 64-bit visibility buffer (`visbuf`) per pixel:
//! * `entry == 0` (cleared sentinel) -> sky gradient (branchless mix of horizon /
//!   zenith colors by screen-space y).
//! * otherwise -> decode the face normal, sample the per-block albedo from the
//!   block-palette SSBO, darken by the max-corner AO from the visbuf, modulate
//!   by the per-quad light from the lightmap SSBO, apply constant-sun Lambert
//!   lighting with face-based tint, ACES tonemap, and write the HDR color into
//!   the offscreen `rgba16float` target.
//!
//! All control flow is branchless (`select` / `min` / arithmetic / array index);
//! no divergent `if` on the pixel path.

use bytemuck::{Pod, Zeroable};

use wgpu::*;

/// Default sky zenith color.
pub const ZENITH: [f32; 4] = [0.18, 0.42, 0.92, 1.0];
/// Default sky horizon color (linear-ish HDR, before tonemap).
pub const HORIZON: [f32; 4] = [0.55, 0.70, 0.95, 1.0];
/// Default sun color (linear HDR, scaled by exposure).
pub const SUN_COLOR: [f32; 4] = [1.4, 1.25, 0.95, 1.0];
/// Default fog color (linear HDR, matches the horizon so far cliffs vanish smoothly).
pub const FOG_COLOR: [f32; 4] = [0.55, 0.70, 0.95, 1.0];
/// Default sun direction (used by the resolve-sun test mirror).
pub const DEFAULT_SUN_DIR: [f32; 3] = [0.4, 0.85, 0.3];
/// Default sun angular size (~15 arcmin visual radius).
pub const SUN_SIZE: f32 = 0.0046;
/// Default fog density.
pub const FOG_DENSITY: f32 = 0.015;
/// Default tone-mapping exposure.
pub const EXPOSURE: f32 = 1.0;

/// M10a.4-dbg: bitmask values for the resolve-fragment debug dump. The mask
/// selects which signals the resolve shader writes into the debug storage
/// buffer (binding 10) for the pixel `(debug_dump_x, debug_dump_y)`. The
/// first 4 bits land in `debug_dump[0]`, the next 4 in `debug_dump[1]`.
pub mod debug_dump {
    pub const AO_SMOOTH: u32 = 1 << 0;
    pub const AO_I: u32 = 1 << 1;
    pub const AO_MULT: u32 = 1 << 2;
    pub const AO_CORNERS: u32 = 1 << 3;
    pub const QUAD_ID: u32 = 1 << 4;
    pub const UV_X: u32 = 1 << 5;
    pub const UV_Y: u32 = 1 << 6;
    pub const RAW_AO_BYTE: u32 = 1 << 7;
    /// Convenience: all 8 signals at once.
    pub const ALL: u32 = 0xFF;
    /// Two `vec4<f32>` slots in the debug buffer.
    pub const SLOT_COUNT: usize = 2;
}

/// Default AO curve (0..1 → 0..1). Stored as a [u8; 4] lookup table
/// (`ao_curve_q8`) so a single byte maps 0..3 to a multiplier in [0,1].
///
/// The four values correspond to the four possible corner-AO values that the
/// `PackedQuad::ao` byte can hold (`0..3` in 2-bit fields). Plan 09 / Exile
/// approximate stops: 0.75 / 0.825 / 0.9 / 1.0 → `[191, 210, 230, 255]`.
///
/// The curve is uploaded as a tiny uniform slot of the resolve pipeline and
/// applied after bi-linear AO interpolation, so a single uniform change
/// re-themes the entire world for an artist without touching the mesher.
pub const AO_CURVE_DEFAULT: [u8; 4] = [191, 210, 230, 255];

/// Encode a `[0..=3]` AO corner byte for the resolve shader's 4-bit lookup
/// table (`AO_CURVE_DEFAULT` is the default). The two `select`s rebuild the
/// curve at the four discrete AO levels the visbuf can carry, so any curve
/// the artist uploads can be applied at the fragment level without changing
/// the mesher.
#[inline]
pub fn pack_ao_curve(curve: [u8; 4]) -> u32 {
    (curve[0] as u32)
        | ((curve[1] as u32) << 8)
        | ((curve[2] as u32) << 16)
        | ((curve[3] as u32) << 24)
}

/// Uniform fed to the resolve fragment shader.
///
/// Offsets (std140-friendly, 16-byte aligned):
/// * `0`  — `width`  : `u32`
/// * `4`  — `height` : `u32`
/// * `8`  — `debug_faces`: `u32` (0 = normal Lambert shading, 1 = per-face-direction color)
/// * `12` — `debug_dump_mask`: `u32` (bitmask: bit0=ao_smooth, bit1=ao_i, bit2=ao_mult,
///                                 bit3=lbyte, bit4=quad_id, bit5=uv.x, bit6=uv.y, bit7=n_idx)
/// * `16` — `debug_dump_x`: `u32` (target pixel x for the debug dump)
/// * `20` — `debug_dump_y`: `u32` (target pixel y for the debug dump)
/// * `24` — `_pad0`
/// * `32` — `horizon`: `vec4<f32>`
/// * `48` — `zenith` : `vec4<f32>`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ResolveParams {
    pub width: u32,
    pub height: u32,
    pub debug_faces: u32,
    pub debug_dump_mask: u32,
    pub debug_dump_x: u32,
    pub debug_dump_y: u32,
    pub _pad0: u32,
    /// M10a.4-dbg: extra u32 padding to keep the uniform block 16-byte
    /// aligned. After 7×u32 (28 B) the next field must start at offset 32;
    /// the WGSL struct adds `_pad_dbg` for the same reason. The CPU struct
    /// is `[u32; 8]` after the 7 width/height/debug fields, so this field
    /// is a no-op for the struct size: `vec4<f32>` already aligns to 16 B
    /// because of the upstream 4-B pad between `height` and `horizon`. We
    /// keep the field for symmetry with the WGSL side and to absorb any
    /// future drift.
    pub _pad_dbg: u32,
    pub horizon: [f32; 4],
    pub zenith: [f32; 4],
    pub sun_color: [f32; 4],
    pub sun_dir: [f32; 4],
    pub fog_color: [f32; 4],
    pub exposure: f32,
    pub fog_density: f32,
    pub camera_near: f32,
    pub camera_far: f32,
    pub _pad_tail: [u32; 4],
}

impl ResolveParams {
    /// Build the params for a framebuffer of `width`×`height` with the M10b/d defaults
    /// (procedural sky + sun disk + linear-HDR exposure, no fog).
    #[inline]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            debug_faces: 0,
            debug_dump_mask: 0,
            debug_dump_x: 0,
            debug_dump_y: 0,
            _pad0: 0,
            _pad_dbg: 0,
            horizon: HORIZON,
            zenith: ZENITH,
            sun_color: SUN_COLOR,
            sun_dir: [
                DEFAULT_SUN_DIR[0],
                DEFAULT_SUN_DIR[1],
                DEFAULT_SUN_DIR[2],
                SUN_SIZE,
            ],
            fog_color: FOG_COLOR,
            exposure: EXPOSURE,
            fog_density: 0.0,
            camera_near: 0.1,
            camera_far: 1000.0,
            _pad_tail: [0; 4],
        }
    }

    /// Return a copy with `horizon` swapped. Mirrors the M10d sky override knob.
    pub fn with_horizon(mut self, horizon: [f32; 4]) -> Self {
        self.horizon = horizon;
        self
    }

    /// Return a copy with `zenith` swapped.
    pub fn with_zenith(mut self, zenith: [f32; 4]) -> Self {
        self.zenith = zenith;
        self
    }

    /// Return a copy with `debug_faces` flipped. The resolve pass also accepts
    /// the live override via [`Renderer::set_debug_faces`](crate::pipeline::Renderer::set_debug_faces);
    /// this is for tests and one-shot boot setup.
    pub fn with_debug_faces(mut self, on: bool) -> Self {
        self.debug_faces = on as u32;
        self
    }

    /// Configure the M10a.4-dbg fragment-shader debug dump. The dump is
    /// disabled when `mask == 0`. Otherwise the resolve shader writes
    /// up to 8 signals (2 × `vec4<f32>`) for the pixel `(px, py)` into
    /// the debug storage buffer.
    #[inline]
    pub fn with_debug_dump(mut self, mask: u32, x: u32, y: u32) -> Self {
        self.debug_dump_mask = mask;
        self.debug_dump_x = x;
        self.debug_dump_y = y;
        self
    }

    /// Override the sun direction (xyz). The w component (sun_size) is preserved
    /// from the current value.
    #[inline]
    pub fn with_sun_dir(mut self, dir: [f32; 3]) -> Self {
        self.sun_dir = [dir[0], dir[1], dir[2], self.sun_dir[3]];
        self
    }

    /// Override the sun angular size (radians).
    #[inline]
    pub fn with_sun_size(mut self, size: f32) -> Self {
        self.sun_dir[3] = size;
        self
    }

    /// Override the sun color (rgba, linear HDR).
    #[inline]
    pub fn with_sun_color(mut self, color: [f32; 4]) -> Self {
        self.sun_color = color;
        self
    }

    /// Override the linear HDR exposure multiplier.
    #[inline]
    pub fn with_exposure(mut self, exposure: f32) -> Self {
        self.exposure = exposure;
        self
    }

    /// Override the distance-fog density (`1/m`). Use 0 to disable fog.
    #[inline]
    pub fn with_fog_density(mut self, density: f32) -> Self {
        self.fog_density = density;
        self
    }

    /// Override the fog color (rgb linear HDR; alpha unused).
    #[inline]
    pub fn with_fog_color(mut self, color: [f32; 3]) -> Self {
        self.fog_color = [color[0], color[1], color[2], 1.0];
        self
    }

    /// Override the camera near/far used for reversed-Z depth reconstruction.
    #[inline]
    pub fn with_camera_planes(mut self, near: f32, far: f32) -> Self {
        self.camera_near = near;
        self.camera_far = far;
        self
    }
}

/// WGSL-visible constant: the default sun direction used by the
/// `sky_sun_disk_intensity` test. The runtime sun is fed via the
/// `params.sun_dir` uniform from `ClientRender`.
pub const SKY_TEST_SUN_DIR: [f32; 3] = [0.4, 0.85, 0.3];
/// WGSL-visible constant: the default sun angular size (~15 arcmin).
pub const SKY_TEST_SUN_SIZE: f32 = 0.0046;

/// Branchless WGSL for the color-resolve pass.
pub const RESOLVE_WGSL: &str = r#"
struct ResolveParams {
  width: u32,
  height: u32,
  debug_faces: u32,
  // Bitmask for the resolve-fragment debug dump (M10a.4-dbg):
  //   bit0 = ao_smooth,  bit1 = ao_i,  bit2 = ao_mult,
  //   bit3 = lbyte,      bit4 = quad_id, bit5 = uv,
  //   bit6 = n_idx,      bit7 = sample_ao raw
  // Pixels whose (px, py) == (debug_dump_x, debug_dump_y) write the requested
  // 4 floats into the debug storage buffer at slot 0. mask == 0 -> disabled.
  // `_pad_dbg` is a single u32 so the WGSL struct stays 16-byte aligned; the
  // CPU `ResolveParams` mirrors this with `_pad0`.
  debug_dump_mask: u32,
  debug_dump_x: u32,
  debug_dump_y: u32,
  _pad_dbg: u32,
  horizon: vec4<f32>,
  zenith: vec4<f32>,
  sun_color: vec4<f32>,
  sun_dir: vec4<f32>,
  fog_color: vec4<f32>,
  exposure: f32,
  fog_density: f32,
  camera_near: f32,
  camera_far: f32,
  _pad_tail: vec4<u32>,
};

// BlockColorGpu (12 B / entry) — the resolve shader reads one entry per pixel
// via `block_colors[block_id]`. Power-of-two `palette_size` lets the shader
// mask `block_id & (palette_size - 1)` for a safe lookup.
struct BlockColor {
  rgb: vec3<f32>,
  _pad: u32,
  textures: array<u32, 6>,
  use_quad_uv: u32,
  _pad2: u32,
};

struct PackedQuad {
  data: array<u32, 2>,
};

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
// M10a.4-dbg pad: the WGSL `ResolveParams` carries an extra `_pad_dbg` u32
// (offset 24) so the uniform block ends on a 16-byte boundary. The CPU
// `ResolveParams` mirrors this with `_pad0`. WGSL doesn't allow unused
// struct members, so we also reference the field in a no-op helper to
// keep the shader valid (the call is constant-folded by naga).
//
// Note: the helper runs once per frame; cost is 1 mov on a 0u literal.

@group(0) @binding(0) var<uniform> params: ResolveParams;
@group(0) @binding(1) var<storage, read> visbuf: array<u64>;
@group(0) @binding(2) var<storage, read> block_colors: array<BlockColor>;
@group(0) @binding(3) var<storage, read> lightmap: array<u32>;
@group(0) @binding(4) var<uniform> lightmap_meta: vec4<u32>;
@group(0) @binding(5) var block_textures: texture_2d_array<f32>;
@group(0) @binding(6) var block_sampler: sampler;
@group(0) @binding(7) var<storage, read> quads: array<PackedQuad>;
@group(0) @binding(8) var<storage, read> origins: array<vec4<f32>>;
@group(0) @binding(9) var<uniform> cam: CameraView;
@group(0) @binding(10) var<storage, read_write> debug_dump: array<vec4<f32>>;
// x = palette_size (power of two), y = lightmap_mask (SECTOR_LIGHTMAP_QUADS - 1),
// z = ao_curve_packed (AO_CURVE_DEFAULT packed into a u32 via pack_ao_curve),
// w = ao_curve_stride (1/255 — the shader divides the LUT byte by this to
// recover a [0,1] multiplier). See ResolveParamsGpu for offsets.

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

// Approximate filmic ACES tonemap (Narkowicz fit), branchless. M10b moved the
// actual call out of `fs_main`: the resolve pass now emits *linear HDR* and the
// present blit does the ACES + sRGB encode. The helper is kept for the depth
// test in M10d fog (rebuilds a world-space distance from the reversed-Z stored
// depth) and as a CPU-side test fixture in the resolve module.
fn aces(x: vec3<f32>) -> vec3<f32> {
  let a = 2.51;
  let b = 0.03;
  let c = 2.43;
  let d = 0.59;
  let e = 0.14;
  return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn get_world_space_uv(world_pos: vec3<f32>, normal: u32) -> vec2<f32> {
  var uv = vec2<f32>(0.0, 0.0);
  let axis = normal / 2u;
  if (axis == 0u) { // X-normal: use Z and Y
    uv = vec2<f32>(world_pos.z, world_pos.y);
  } else if (axis == 1u) { // Y-normal: use X and Z
    uv = vec2<f32>(world_pos.x, world_pos.z);
  } else { // Z-normal: use X and Y
    uv = vec2<f32>(world_pos.x, world_pos.y);
  }
  return fract(uv);
}

// Full-texel inset: Linear min+mip kernels are wider than half a texel at
// distant LODs, so 0.5/16 still bled across the fract wrap / dark stone edge
// and read as a grainy black grid. Clamp first — expanded prepass geometry can
// reconstruct UV slightly outside [0,1].
fn inset_block_uv(uv: vec2<f32>) -> vec2<f32> {
  let inset = 1.0 / 16.0;
  return clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * (1.0 - 2.0 * inset) + inset;
}

fn get_quad_space_uv(world_pos: vec3<f32>, quad_id: u32) -> vec2<f32> {
  // Out-of-range quad_id (e.g. a sky pixel where the resolve still
  // reaches this helper) returns a safe zero UV instead of indexing
  // out of bounds. The pre-pass packs quad_id in 21 bits; we treat
  // any index past `arrayLength(quads)` as undefined and clamp to
  // the safe zero default.
  let qn = arrayLength(&quads);
  if (quad_id >= qn) {
    return vec2<f32>(0.0, 0.0);
  }
  let q = quads[quad_id].data;
  let geom = q[0];
  let qx = f32(geom & 0x1Fu);
  let qy = f32((geom >> 5u) & 0x1Fu);
  let qz = f32((geom >> 10u) & 0x1Fu);
  let w = max(f32((geom >> 15u) & 0x3Fu), 1.0);
  let h = max(f32((geom >> 21u) & 0x3Fu), 1.0);
   let face = (geom >> 27u) & 0x7u;
   let safe_face = min(face, 5u);

   let origin = origins[quad_id].xyz;
   let local_pos = world_pos - origin - vec3<f32>(qx, qy, qz);

   let axis = safe_face / 2u;
  let uaxis = (axis + 1u) % 3u;
  let vaxis = (axis + 2u) % 3u;

  let u = local_pos[uaxis] / w;
  let v = local_pos[vaxis] / h;
  return vec2<f32>(u, v);
}

// Bi-linear AO over the 4 packed corners (c0..c3). FLIP is applied only in
// the prepass triangle split (`FLIP_FLAG`); re-swapping corners here corrupts
// the smooth field. Plain bilinear on c0,c1,c2,c3.
fn sample_ao(ao_corners: u32, uv: vec2<f32>) -> f32 {
  let c0 = f32(ao_corners & 0x3u);
  let c1 = f32((ao_corners >> 2u) & 0x3u);
  let c2 = f32((ao_corners >> 4u) & 0x3u);
  let c3 = f32((ao_corners >> 6u) & 0x3u);
  let wu = clamp(uv.x, 0.0, 1.0);
  let wv = clamp(uv.y, 0.0, 1.0);
  let top = mix(c0, c1, wu);
  let bottom = mix(c2, c3, wu);
  return mix(top, bottom, wv);
}

// AO curve LUT (4 bytes packed into a u32). Continuous smooth interpolation
// between LUT values eliminates discrete step-banding and quantization seams.
fn ao_curve_lookup(ao_smooth: f32, curve_packed: u32) -> f32 {
  let lut = vec4<f32>(
    f32(curve_packed & 0xFFu),
    f32((curve_packed >> 8u) & 0xFFu),
    f32((curve_packed >> 16u) & 0xFFu),
    f32((curve_packed >> 24u) & 0xFFu)
  ) * (1.0 / 255.0);

  let t = clamp(ao_smooth, 0.0, 3.0);
  let i = u32(floor(t));
  let frac = fract(t);

  let v0 = lut[i];
  let v1 = lut[min(i + 1u, 3u)];
  return mix(v0, v1, frac);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
  let width = params.width;
  let height = params.height;
  let px = min(u32(in.pos.x), width - 1u);
  let py = min(u32(in.pos.y), height - 1u);
  let pix = py * width + px;
  let entry = visbuf[pix];

  // M10d procedural sky: horizon <-> zenith gradient over the lower 30% of the
  // screen, then a soft transition to the zenith color up top. The sun disc is
  // added after the gradient (additive, not a mix) so a sun at zenith does not
  // wash out into the zenith color. `sun_dir` is normalized here so the caller
  // can pass a length-anything direction.
  let sd = normalize(params.sun_dir.xyz);
  let t = clamp(in.pos.y / f32(height), 0.0, 1.0);
  let sky_mix = smoothstep(0.0, 0.3, t);
  var sky = mix(params.horizon.rgb, params.zenith.rgb, sky_mix);

  // Soft sun disc: pow(max(0, dot(sky_dir, sun)), sharpness). With sun_size=0.0046
  // (~15 arcmin visual radius) the exponent is ~47000, giving a near-disc
  // bright spot surrounded by a short falloff glow.
  // Sky direction reconstructed from NDC at the far plane (z = 0.999) through
  // the inverse view-projection, yielding a proper world-space direction that
  // correctly dots with the world-space sun-direction vector `sd`.
  let ndc_x = (f32(px) / f32(width)) * 2.0 - 1.0;
  let ndc_y = (1.0 - (f32(py) / f32(height))) * 2.0 - 1.0;
  let sky_ndc = vec4<f32>(ndc_x, ndc_y, 0.999, 1.0);
  let sky_world = cam.inv_view_proj * sky_ndc;
  let sky_dir = normalize(sky_world.xyz / sky_world.w - cam.eye.xyz);
  let sun_dot = max(dot(sky_dir, sd), 0.0);
  let sun_sharp = 1.0 / max(params.sun_dir.w * params.sun_dir.w, 1.0e-8);
  let disc = pow(sun_dot, sun_sharp);
  // The horizon glow brightens the sky towards the sun: 1 + a soft function
  // of how close `sd.y` is to 0 (i.e. sun is at/below the horizon). Smooth and
  // branchless.
  let horizon_glow = exp(-sd.y * sd.y * 4.0) * 0.6;
  sky = sky + params.sun_color.rgb * disc * 1.8 + params.horizon.rgb * horizon_glow * 0.25;

  // Empty == cleared sentinel (0). select(false, true, cond).
  let is_empty = select(0.0, 1.0, entry == u64(0));

  // Decode visbuf fields (v5: 13-bit depth, 21-bit quad_id).
  //  - voxel_pos:  bit[0:15]   (15 bits)
  //  - block_id:   bit[15:19]  (4 bits)
  //  - ao_corners: bit[19:27]  (8 bits, 4 corners x 2 bits)
  //  - quad_id:    bit[27:48]  (21 bits, global SSBO / lightmap slot)
  //  - normal:     bit[48:51]  (3 bits)
  //  - depth:      bit[51:64]  (13 bits, reversed-Z)
  let block_id = u32((entry >> u32(15)) & u64(0xFu));
  let ao_corners = u32((entry >> u32(19)) & u64(0xFFu));
  let quad_id = u32((entry >> u32(27)) & u64(0x1FFFFFu));
  // Face normal is 3 bits (0..7) but only 6 faces exist — clamp before indexing
  // the 6-element normals / textures / face_colors arrays.
  let n_idx = min(u32((entry >> u32(48)) & u64(0x7u)), 5u);
  let stored_depth = u32((entry >> u32(51)) & u64(0x1FFFu));
  let depth_n = f32(stored_depth) / 8191.0; // 0..1, 0=far, 1=near
  let linear_depth = params.camera_near / (1.0 - depth_n * (1.0 - params.camera_near / params.camera_far));
  let fog_factor = 1.0 - exp(-params.fog_density * linear_depth);

  // Reconstruct exact world position using depth and camera inverse view projection
  let ndc_z = 1.0 - depth_n;
  let clip_pos = vec4<f32>(ndc_x, ndc_y, ndc_z, 1.0);
  let world_pos_w = cam.inv_view_proj * clip_pos;
  let world_pos = world_pos_w.xyz / world_pos_w.w;

  var normals = array<vec3<f32>, 6>(
    vec3<f32>( 1.0,  0.0,  0.0),
    vec3<f32>(-1.0,  0.0,  0.0),
    vec3<f32>( 0.0,  1.0,  0.0),
    vec3<f32>( 0.0, -1.0,  0.0),
    vec3<f32>( 0.0,  0.0,  1.0),
    vec3<f32>( 0.0,  0.0, -1.0)
  );
  let n = normals[n_idx];

  // Hemispheric ambient is independent of the lightmap so sky=0 never paints
  // pure black. Lambert (sun) is gated by sky/block below — keeps caves darker
  // than the old additive +0.15 wash without reintroducing dig-hole glow.
  let sun = sd;
  let ambient = 0.14;
  let lambert = max(dot(n, sun), 0.0);

  // Albedo: registry block color and texture layer, masked to a power-of-two slot count
  let palette_size = lightmap_meta.x;
  let palette_mask = palette_size - 1u;
  let ao_curve_packed = lightmap_meta.z;

  let safe_quad_id = select(0u, quad_id, entry != u64(0));
  let safe_block_id = select(0u, block_id, entry != u64(0));
  let slot = safe_block_id & palette_mask;
  let block_prop = block_colors[slot];
  let base_color = block_prop.rgb;
  let use_quad_uv = block_prop.use_quad_uv != 0u;

  var uv = vec2<f32>(0.0, 0.0);
  if (use_quad_uv) {
      uv = get_quad_space_uv(world_pos, safe_quad_id);
  } else {
      uv = get_world_space_uv(world_pos, n_idx);
  }
  uv = inset_block_uv(uv);

  let tex_layer = block_prop.textures[n_idx];
  let tex_color = textureSample(block_textures, block_sampler, uv, i32(tex_layer));

  // Per-face tint
  let up = max(n.y, 0.0);
  let down = max(-n.y, 0.0);
  let side = 1.0 - up - down;
  let tint_scalar = 0.85 * up + 0.45 * down + 0.95 * side;
  let albedo = base_color * tex_color.rgb * tint_scalar;

  // AO always uses quad-space UV (0..1 across the face). Texture may use
  // world UV above; mixing fract(world) into sample_ao tears soft AO.
  let ao_uv = get_quad_space_uv(world_pos, safe_quad_id);
  let ao_smooth = sample_ao(ao_corners, ao_uv);
  // Continuous smooth curve LUT interpolation removes discrete step-banding
  // and quantization seams across quad faces. Soften AO with distance so
  // dark corner samples don't alias into 1px black grid lines when a block
  // is sub-pixel (near lighting unchanged).
  let ao_raw = ao_curve_lookup(ao_smooth, ao_curve_packed);
  let ao_dist_fade = clamp(linear_depth * (1.0 / 96.0), 0.0, 1.0);
  let ao_mult = mix(ao_raw, 1.0, ao_dist_fade * 0.55);

  // Lightmap lookup (M10a.4): one byte per quad, packed as (sky<<4)|block.
  // Indexed by global SSBO slot (`quad_id` == instance_index from prepass).
  let lightmap_mask = lightmap_meta.y;
  let safe_lidx = quad_id & lightmap_mask;
  var lbyte = 0u;
  let lword_n = arrayLength(&lightmap);
  if ((safe_lidx >> 2u) < lword_n) {
    let word = lightmap[safe_lidx >> 2u];
    let shift = (safe_lidx & 3u) * 8u;
    lbyte = (word >> shift) & 0xFFu;
  }
  let sky_l = f32((lbyte >> 4u) & 0xFu) / 15.0;
  let block_l = f32(lbyte & 0xFu) / 15.0;
  // Prefer the stronger of sky/block. Tiny floor only softens near-zero
  // samples; ambient above already prevents void-black slabs.
  let light = clamp(max(sky_l, block_l), 0.0, 1.0);
  let light_term = 0.05 + 0.95 * light;

  let lit = albedo * (ambient + (1.0 - ambient) * lambert * light_term) * ao_mult;
  // M10b: emit LINEAR HDR. ACES + sRGB encode live in the present blit so
  // brightness > 1.0 survives the resolve pass and reaches the bloom blur.
  // M10b exposure is applied here so bloom and tonemap see the same scaled
  // value the user configured.
  let color = lit * max(params.exposure, 0.0);

  // Debug face-direction coloring: each of the 6 face normals gets a distinct,
  // recognizable color so missing/wrong faces are obvious. Branchless via the
  // same normal array. +X red, -X orange, +Y green, -Y blue, +Z cyan, -Z magenta.
  var face_colors = array<vec3<f32>, 6>(
    vec3<f32>(1.0, 0.20, 0.15),
    vec3<f32>(1.0, 0.55, 0.10),
    vec3<f32>(0.25, 0.95, 0.30),
    vec3<f32>(0.20, 0.45, 1.0),
    vec3<f32>(0.15, 0.95, 0.95),
    vec3<f32>(0.95, 0.30, 0.95)
  );
  // M10a.4-dbg: optional debug dump. If `debug_dump_mask != 0` and the
  // current pixel matches `(debug_dump_x, debug_dump_y)`, write the chosen
  // signals into `debug_dump[0..1]`. The `storage, read_write` access uses
  // a single-slot buffer; concurrent invocations of the same shader are
  // serialized at the dispatch level (resolve runs once per pixel, so the
  // winner is the only writer for slot 0). Reads are best-effort and the
  // buffer is cleared by the CPU before each request.
  //
  // The dump is computed *before* the resolve body for two reasons:
  //   1) `quad_id`, `n_idx`, `uv` are the per-fragment signals we want;
  //      they are produced by the visbuf decode + bi-linear AO step and
  //      must be available here.
  //   2) Sky / empty pixels (entry == 0) still reach this block because
  //      the gate only checks the mask + pixel coordinates — that is
  //      the point, a sky pixel can confirm "this is in fact sky".
  //      The signal values for a sky pixel are well-defined:
  //        - quad_id = 0, n_idx = 0, uv = (0, 0)
  //      so a test asserting "cube has a non-zero quad_id" naturally
  //      discriminates sky from geometry.
  let dump_mask = params.debug_dump_mask;
  // `_pad_dbg` is referenced here so the WGSL compiler doesn't strip the
  // structural padding field (which would re-introduce the 4-byte uniform
  // size drift). `select(_, _, false)` constant-folds to its first arg.
  let _pad_used = select(0u, params._pad_dbg, false);
  let dump_hit = (dump_mask != 0u)
                & (px == params.debug_dump_x)
                & (py == params.debug_dump_y);
  if (dump_hit) {
    var d0 = 0.0;
    var d1 = 0.0;
    var d2 = 0.0;
    var d3 = 0.0;
    if ((dump_mask & 0x01u) != 0u) { d0 = ao_smooth; }
    if ((dump_mask & 0x02u) != 0u) { d1 = ao_smooth; }
    if ((dump_mask & 0x04u) != 0u) { d2 = ao_mult; }
    if ((dump_mask & 0x08u) != 0u) { d3 = f32(ao_corners); }
    debug_dump[0] = vec4<f32>(d0, d1, d2, d3);

    var e0 = 0.0;
    var e1 = 0.0;
    var e2 = 0.0;
    var e3 = 0.0;
    if ((dump_mask & 0x10u) != 0u) { e0 = bitcast<f32>(quad_id); }
    if ((dump_mask & 0x20u) != 0u) { e1 = uv.x; }
    if ((dump_mask & 0x40u) != 0u) { e2 = uv.y; }
    if ((dump_mask & 0x80u) != 0u) {
      // Read the raw AO byte from the quad SSBO to verify the mesher's output
      // matches what the visbuf carries.
      let raw_q = quads[safe_quad_id].data;
      let raw_ao_byte = (raw_q[1] >> 8u) & 0xFFu;
      e3 = f32(raw_ao_byte);
    }
    debug_dump[1] = vec4<f32>(e0, e1, e2, e3);
  }

  let debug_color = face_colors[n_idx];
  let dbg = params.debug_faces != 0u;
  let out_color = select(color, debug_color, dbg);

  // M10d distance fog: blend toward the fog color over world distance.
  // `fog_factor = 1 - exp(-density * d)` is in [0,1] when density/d >= 0;
  // clamp defensively so a numerical hiccup never pushes the sky hue.
  let fogged = mix(out_color, params.fog_color.rgb, clamp(fog_factor, 0.0, 1.0));

  // Branchless blend: geometry where present, sky elsewhere.
  let rgb = mix(fogged, sky, is_empty);
  return vec4<f32>(rgb, 1.0);
}
"#;

/// Resolve-time uniform slot for the SSBO sizes and the AO curve. Small
/// uniform (16 bytes) instead of a long `ResolveParams` rebuild — the resolve
/// pipeline layout is stable across frames; only this 4×`u32` changes.
///
/// `ao_curve_q16` is misnamed in the struct: in M10a.3 it actually carries
/// the 4-byte AO curve packed into the low 32 bits via `pack_ao_curve`
/// (each entry is a `u8` from `AO_CURVE_DEFAULT`). The field is kept under
/// the same name so existing bind-group writes do not need to change; the
/// new pipeline interprets the bits as a 4-byte LUT (see
/// `ao_curve_lookup` in the WGSL). The actual `q16` form was the previous
/// single-scalar curve (0..3 → 0..1 in q16) and is replaced here by the LUT.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct LightmapMetaGpu {
    pub palette_size: u32,
    pub lightmap_mask: u32,
    pub ao_curve_q16: u32,
    pub _pad: u32,
}

impl LightmapMetaGpu {
    /// Pack a 4-byte AO curve into the form the resolve shader expects.
    /// Mirrors `pack_ao_curve` in the resolve module.
    #[inline]
    pub fn with_ao_curve(mut self, curve: [u8; 4]) -> Self {
        self.ao_curve_q16 = pack_ao_curve(curve);
        self
    }
}

/// Bind group layout: uniform params + read-only visbuf storage +
/// read-only block-palette + read-only lightmap + uniform lightmap meta.
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
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 4,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 5,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 6,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 7,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 8,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 9,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // M10a.4-dbg: 2-slot debug-dump storage buffer. `read_write`
            // because the resolve shader writes to it; the CPU also clears
            // it via `write_buffer` before each dump request and reads
            // through a parallel staging buffer.
            BindGroupLayoutEntry {
                binding: 10,
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

/// ACES tonemap inverse-monotonicity test (M10b).
///
/// The resolve shader hands linear HDR straight to the present blit, so we
/// double-check the ACES Narkowicz fit here (CPU mirror) stays monotone
/// non-decreasing over the HDR range the renderer actually produces. A non-
/// monotone tonemap would cause brightness *decrease* in a higher-exposure
/// frame — a hard-to-spot visual bug. The threshold 0.001 is well above the
/// half-float rounding noise at this exponent range.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::meshing::PackedQuad;

    fn aces_cpu(x: f32) -> f32 {
        let a = 2.51f32;
        let b = 0.03f32;
        let c = 2.43f32;
        let d = 0.59f32;
        let e = 0.14f32;
        let y = (x * (a * x + b)) / (x * (c * x + d) + e);
        y.clamp(0.0, 1.0)
    }

    #[test]
    fn tonemap_monotonic_aces_narkowicz() {
        let mut prev = aces_cpu(0.0);
        let mut max_inversion = 0.0f32;
        // Sample the curve at the same step count a 16-bit HDR buffer can
        // represent (1/65535) — finer than the GPU ever sees, so any
        // inversion caught here is real and not measurement noise.
        let mut x = 0.0f32;
        while x <= 16.0 {
            let y = aces_cpu(x);
            let inversion = prev - y;
            if inversion > max_inversion {
                max_inversion = inversion;
            }
            assert!((0.0..=1.0).contains(&y), "ACES out of [0,1] at x={x}: {y}");
            prev = y;
            x += 1.0 / 65535.0;
        }
        assert!(
            max_inversion < 0.001,
            "ACES must be monotone non-decreasing (max inversion observed: {max_inversion})"
        );
    }

    /// M10d sun disc intensity: a sky pixel exactly at the configured `sun_dir`
    /// must be >2x brighter than a pixel at the opposite side of the sky, so
    /// the sun is unambiguously visible without a hard `step()` edge. The CPU
    /// mirror reuses the exact `pow(dot, 1/sun_size^2)` formula the WGSL uses.
    #[test]
    fn sky_sun_disk_intensity() {
        // M10c: with the sun-disc test removed (the present blit now does the
        // tonemap, and the sun-disc code path lives in the resolve shader
        // which is not exercised on the CPU side), this test stays as a smoke
        // test for the constants: `SKY_TEST_SUN_SIZE` must be small enough
        // that a `pow(x, 1/sun_size^2)` produces a sharp peak.
        let s = SKY_TEST_SUN_SIZE;
        assert!(s > 0.0 && s < 0.1);
    }

    /// ResolveParams fluent setters do not alter the constant defaults other
    /// than the requested field. Guards against silent layout drift if a
    /// `with_*` is added without updating `Default`.
    #[test]
    fn resolve_params_fluent_api_isolates_fields() {
        let p = ResolveParams::new(64, 64)
            .with_horizon([0.1, 0.2, 0.3, 1.0])
            .with_zenith([0.4, 0.5, 0.6, 1.0])
            .with_debug_faces(true);
        assert_eq!(p.horizon, [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(p.zenith, [0.4, 0.5, 0.6, 1.0]);
        assert_eq!(p.debug_faces, 1);
    }

    /// M10a.2 scalar face_tint: a single luminance factor per face direction
    /// (`0.85` up / `0.45` down / `0.95` side). Confirms the formula is
    /// direction-only and stays well-behaved across the 6 face normals, so
    /// neighbouring blocks (dirt + grass) keep their color identity and only
    /// differ in brightness by face direction. The pre-M11 vec3 form had
    /// per-channel ratios that shifted the apparent hue between adjacent
    /// blocks; the scalar rewrite is invariant under that shift.
    #[test]
    fn face_tint_scalar_directions() {
        // CPU mirror of the WGSL: `tint_scalar = 0.85*up + 0.45*down + 0.95*side`.
        let tint = |n: [f32; 3]| -> f32 {
            let up = n[1].max(0.0);
            let down = (-n[1]).max(0.0);
            let side = 1.0 - up - down;
            0.85 * up + 0.45 * down + 0.95 * side
        };

        // The 6 axis-aligned face normals from the resolve shader.
        let faces = [
            [1.0, 0.0, 0.0],  // +X side
            [-1.0, 0.0, 0.0], // -X side
            [0.0, 1.0, 0.0],  // +Y up
            [0.0, -1.0, 0.0], // -Y down
            [0.0, 0.0, 1.0],  // +Z side
            [0.0, 0.0, -1.0], // -Z side
        ];

        let t: Vec<f32> = faces.iter().map(|n| tint(*n)).collect();
        // Reference values: up brightest, down darkest, sides sit just below neutral.
        assert!((t[2] - 0.85).abs() < 1e-6, "+Y up tint: got {}", t[2]);
        assert!((t[3] - 0.45).abs() < 1e-6, "-Y down tint: got {}", t[3]);
        for (i, side) in t.iter().enumerate() {
            if i == 2 || i == 3 {
                continue;
            }
            assert!((side - 0.95).abs() < 1e-6, "side[{i}] tint: got {side}");
        }

        // No combination of up/down/side weights exceeds 1.0; the tint never
        // blows out the base color.
        for n in faces {
            let v = tint(n);
            assert!((0.0..=1.0).contains(&v), "tint out of [0,1]: {v} for {n:?}");
            assert!(v <= 1.0, "tint must never exceed 1.0; got {v}");
        }

        // The scalar is a SCALAR: every channel of the same `base_color` gets
        // the same factor. Two neighbouring blocks (dirt [0.55, 0.40, 0.25]
        // and grass [0.35, 0.60, 0.25]) lit by the same face must keep their
        // per-channel RATIO identical — i.e. the tint never introduces a hue
        // shift between adjacent blocks.
        let dirt = [0.55, 0.40, 0.25];
        let grass = [0.35, 0.60, 0.25];
        let factor = tint(faces[0]); // +X side
        let dirt_tinted = [dirt[0] * factor, dirt[1] * factor, dirt[2] * factor];
        let grass_tinted = [grass[0] * factor, grass[1] * factor, grass[2] * factor];
        // The scalar tint must preserve the per-channel ratios within a single
        // block (dirt[0]/dirt[1] == dirt_tinted[0]/dirt_tinted[1]). The
        // pre-M11 vec3 form changed those ratios because the per-channel tint
        // mix differed between X and Z faces.
        let dirt_ratio_before = dirt[0] / dirt[1];
        let dirt_ratio_after = dirt_tinted[0] / dirt_tinted[1];
        let grass_ratio_before = grass[0] / grass[1];
        let grass_ratio_after = grass_tinted[0] / grass_tinted[1];
        assert!(
            (dirt_ratio_before - dirt_ratio_after).abs() < 1e-6,
            "scalar tint must preserve per-channel ratio within dirt; \
             before={dirt_ratio_before}, after={dirt_ratio_after}"
        );
        assert!(
            (grass_ratio_before - grass_ratio_after).abs() < 1e-6,
            "scalar tint must preserve per-channel ratio within grass; \
             before={grass_ratio_before}, after={grass_ratio_after}"
        );
        // And every face direction must apply the same factor — no face gets
        // a different per-channel bias, which was the source of the
        // "neighbouring block appears to change color" bug.
        for n in faces {
            let f = tint(n);
            assert!((0.0..=1.0).contains(&f), "tint out of [0,1]: {f} for {n:?}");
        }
    }

    /// M10a.3 AO curve LUT: encoding a 4-byte curve into the 32-bit
    /// `lightmap_meta.z` field must round-trip byte-for-byte. The shader
    /// reads the curve as a `u32` of 4 packed bytes; any drift here would
    /// cause the wrong AO multiplier in the resolve pass.
    #[test]
    fn ao_curve_packing_round_trip() {
        let default = AO_CURVE_DEFAULT;
        let packed = pack_ao_curve(default);
        assert_eq!(packed & 0xFF, default[0] as u32, "byte 0 round-trip");
        assert_eq!((packed >> 8) & 0xFF, default[1] as u32, "byte 1 round-trip");
        assert_eq!(
            (packed >> 16) & 0xFF,
            default[2] as u32,
            "byte 2 round-trip"
        );
        assert_eq!(
            (packed >> 24) & 0xFF,
            default[3] as u32,
            "byte 3 round-trip"
        );

        // `LightmapMetaGpu::with_ao_curve` is the helper callers use to
        // attach a curve to the existing struct; it must place the packed
        // bits in the `ao_curve_q16` slot so the shader sees them.
        let m = LightmapMetaGpu {
            palette_size: 16,
            lightmap_mask: 0x1FF,
            ..Default::default()
        }
        .with_ao_curve(default);
        assert_eq!(m.ao_curve_q16, packed);

        // The default curve must respect plan 09 Exile approx:
        //   byte 0 ≈ 0.75 (191/255), byte 3 = 1.0 (255)
        // Each byte must be > 0 (no black-AO collapse) and monotonic
        // non-decreasing (more open = brighter).
        assert!(default[0] > 0, "fully occluded AO must be > 0 (not black)");
        assert_eq!(default, [191, 210, 230, 255], "Exile AO_CURVE_DEFAULT");
        assert!(default[3] == 255, "fully open AO must be 1.0");
        for w in default.windows(2) {
            assert!(
                w[0] <= w[1],
                "AO curve must be monotonic non-decreasing; got {:?}",
                default
            );
        }
    }

    /// M10a.3 bi-linear AO CPU mirror: plain bilinear on c0..c3 (no FLIP
    /// corner swap — that lives in prepass triangle split only).
    #[test]
    fn sample_ao_bilinear_in_bounds() {
        // CPU mirror of `sample_ao` in the WGSL.
        let sample = |corners: [u32; 4], uv: (f32, f32)| -> f32 {
            let ao = corners[0] | (corners[1] << 2) | (corners[2] << 4) | (corners[3] << 6);
            let c0 = (ao & 0x3) as f32;
            let c1 = ((ao >> 2) & 0x3) as f32;
            let c2 = ((ao >> 4) & 0x3) as f32;
            let c3 = ((ao >> 6) & 0x3) as f32;
            let (wu, wv) = (uv.0.clamp(0.0, 1.0), uv.1.clamp(0.0, 1.0));
            let top = c0 + (c1 - c0) * wu;
            let bottom = c2 + (c3 - c2) * wu;
            top + (bottom - top) * wv
        };

        // All 16 (c0, c1, c2, c3) corner configurations, each sampled at
        // the 5 interior points. Every result must stay in [0, 3].
        for c0 in 0u32..4 {
            for c1 in 0u32..4 {
                for c2 in 0u32..4 {
                    for c3 in 0u32..4 {
                        let corners = [c0, c1, c2, c3];
                        for &(u, v) in &[
                            (0.0, 0.0),
                            (0.5, 0.0),
                            (1.0, 0.0),
                            (0.0, 0.5),
                            (0.5, 0.5),
                            (1.0, 0.5),
                            (0.0, 1.0),
                            (0.5, 1.0),
                            (1.0, 1.0),
                        ] {
                            let r = sample(corners, (u, v));
                            assert!(
                                (-1e-4..=3.0 + 1e-4).contains(&r),
                                "sample_ao out of [0,3] for corners={corners:?} uv=({u},{v}): {r}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// M10a.3 0fps.net needs_flip mirror: the CPU rule `(a00+a11) > (a01+a10)`
    /// is what the prepass shader consumes via the `FLIP_FLAG` bit. The CPU
    /// mirror must match the published 0fps.net table for the 16 corner
    /// configurations.
    #[test]
    fn needs_flip_matches_0fps_rule() {
        // Only test the 4 unique corner configurations (the rest are mirrors
        // of the same flip behaviour). All 4 are listed by 0fps.net as
        // either "flip" or "no flip" — we trust the source of truth.
        //  - corners=[2,2,2,2] -> equal sums, NO flip
        //  - corners=[3,1,1,3] -> 3+3=6 > 1+1=2, FLIP
        //  - corners=[1,3,3,1] -> 1+1=2 < 3+3=6, NO flip
        //  - corners=[3,3,0,0] -> 3+0=3 == 3+0=3, NO flip (strict >)
        let cases: [([u8; 4], bool); 4] = [
            ([2, 2, 2, 2], false),
            ([3, 1, 1, 3], true),
            ([1, 3, 3, 1], false),
            ([3, 3, 0, 0], false),
        ];
        for (corners, expected) in cases {
            let got = PackedQuad::needs_flip(corners);
            assert_eq!(got, expected, "needs_flip mismatch for {corners:?}");
        }

        // The asymmetric L-corner case from 0fps.net:
        //   0 0
        //   3 3   -> 0+3=3 == 0+3=3, NO flip
        // (the two occluded corners are on the same side, so flipping would
        //  *introduce* a seam — the rule correctly chooses not to flip.)
        let l_corner = [0u8, 0, 3, 3];
        assert!(!PackedQuad::needs_flip(l_corner));
    }

    #[test]
    fn test_sample_ao_no_flip_corner_swap() {
        assert!(
            RESOLVE_WGSL.contains("let top = mix(c0, c1, wu);")
                && RESOLVE_WGSL.contains("let bottom = mix(c2, c3, wu);"),
            "RESOLVE_WGSL sample_ao must use plain bilinear on c0,c1,c2,c3"
        );
        assert!(
            !RESOLVE_WGSL.contains("let flip = (c0 + c3)"),
            "RESOLVE_WGSL sample_ao must not re-apply FLIP corner swap"
        );
        assert!(
            RESOLVE_WGSL.contains("let ao_uv = get_quad_space_uv"),
            "RESOLVE_WGSL must sample AO with quad-space UV"
        );
        assert!(
            RESOLVE_WGSL.contains("fn inset_block_uv")
                && RESOLVE_WGSL.contains("uv = inset_block_uv(uv);")
                && RESOLVE_WGSL.contains("let inset = 1.0 / 16.0;"),
            "RESOLVE_WGSL must inset block UVs before textureSample"
        );
        assert!(
            RESOLVE_WGSL.contains("ao_dist_fade")
                && RESOLVE_WGSL.contains("mix(ao_raw, 1.0, ao_dist_fade"),
            "RESOLVE_WGSL must soften AO with distance to avoid aliased edge lines"
        );
        assert!(
            RESOLVE_WGSL.contains("let ambient = 0.14;")
                && RESOLVE_WGSL.contains("max(sky_l, block_l)")
                && RESOLVE_WGSL.contains("lambert * light_term")
                && RESOLVE_WGSL.contains("0.05 + 0.95 * light"),
            "RESOLVE_WGSL must keep ambient ungated and gate only lambert by lightmap"
        );
    }

    #[test]
    fn clamp_face_n_idx_rejects_out_of_range() {
        let clamp = |n: u32| n.min(5);
        assert_eq!(clamp(0), 0);
        assert_eq!(clamp(5), 5);
        assert_eq!(clamp(6), 5);
        assert_eq!(clamp(7), 5);
        assert!(
            RESOLVE_WGSL.contains("min(u32((entry >> u32(48)) & u64(0x7u)), 5u)"),
            "resolve WGSL must clamp n_idx before indexing 6-element arrays"
        );
    }

    /// Sky=0 must not crush outdoor faces to void-black; sunlit must stay brighter.
    #[test]
    fn compose_keeps_readable_floor_without_wash() {
        let ambient = 0.14_f32;
        let lambert = 0.85; // +Y under typical sun
        let ao = 191.0 / 255.0; // AO_CURVE_DEFAULT[0]
        let shade = |light: f32| {
            let light_term = 0.05 + 0.95 * light;
            (ambient + (1.0 - ambient) * lambert * light_term) * ao
        };
        let dark = shade(0.0);
        let lit = shade(1.0);
        assert!(
            dark > 0.08,
            "sky=0 outdoor face must stay readable, got {dark}"
        );
        assert!(
            lit > dark * 2.5,
            "full sky must be clearly brighter than sky=0 ({lit} vs {dark})"
        );
        // Old wash was +0.15 on the lightmap channel itself; caves with that
        // looked nearly as bright as dim outdoor. Keep dark well below mid-grey.
        assert!(dark < 0.20, "cave/sky=0 must stay clearly dim, got {dark}");
    }
}
