//! GPU pipeline bootstrap for Strata (M4 — wgpu bootstrap + headless offscreen clear).
//!
//! M4a built the offscreen HDR clear target. M4c adds the GPU depth pre-pass: a
//! vertex-pulling pipeline that rasterizes greedy quads into a 64-bit visibility
//! buffer (`visbuf`) via `atomicMax`, so the nearest fragment (reversed-Z) wins
//! each pixel. The pre-pass is driven by [`Renderer::render_prepass`], and the
//! result can be read back with [`Renderer::read_visbuf`].

pub mod block_palette;
pub mod bloom;
pub mod camera;
pub mod cull;
pub mod lightmap;
pub mod overlay;
pub mod prepass;
pub mod resolve;
pub mod visbuf;

pub mod bevy_bridge;

pub use block_palette::{BlockColorGpu, BlockPalette, build_block_colors};
pub use bloom::{
    BLOOM_WGSL, BloomParams, BloomPipelines, BlurBindGroups, DEFAULT_INTENSITY, DEFAULT_MIP_COUNT,
    DEFAULT_THRESHOLD, blur_bind_group_layout, bright_bind_group_layout,
    composite_bind_group_layout, make_bloom_params_buffer, make_mip_pyramid, smallest_mip_dim,
};
pub use camera::CameraView;
pub use cull::{Aabb, cull_visible};
pub use lightmap::{LightmapEntry, LightmapSSBO, SECTOR_LIGHTMAP_QUADS};
pub use visbuf::{PackedQuadGpu, VisBufEntry, mesh_prepass_bytes, meshdata_to_gpu_bytes};

use std::sync::Arc;

use wgpu::*;

use crate::meshing::MeshData;
use crate::pipeline::bevy_bridge::{RenderDeviceRef, RenderQueueRef};
use crate::pipeline::cull::aabb_of_mesh;
use crate::pipeline::prepass::{
    PREPASS_COLOR_FORMAT, make_origins_buffer, make_quad_buffer, make_quad_ids_buffer,
    make_sector_ids_buffer, prepass_bind_group_layout, prepass_pipeline,
};
use crate::pipeline::resolve::{
    LightmapMetaGpu, ResolveParams, pack_ao_curve, resolve_bind_group_layout, resolve_pipeline,
};

/// `rgba16float` uses 8 bytes per texel (4 channels × half-float).
const BYTES_PER_PIXEL: u32 = 8;

/// Block albedo atlas tile size (texels). All registry PNGs are resized here.
const BLOCK_TEX_SIZE: u32 = 16;
/// Full mip chain length for a power-of-two square (`16 → 5` levels).
#[inline]
fn block_tex_mip_count(size: u32) -> u32 {
    size.ilog2() + 1
}

/// Returns true when any texel is not fully opaque.
#[inline]
fn rgba_image_has_partial_alpha(img: &image::RgbaImage) -> bool {
    img.pixels().any(|p| p[3] != 255)
}

/// Composite semi-transparent overlay texels onto an opaque backdrop.
///
/// Minecraft-style `grass_side` stores grass fringe shape in alpha with rgb=0;
/// the resolve pass samples rgb only, so transparent texels must be baked before
/// GPU upload.
fn bake_texture_alpha_over_backdrop(overlay: &mut image::RgbaImage, backdrop: &image::RgbaImage) {
    let w = overlay.width().min(backdrop.width());
    let h = overlay.height().min(backdrop.height());
    for x in 0..w {
        for y in 0..h {
            let fg = overlay.get_pixel(x, y);
            if fg[3] == 255 {
                continue;
            }
            let bg = backdrop.get_pixel(x, y);
            let a = fg[3] as f32 / 255.0;
            let inv = 1.0 - a;
            let r = (fg[0] as f32 * a + bg[0] as f32 * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            let g = (fg[1] as f32 * a + bg[1] as f32 * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            let b = (fg[2] as f32 * a + bg[2] as f32 * inv)
                .round()
                .clamp(0.0, 255.0) as u8;
            overlay.put_pixel(x, y, image::Rgba([r, g, b, 255]));
        }
    }
}

/// Build mip 0..=N for a block albedo layer (box/triangle downsample).
fn generate_block_tex_mips(base: &image::RgbaImage) -> Vec<image::RgbaImage> {
    let mut mips = Vec::with_capacity(block_tex_mip_count(base.width().max(1)) as usize);
    mips.push(base.clone());
    let mut w = base.width().max(1);
    let mut h = base.height().max(1);
    while w > 1 || h > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        let prev = mips.last().expect("mip0 present");
        mips.push(image::imageops::resize(
            prev,
            w,
            h,
            image::imageops::FilterType::Triangle,
        ));
    }
    mips
}

/// Sampler policy for block textures.
///
/// Block textures are 16×16 layers in a 2D array (not a sub-rect atlas).
/// Nearest magnification keeps pixel-art crisp up close; linear mip minification
/// still smooths distant tiles without WebGPU's anisotropy→linear-mag rule.
#[inline]
fn block_sampler_descriptor<'a>() -> SamplerDescriptor<'a> {
    SamplerDescriptor {
        label: Some("strata_block_sampler"),
        address_mode_u: AddressMode::Repeat,
        address_mode_v: AddressMode::Repeat,
        address_mode_w: AddressMode::ClampToEdge,
        mag_filter: FilterMode::Nearest,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..Default::default()
    }
}

/// Bytes per entry in the fattest per-quad storage buffer bound as an entire
/// SSBO in the resolve pass (`origins` = `vec4<f32>` at binding 8). Growth must
/// keep `capacity * ORIGIN_BYTES_PER_QUAD ≤ max_storage_buffer_binding_size`.
pub const ORIGIN_BYTES_PER_QUAD: usize = std::mem::size_of::<[f32; 4]>();

/// Hard upper bound on global quad SSBO slots — matches visbuf `quad_id`
/// (21 bits, up to 2M addressable indices). Growing past this is useless
/// (ids wrap in the packer) and, for origins, exceeds common 128 MiB binding
/// limits once capacity reaches 2²⁴ (256 MiB).
pub const MAX_QUAD_SSBO_SLOTS: usize = 1 << 21;

/// Largest power-of-two quad capacity that fits both the device storage
/// binding limit (origins are the widest per-slot buffer) and
/// [`MAX_QUAD_SSBO_SLOTS`].
pub fn max_quad_ssbo_capacity(max_storage_buffer_binding_size: u64) -> usize {
    let by_binding = (max_storage_buffer_binding_size as usize) / ORIGIN_BYTES_PER_QUAD.max(1);
    let capped = by_binding.min(MAX_QUAD_SSBO_SLOTS).max(1);
    1usize << capped.ilog2()
}

/// Round SSBO quad capacity up to a power of two so
/// `lightmap_mask = capacity - 1` is a valid bitmask index (not a soft clamp).
/// Never exceeds `max_cap` (device + visbuf limit).
pub(crate) fn next_quad_capacity(needed: usize, old_cap: usize, max_cap: usize) -> usize {
    let max_cap = max_cap.max(1);
    let raw = needed
        .max(old_cap.saturating_mul(2))
        .max(1)
        .next_power_of_two();
    raw.min(max_cap)
}

/// Inclusive end (byte/quad index, 4-aligned) of a lightmap region upload at
/// global SSBO slot `base`. Callers must grow the lightmap to at least this
/// before `Queue::write_buffer`.
#[inline]
pub(crate) fn lightmap_region_end(base: u32, byte_len: usize) -> usize {
    let end = (base as usize).saturating_add(byte_len);
    end.saturating_add(3) & !3
}

/// Merge overlapping or adjacent `[start, end)` spans; leave gaps intact.
///
/// Used by [`Renderer::flush_quad_uploads`] so non-adjacent sector uploads never
/// become one bounding-box `write_buffer` that would stomp live SSBO slots with
/// stale staging bytes in the gap.
pub(crate) fn coalesce_upload_spans(spans: &mut Vec<(usize, usize)>) {
    if spans.len() <= 1 {
        return;
    }
    spans.sort_unstable_by_key(|&(s, _)| s);
    let mut w = 0;
    for i in 1..spans.len() {
        let (s, e) = spans[i];
        if s <= spans[w].1 {
            spans[w].1 = spans[w].1.max(e);
        } else {
            w += 1;
            spans[w] = (s, e);
        }
    }
    spans.truncate(w + 1);
}

/// Fullscreen-triangle blit that samples the offscreen HDR target, applies
/// ACES tonemapping + sRGB encoding, and writes the result to the window
/// surface (M10b). Uses `textureLoad` (no sampler) so it works on adapters
/// that lack `FLOAT16_FILTERABLE`.
const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    var verts = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let xy = verts[vi];
    var out: VsOut;
    out.pos = vec4<f32>(xy, 0.0, 1.0);
    out.uv = vec2<f32>((xy.x + 1.0) * 0.5, 1.0 - (xy.y + 1.0) * 0.5);
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

// Linear -> sRGB transfer, branchless.
fn linear_to_srgb(x: vec3<f32>) -> vec3<f32> {
    let cutoff = vec3<f32>(0.0031308);
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(0.41666)) - 0.055;
    return select(hi, lo, x < cutoff);
}

// Filtering-independent blit: `src` is rgba16float which is non-filterable on
// adapters without FLOAT16_FILTERABLE, so sampling with a filtering sampler
// fails validation and the pass is silently skipped (black screen). textureLoad
// needs no sampler and works unconditionally.
@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let coord = vec2<i32>(i32(in.pos.x), i32(in.pos.y));
    let hdr = textureLoad(src, coord, 0).rgb;
    let mapped = aces(hdr);
    let srgb = linear_to_srgb(mapped);
    return vec4<f32>(srgb, 1.0);
}
"#;

/// Device features required to build/run the depth pre-pass (`u64` types and
/// `atomicMax` on `u64`). Native-only; the pre-pass is skipped on devices that
/// lack them (M4a offscreen clear still works).
pub fn prepass_features() -> Features {
    Features::SHADER_INT64 | Features::SHADER_INT64_ATOMIC_MIN_MAX
}

/// Offscreen HDR renderer + GPU depth pre-pass driver (M4a + M4c).
///
/// Holds the GPU device/queue and the M4a HDR clear target. The M4c pre-pass
/// resources are created lazily on first use (see [`Renderer::ensure_prepass`])
/// and only when the device exposes the required `u64`-atomic features.
#[allow(dead_code)]
pub struct Renderer {
    device: Device,
    queue: Queue,
    width: u32,
    height: u32,
    offscreen: Texture,
    offscreen_view: TextureView,
    staging: Buffer,
    bytes_per_row: u32,
    /// Cap for quad/origins/lightmap SSBO growth (device binding limit ∩
    /// [`MAX_QUAD_SSBO_SLOTS`]). `ensure_quad_capacity` never allocates past this.
    max_quad_capacity: usize,
    /// When true the resolve pass colors each voxel face by its direction
    /// (+X red, +Y green, ...) instead of Lambert shading — a debug aid for
    /// spotting missing/wrong faces.
    debug_faces: bool,
    /// M10a.4-dbg: last (mask, x, y) written by `set_debug_dump`. `dump_debug`
    /// reads this back to issue the request without forcing the caller to
    /// re-supply the parameters.
    last_debug_dump: Option<(u32, u32, u32)>,
    prepass: Option<Prepass>,
    /// M10c bloom pipeline bundle; built lazily on first frame that uses it.
    bloom: Option<BloomPipelines>,
    /// Staging buffers for batched quad/origin uploads (see `upload_quad_region`
    /// / `flush_quad_uploads`). Each sector is staged at its own SSBO offset;
    /// flush issues one `write_buffer` per discrete span (overlapping/adjacent
    /// spans coalesce). A single min‥max bounding-box flush is incorrect: staging
    /// only holds fresh bytes at uploaded offsets, so copying the gap would
    /// stomp other sectors' live SSBO slots with stale staging garbage (dig →
    /// self+neighbor remesh at distant bases → garbled geometry/textures).
    quad_upload_staging: Vec<u8>,
    origin_upload_staging: Vec<[f32; 4]>,
    /// Per-quad `quad_id_in_sector` staging, parallel to `quad_upload_staging`.
    /// M10a.4: the resolve shader uses this to index the per-sector lightmap
    /// SSBO.
    quad_id_upload_staging: Vec<u32>,
    /// Per-quad `sector_id` staging, parallel to `quad_upload_staging`
    /// (M11 visbuf v2). Same instance-index addressing as `quad_id_upload_staging`;
    /// the renderer auto-fills this buffer in its draw paths
    /// (`render_frame`, `draw_quad_ranges`, `run_prepass`).
    sector_id_upload_staging: Vec<u32>,
    /// Discrete [start, end) byte spans written this frame in `quad_upload_staging`.
    upload_spans_quad: Vec<(usize, usize)>,
    /// Discrete [start, end) float spans written this frame in `origin_upload_staging`.
    upload_spans_origin: Vec<(usize, usize)>,
    /// Dirty flag: true when new spans were pushed since the last `coalesce_upload_spans`
    /// call. Lets `flush_quad_uploads` skip the O(n log n) sort when the spans haven't
    /// changed since the previous flush (e.g. `flush_quad_uploads` called again without
    /// intervening `upload_quad_region`).
    spans_dirty_quad: bool,
    spans_dirty_origin: bool,
    pending_upload: bool,
    /// Lazily-built fullscreen blit used to present the offscreen HDR target to a
    /// window surface (M9b). One per surface format.
    blit: Option<Blit>,
    /// Selection wireframe + center crosshair (built lazily per surface format).
    hud: Option<overlay::HudOverlay>,
}

/// GPU resources for the M9b offscreen->surface present blit (created lazily).
#[allow(dead_code)]
struct Blit {
    pipeline: RenderPipeline,
    layout: BindGroupLayout,
    bind_group: BindGroup,
    format: TextureFormat,
}

/// GPU resources for the M4c depth pre-pass + M4d color-resolve (created lazily).
#[allow(dead_code)]
struct Prepass {
    pipeline: RenderPipeline,
    bgl: BindGroupLayout,
    camera_buffer: Buffer,
    quad_buffer: Arc<Buffer>,
    quad_capacity: usize,
    /// Per-quad world origins (sector_coord * 32), parallel to `quad_buffer` and
    /// grown alongside it. `.xyz` = offset, `.w` = padding.
    origins_buffer: Arc<Buffer>,
    /// Per-quad sector-local id (0..N-1) so the resolve shader can index the
    /// lightmap SSBO. `instance_index` in the WGSL picks the slot.
    quad_ids_buffer: Arc<Buffer>,
    /// M11 visbuf v2: per-quad sector id (0..N-1 over the meshes in the AOI)
    /// so the resolve shader can disambiguate pixels written by quads from
    /// different sectors in the same frame. Parallel to `quad_ids_buffer`.
    sector_ids_buffer: Arc<Buffer>,
    bind_group: BindGroup,
    visbuf: Buffer,
    visbuf_pixels: u32,
    visbuf_staging: Arc<Buffer>,
    dummy_color: TextureView,
    resolve_pipeline: RenderPipeline,
    /// Cached bind-group layout for the resolve pass (rebuilt when the
    /// per-pass storage layout changes; holds the block-palette + lightmap
    /// SSBOs and the lightmap meta uniform).
    #[allow(dead_code)]
    resolve_bgl: BindGroupLayout,
    resolve_params: Buffer,
    resolve_bind_group: BindGroup,
    /// M10a.4-dbg: 2-slot debug dump SSBO. The resolve fragment shader writes
    /// the first 4 selected signals to `debug_dump[0]` and the next 4 to
    /// `debug_dump[1]` for the pixel `(debug_dump_x, debug_dump_y)`. The CPU
    /// clears the buffer before each `dump_debug` request and reads it back
    /// after the next frame. Sized to `2 * vec4<f32>` = 32 bytes.
    debug_dump_buffer: Arc<Buffer>,
    debug_dump_staging: Arc<Buffer>,
    /// Read-only block-palette SSBO (M10a.2). Owned by the renderer because
    /// the registry is static for a session.
    #[allow(dead_code)]
    block_palette: BlockPalette,
    /// Per-sector lightmap SSBO (M10a.4). Lazily filled by the client each
    /// frame via `upload_lightmap`.
    #[allow(dead_code)]
    lightmap: LightmapSSBO,
    /// `LightmapMetaGpu` uniform: palette power-of-two size + lightmap mask
    /// + AO curve + padding. Re-uploaded when the palette or lightmap buffer
    ///   changes shape.
    #[allow(dead_code)]
    lightmap_meta_buffer: Buffer,
    #[allow(dead_code)]
    lightmap_meta: LightmapMetaGpu,
    #[allow(dead_code)]
    block_textures: Texture,
    #[allow(dead_code)]
    block_textures_view: TextureView,
    #[allow(dead_code)]
    block_sampler: Sampler,
}

impl Renderer {
    /// Build a [`Renderer`] around Bevy's [`RenderDevice`]/[`RenderQueue`],
    /// extracting the underlying wgpu device/queue via `wgpu_device()`. This
    /// ensures the renderer shares the single wgpu device that Bevy's
    /// `RenderPlugin` created, avoiding a renderer-ownership conflict.
    pub fn from_render_device(
        render_device: &impl RenderDeviceRef,
        render_queue: &impl RenderQueueRef,
        width: u32,
        height: u32,
    ) -> Self {
        Self::new(
            render_device.wgpu_device().clone(),
            render_queue.wgpu_queue().clone(),
            width,
            height,
        )
    }

    /// Build a [`Renderer`] around an existing device/queue, sizing the
    /// offscreen target to `width`×`height`. The M4c pre-pass resources are
    /// created on first [`Renderer::render_prepass`] call.
    pub fn new(device: Device, queue: Queue, width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "offscreen target must be non-empty"
        );

        let offscreen = device.create_texture(&TextureDescriptor {
            label: Some("strata_offscreen"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::COPY_SRC
                | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let offscreen_view = offscreen.create_view(&TextureViewDescriptor::default());

        // COPY_DST + MAP_READ staging buffer; rows are 256-byte aligned.
        let bytes_per_row = (width * BYTES_PER_PIXEL).div_ceil(256) * 256;
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("strata_offscreen_staging"),
            size: (bytes_per_row * height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let max_quad_capacity =
            max_quad_ssbo_capacity(u64::from(device.limits().max_storage_buffer_binding_size));

        Self {
            device,
            queue,
            width,
            height,
            offscreen,
            offscreen_view,
            staging,
            bytes_per_row,
            max_quad_capacity,
            debug_faces: false,
            last_debug_dump: None,
            prepass: None,
            bloom: None,
            blit: None,
            hud: None,
            quad_upload_staging: Vec::new(),
            origin_upload_staging: Vec::new(),
            quad_id_upload_staging: Vec::new(),
            sector_id_upload_staging: Vec::new(),
            upload_spans_quad: Vec::new(),
            upload_spans_origin: Vec::new(),
            spans_dirty_quad: false,
            spans_dirty_origin: false,
            pending_upload: false,
        }
    }

    /// Borrow the GPU device (used by the client to configure its surface).
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Borrow the offscreen HDR texture view. The client renders to this view
    /// and then blits it to the window surface (owned by Bevy's RenderPlugin).
    pub fn offscreen_view(&self) -> &TextureView {
        &self.offscreen_view
    }

    /// Maximum quad SSBO slots this renderer will allocate (device binding
    /// limit ∩ [`MAX_QUAD_SSBO_SLOTS`]). Slot allocators must not request past
    /// this — origins at binding 8 are 16 B/slot and will fail validation.
    #[inline]
    pub fn max_quad_capacity(&self) -> usize {
        self.max_quad_capacity
    }

    /// Lazily build the pre-pass GPU resources (pipeline, buffers, bind group)
    /// if the device supports the required `u64`-atomic features. No-op if the
    /// resources already exist or the features are unavailable.
    pub fn ensure_prepass(&mut self) {
        if self.prepass.is_some() {
            return;
        }
        if !self.device.features().contains(prepass_features()) {
            return;
        }

        let visbuf_pixels = self.width * self.height;
        let visbuf = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_visbuf"),
            size: (visbuf_pixels as u64) * std::mem::size_of::<u64>() as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let visbuf_staging = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_visbuf_staging"),
            size: (visbuf_pixels as u64) * std::mem::size_of::<u64>() as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let camera_buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_camera"),
            size: std::mem::size_of::<CameraView>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dummy_color = self.device.create_texture(&TextureDescriptor {
            label: Some("strata_prepass_dummy_color"),
            size: Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: PREPASS_COLOR_FORMAT,
            usage: TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let dummy_color_view = dummy_color.create_view(&TextureViewDescriptor::default());

        let bgl = prepass_bind_group_layout(&self.device);
        let pipeline = prepass_pipeline(&self.device, &bgl, PREPASS_COLOR_FORMAT);

        let quad_capacity = 0usize;
        let quad_buffer = make_quad_buffer(&self.device, quad_capacity);
        let origins_buffer = make_origins_buffer(&self.device, quad_capacity);
        let quad_ids_buffer = make_quad_ids_buffer(&self.device, quad_capacity);
        let sector_ids_buffer = make_sector_ids_buffer(&self.device, quad_capacity);
        let bind_group = Self::make_bind_group(
            &self.device,
            &bgl,
            &camera_buffer,
            &quad_buffer,
            &visbuf,
            &origins_buffer,
            &quad_ids_buffer,
            &sector_ids_buffer,
        );

        // M10a.2 — block-palette SSBO. The registry is required for color data;
        // until the client calls `set_block_registry` the palette holds a
        // single AIR (black) entry and every voxel renders as black geometry
        // — wrong but visible, never silent.
        let block_palette = BlockPalette::empty(&self.device, &self.queue);

        // M10a.4 — per-sector lightmap SSBO.
        let lightmap = LightmapSSBO::new(&self.device, SECTOR_LIGHTMAP_QUADS);

        // M10a meta uniform (palette size, lightmap mask, AO curve). All-zero
        // is the safe "no palette / no light" default; the resolve shader
        // masks every lookup so an empty palette yields AIR-color (black) and
        // an empty lightmap yields light=0 (dark) — both correct fallbacks.
        let lightmap_meta = LightmapMetaGpu {
            palette_size: 2, // Min viable: at least AIR + 1 slot so mask ≥ 1
            lightmap_mask: (SECTOR_LIGHTMAP_QUADS - 1) as u32,
            // M10a.3: pack the 4-byte AO curve into the high half of the
            // uniform field. Default is plan 09 Exile approx
            // [191, 210, 230, 255]. Without this, `ao_curve_lookup`
            // sees `lightmap_meta.z == 1 << 16` and indexes the wrong LUT
            // slot, producing a near-black multiplier and a fully black
            // frame. The fix: pre-pack `AO_CURVE_DEFAULT` via `pack_ao_curve`
            // and the helper's `with_ao_curve`.
            ao_curve_q16: pack_ao_curve(crate::pipeline::resolve::AO_CURVE_DEFAULT),
            _pad: 0,
        };
        let lightmap_meta_buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_lightmap_meta"),
            size: std::mem::size_of::<LightmapMetaGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&lightmap_meta_buffer, 0, bytemuck::bytes_of(&lightmap_meta));

        // Create dummy texture array and sampler for resolved blocks
        let dummy_block_textures = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("strata_dummy_block_textures"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let dummy_magenta = vec![255u8, 0, 255, 255].repeat(16 * 16);
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &dummy_block_textures,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &dummy_magenta,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(16 * 4),
                rows_per_image: Some(16),
            },
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
        );
        let dummy_block_textures_view =
            dummy_block_textures.create_view(&wgpu::TextureViewDescriptor {
                label: Some("strata_dummy_block_textures_view"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..wgpu::TextureViewDescriptor::default()
            });

        let block_sampler = self.device.create_sampler(&block_sampler_descriptor());

        // M4d color-resolve resources.
        let resolve_bgl = resolve_bind_group_layout(&self.device);
        let resolve_pipeline = resolve_pipeline(&self.device, &resolve_bgl);
        let resolve_params = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_resolve_params"),
            size: std::mem::size_of::<ResolveParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // M10a.4-dbg: 2-slot debug-dump SSBO (vec4<f32> * 2 = 32 B) plus
        // a matching staging buffer for the readback path. The shader writes
        // atomically; the CPU clears the SSBO before each `dump_debug` and
        // reads it back via the staging buffer.
        let debug_dump_buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_debug_dump"),
            size: (crate::pipeline::resolve::debug_dump::SLOT_COUNT as u64) * 16,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let debug_dump_staging = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_debug_dump_staging"),
            size: (crate::pipeline::resolve::debug_dump::SLOT_COUNT as u64) * 16,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let resolve_bind_group = make_resolve_bind_group(
            &self.device,
            &resolve_bgl,
            &resolve_params,
            &visbuf,
            &block_palette,
            &lightmap,
            &lightmap_meta_buffer,
            &dummy_block_textures_view,
            &block_sampler,
            &quad_buffer,
            &origins_buffer,
            &camera_buffer,
            &debug_dump_buffer,
        );

        // Upload the (constant) resolve params once.
        self.queue.write_buffer(
            &resolve_params,
            0,
            bytemuck::bytes_of(&ResolveParams {
                debug_faces: self.debug_faces as u32,
                ..ResolveParams::new(self.width, self.height)
            }),
        );

        self.prepass = Some(Prepass {
            pipeline,
            bgl,
            camera_buffer,
            quad_buffer,
            quad_capacity,
            origins_buffer,
            quad_ids_buffer,
            sector_ids_buffer,
            bind_group,
            visbuf,
            visbuf_pixels,
            visbuf_staging: Arc::new(visbuf_staging),
            dummy_color: dummy_color_view,
            resolve_pipeline,
            resolve_bgl,
            resolve_params,
            resolve_bind_group,
            debug_dump_buffer: Arc::new(debug_dump_buffer),
            debug_dump_staging: Arc::new(debug_dump_staging),
            block_palette,
            lightmap,
            lightmap_meta_buffer,
            lightmap_meta,
            block_textures: dummy_block_textures,
            block_textures_view: dummy_block_textures_view,
            block_sampler,
        });
    }

    /// Lazily build the M10c bloom pipeline bundle (mip pyramid, ping-pong
    /// textures, 5 pairs of blur pipelines, the bright-extract + composite
    /// pipelines, and the param uniform). Idempotent; subsequent calls are
    /// no-ops. The build is expensive (creates ~12 textures and 12 pipelines)
    /// so it lives in `ensure_*` instead of being part of `Renderer::new`.
    pub fn ensure_bloom(&mut self) {
        if self.bloom.is_some() {
            return;
        }
        self.bloom = Some(self.build_bloom());
    }

    fn build_bloom(&self) -> BloomPipelines {
        let mip_count = DEFAULT_MIP_COUNT as usize;
        let (mip_textures, mip_views) =
            make_mip_pyramid(&self.device, self.width, self.height, DEFAULT_MIP_COUNT);
        let (ping_textures, ping_views) =
            make_mip_pyramid(&self.device, self.width, self.height, DEFAULT_MIP_COUNT);

        let params_buffer =
            make_bloom_params_buffer(&self.device, &self.queue, BloomParams::default());

        let sampler = self.device.create_sampler(&SamplerDescriptor {
            label: Some("strata_bloom_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // Build the shared shader module once; every pipeline reuses it.
        let module = self.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_bloom_shader"),
            source: ShaderSource::Wgsl(BLOOM_WGSL.into()),
        });

        // Bright extract.
        let bright_bgl = bright_bind_group_layout(&self.device);
        let bright_pl = self
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("strata_bloom_bright_pl"),
                bind_group_layouts: &[Some(&bright_bgl)],
                immediate_size: 0,
            });
        let bright_pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("strata_bloom_bright_pipeline"),
                layout: Some(&bright_pl),
                vertex: VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(FragmentState {
                    module: &module,
                    entry_point: Some("fs_bright"),
                    targets: &[Some(ColorTargetState {
                        format: TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        let bright_bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_bloom_bright_bg"),
            layout: &bright_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&self.offscreen_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        // Blur (H + V) — one pair per mip. The blur bind group layout is the
        // same for H and V because both sample an input texture and a sampler.
        let blur_bgl = blur_bind_group_layout(&self.device);
        let blur_pl = self
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("strata_bloom_blur_pl"),
                bind_group_layouts: &[Some(&blur_bgl)],
                immediate_size: 0,
            });
        let mut blur_h_pipelines: Vec<RenderPipeline> = Vec::with_capacity(mip_count);
        let mut blur_v_pipelines: Vec<RenderPipeline> = Vec::with_capacity(mip_count);
        for mip in 0..mip_count {
            let blur_h = self
                .device
                .create_render_pipeline(&RenderPipelineDescriptor {
                    label: Some(&format!("strata_bloom_blur_h_{mip}")),
                    layout: Some(&blur_pl),
                    vertex: VertexState {
                        module: &module,
                        entry_point: Some("vs_fullscreen"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(FragmentState {
                        module: &module,
                        entry_point: Some("fs_blur_h"),
                        targets: &[Some(ColorTargetState {
                            format: TextureFormat::Rgba16Float,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: PrimitiveState {
                        topology: PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
            let blur_v = self
                .device
                .create_render_pipeline(&RenderPipelineDescriptor {
                    label: Some(&format!("strata_bloom_blur_v_{mip}")),
                    layout: Some(&blur_pl),
                    vertex: VertexState {
                        module: &module,
                        entry_point: Some("vs_fullscreen"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: Some(FragmentState {
                        module: &module,
                        entry_point: Some("fs_blur_v"),
                        targets: &[Some(ColorTargetState {
                            format: TextureFormat::Rgba16Float,
                            blend: None,
                            write_mask: ColorWrites::ALL,
                        })],
                        compilation_options: Default::default(),
                    }),
                    primitive: PrimitiveState {
                        topology: PrimitiveTopology::TriangleList,
                        ..Default::default()
                    },
                    depth_stencil: None,
                    multisample: MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
            blur_h_pipelines.push(blur_h);
            blur_v_pipelines.push(blur_v);
        }
        // The WGSL constant `DEFAULT_MIP_COUNT` is the array length the WGSL
        // expects at @group(0)@binding(2..6); if the host ever lowers the mip
        // count below 5, fewer textures are bound and the WGSL would still
        // sample the unused slots. We pin to 5 mips to match the WGSL — the
        // constant `DEFAULT_MIP_COUNT` and the `fs_composite_5` entry point
        // are kept in lockstep.

        let mut blur_bgs: Vec<BlurBindGroups> = Vec::with_capacity(mip_count);
        for mip in 0..mip_count {
            let h = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some(&format!("strata_bloom_blur_h_bg_{mip}")),
                layout: &blur_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::TextureView(&mip_views[mip]),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::Sampler(&sampler),
                    },
                ],
            });
            let v = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some(&format!("strata_bloom_blur_v_bg_{mip}")),
                layout: &blur_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::TextureView(&ping_views[mip]),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::Sampler(&sampler),
                    },
                ],
            });
            blur_bgs.push(BlurBindGroups { h, v });
        }
        let blur_h_pipelines: [RenderPipeline; DEFAULT_MIP_COUNT as usize] =
            blur_h_pipelines.try_into().expect("mip count matches");
        let blur_v_pipelines: [RenderPipeline; DEFAULT_MIP_COUNT as usize] =
            blur_v_pipelines.try_into().expect("mip count matches");
        let blur_bgs: [BlurBindGroups; DEFAULT_MIP_COUNT as usize] =
            blur_bgs.try_into().expect("mip count matches");

        // Downsample: fullscreen bilinear mip i → mip i+1. Uses the same
        // bind group layout as blur (blur_src @3, blur_sampler @4) and a
        // trivial fragment shader that just samples at the UV.
        let downsample_pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("strata_bloom_downsample_pipeline"),
                layout: Some(&blur_pl),
                vertex: VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(FragmentState {
                    module: &module,
                    entry_point: Some("fs_downsample"),
                    targets: &[Some(ColorTargetState {
                        format: TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        // Downsample bind groups: one per mip except the last. Entry i reads
        // mip_views[i] (the source) via the blur BGL bindings (3=texture, 4=sampler).
        let mut downsample_bgs: Vec<BindGroup> = Vec::with_capacity(mip_count.saturating_sub(1));
        for (mip, view) in mip_views
            .iter()
            .enumerate()
            .take(mip_count.saturating_sub(1))
        {
            let bg = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some(&format!("strata_bloom_downsample_bg_{mip}")),
                layout: &blur_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::TextureView(view),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: BindingResource::Sampler(&sampler),
                    },
                ],
            });
            downsample_bgs.push(bg);
        }

        // Composite: 5 mip textures + params + sampler.
        let composite_bgl = composite_bind_group_layout(&self.device, DEFAULT_MIP_COUNT);
        let composite_pl = self
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("strata_bloom_composite_pl"),
                bind_group_layouts: &[Some(&composite_bgl)],
                immediate_size: 0,
            });
        let composite_pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("strata_bloom_composite_pipeline"),
                layout: Some(&composite_pl),
                vertex: VertexState {
                    module: &module,
                    entry_point: Some("vs_fullscreen"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(FragmentState {
                    module: &module,
                    entry_point: Some("fs_composite_5"),
                    targets: &[Some(ColorTargetState {
                        format: TextureFormat::Rgba16Float,
                        // Additive blend (one / one) so the composite
                        // contributes to the HDR target without overwriting
                        // the resolve-pass color underneath. wgpu 0.27+
                        // requires the explicit factor fields.
                        blend: Some(BlendState {
                            color: BlendComponent {
                                src_factor: BlendFactor::One,
                                dst_factor: BlendFactor::One,
                                operation: BlendOperation::Add,
                            },
                            alpha: BlendComponent {
                                src_factor: BlendFactor::One,
                                dst_factor: BlendFactor::One,
                                operation: BlendOperation::Add,
                            },
                        }),
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        let composite_bg = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_bloom_composite_bg"),
            layout: &composite_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 5,
                    resource: params_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 6,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 7,
                    resource: BindingResource::TextureView(&mip_views[0]),
                },
                BindGroupEntry {
                    binding: 8,
                    resource: BindingResource::TextureView(&mip_views[1]),
                },
                BindGroupEntry {
                    binding: 9,
                    resource: BindingResource::TextureView(&mip_views[2]),
                },
                BindGroupEntry {
                    binding: 10,
                    resource: BindingResource::TextureView(&mip_views[3]),
                },
                BindGroupEntry {
                    binding: 11,
                    resource: BindingResource::TextureView(&mip_views[4]),
                },
            ],
        });

        BloomPipelines {
            params_buffer,
            bright_pipeline,
            blur_h_pipelines,
            blur_v_pipelines,
            downsample_pipeline,
            downsample_bgs,
            composite_pipeline,
            mip_textures,
            mip_views,
            ping_textures,
            ping_views,
            bright_bg,
            blur_bgs,
            composite_bg,
            sampler,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn make_bind_group(
        device: &Device,
        bgl: &BindGroupLayout,
        camera_buffer: &Buffer,
        quad_buffer: &Buffer,
        visbuf: &Buffer,
        origins_buffer: &Buffer,
        quad_ids_buffer: &Buffer,
        sector_ids_buffer: &Buffer,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_prepass_bg"),
            layout: bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: camera_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: quad_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: visbuf.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: origins_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: quad_ids_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: sector_ids_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Record and submit a render pass that clears the offscreen target to `color`.
    pub fn render_clear(&mut self, color: [f32; 4]) {
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_clear_pass"),
            });

        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_clear_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: color[0] as f64,
                            g: color[1] as f64,
                            b: color[2] as f64,
                            a: color[3] as f64,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// GPU depth pre-pass (M4c).
    ///
    /// Flattens every `MeshData` into one SSBO, clears the
    /// visibility buffer to the smallest stored value (reversed-Z: far/no-data =
    /// 0), uploads the camera, then runs a single vertex-pulling render pass that
    /// writes the 64-bit visbuf via `atomicMax` (nearest fragment wins).
    ///
    /// Transparent quads are appended after opaque via [`mesh_prepass_bytes`]
    /// until a dedicated blended transparent pass exists (plan 09 §11b).
    /// No-op when the device lacks the `u64`-atomic features required by the
    /// pre-pass.
    pub fn render_prepass(&mut self, meshes: &[MeshData], camera: &CameraView) {
        let refs: Vec<&MeshData> = meshes.iter().collect();
        // Single-sector / test path: every mesh is treated as living at world
        // origin (0,0,0).
        let origins = vec![[0.0f32; 3]; refs.len()];
        self.run_prepass(&refs, &origins, camera);
    }

    /// Shared pre-pass body used by both [`Renderer::render_prepass`] and
    /// [`Renderer::render_frame`]: flatten opaque + transparent quads (interim
    /// single-draw path), upload them + per-quad world origins + camera, record
    /// the vertex-pulling pass into `encoder`, and submit. `origins[i]` is the
    /// world offset of `meshes[i]`.
    pub(crate) fn run_prepass(
        &mut self,
        meshes: &[&MeshData],
        origins: &[[f32; 3]],
        camera: &CameraView,
    ) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };

        // Flatten prepass quads across all meshes into one byte buffer, building
        // the parallel per-quad world-origin + per-quad sector-id + per-quad
        // quad-id buffers as we go. The sector id is the mesh index in the
        // AOI (i.e. position in the `meshes` slice, 0..N-1), NOT a global
        // sector coord — `4-bit sector_id` only needs to disambiguate
        // overlapping sectors in the current frame.
        let mut bytes: Vec<u8> = Vec::new();
        let mut origin_data: Vec<[f32; 4]> = Vec::new();
        let mut sector_id_data: Vec<u32> = Vec::new();
        let mut quad_id_data: Vec<u32> = Vec::new();
        let mut next_quad_id: u32 = 0;
        for (i, (m, o)) in meshes.iter().zip(origins).enumerate() {
            // Worker thread fills `opaque_gpu` / `transparent_gpu`; concat for
            // the interim single-draw prepass (see `mesh_prepass_bytes`).
            let packed = mesh_prepass_bytes(m);
            let n = packed.len() / std::mem::size_of::<PackedQuadGpu>();
            bytes.extend_from_slice(&packed);
            origin_data.extend(std::iter::repeat_n([o[0], o[1], o[2], 0.0], n));
            // M11: every quad of mesh `i` carries the same sector id (= i).
            // 4-bit mask in WGSL means AOI must stay under 16 sectors; the
            // current frustum cull cap matches that.
            sector_id_data.extend(std::iter::repeat_n(i as u32, n));
            // M10a.4: per-quad sector-local id (0..N-1) so the resolve shader
            // can index the per-sector lightmap. Sequential across the whole
            // frame batch — the SSBO is one flat array.
            quad_id_data.extend(next_quad_id..next_quad_id + n as u32);
            next_quad_id += n as u32;
        }
        let quad_count = bytes.len() / std::mem::size_of::<PackedQuadGpu>();

        // Grow the quad + origins + quad_ids + sector_ids SSBOs (and the
        // bind group) if the batch no longer fits.
        if quad_count > prepass.quad_capacity {
            let cap = next_quad_capacity(quad_count, prepass.quad_capacity, self.max_quad_capacity);
            if cap < quad_count {
                // Past device/visbuf cap — refuse rather than allocate an
                // illegal origins SSBO (resolve binding 8).
                return;
            }
            prepass.quad_buffer = make_quad_buffer(&self.device, cap);
            prepass.origins_buffer = make_origins_buffer(&self.device, cap);
            prepass.quad_ids_buffer = make_quad_ids_buffer(&self.device, cap);
            prepass.sector_ids_buffer = make_sector_ids_buffer(&self.device, cap);
            prepass.quad_capacity = cap;
            prepass.bind_group = Self::make_bind_group(
                &self.device,
                &prepass.bgl,
                &prepass.camera_buffer,
                &prepass.quad_buffer,
                &prepass.visbuf,
                &prepass.origins_buffer,
                &prepass.quad_ids_buffer,
                &prepass.sector_ids_buffer,
            );
        }

        if quad_count > 0 {
            self.queue.write_buffer(&prepass.quad_buffer, 0, &bytes);
            self.queue.write_buffer(
                &prepass.origins_buffer,
                0,
                bytemuck::cast_slice(&origin_data),
            );
            self.queue.write_buffer(
                &prepass.quad_ids_buffer,
                0,
                bytemuck::cast_slice(&quad_id_data),
            );
            self.queue.write_buffer(
                &prepass.sector_ids_buffer,
                0,
                bytemuck::cast_slice(&sector_id_data),
            );
        }

        // Clear the visbuf to the smallest stored value (0) under reversed-Z, where
        // a nearer fragment carries a LARGER value. With `atomicMax` this makes the
        // nearest fragment win every pixel. `VisBufEntry::empty()` (max-depth) is the
        // CPU-side far sentinel and is intentionally NOT the GPU clear value here.
        // Cleared GPU-side (no per-frame CPU allocation / PCIe upload of the
        // full framebuffer-sized buffer); the visbuf carries COPY_DST.

        // Upload the camera uniform.
        self.queue
            .write_buffer(&prepass.camera_buffer, 0, bytemuck::bytes_of(camera));

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_prepass"),
            });

        encoder.clear_buffer(&prepass.visbuf, 0, None);

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_prepass_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &prepass.dummy_color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&prepass.pipeline);
            pass.set_bind_group(0, &prepass.bind_group, &[]);
            pass.draw(0..6, 0..quad_count as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Full frame (M4d): CPU frustum-cull the meshes, run the GPU depth pre-pass
    /// into `visbuf`, then run the color-resolve pass that reads `visbuf` and
    /// writes the offscreen `rgba16float` target. Headless: the result lands in
    /// the offscreen texture, readable via [`Renderer::readback`].
    ///
    /// No-op (sky-only clear) when the device lacks the `u64`-atomic features the
    /// pre-pass needs; `render_frame` still resolves to the sky gradient in that
    /// case so the pipeline stays headless-capable.
    pub fn render_frame(&mut self, meshes: &[MeshData], origins: &[[f32; 3]], camera: &CameraView) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_ref() {
            Some(p) => p,
            None => {
                // No pre-pass support: just clear to the sky horizon so the
                // offscreen target is valid (no geometry can be drawn).
                self.render_clear([0.55, 0.70, 0.95, 1.0]);
                return;
            }
        };
        let _ = prepass;

        // Frustum-cull whole meshes (per-sector AABBs, translated to world space
        // by each sector's origin); render visible only, but fall back to all
        // meshes if culling drops everything (defensive).
        let boxes: Vec<Aabb> = meshes
            .iter()
            .zip(origins)
            .map(|(m, o)| aabb_of_mesh(m).translated(*o))
            .collect();
        let visible = cull_visible(&boxes, camera);
        let to_render: Vec<usize> = if visible.is_empty() {
            (0..meshes.len()).collect()
        } else {
            visible
        };

        // Flatten + upload prepass data (visbuf clear, quad SSBO, per-quad world
        // origins, per-quad sector ids, per-quad quad ids, camera).
        let prepass = self.prepass.as_mut().expect("prepass ensured above");
        let mut bytes: Vec<u8> = Vec::new();
        let mut origin_data: Vec<[f32; 4]> = Vec::new();
        let mut sector_id_data: Vec<u32> = Vec::new();
        let mut quad_id_data: Vec<u32> = Vec::new();
        let mut next_quad_id: u32 = 0;
        for (local_idx, &mesh_idx) in to_render.iter().enumerate() {
            let opaque = &meshes[mesh_idx].opaque_gpu;
            let n = opaque.len() / std::mem::size_of::<PackedQuadGpu>();
            bytes.extend_from_slice(opaque);
            let o = origins[mesh_idx];
            origin_data.extend(std::iter::repeat_n([o[0], o[1], o[2], 0.0], n));
            // M11: sector id is the mesh's position in the *to_render* slice
            // (post cull), kept small enough to fit the 4-bit WGSL mask.
            sector_id_data.extend(std::iter::repeat_n(local_idx as u32, n));
            // M10a.4: per-quad sector-local id for the per-sector lightmap.
            quad_id_data.extend(next_quad_id..next_quad_id + n as u32);
            next_quad_id += n as u32;
        }
        let quad_count = bytes.len() / std::mem::size_of::<PackedQuadGpu>();
        if quad_count > prepass.quad_capacity {
            let cap = next_quad_capacity(quad_count, prepass.quad_capacity, self.max_quad_capacity);
            if cap < quad_count {
                return;
            }
            prepass.quad_buffer = make_quad_buffer(&self.device, cap);
            prepass.origins_buffer = make_origins_buffer(&self.device, cap);
            prepass.quad_ids_buffer = make_quad_ids_buffer(&self.device, cap);
            prepass.sector_ids_buffer = make_sector_ids_buffer(&self.device, cap);
            prepass.quad_capacity = cap;
            prepass.bind_group = Self::make_bind_group(
                &self.device,
                &prepass.bgl,
                &prepass.camera_buffer,
                &prepass.quad_buffer,
                &prepass.visbuf,
                &prepass.origins_buffer,
                &prepass.quad_ids_buffer,
                &prepass.sector_ids_buffer,
            );
        }
        if quad_count > 0 {
            self.queue.write_buffer(&prepass.quad_buffer, 0, &bytes);
            self.queue.write_buffer(
                &prepass.origins_buffer,
                0,
                bytemuck::cast_slice(&origin_data),
            );
            self.queue.write_buffer(
                &prepass.quad_ids_buffer,
                0,
                bytemuck::cast_slice(&quad_id_data),
            );
            self.queue.write_buffer(
                &prepass.sector_ids_buffer,
                0,
                bytemuck::cast_slice(&sector_id_data),
            );
        }
        self.queue
            .write_buffer(&prepass.camera_buffer, 0, bytemuck::bytes_of(camera));
        let resolve_params = Self::build_resolve_params(
            camera.width,
            camera.height,
            self.debug_faces,
            self.last_debug_dump,
        );
        self.queue.write_buffer(
            &prepass.resolve_params,
            0,
            bytemuck::bytes_of(&resolve_params),
        );

        // Single encoder: pre-pass writes visbuf, resolve reads it (WebGPU
        // inserts the storage barrier between the two passes). The visbuf is
        // cleared GPU-side below (no per-frame CPU allocation / PCIe upload of the
        // full framebuffer-sized buffer); the buffer carries COPY_DST.
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_frame"),
            });

        encoder.clear_buffer(&prepass.visbuf, 0, None);

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_prepass_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &prepass.dummy_color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&prepass.pipeline);
            pass.set_bind_group(0, &prepass.bind_group, &[]);
            pass.draw(0..6, 0..quad_count as u32);
        }

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_resolve_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&prepass.resolve_pipeline);
            pass.set_bind_group(0, &prepass.resolve_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Grow the quad + origins + lightmap SSBOs to hold at least `needed` quads,
    /// rebuilding the bind group. Call before [`Renderer::upload_quad_region`] /
    /// [`Renderer::upload_lightmap_region`] when the per-sector slot allocator
    /// needs more room.
    ///
    /// Capacity is the min of CPU staging and live GPU buffer sizes. After
    /// [`Renderer::resize`] the pre-pass is dropped while staging stays large;
    /// a staging-only check would early-return and leave the recreated lightmap
    /// at its 32 KB default — then a global-slot `write_buffer` overruns.
    ///
    /// Never grows past [`Renderer::max_quad_capacity`] (device
    /// `max_storage_buffer_binding_size` ÷ 16 B origins ∩ 21-bit visbuf). Requests
    /// above the cap are clamped; the caller must recycle slots instead.
    pub fn ensure_quad_capacity(&mut self, needed: u32) {
        let max_cap = self.max_quad_capacity;
        let needed = (needed as usize).min(max_cap);
        self.ensure_prepass();

        let staging_cap = self.quad_upload_staging.len() / 8;
        let (gpu_quad_cap, lightmap_cap) = match self.prepass.as_ref() {
            Some(p) => (p.quad_capacity, p.lightmap.buffer().size() as usize),
            None => {
                // Pre-pass unavailable (missing device features). Still grow
                // staging so a later successful ensure_prepass can catch up.
                if needed > staging_cap {
                    let new_cap = next_quad_capacity(needed, staging_cap, max_cap);
                    self.quad_upload_staging.resize(new_cap * 8, 0);
                    self.origin_upload_staging.resize(new_cap, [0.0; 4]);
                    self.quad_id_upload_staging.resize(new_cap, 0);
                    self.sector_id_upload_staging.resize(new_cap, 0);
                }
                return;
            }
        };

        if needed <= staging_cap && needed <= gpu_quad_cap && needed <= lightmap_cap {
            return;
        }

        let new_cap = next_quad_capacity(
            needed,
            staging_cap.max(gpu_quad_cap).max(lightmap_cap),
            max_cap,
        );
        // Already at the hard cap — refuse further growth (origins binding 8
        // would exceed max_storage_buffer_binding_size past this point).
        if new_cap <= gpu_quad_cap && new_cap <= staging_cap && new_cap <= lightmap_cap {
            return;
        }

        if new_cap > staging_cap {
            self.quad_upload_staging.resize(new_cap * 8, 0);
            self.origin_upload_staging.resize(new_cap, [0.0; 4]);
            self.quad_id_upload_staging.resize(new_cap, 0);
            self.sector_id_upload_staging.resize(new_cap, 0);
        }

        let prepass = self.prepass.as_mut().expect("prepass checked above");

        let new_quad_buffer = make_quad_buffer(&self.device, new_cap);
        let new_origins_buffer = make_origins_buffer(&self.device, new_cap);
        let new_quad_ids_buffer = make_quad_ids_buffer(&self.device, new_cap);
        let new_sector_ids_buffer = make_sector_ids_buffer(&self.device, new_cap);
        let new_lightmap = LightmapSSBO::new(&self.device, new_cap);

        // Copy only what the *GPU* buffers actually hold — never staging_cap
        // (after resize, staging can be 2M while fresh GPU buffers are tiny).
        let copy_quads = gpu_quad_cap.min(new_cap);
        let copy_lightmap = (lightmap_cap.min(new_cap)) as u64;
        if copy_quads > 0 || copy_lightmap > 0 {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("strata_grow_copy"),
                });
            if copy_quads > 0 {
                let n = copy_quads as u64;
                encoder.copy_buffer_to_buffer(&prepass.quad_buffer, 0, &new_quad_buffer, 0, n * 8);
                encoder.copy_buffer_to_buffer(
                    &prepass.origins_buffer,
                    0,
                    &new_origins_buffer,
                    0,
                    n * std::mem::size_of::<[f32; 4]>() as u64,
                );
                encoder.copy_buffer_to_buffer(
                    &prepass.quad_ids_buffer,
                    0,
                    &new_quad_ids_buffer,
                    0,
                    n * std::mem::size_of::<u32>() as u64,
                );
                encoder.copy_buffer_to_buffer(
                    &prepass.sector_ids_buffer,
                    0,
                    &new_sector_ids_buffer,
                    0,
                    n * std::mem::size_of::<u32>() as u64,
                );
            }
            if copy_lightmap > 0 {
                encoder.copy_buffer_to_buffer(
                    prepass.lightmap.buffer(),
                    0,
                    new_lightmap.buffer(),
                    0,
                    copy_lightmap,
                );
            }
            self.queue.submit(std::iter::once(encoder.finish()));
        }

        prepass.quad_buffer = new_quad_buffer;
        prepass.origins_buffer = new_origins_buffer;
        prepass.quad_ids_buffer = new_quad_ids_buffer;
        prepass.sector_ids_buffer = new_sector_ids_buffer;
        prepass.lightmap = new_lightmap;
        prepass.quad_capacity = new_cap;

        // Update lightmap mask to match new capacity (capacity is power-of-two
        // so `quad_id & (cap - 1)` is a valid index, never past the array).
        debug_assert!(new_cap.is_power_of_two() || new_cap == 0);
        prepass.lightmap_meta.lightmap_mask = new_cap.saturating_sub(1) as u32;
        self.queue.write_buffer(
            &prepass.lightmap_meta_buffer,
            0,
            bytemuck::bytes_of(&prepass.lightmap_meta),
        );

        // Recreate resolve bind group to point to the new lightmap buffer
        prepass.resolve_bind_group = make_resolve_bind_group(
            &self.device,
            &prepass.resolve_bgl,
            &prepass.resolve_params,
            &prepass.visbuf,
            &prepass.block_palette,
            &prepass.lightmap,
            &prepass.lightmap_meta_buffer,
            &prepass.block_textures_view,
            &prepass.block_sampler,
            &prepass.quad_buffer,
            &prepass.origins_buffer,
            &prepass.camera_buffer,
            &prepass.debug_dump_buffer,
        );

        prepass.bind_group = Self::make_bind_group(
            &self.device,
            &prepass.bgl,
            &prepass.camera_buffer,
            &prepass.quad_buffer,
            &prepass.visbuf,
            &prepass.origins_buffer,
            &prepass.quad_ids_buffer,
            &prepass.sector_ids_buffer,
        );
    }

    /// Upload one sector's quads into the SSBO at quad offset `base`. Called by the
    /// client's per-sector slot allocator so only changed/new sectors are written,
    /// never the whole visible set.
    pub fn upload_quad_region(&mut self, base: u32, bytes: &[u8], origins: &[[f32; 4]]) {
        self.ensure_prepass();
        let n = bytes.len() / 8;
        if n == 0 {
            return;
        }
        let needed = base + (n.max(origins.len())) as u32;
        self.ensure_quad_capacity(needed);
        // Stage each sector at its OWN SSBO offset inside a shared scratch vec.
        // Flush writes each discrete span (coalescing only overlap/adjacent) —
        // never a min‥max bounding box across gaps (stale staging in the gap
        // would corrupt other sectors' live slots).
        let quad_off = (base as usize) * 8;
        // `origin_upload_staging` is a float vec with ONE `[f32;4]` entry per quad,
        // indexed by quad number (same as `base`). So the staging index is just
        // `base`, and the slice length is `origins.len()` quads — both in quad
        // units, matching the SSBO's `base` quad offset. The SSBO byte offset in
        // `flush` is `o_start * 16` (4 floats × 4 bytes).
        let origin_idx = base as usize;
        let origin_len = origins.len();
        debug_assert!(self.quad_upload_staging.len() >= quad_off + n * 8);
        debug_assert!(self.origin_upload_staging.len() >= origin_idx + origin_len);
        self.quad_upload_staging[quad_off..quad_off + n * 8].copy_from_slice(&bytes[..n * 8]);
        self.origin_upload_staging[origin_idx..origin_idx + origin_len].copy_from_slice(origins);
        self.upload_spans_quad.push((quad_off, quad_off + n * 8));
        self.upload_spans_origin
            .push((origin_idx, origin_idx + origin_len));
        self.spans_dirty_quad = true;
        self.spans_dirty_origin = true;
        self.pending_upload = true;
    }

    /// Flush all queued `upload_quad_region` calls. Each discrete span becomes
    /// one `write_buffer` (overlapping/adjacent spans coalesce first). Gaps
    /// between non-adjacent sector slots are never copied — staging only holds
    /// fresh bytes at uploaded offsets; writing the gap would stomp live GPU
    /// geometry with stale scratch (classic dig → self+neighbor remesh bug).
    pub fn flush_quad_uploads(&mut self) {
        if !self.pending_upload {
            return;
        }
        self.ensure_prepass();
        let Some(prepass) = self.prepass.as_ref() else {
            // Device lacks u64-atomic features — drop the batch; staging spans
            // are useless without a live quad SSBO.
            self.pending_upload = false;
            self.spans_dirty_quad = false;
            self.spans_dirty_origin = false;
            self.upload_spans_quad.clear();
            self.upload_spans_origin.clear();
            return;
        };
        if self.spans_dirty_quad {
            coalesce_upload_spans(&mut self.upload_spans_quad);
            self.spans_dirty_quad = false;
        }
        if self.spans_dirty_origin {
            coalesce_upload_spans(&mut self.upload_spans_origin);
            self.spans_dirty_origin = false;
        }
        for &(q_start, q_end) in &self.upload_spans_quad {
            if q_start >= q_end {
                continue;
            }
            self.queue.write_buffer(
                &prepass.quad_buffer,
                q_start as u64,
                &self.quad_upload_staging[q_start..q_end],
            );
        }
        for &(o_start, o_end) in &self.upload_spans_origin {
            if o_start >= o_end {
                continue;
            }
            // `o_start`/`o_end` are quad (float) indices; the SSBO byte offset is
            // `o_start * 16` (4 floats × 4 bytes).
            self.queue.write_buffer(
                &prepass.origins_buffer,
                (o_start * 16) as u64,
                bytemuck::cast_slice(&self.origin_upload_staging[o_start..o_end]),
            );
        }
        self.pending_upload = false;
        self.spans_dirty_quad = false;
        self.spans_dirty_origin = false;
        self.upload_spans_quad.clear();
        self.upload_spans_origin.clear();
    }

    /// Upload the camera uniform (cheap; call once per frame).
    pub fn set_camera(&mut self, camera: &CameraView) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        self.queue
            .write_buffer(&prepass.camera_buffer, 0, bytemuck::bytes_of(camera));
    }

    /// Build resolve uniforms for the current frame (clip planes + debug knobs).
    fn build_resolve_params(
        width: u32,
        height: u32,
        debug_faces: bool,
        last_debug_dump: Option<(u32, u32, u32)>,
    ) -> ResolveParams {
        let (mask, x, y) = last_debug_dump.unwrap_or((0, 0, 0));
        ResolveParams {
            width,
            height,
            debug_faces: debug_faces as u32,
            debug_dump_mask: mask,
            debug_dump_x: x,
            debug_dump_y: y,
            ..ResolveParams::new(width, height)
        }
    }

    /// Re-upload the resolve params so a debug-faces toggle is live. Only call
    /// when `debug_faces` actually changed (it is otherwise constant across
    /// frames), so the per-frame path never re-writes 32 static bytes.
    pub fn set_debug_faces(&mut self, on: bool) {
        if on == self.debug_faces {
            return;
        }
        self.debug_faces = on;
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        let params = Self::build_resolve_params(
            self.width,
            self.height,
            self.debug_faces,
            self.last_debug_dump,
        );
        self.queue
            .write_buffer(&prepass.resolve_params, 0, bytemuck::bytes_of(&params));
    }

    /// Configure the resolve-fragment debug dump. Pass `mask = 0` to disable.
    /// The dump is sampled at pixel `(x, y)` on the next frame. The
    /// [`Renderer::dump_debug`] method clears the SSBO, re-issues the frame
    /// with the requested mask and target pixel, and reads back the two
    /// `vec4<f32>` slots containing the selected signals (see
    /// [`crate::pipeline::resolve::debug_dump`]). Returns `None` if the
    /// pre-pass is not available on this device.
    pub fn set_debug_dump(&mut self, mask: u32, x: u32, y: u32) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        // M10a.4-dbg: live-toggle the mask + target pixel. The dump_mask is
        // also captured in the `last_debug_dump` field so `dump_debug` can
        // read the SSBO back without re-uploading params.
        self.queue.write_buffer(
            &prepass.resolve_params,
            0,
            bytemuck::bytes_of(&ResolveParams {
                debug_faces: self.debug_faces as u32,
                debug_dump_mask: mask,
                debug_dump_x: x,
                debug_dump_y: y,
                ..ResolveParams::new(self.width, self.height)
            }),
        );
        self.last_debug_dump = Some((mask, x, y));
    }

    /// Run a debug-dump request: ensures the mask is uploaded, copies the
    /// SSBO to a staging buffer, and `eprintln!`s a labelled summary. The
    /// caller is responsible for having issued a real `render_frame` with
    /// a mesh *before* calling `dump_debug` — the resolve pass writes the
    /// dump slot only on the geometry frame, not on a cleared/no-geometry
    /// frame (sky pixels have `quad_id == 0` and would zero the dump).
    /// Returns `None` if the pre-pass is not available or no
    /// `set_debug_dump` was issued yet.
    pub fn dump_debug(&mut self, label: &str) -> Option<[f32; 8]> {
        use crate::pipeline::resolve::debug_dump;
        let (mask, x, y) = self.last_debug_dump?;
        if mask == 0 {
            eprintln!("[debug:{}] mask=0 — nothing to dump", label);
            return None;
        }
        // The caller must have already rendered a geometry frame. We
        // don't re-issue a no-geometry frame here because that would
        // clear the dump pixel (sky has no quad_id and the resolve
        // shader never re-writes the slot when the underlying pixel is
        // empty — the SSBO keeps its prior content but only if we
        // never submit another resolve pass that overwrites it).
        // The CPU-side fence below is the real synchronization point.
        self.ensure_prepass();
        let (buffer, staging) = {
            let prepass = self.prepass.as_mut()?;
            (
                prepass.debug_dump_buffer.clone(),
                prepass.debug_dump_staging.clone(),
            )
        };
        // CPU-side fence: make sure the GPU is done with the *previous*
        // (geometry) submission before we copy the SSBO out. Without
        // this, `map_async(wait_indefinitely)` can still race against
        // an in-flight submission and read stale (zeroed) data.
        self.queue.submit(std::iter::empty());
        // Copy the SSBO to the staging buffer and map for read.
        let size = (debug_dump::SLOT_COUNT as u64) * 16;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("strata_debug_dump_copy"),
            });
        encoder.copy_buffer_to_buffer(&buffer, 0, &staging, 0, size);
        self.queue.submit(std::iter::once(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let mapped = slice.get_mapped_range();
        let slots: [[f32; 4]; debug_dump::SLOT_COUNT] = *bytemuck::from_bytes(&mapped);
        drop(mapped);
        staging.unmap();
        let out = [
            slots[0][0],
            slots[0][1],
            slots[0][2],
            slots[0][3],
            slots[1][0],
            slots[1][1],
            slots[1][2],
            slots[1][3],
        ];
        eprintln!(
            "[debug:{}] mask=0x{:02x} px=({},{})  slot0(ao_smooth,ao_i,ao_mult,ao_corners)={:?}  slot1(quad_id,uv.x,uv.y,raw_ao)={:?}",
            label, mask, x, y, &slots[0], &slots[1]
        );
        Some(out)
    }

    /// Install the block registry. The base-color table is uploaded to a fresh
    /// block-palette SSBO and the resolve bind group is rebuilt to point at it
    /// (M10a.2). Cheap to call repeatedly; the upload only happens when the
    /// storage buffer's identity changes.
    pub fn set_block_registry(&mut self, registry: &strata_core::registry::BlockRegistry) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };

        // 1. Collect unique texture names and map them to indices
        let mut unique_textures = std::collections::BTreeSet::new();
        for textures_arr in &registry.textures {
            for tex in textures_arr {
                if tex != "air" {
                    unique_textures.insert(tex.clone());
                }
            }
        }
        let unique_textures: Vec<String> = unique_textures.into_iter().collect();
        let mut texture_mapping = std::collections::HashMap::new();
        for (i, name) in unique_textures.iter().enumerate() {
            texture_mapping.insert(name.clone(), i as u32);
        }

        // 2. Load the actual PNG images (resize first, then alpha-bake overlays).
        let mut loaded = std::collections::HashMap::new();
        for name in &unique_textures {
            let path = format!("assets/textures/{}.png", name);
            let img = match image::open(&path) {
                Ok(img) => img.to_rgba8(),
                Err(e) => {
                    bevy::log::warn!(
                        "Failed to load texture '{}' from '{}': {}, using fallback magenta",
                        name,
                        path,
                        e
                    );
                    image::RgbaImage::from_fn(16, 16, |_, _| image::Rgba([255, 0, 255, 255]))
                }
            };
            let img = if img.width() != BLOCK_TEX_SIZE || img.height() != BLOCK_TEX_SIZE {
                image::imageops::resize(
                    &img,
                    BLOCK_TEX_SIZE,
                    BLOCK_TEX_SIZE,
                    image::imageops::FilterType::Nearest,
                )
            } else {
                img
            };
            loaded.insert(name.clone(), img);
        }

        if loaded
            .get("grass_side")
            .is_some_and(rgba_image_has_partial_alpha)
        {
            let dirt = loaded.get("dirt").cloned();
            if let (Some(grass), Some(dirt)) = (loaded.get_mut("grass_side"), dirt) {
                bake_texture_alpha_over_backdrop(grass, &dirt);
            }
        }

        let layers: Vec<image::RgbaImage> = unique_textures
            .iter()
            .map(|name| loaded.remove(name).expect("texture loaded above"))
            .collect();

        // 3. Create the 2D Texture Array with a full mip chain (16→8→4→2→1).
        // Without mips, distant faces alias into grainy "ants" under Nearest.
        let layer_count = layers.len().max(1);
        let mip_count = block_tex_mip_count(BLOCK_TEX_SIZE);
        let textures_array = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("strata_block_textures_array"),
            size: wgpu::Extent3d {
                width: BLOCK_TEX_SIZE,
                height: BLOCK_TEX_SIZE,
                depth_or_array_layers: layer_count as u32,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        if layers.is_empty() {
            let magenta = image::RgbaImage::from_pixel(
                BLOCK_TEX_SIZE,
                BLOCK_TEX_SIZE,
                image::Rgba([255, 0, 255, 255]),
            );
            for (level, mip) in generate_block_tex_mips(&magenta).iter().enumerate() {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &textures_array,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    mip.as_raw(),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(mip.width() * 4),
                        rows_per_image: Some(mip.height()),
                    },
                    wgpu::Extent3d {
                        width: mip.width(),
                        height: mip.height(),
                        depth_or_array_layers: 1,
                    },
                );
            }
        } else {
            for (i, layer) in layers.iter().enumerate() {
                for (level, mip) in generate_block_tex_mips(layer).iter().enumerate() {
                    self.queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &textures_array,
                            mip_level: level as u32,
                            origin: wgpu::Origin3d {
                                x: 0,
                                y: 0,
                                z: i as u32,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        mip.as_raw(),
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(mip.width() * 4),
                            rows_per_image: Some(mip.height()),
                        },
                        wgpu::Extent3d {
                            width: mip.width(),
                            height: mip.height(),
                            depth_or_array_layers: 1,
                        },
                    );
                }
            }
        }

        let textures_view = textures_array.create_view(&wgpu::TextureViewDescriptor {
            label: Some("strata_block_textures_view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..wgpu::TextureViewDescriptor::default()
        });

        // Store them
        prepass.block_textures = textures_array;
        prepass.block_textures_view = textures_view;

        // 4. Upload block palette (with mapping!)
        let new_palette =
            BlockPalette::upload(&self.device, &self.queue, registry, &texture_mapping);
        prepass.block_palette = new_palette;
        prepass.lightmap_meta.palette_size = prepass.block_palette.capacity();
        self.queue.write_buffer(
            &prepass.lightmap_meta_buffer,
            0,
            bytemuck::bytes_of(&prepass.lightmap_meta),
        );

        // 5. Rebuild bind group
        prepass.resolve_bind_group = make_resolve_bind_group(
            &self.device,
            &prepass.resolve_bgl,
            &prepass.resolve_params,
            &prepass.visbuf,
            &prepass.block_palette,
            &prepass.lightmap,
            &prepass.lightmap_meta_buffer,
            &prepass.block_textures_view,
            &prepass.block_sampler,
            &prepass.quad_buffer,
            &prepass.origins_buffer,
            &prepass.camera_buffer,
            &prepass.debug_dump_buffer,
        );
    }
    /// shader is about to sample; the resolve shader uses a single global
    /// lightmap SSBO, so the caller is responsible for picking which sector's
    /// light is bound (the current focus sector near the camera is the usual
    /// choice). Trailing bytes beyond `bytes.len()` are zeroed (already-zero
    /// at allocation; the upload routine pads in-place).
    pub fn upload_lightmap(&mut self, bytes: &[LightmapEntry]) {
        self.upload_lightmap_region(0, bytes);
    }

    /// Upload one sector's per-quad lightmap at global SSBO slot `base`.
    ///
    /// `base` is the same bump-allocator offset used by [`Self::upload_quad_region`],
    /// not a sector-local index. Grows the lightmap SSBO when `base + len` exceeds
    /// the current buffer (initial size is only [`SECTOR_LIGHTMAP_QUADS`]).
    pub fn upload_lightmap_region(&mut self, base: u32, bytes: &[LightmapEntry]) {
        if bytes.is_empty() {
            return;
        }
        let needed = lightmap_region_end(base, bytes.len());
        self.ensure_quad_capacity(needed as u32);

        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        // Pad size to a multiple of 4 bytes (wgpu COPY_BUFFER_ALIGNMENT requirement)
        if bytes.len() % 4 == 0 {
            prepass
                .lightmap
                .write_offset(&self.queue, base as u64, bytes);
        } else {
            let mut padded = bytes.to_vec();
            while padded.len() % 4 != 0 {
                padded.push(LightmapEntry(0));
            }
            prepass
                .lightmap
                .write_offset(&self.queue, base as u64, &padded);
        }
    }

    /// Upload a batch of per-quad ids into the SSBO. Prefer identity: the
    /// pre-pass writes `out.quad_id = instance_index` (global SSBO slot), which
    /// matches `draw_quad_ranges` / `upload_lightmap_region` addressing. This
    /// entry point remains for optional non-identity remaps.
    pub fn upload_quad_ids(&mut self, base: u32, ids: &[u32]) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        self.queue.write_buffer(
            &prepass.quad_ids_buffer,
            (base as u64) * 4,
            bytemuck::cast_slice(ids),
        );
    }

    /// Clear the visibility buffer and run the depth pre-pass + color-resolve
    /// passes, drawing the resident sectors. The `ranges` (base, count in quads)
    /// are the per-sector SSBO slot spans produced by the client's slot
    /// allocator. Because quads live in one shared SSBO and each quad carries its
    /// own world origin (see `PREPASS_WGSL`), adjacent spans are merged into
    /// contiguous runs and drawn with a single `draw` per run — collapsing the
    /// handful-of-runs result into a few draw calls instead of one-per-sector
    /// (the prior 343 draws/frame dominated `us_draw`). Call after
    /// [`Renderer::upload_quad_region`] / [`Renderer::set_camera`].
    ///
    /// After the resolve pass, M10c runs the bloom composite into the same HDR
    /// target (bright extract → 5 separable-blur mips → additive blend). Pass
    /// `bloom` to override the default intensity/threshold; pass `None` to keep
    /// the values currently in the GPU uniform.
    /// Returns the number of draw calls issued (one per merged run) for stats.
    pub fn draw_quad_ranges(
        &mut self,
        ranges: &[(u32, u32)],
        bloom: Option<&BloomParams>,
    ) -> usize {
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return 0,
        };

        // Merge contiguous slot spans into a minimal set of runs so the GPU
        // issues one draw per run, not one per sector. Spans are sorted by base;
        // bit-adjacent spans (start == prev_end) fuse into one.
        let mut sorted = ranges.to_vec();
        sorted.sort_by_key(|a| a.0);
        let mut runs: Vec<(u32, u32)> = Vec::with_capacity(sorted.len());
        for &(base, count) in &sorted {
            if count == 0 {
                continue;
            }
            if let Some(last) = runs.last_mut() {
                let prev_end = last.0 + last.1;
                if base == prev_end {
                    last.1 += count;
                    continue;
                }
            }
            runs.push((base, count));
        }

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_frame"),
            });

        // Clear the visbuf GPU-side (no per-frame CPU allocation / PCIe upload of
        // the full framebuffer-sized buffer); the visbuf carries COPY_DST.
        encoder.clear_buffer(&prepass.visbuf, 0, None);

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_prepass_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &prepass.dummy_color,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&prepass.pipeline);
            pass.set_bind_group(0, &prepass.bind_group, &[]);
            for (base, count) in &runs {
                if *count > 0 {
                    pass.draw(0..6, *base..*base + *count);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_resolve_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &self.offscreen_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&prepass.resolve_pipeline);
            pass.set_bind_group(0, &prepass.resolve_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // M10c: bloom (bright extract → 5 separable-blur mips → composite
        // additive blend into the HDR target). No-op when the user disabled
        // bloom (params.intensity == 0) or the resources haven't been built
        // (the call site asks for the build lazily; this never allocates).
        if let Some(params) = bloom
            && let Some(bloom) = if self.bloom.is_some() {
                self.bloom.as_ref()
            } else {
                self.ensure_bloom();
                self.bloom.as_ref()
            }
        {
            run_bloom(
                &self.queue,
                &mut encoder,
                bloom,
                &self.offscreen_view,
                params,
            );
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        runs.len()
    }

    /// Copy the visibility buffer into the staging buffer, map it, and return
    /// the raw `u64` entries (one per pixel, row-major). Empty when the pre-pass
    /// is unavailable on this device.
    pub fn read_visbuf(&self) -> Vec<u64> {
        let prepass = match self.prepass.as_ref() {
            Some(p) => p,
            None => return Vec::new(),
        };

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_visbuf_readback"),
            });
        encoder.copy_buffer_to_buffer(
            &prepass.visbuf,
            0,
            &prepass.visbuf_staging,
            0,
            (prepass.visbuf_pixels as u64) * std::mem::size_of::<u64>() as u64,
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = prepass.visbuf_staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(PollType::wait_indefinitely());
        rx.recv().expect("visbuf map signal").expect("visbuf map");

        let mapped = slice.get_mapped_range();
        let out: Vec<u64> = bytemuck::cast_slice(&mapped).to_vec();
        drop(mapped);
        prepass.visbuf_staging.unmap();
        out
    }

    /// Copy the offscreen HDR target into the staging buffer, map it, and return
    /// the raw RGBA16 (8 bytes/pixel, little-endian half-floats) bytes with row
    /// padding stripped.
    pub fn readback(&self) -> Vec<u8> {
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_readback"),
            });

        encoder.copy_texture_to_buffer(
            TexelCopyTextureInfo {
                texture: &self.offscreen,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            TexelCopyBufferInfo {
                buffer: &self.staging,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = self.staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(PollType::wait_indefinitely());
        rx.recv()
            .expect("readback map signal")
            .expect("readback map");

        let mapped = slice.get_mapped_range();
        let mut out = vec![0u8; (self.width * self.height * BYTES_PER_PIXEL) as usize];
        let row_bytes = (self.width * BYTES_PER_PIXEL) as usize;
        for y in 0..self.height as usize {
            let src_start = y * self.bytes_per_row as usize;
            let dst_start = y * row_bytes;
            out[dst_start..dst_start + row_bytes]
                .copy_from_slice(&mapped[src_start..src_start + row_bytes]);
        }
        drop(mapped);
        self.staging.unmap();

        out
    }

    /// Resize the offscreen HDR target (and staging buffer) to `width`x`height`.
    /// Drops the pre-pass and present blit so they are rebuilt lazily at the new
    /// resolution/format. Cheap relative to a frame; call only on window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        assert!(
            width > 0 && height > 0,
            "offscreen target must be non-empty"
        );
        self.width = width;
        self.height = height;

        self.offscreen = self.device.create_texture(&TextureDescriptor {
            label: Some("strata_offscreen_hdr"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::COPY_SRC
                | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.offscreen_view = self
            .offscreen
            .create_view(&TextureViewDescriptor::default());

        self.bytes_per_row = (width * BYTES_PER_PIXEL).div_ceil(256) * 256;
        self.staging = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_offscreen_staging"),
            size: (self.bytes_per_row * height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        self.prepass = None;
        self.blit = None;
        if let Some(hud) = self.hud.as_mut() {
            hud.resize_crosshair(&self.queue, width, height);
        }
    }

    /// Present the offscreen HDR target to a window `surface_view` via a tiny
    /// fullscreen-triangle blit that samples the `rgba16float` offscreen texture
    /// and writes (linear) color to the surface (format-converted implicitly by
    /// the pipeline's target format). The blit pipeline/bind group are built
    /// lazily for the requested `surface_format`.
    pub fn present(&mut self, surface_view: &TextureView, surface_format: TextureFormat) {
        let needs_build = self
            .blit
            .as_ref()
            .is_none_or(|b| b.format != surface_format);
        if needs_build {
            let blit = self.build_blit(surface_format);
            self.blit = Some(blit);
        }
        let blit = self.blit.as_ref().expect("blit built above");

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_present"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("strata_present_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Load,
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&blit.pipeline);
            pass.set_bind_group(0, &blit.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw the Minecraft-style selection wireframe (optional) and center
    /// crosshair onto the swapchain after [`Renderer::present`].
    ///
    /// When a prepass visbuf exists for the current frame, wireframe fragments
    /// behind scene geometry are discarded (packed 17-bit reversed-Z).
    pub fn draw_selection_hud(
        &mut self,
        surface_view: &TextureView,
        surface_format: TextureFormat,
        camera: &CameraView,
        outline_lines: Option<&[[f32; 3]]>,
    ) {
        let needs_build = self.hud.as_ref().is_none_or(|h| h.format != surface_format);
        if needs_build {
            self.hud = Some(overlay::HudOverlay::build(
                &self.device,
                surface_format,
                self.width,
                self.height,
            ));
        }
        let visbuf = self.prepass.as_ref().map(|p| &p.visbuf);
        overlay::draw_hud(
            &self.device,
            &self.queue,
            self.hud.as_mut().expect("hud built above"),
            surface_view,
            camera,
            outline_lines,
            visbuf,
        );
    }

    /// Build the present blit pipeline + bind group for `surface_format`. The bind
    /// group samples our own `offscreen_view`, so it must be rebuilt whenever the
    /// offscreen texture is recreated (resize).
    fn build_blit(&self, surface_format: TextureFormat) -> Blit {
        let layout = self
            .device
            .create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("strata_present_bgl"),
                entries: &[BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: false },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });

        let pipeline_layout = self
            .device
            .create_pipeline_layout(&PipelineLayoutDescriptor {
                label: Some("strata_present_pl"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });

        let module = self.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_present"),
            source: ShaderSource::Wgsl(BLIT_WGSL.into()),
        });

        let pipeline = self
            .device
            .create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("strata_present_pipeline"),
                layout: Some(&pipeline_layout),
                vertex: VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    targets: &[Some(ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });

        let bind_group = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_present_bg"),
            layout: &layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(&self.offscreen_view),
            }],
        });

        Blit {
            pipeline,
            layout,
            bind_group,
            format: surface_format,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// Decode an IEEE-754 `binary16` half-float (little-endian `u16`) to `f32`.
#[cfg(test)]
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 0x1;
    let exp = (bits >> 10) & 0x1f;
    let mant = bits & 0x3ff;

    let value = if exp == 0 {
        (mant as f32) * (2f32).powi(-24)
    } else if exp == 0x1f {
        if mant == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        (1.0 + (mant as f32) / 1024.0) * (2f32).powi((exp as i32) - 15)
    };

    if sign == 1 { -value } else { value }
}

/// M10c: run the full bloom pipeline (bright extract → 5 mips of H+V blur →
/// additive composite into the HDR target) using the resources cached in
/// `self.bloom`. No-op when bloom is disabled (`params.intensity == 0`), when
/// the user opted out entirely, or when the resources haven't been built yet
/// (the call site asks for the build via `ensure_bloom` before the first
/// frame). The encoder is borrowed mutably; no allocations happen on this
/// path beyond the per-pass `RenderPassDescriptor` which is stack-only.
fn run_bloom(
    queue: &Queue,
    encoder: &mut CommandEncoder,
    bloom: &BloomPipelines,
    offscreen_view: &TextureView,
    params: &BloomParams,
) {
    if params.intensity <= 0.0 {
        return;
    }

    // Upload the params uniform once per frame (16 bytes; cheap).
    queue.write_buffer(&bloom.params_buffer, 0, bytemuck::bytes_of(params));

    let mip_count = bloom.mip_views.len();

    // 1) Bright extract: HDR target → mip 0.
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("strata_bloom_bright"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &bloom.mip_views[0],
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&bloom.bright_pipeline);
        pass.set_bind_group(0, &bloom.bright_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    // 2) Downsample chain: mip 0 → mip 1 → mip 2 → … so each mip has
    // valid bright-extract data from its parent before the Gaussian blur
    // is applied. The linear sampler does 2×2 bilinear averaging at half
    // resolution, which is the standard half-res bloom downsample.
    for mip in 0..mip_count.saturating_sub(1) {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(&format!("strata_bloom_downsample_{mip}")),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &bloom.mip_views[mip + 1],
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&bloom.downsample_pipeline);
        pass.set_bind_group(0, &bloom.downsample_bgs[mip], &[]);
        pass.draw(0..3, 0..1);
    }

    // 3) Per-mip separable Gaussian. For each mip, alternate ping-pong so
    // H reads mip_textures[i] and writes ping_textures[i], V reads
    // ping_textures[i] and writes mip_textures[i]. After both passes
    // mip_textures[i] holds the blurred output and is sampled by the
    // composite pass.
    for mip in 0..mip_count {
        let bgs = &bloom.blur_bgs[mip];
        // H
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(&format!("strata_bloom_blur_h_{mip}")),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &bloom.ping_views[mip],
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&bloom.blur_h_pipelines[mip]);
            pass.set_bind_group(0, &bgs.h, &[]);
            pass.draw(0..3, 0..1);
        }
        // V (write back into the mip texture)
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some(&format!("strata_bloom_blur_v_{mip}")),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &bloom.mip_views[mip],
                    resolve_target: None,
                    depth_slice: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&bloom.blur_v_pipelines[mip]);
            pass.set_bind_group(0, &bgs.v, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    // 4) Composite: additive blend of the mip pyramid into the HDR target.
    // The composite pipeline reads each mip at full-rate (it samples every
    // pixel) and writes a final color value with `intensity` baked in. We
    // use the standard blend equation (one / one) to add to whatever the
    // resolve pass left in the HDR target.
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("strata_bloom_composite"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: offscreen_view,
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Load,
                    store: StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&bloom.composite_pipeline);
        pass.set_bind_group(0, &bloom.composite_bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn make_resolve_bind_group(
    device: &wgpu::Device,
    resolve_bgl: &wgpu::BindGroupLayout,
    resolve_params: &wgpu::Buffer,
    visbuf: &wgpu::Buffer,
    block_palette: &BlockPalette,
    lightmap: &LightmapSSBO,
    lightmap_meta_buffer: &wgpu::Buffer,
    block_textures_view: &wgpu::TextureView,
    block_sampler: &wgpu::Sampler,
    quad_buffer: &wgpu::Buffer,
    origins_buffer: &wgpu::Buffer,
    camera_buffer: &wgpu::Buffer,
    debug_dump_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("strata_resolve_bg"),
        layout: resolve_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: resolve_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: visbuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: block_palette.buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: lightmap.buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: lightmap_meta_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(block_textures_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(block_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: quad_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: origins_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: debug_dump_buffer.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use strata_core::prelude::*;
    use strata_core::registry::load_block_registry;

    use crate::meshing::{GreedyMesher, Mesher, NeighborView};
    use crate::pipeline::camera::{look_at_rh, perspective_rh_zo};

    #[test]
    fn test_offscreen_clear() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());

        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(adapter) => adapter,
            Err(_) => {
                // Headless/no-GPU environment: the test cannot create a device.
                eprintln!(
                    "test_offscreen_clear IGNORED: no wgpu adapter available in this \
                      headless/no-GPU environment"
                );
                return;
            }
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            ..Default::default()
        }))
        .expect("request_device failed");

        const W: u32 = 64;
        const H: u32 = 64;
        let mut renderer = Renderer::new(device, queue, W, H);

        let clear = [0.2f32, 0.4, 0.8, 1.0];
        renderer.render_clear(clear);
        let pixels = renderer.readback();

        let cx = (W / 2) as usize;
        let cy = (H / 2) as usize;
        let base = (cy * W as usize + cx) * BYTES_PER_PIXEL as usize;

        let r = f16_to_f32(u16::from_le_bytes([pixels[base], pixels[base + 1]]));
        let g = f16_to_f32(u16::from_le_bytes([pixels[base + 2], pixels[base + 3]]));
        let b = f16_to_f32(u16::from_le_bytes([pixels[base + 4], pixels[base + 5]]));
        let a = f16_to_f32(u16::from_le_bytes([pixels[base + 6], pixels[base + 7]]));

        let tol = 0.01f32;
        assert!(
            (r - clear[0]).abs() < tol,
            "red channel mismatch: got {r}, want {}",
            clear[0]
        );
        assert!(
            (g - clear[1]).abs() < tol,
            "green channel mismatch: got {g}, want {}",
            clear[1]
        );
        assert!(
            (b - clear[2]).abs() < tol,
            "blue channel mismatch: got {b}, want {}",
            clear[2]
        );
        assert!(
            (a - clear[3]).abs() < tol,
            "alpha channel mismatch: got {a}, want {}",
            clear[3]
        );
    }

    /// Build one solid voxel in an otherwise-empty 32³ sector, mesh it, run the
    /// GPU depth pre-pass, and assert that at least one visbuf pixel was written
    /// (a non-zero entry) — i.e. the cube was rasterized.
    ///
    /// The pre-pass needs native `u64`-atomic features; if no adapter exposes
    /// them the test is skipped (the WGSL still compiles).
    #[test]
    fn test_prepass_writes_visbuf() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());

        // Search all adapters for one that exposes the `u64`-atomic features the
        // pre-pass requires, so the test runs on capable GPUs.
        let adapter = match pollster::block_on(instance.enumerate_adapters(Backends::all()))
            .into_iter()
            .find(|a| a.features().contains(prepass_features()))
        {
            Some(a) => a,
            None => {
                eprintln!(
                    "test_prepass_writes_visbuf IGNORED: no wgpu adapter with \
                      SHADER_INT64 + SHADER_INT64_ATOMIC_MIN_MAX available"
                );
                return;
            }
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_prepass_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        }))
        .expect("request_device failed");

        const W: u32 = 64;
        const H: u32 = 64;
        let mut renderer = Renderer::new(device, queue, W, H);

        // Single solid voxel at sector-local (16,16,16).
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(16, 16, 16),
            BlockId(1),
        );

        let registry = load_block_registry();
        let mesher = GreedyMesher::new(&registry);
        let none_nv = NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        };
        let neighbors = [none_nv; 6];
        let mesh = mesher.mesh_sector(&map, &palette, &pool, &registry, &neighbors);
        assert!(
            !mesh.opaque.is_empty(),
            "single solid voxel must produce opaque quads"
        );

        let aspect = W as f32 / H as f32;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 100.0);
        let eye = [36.0, 36.0, 36.0];
        let view = look_at_rh(eye, [16.0, 16.0, 16.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, W, H);

        renderer.render_prepass(std::slice::from_ref(&mesh), &cam);

        let visbuf = renderer.read_visbuf();
        // The GPU clear sentinel is the smallest stored value (0 = far/no-data);
        // any rasterized fragment carries a non-zero entry (reversed-Z, near=large).
        let cleared = VisBufEntry(0).raw();
        let written = visbuf.iter().any(|&e| e != cleared);
        assert!(
            written,
            "depth pre-pass must rasterize the cube into at least one visbuf pixel"
        );
    }

    /// Full M4d frame: build a single solid voxel, run `render_frame`, read back
    /// the offscreen HDR target, and assert that NOT every pixel equals the sky
    /// gradient (i.e. the cube's geometry pixels are visible).
    ///
    /// Requires an adapter exposing `SHADER_INT64` atomics; otherwise it skips.
    #[test]
    fn test_render_frame_produces_terrain() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());

        let adapter = match pollster::block_on(instance.enumerate_adapters(Backends::all()))
            .into_iter()
            .find(|a| a.features().contains(prepass_features()))
        {
            Some(a) => a,
            None => {
                eprintln!(
                    "test_render_frame_produces_terrain IGNORED: no wgpu adapter with \
                       SHADER_INT64 + SHADER_INT64_ATOMIC_MIN_MAX available"
                );
                return;
            }
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_frame_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        }))
        .expect("request_device failed");

        const W: u32 = 64;
        const H: u32 = 64;
        let mut renderer = Renderer::new(device, queue, W, H);

        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        // A 2x2x2 block so the cube is unambiguously larger than a single voxel.
        for dx in 0..2u32 {
            for dy in 0..2u32 {
                for dz in 0..2u32 {
                    map.set_block(
                        &mut pool,
                        &mut palette,
                        VoxelCoord::new(16 + dx, 16 + dy, 16 + dz),
                        BlockId(1),
                    );
                }
            }
        }

        let registry = load_block_registry();
        let mesher = GreedyMesher::new(&registry);
        let none_nv = NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        };
        let neighbors = [none_nv; 6];
        let mesh = mesher.mesh_sector(&map, &palette, &pool, &registry, &neighbors);
        assert!(
            !mesh.opaque.is_empty(),
            "2x2x2 block must produce opaque quads"
        );

        let aspect = W as f32 / H as f32;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 100.0);
        let eye = [36.0, 36.0, 36.0];
        let view = look_at_rh(eye, [17.0, 17.0, 17.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, W, H);

        renderer.render_frame(std::slice::from_ref(&mesh), &[[0.0; 3]], &cam);

        let pixels = renderer.readback();
        let row_bytes = (W * BYTES_PER_PIXEL) as usize;

        // Reference sky color at the vertical center (should be near horizon).
        let cy = (H / 2) as usize;
        let base = cy * row_bytes + (W as usize / 2) * BYTES_PER_PIXEL as usize;
        let sky_r = f16_to_f32(u16::from_le_bytes([pixels[base], pixels[base + 1]]));
        let sky_g = f16_to_f32(u16::from_le_bytes([pixels[base + 2], pixels[base + 3]]));
        let sky_b = f16_to_f32(u16::from_le_bytes([pixels[base + 4], pixels[base + 5]]));

        // Count pixels that differ from the center sky color (geometry pixels).
        let mut non_sky = 0usize;
        let tol = 0.02f32;
        for y in 0..H as usize {
            for x in 0..W as usize {
                let o = y * row_bytes + x * BYTES_PER_PIXEL as usize;
                let r = f16_to_f32(u16::from_le_bytes([pixels[o], pixels[o + 1]]));
                let g = f16_to_f32(u16::from_le_bytes([pixels[o + 2], pixels[o + 3]]));
                let b = f16_to_f32(u16::from_le_bytes([pixels[o + 4], pixels[o + 5]]));
                let diff = (r - sky_r).abs() + (g - sky_g).abs() + (b - sky_b).abs();
                if diff > tol {
                    non_sky += 1;
                }
            }
        }

        assert!(
            non_sky > 0,
            "frame must contain geometry pixels distinct from the sky gradient \
               (terrain must be visible); non_sky={non_sky}, sky=({sky_r},{sky_g},{sky_b})"
        );
    }

    /// Regression for the "every sector stacked at the world origin" bug: a
    /// sector rendered with a non-zero world origin must rasterize at that WORLD
    /// position, not at the sector-local 0..31 cube. Uses the same camera (aimed
    /// at the local cube's world position, 17,17,17) for both origins and counts
    /// rasterized visbuf fragments: with origin 0 the geometry is in view; with a
    /// large origin it moves out of view. If `render_frame` ignored the origin
    /// (the old bug) both counts would be equal and non-zero.
    #[test]
    fn test_render_frame_applies_sector_origin() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());

        let adapter = match pollster::block_on(instance.enumerate_adapters(Backends::all()))
            .into_iter()
            .find(|a| a.features().contains(prepass_features()))
        {
            Some(a) => a,
            None => {
                eprintln!(
                    "test_render_frame_applies_sector_origin IGNORED: no wgpu adapter with \
                       SHADER_INT64 + SHADER_INT64_ATOMIC_MIN_MAX available"
                );
                return;
            }
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_origin_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        }))
        .expect("request_device failed");

        const W: u32 = 64;
        const H: u32 = 64;
        let mut renderer = Renderer::new(device, queue, W, H);

        // 2x2x2 block at sector-local (16..18) in a sector whose world origin is
        // (64,64,64) — i.e. real world center ~ (81,81,81).
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(2, 2, 2));
        for dx in 0..2u32 {
            for dy in 0..2u32 {
                for dz in 0..2u32 {
                    map.set_block(
                        &mut pool,
                        &mut palette,
                        VoxelCoord::new(16 + dx, 16 + dy, 16 + dz),
                        BlockId(1),
                    );
                }
            }
        }

        let registry = load_block_registry();
        let mesher = GreedyMesher::new(&registry);
        let none_nv = NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        };
        let neighbors = [none_nv; 6];
        let mesh = mesher.mesh_sector(&map, &palette, &pool, &registry, &neighbors);
        assert!(!mesh.opaque.is_empty(), "2x2x2 block must produce quads");

        let aspect = W as f32 / H as f32;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 200.0);

        // Camera aimed at the sector-LOCAL cube position in world space (17,17,17).
        let eye = [36.0f32, 36.0, 36.0];
        let view = look_at_rh(eye, [17.0, 17.0, 17.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, W, H);

        // Rasterized fragments = non-zero visbuf entries (unambiguous, no color heuristic).
        let rasterized = |renderer: &Renderer| -> usize {
            renderer.read_visbuf().iter().filter(|&&e| e != 0).count()
        };

        // origin (0,0,0): geometry sits at world 16..18, squarely in view.
        renderer.render_frame(std::slice::from_ref(&mesh), &[[0.0; 3]], &cam);
        let at_origin = rasterized(&renderer);
        assert!(
            at_origin > 0,
            "control: geometry at origin 0 must be in view; at_origin={at_origin}"
        );

        // origin (64,64,64): the SAME camera now looks at empty space (geometry
        // moved to world ~81, behind/out of view) — so far fewer fragments.
        renderer.render_frame(std::slice::from_ref(&mesh), &[[64.0, 64.0, 64.0]], &cam);
        let offset = rasterized(&renderer);
        assert!(
            offset < at_origin,
            "geometry must move with its sector origin (was stacked at local 0..31); \
               at_origin={at_origin}, offset={offset}"
        );
    }

    /// M10a.4-dbg: round-trip for the resolve-fragment debug dump.
    /// Configures the dump on a 2x2x2 voxel, runs a frame, and asserts
    /// the SSBO receives the expected signals (lbyte non-zero, n_idx in
    /// [0, 6), ao_mult in the Exile AO-curve band ≈ [0.75, 1.0]).
    #[test]
    fn test_debug_dump_round_trip() {
        use crate::pipeline::resolve::debug_dump;

        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.enumerate_adapters(Backends::all()))
            .into_iter()
            .find(|a| a.features().contains(prepass_features()))
        {
            Some(a) => a,
            None => {
                eprintln!(
                    "test_debug_dump_round_trip IGNORED: no wgpu adapter with \
                       SHADER_INT64 + SHADER_INT64_ATOMIC_MIN_MAX available"
                );
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_dbg_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        }))
        .expect("request_device failed");

        const W: u32 = 64;
        const H: u32 = 64;
        let mut renderer = Renderer::new(device, queue, W, H);

        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        for dx in 0..2u32 {
            for dy in 0..2u32 {
                for dz in 0..2u32 {
                    map.set_block(
                        &mut pool,
                        &mut palette,
                        VoxelCoord::new(16 + dx, 16 + dy, 16 + dz),
                        BlockId(1),
                    );
                }
            }
        }
        let registry = load_block_registry();
        let mesher = GreedyMesher::new(&registry);
        let none_nv = NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        };
        let neighbors = [none_nv; 6];
        let mesh = mesher.mesh_sector(&map, &palette, &pool, &registry, &neighbors);
        assert!(
            !mesh.opaque.is_empty(),
            "2x2x2 block must produce opaque quads"
        );

        let aspect = W as f32 / H as f32;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 100.0);
        let eye = [36.0f32, 36.0, 36.0];
        let view = look_at_rh(eye, [17.0, 17.0, 17.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, W, H);

        // The center of the screen lies on a geometry pixel for the 2x2x2
        // voxel viewed from (36,36,36). Sample with the full mask to assert
        // every signal is reachable. We configure the dump *before* the
        // geometry frame so the mask + target pixel are uploaded in the
        // same `write_buffer` call that runs the resolve pass, which is
        // the only way the GPU can see the mask at sample time.
        renderer.set_debug_dump(debug_dump::ALL, W / 2, H / 2);
        renderer.render_frame(std::slice::from_ref(&mesh), &[[0.0; 3]], &cam);
        // Sanity check: the same geometry frame must have rasterized the
        // cube at the centre of the screen. If the visbuf is empty, the
        // resolve shader never saw a non-zero entry and the dump will
        // always read back a sky pixel (quad_id=0) — making this test
        // degenerate. We also locate the rasterized pixel so we can dump
        // *exactly* a non-sky fragment rather than guessing (32, 32)
        // which might be sky.
        let visbuf = renderer.read_visbuf();
        let non_zero = visbuf.iter().filter(|&&e| e != 0).count();
        assert!(
            non_zero > 0,
            "pre-pass did not rasterize any fragments; debug dump will read back sky only. \
            visbuf non-zero count = {non_zero}"
        );
        // Find the first non-zero visbuf index — that's guaranteed to be
        // a geometry fragment.
        let target_px = visbuf
            .iter()
            .position(|&e| e != 0)
            .expect("at least one visbuf entry is non-zero (asserted above)");
        let target_x = (target_px as u32) % W;
        let target_y = (target_px as u32) / W;
        // Re-configure the dump to that pixel and re-render the frame
        // so the resolve shader writes a fresh sample.
        renderer.set_debug_dump(debug_dump::ALL, target_x, target_y);
        renderer.render_frame(std::slice::from_ref(&mesh), &[[0.0; 3]], &cam);
        let result = renderer
            .dump_debug("center")
            .expect("dump_debug returns Some when the pre-pass is available");
        // ao_smooth in [0, 3], ao_i in {0,1,2,3}, ao_mult in Exile band
        // ≈ [0.75, 1.0] (AO_CURVE_DEFAULT), quad_id non-zero (the cube has at
        // least one quad), lbyte 0 (no light baked yet by the test).
        assert!(
            result[0] >= 0.0 && result[0] <= 3.0,
            "ao_smooth={}",
            result[0]
        );
        assert!(
            result[1] >= 0.0 && result[1] <= 3.0,
            "ao_i={} (must be 0..=3 after round)",
            result[1]
        );
        let ao_floor = 191.0 / 255.0;
        assert!(
            result[2] >= ao_floor - 1e-3 && result[2] <= 1.0 + 1e-3,
            "ao_mult={} outside the [{ao_floor}, 1.0] Exile AO-curve band",
            result[2]
        );
        // ao_corners is the packed 4-corner byte (4×2 bits, 0..255).
        assert!(
            result[3] >= 0.0 && result[3] < 256.0,
            "ao_corners={}",
            result[3]
        );
        // Slot 1: quad_id is u32 bit-cast to f32 by the shader, so non-zero
        // means the cube has at least one quad. We compare via bit-pattern:
        // a non-zero u32 quantised to f32 stays non-zero in the host read.
        // A non-zero bit pattern is the actual contract — for a u32 like
        // 1, 2, ... the f32 representation has the same lower bits and the
        // host read recovers the original integer via to_bits().
        let quad_bits = result[4].to_bits();
        assert!(
            quad_bits > 0,
            "quad_id bits=0x{:x} (cube must have a non-zero quad); \
            raw f32={:?} slot1={:?}",
            quad_bits,
            result[4],
            &result[4..8]
        );
        assert!(result[5].is_finite(), "uv.x={}", result[5]);
        assert!(result[6].is_finite(), "uv.y={}", result[6]);
        assert!(result[7] >= 0.0 && result[7] < 6.0, "n_idx={}", result[7]);

        // Disable the dump to confirm mask=0 round-trips.
        renderer.set_debug_dump(0, 0, 0);
        let disabled = renderer.dump_debug("disabled");
        assert!(disabled.is_none(), "mask=0 must short-circuit dump_debug");
    }

    #[test]
    fn block_sampler_uses_nearest_mag_with_linear_mip_minification() {
        let sampler = block_sampler_descriptor();
        assert_eq!(sampler.address_mode_u, AddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, AddressMode::Repeat);
        assert_eq!(sampler.address_mode_w, AddressMode::ClampToEdge);
        assert_eq!(sampler.mag_filter, FilterMode::Nearest);
        assert_eq!(sampler.min_filter, FilterMode::Linear);
        assert_eq!(sampler.mipmap_filter, MipmapFilterMode::Linear);
        assert_eq!(sampler.anisotropy_clamp, 1);
    }

    #[test]
    fn bake_grass_side_alpha_composites_over_dirt_backdrop() {
        let mut grass = image::RgbaImage::from_fn(2, 2, |x, y| {
            if y == 0 {
                image::Rgba([0, 200, 0, 255])
            } else if x == 0 {
                image::Rgba([0, 0, 0, 128])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let dirt = image::RgbaImage::from_pixel(2, 2, image::Rgba([120, 80, 50, 255]));
        bake_texture_alpha_over_backdrop(&mut grass, &dirt);

        assert_eq!(grass.get_pixel(0, 1).0, [60, 40, 25, 255]);
        assert_eq!(grass.get_pixel(1, 1).0, [120, 80, 50, 255]);
        assert!(!rgba_image_has_partial_alpha(&grass));
    }

    /// Bug 5 regression: sector ids written to the SSBO must be the post-cull
    /// index (position in `to_render`), not the original mesh index. Otherwise
    /// the resolve shader's 4-bit sector mask sees stale indices when culling
    /// drops meshes.
    #[test]
    fn sector_id_uses_post_cull_index() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("sector_id_uses_post_cull_index IGNORED: no adapter");
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            ..Default::default()
        }))
        .expect("request_device failed");
        let mut renderer = Renderer::new(device, queue, 64, 64);

        let reg = load_block_registry();
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map0 = XBrickMap::new(SectorCoord(0, 0, 0));
        let mut map1 = XBrickMap::new(SectorCoord(1, 0, 0));
        let stone = reg.id_by_name("stone").unwrap_or(BlockId(1));
        map0.set_block(&mut pool, &mut palette, VoxelCoord::new(16, 16, 16), stone);
        map1.set_block(&mut pool, &mut palette, VoxelCoord::new(16, 16, 16), stone);
        let mesher = GreedyMesher::new(&reg);
        let none_nv = [crate::meshing::NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        }; 6];
        let mesh0 = mesher.mesh_sector(&map0, &palette, &pool, &reg, &none_nv);
        let mesh1 = mesher.mesh_sector(&map1, &palette, &pool, &reg, &none_nv);

        let aspect = 1.0;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 100.0);
        let eye = [36.0, 36.0, 36.0];
        let view = look_at_rh(eye, [16.0, 16.0, 16.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, 64, 64);

        renderer.ensure_prepass();
        if renderer.prepass.is_none() {
            return;
        }
        let meshes: [&MeshData; 2] = [&mesh0, &mesh1];
        let origins: [[f32; 3]; 2] = [[0.0; 3], [0.0; 3]];
        renderer.run_prepass(&meshes, &origins, &cam);
        let visbuf = renderer.read_visbuf();
        let written = visbuf.iter().filter(|&&e| e != 0).count();
        assert!(written > 0, "prepass must rasterize geometry");
    }

    /// Bug 6 regression: ensure_quad_capacity must preserve existing lightmap
    /// data when growing the SSBO.
    #[test]
    fn ensure_quad_capacity_preserves_lightmap_data() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("ensure_quad_capacity_preserves_lightmap_data IGNORED: no adapter");
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            ..Default::default()
        }))
        .expect("request_device failed");
        let mut renderer = Renderer::new(device.clone(), queue, 64, 64);
        renderer.ensure_prepass();
        let prepass = match renderer.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };
        let offset = 100usize;
        let data = vec![LightmapEntry(0xAB); 10];
        prepass
            .lightmap
            .write_offset(&renderer.queue, offset as u64, &data);
        renderer.ensure_quad_capacity(2048);
        let prepass = renderer.prepass.as_ref().unwrap();
        let size = prepass.lightmap.buffer().size();
        let staging = device.create_buffer(&BufferDescriptor {
            label: Some("strata_test_staging"),
            size,
            usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("strata_test_copy"),
        });
        encoder.copy_buffer_to_buffer(prepass.lightmap.buffer(), 0, &staging, 0, size);
        renderer.queue.submit(std::iter::once(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(PollType::wait_indefinitely());
        rx.recv().expect("map signal").expect("map");
        let mapped = slice.get_mapped_range();
        let bytes = bytemuck::cast_slice::<u8, LightmapEntry>(&mapped);
        for i in offset..offset + 10 {
            assert_eq!(
                bytes[i],
                LightmapEntry(0xAB),
                "lightmap data at offset {} must be preserved",
                i
            );
        }
        drop(mapped);
        staging.unmap();
    }

    #[test]
    fn test_upload_quad_region_auto_grows_capacity() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => return,
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            ..Default::default()
        })) {
            Ok(dq) => dq,
            Err(_) => return,
        };
        let mut renderer = Renderer::new(device, queue, 64, 64);
        let bytes = vec![0u8; 16]; // 2 quads
        let origins = vec![[0.0f32; 4]; 2];
        // Base = 100 with initial capacity 0 would panic without auto ensure_quad_capacity
        renderer.upload_quad_region(100, &bytes, &origins);
        assert!(
            renderer.quad_upload_staging.len() >= 102 * 8,
            "upload_quad_region must auto-grow staging to accommodate base + count"
        );
    }

    #[test]
    fn lightmap_region_end_matches_panic_offsets() {
        // Regression: Queue::write_buffer 1456936..1463624 into a 32768-byte
        // lightmap — global slot base must force grow past SECTOR_LIGHTMAP_QUADS.
        let base = 1_456_936u32;
        let len = 1_463_624usize - 1_456_936;
        let end = lightmap_region_end(base, len);
        assert_eq!(end, 1_463_624);
        assert!(
            end > SECTOR_LIGHTMAP_QUADS,
            "global slot uploads must exceed the legacy 32KB per-sector lightmap"
        );
        assert_eq!(
            lightmap_region_end(0, 1),
            4,
            "wgpu COPY_BUFFER_ALIGNMENT pad"
        );
        assert_eq!(lightmap_region_end(100, 4), 104);
    }

    #[test]
    fn upload_lightmap_region_grows_after_resize_drops_prepass() {
        // Staging stays large across resize; GPU lightmap resets to 32KB.
        // upload_lightmap_region at a high global base must re-grow, not panic.
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => return,
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        })) {
            Ok(dq) => dq,
            Err(_) => return,
        };
        let mut renderer = Renderer::new(device, queue, 64, 64);
        renderer.ensure_quad_capacity(1 << 20);
        let staging_before = renderer.quad_upload_staging.len();
        assert!(staging_before >= (1 << 20) * 8);

        // Drop pre-pass (same as window resize); staging intentionally kept.
        renderer.resize(128, 128);
        assert!(renderer.prepass.is_none());
        assert_eq!(renderer.quad_upload_staging.len(), staging_before);

        let base = 100_000u32;
        let data = vec![LightmapEntry(0x3C); 64];
        renderer.upload_lightmap_region(base, &data);

        let prepass = match renderer.prepass.as_ref() {
            Some(p) => p,
            None => return,
        };
        assert!(
            prepass.lightmap.buffer().size() as usize >= lightmap_region_end(base, data.len()),
            "lightmap must cover global slot base after resize recreate"
        );
    }

    #[test]
    fn ensure_quad_capacity_clamps_past_binding_limit() {
        // Regression: requesting 2^24 slots used to allocate origins at
        // 256 MiB and crash create_bind_group (binding 8 > 128 MiB limit).
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => return,
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        })) {
            Ok(dq) => dq,
            Err(_) => return,
        };
        let limit = u64::from(device.limits().max_storage_buffer_binding_size);
        let mut renderer = Renderer::new(device, queue, 64, 64);
        let max = renderer.max_quad_capacity();
        assert!(max * ORIGIN_BYTES_PER_QUAD <= limit as usize);
        renderer.ensure_quad_capacity((1u32 << 24).max(max as u32 + 1));
        let prepass = match renderer.prepass.as_ref() {
            Some(p) => p,
            None => return,
        };
        assert!(
            prepass.quad_capacity <= max,
            "cap {} must stay ≤ max {}",
            prepass.quad_capacity,
            max
        );
        assert!(
            prepass.origins_buffer.size() as usize <= limit as usize,
            "origins binding 8 size {} exceeds device limit {}",
            prepass.origins_buffer.size(),
            limit
        );
    }

    #[test]
    fn next_quad_capacity_is_always_power_of_two() {
        // Non-PoT needed must round up so lightmap_mask = cap-1 is a bitmask.
        let max = MAX_QUAD_SSBO_SLOTS;
        assert_eq!(next_quad_capacity(100, 0, max), 128);
        assert_eq!(next_quad_capacity(1, 0, max), 1);
        assert_eq!(next_quad_capacity(2048, 0, max), 2048);
        assert_eq!(next_quad_capacity(100, 64, max), 128);
        assert_eq!(next_quad_capacity(300, 128, max), 512); // max(300, 256) → 512
        for needed in [3usize, 7, 9, 100, 1000, 3000] {
            let cap = next_quad_capacity(needed, 0, max);
            assert!(
                cap.is_power_of_two() && cap >= needed,
                "cap={cap} needed={needed}"
            );
            let mask = cap.saturating_sub(1);
            assert_eq!(
                9999 & mask,
                9999 % cap,
                "mask must equal capacity-1 for PoT"
            );
        }
    }

    #[test]
    fn next_quad_capacity_never_exceeds_max() {
        let max = MAX_QUAD_SSBO_SLOTS;
        // Doubling past 2M (the crash path: 2^21 → 2^22 → … → 2^24 origins = 256 MiB)
        // must clamp instead of allocating past the device binding limit.
        assert_eq!(next_quad_capacity(1 << 24, 0, max), max);
        assert_eq!(next_quad_capacity(1 << 22, 1 << 21, max), max);
        assert_eq!(next_quad_capacity(100, 0, 64), 64);
        assert!(next_quad_capacity(1 << 20, 1 << 20, max) <= max);
    }

    #[test]
    fn max_quad_ssbo_capacity_keeps_origins_under_binding_limit() {
        // Default wgpu limit on many adapters (matches the crash report).
        let limit = 134_217_728u64; // 128 MiB
        let cap = max_quad_ssbo_capacity(limit);
        assert!(cap.is_power_of_two());
        assert!(cap <= MAX_QUAD_SSBO_SLOTS);
        assert!(
            cap * ORIGIN_BYTES_PER_QUAD <= limit as usize,
            "origins SSBO (binding 8) must fit: {cap}×16 > {limit}"
        );
        // Tiny limit floors to a PoT that still fits.
        let tiny = max_quad_ssbo_capacity(16 * 1024);
        assert!(tiny * ORIGIN_BYTES_PER_QUAD <= 16 * 1024);
        assert!(tiny.is_power_of_two());
    }

    #[test]
    fn prepass_wgsl_quad_id_is_instance_index() {
        use crate::pipeline::prepass::PREPASS_WGSL;
        assert!(
            PREPASS_WGSL.contains("out.quad_id = ii;"),
            "prepass must assign global SSBO slot (instance_index) as quad_id \
             so lightmap[base+i] matches visbuf.quad_id under draw_quad_ranges"
        );
        assert!(
            !PREPASS_WGSL.contains("quad_ids[min(ii"),
            "prepass must not index the (unfilled) quad_ids SSBO for lightmap id"
        );
        assert!(
            PREPASS_WGSL.contains("mix(0.35, 0.85, grazing * grazing)")
                && PREPASS_WGSL.contains("p[uaxis] = p[uaxis] + f32(du * w)"),
            "prepass must distance-expand greedy faces to close sub-pixel cracks"
        );
    }

    #[test]
    fn block_tex_mips_cover_full_chain() {
        assert_eq!(block_tex_mip_count(16), 5);
        let base = image::RgbaImage::from_pixel(16, 16, image::Rgba([10, 20, 30, 255]));
        let mips = generate_block_tex_mips(&base);
        assert_eq!(mips.len(), 5);
        assert_eq!((mips[0].width(), mips[0].height()), (16, 16));
        assert_eq!((mips[1].width(), mips[1].height()), (8, 8));
        assert_eq!((mips[2].width(), mips[2].height()), (4, 4));
        assert_eq!((mips[3].width(), mips[3].height()), (2, 2));
        assert_eq!((mips[4].width(), mips[4].height()), (1, 1));
    }

    /// Dig remeshes self + neighbor at distant SSBO bases. A min‥max bounding-box
    /// flush would copy stale staging through the gap and corrupt live geometry.
    #[test]
    fn coalesce_upload_spans_keeps_gap_between_distant_sectors() {
        // Two 10-quad sectors (80 bytes each) with a large unused gap between.
        let mut spans = vec![(0, 80), (8_000, 8_080)];
        coalesce_upload_spans(&mut spans);
        assert_eq!(
            spans,
            vec![(0, 80), (8_000, 8_080)],
            "non-adjacent uploads must stay discrete (not one 0..8080 bbox)"
        );
        // Document the buggy bounding-box that caused garbled dig remesh:
        let bbox = (
            spans.iter().map(|s| s.0).min().unwrap(),
            spans.iter().map(|s| s.1).max().unwrap(),
        );
        assert_eq!(bbox, (0, 8_080));
        assert_ne!(
            spans.len(),
            1,
            "bounding-box collapse is the corruption bug"
        );
    }

    #[test]
    fn coalesce_upload_spans_merges_adjacent_and_overlapping() {
        let mut adjacent = vec![(80, 160), (0, 80)];
        coalesce_upload_spans(&mut adjacent);
        assert_eq!(adjacent, vec![(0, 160)]);

        let mut overlap = vec![(50, 150), (0, 100)];
        coalesce_upload_spans(&mut overlap);
        assert_eq!(overlap, vec![(0, 150)]);
    }

    #[test]
    fn upload_quad_region_records_discrete_spans_not_bbox() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => return,
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_test_device"),
            required_features: prepass_features(),
            ..Default::default()
        })) {
            Ok(dq) => dq,
            Err(_) => return,
        };
        let mut renderer = Renderer::new(device, queue, 64, 64);
        let bytes_a = vec![0xAAu8; 16]; // 2 quads
        let bytes_b = vec![0xBBu8; 16];
        let origins = vec![[0.0f32; 4]; 2];
        renderer.upload_quad_region(0, &bytes_a, &origins);
        renderer.upload_quad_region(1000, &bytes_b, &origins);
        assert_eq!(
            renderer.upload_spans_quad,
            vec![(0, 16), (8000, 8016)],
            "distant sector uploads must not collapse to a single bbox span"
        );
        // Flush must clear spans without panicking (write_buffer to GPU).
        renderer.flush_quad_uploads();
        assert!(renderer.upload_spans_quad.is_empty());
        assert!(!renderer.pending_upload);
    }
}
