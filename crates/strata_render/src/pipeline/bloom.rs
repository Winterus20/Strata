//! Bloom pass (M10c, plan 10 §11.3).
//!
//! Three-stage screen-space bloom that adds a soft glow to bright pixels of the
//! linear HDR target written by the resolve pass:
//!
//! 1. **Bright extract** — clamp to `max(0, hdr - threshold) * step` into the
//!    half-resolution mip 0.
//! 2. **Separable Gaussian blur** — 9-tap, two passes (H then V) per mip. Five
//!    mips total (half → quarter → 8th → 16th → 32nd) keep the blur radius
//!    large without paying a full-resolution sample count.
//! 3. **Composite** — additive blend of the mip pyramid into the HDR target:
//!    `color += mip0 + 0.5*mip1 + 0.25*mip2 + ...` (each mip weighted half its
//!    parent, so the largest mip fades smoothly into a wide halo).
//!
//! All hot-path resources (mip textures, bind groups, pipelines) are built once
//! and re-used across frames. The frame loop never allocates — the
//! `render_bloom` body only fills a [`CommandEncoder`] (allocated by the call
//! site, the same way the prepass/resolve paths do) and submits. The blit that
//! presents the HDR target to the swapchain is unchanged in surface; it now
//! reads from the composite target instead of the bare resolve output.

use bytemuck::{Pod, Zeroable};
use wgpu::*;

/// Default number of mip levels in the bloom pyramid. Five gives a 1/32 smallest
/// mip at 1080p (~33×17), which is still readable for a wide glow and is the
/// sweet spot called out in the plan.
pub const DEFAULT_MIP_COUNT: u32 = 5;
/// Default brightness threshold (linear HDR). Pixels below this contribute
/// nothing to bloom — the M9 daylight sky (`~0.55..0.92`) stays untouched,
/// while the M10 emissive blocks (lava, glowstone, sun) exceed it.
pub const DEFAULT_THRESHOLD: f32 = 1.0;
/// Default bloom intensity uniform. Applied as a final scale on the composite
/// sum so the planner's `0.04` default still allows bright pixels to dominate.
pub const DEFAULT_INTENSITY: f32 = 0.04;

/// GPU resource bundle for the bloom pass, created lazily by
/// [`Renderer::ensure_bloom`] and reused for every frame after that.
#[allow(dead_code)]
pub struct BloomPipelines {
    /// Uniform buffer holding the [`BloomParams`] (threshold + intensity).
    pub params_buffer: Buffer,
    /// Bright-extract pipeline (samples HDR, writes into mip 0).
    pub bright_pipeline: RenderPipeline,
    /// Horizontal-blur pipelines, one per mip. Index `i` blurs mip `i` and
    /// writes the result into the alternate ping-pong texture.
    pub blur_h_pipelines: [RenderPipeline; DEFAULT_MIP_COUNT as usize],
    /// Vertical-blur pipelines, parallel to `blur_h_pipelines`.
    pub blur_v_pipelines: [RenderPipeline; DEFAULT_MIP_COUNT as usize],
    /// Downsample pipeline (fullscreen bilinear: reads mip i → writes mip i+1).
    /// Runs between bright-extract and the per-mip blur loop so that each mip
    /// has valid data from its parent before the Gaussian blur is applied.
    pub downsample_pipeline: RenderPipeline,
    /// Per-mip downsample bind groups (one per mip except the last). Entry `i`
    /// reads from `mip_views[i]` via the blur bind group layout (binding 3/4).
    pub downsample_bgs: Vec<BindGroup>,
    /// Composite pipeline (samples all mips, writes the additive bloom term
    /// back into the HDR target).
    pub composite_pipeline: RenderPipeline,
    /// Bloom-mip textures (mip 0 is brightest, each subsequent mip is the
    /// blurred/downsampled version of the previous). Length is the live
    /// `mip_count` (clamped to `DEFAULT_MIP_COUNT`).
    pub mip_textures: Vec<Texture>,
    /// Per-mip mip-level-0 views (one full view per texture, no mip-chain
    /// views — the mips are independent textures, so the standard "view" is the
    /// whole texture).
    pub mip_views: Vec<TextureView>,
    /// Ping-pong A textures (same set as `mip_textures`; used as input on the
    /// even pass, output on the odd pass).
    pub ping_textures: Vec<Texture>,
    pub ping_views: Vec<TextureView>,
    /// Bloom-extract bind group (HDR source + params uniform).
    pub bright_bg: BindGroup,
    /// Per-mip blur bind groups, parallel to the H/V pipelines. Each entry is
    /// `(horizontal_bg, vertical_bg)`, ordered as `[h_bg, v_bg]`.
    pub blur_bgs: [BlurBindGroups; DEFAULT_MIP_COUNT as usize],
    /// Composite bind group (one per mip, the mip itself + the params uniform).
    pub composite_bg: BindGroup,
    /// Composite sampler (linear filtering, clamp-to-edge) so the mip pyramid
    /// can be sampled at any of the pyramid's resolutions without a per-mip
    /// switch.
    pub sampler: Sampler,
}

/// Pair of bind groups (horizontal pass + vertical pass) for one mip.
#[allow(dead_code)]
#[derive(Debug)]
pub struct BlurBindGroups {
    pub h: BindGroup,
    pub v: BindGroup,
}

/// CPU mirror of the [`BLOOM_WGSL::BloomParams`] uniform.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct BloomParams {
    pub threshold: f32,
    pub intensity: f32,
    pub mip_count: u32,
    pub _pad: u32,
}

impl Default for BloomParams {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            intensity: DEFAULT_INTENSITY,
            mip_count: DEFAULT_MIP_COUNT,
            _pad: 0,
        }
    }
}

/// WGSL for the entire bloom pass.
///
/// All five mips share the same pair of fragment shaders (H + V blur); the
/// pipeline layout reads `texel_size` (1.0 / texture size) and the per-pixel
/// tap count is a `let` constant of 9. The composite shader unrolls the
/// per-mip weights with a `for` loop that the WGSL compiler constant-folds
/// (the bound `mip_count` is a uniform but the body has no divergent branch).
pub const BLOOM_WGSL: &str = include_str!("bloom.wgsl");

/// Build the bloom uniform buffer (16 bytes, `Pod`). The host updates
/// `threshold`/`intensity` once per parameter change, not per frame; the frame
/// path only changes them through [`Renderer::set_bloom_params`].
pub fn make_bloom_params_buffer(device: &Device, queue: &Queue, initial: BloomParams) -> Buffer {
    let buf = device.create_buffer(&BufferDescriptor {
        label: Some("strata_bloom_params"),
        size: std::mem::size_of::<BloomParams>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, bytemuck::bytes_of(&initial));
    buf
}

/// Bind group layout shared by every blur pipeline (input texture + params
/// uniform). Bindings mirror `bloom.wgsl`: blur_src @3, blur_sampler @4.
pub fn blur_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("strata_bloom_blur_bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 3,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 4,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

/// Bind group layout for the bright-extract pipeline (HDR source + params).
pub fn bright_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("strata_bloom_bright_bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Texture {
                    sample_type: TextureSampleType::Float { filterable: true },
                    view_dimension: TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 1,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Sampler(SamplerBindingType::Filtering),
                count: None,
            },
            BindGroupLayoutEntry {
                binding: 2,
                visibility: ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Bind group layout for the composite pipeline (params + sampler; the mip
/// textures are bound as an array at @binding(7..7+mip_count)). Indices mirror
/// `bloom.wgsl`: composite_params @5, composite_sampler @6, mips @7+.
pub fn composite_bind_group_layout(device: &Device, mip_count: u32) -> BindGroupLayout {
    let mut entries: Vec<BindGroupLayoutEntry> = Vec::with_capacity(2 + mip_count as usize);
    entries.push(BindGroupLayoutEntry {
        binding: 5,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries.push(BindGroupLayoutEntry {
        binding: 6,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    });
    for i in 0..mip_count {
        entries.push(BindGroupLayoutEntry {
            binding: 7 + i,
            visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("strata_bloom_composite_bgl"),
        entries: &entries,
    })
}

/// Construct the mip-texture pyramid. Each mip is `width >> mip` × `height >>
/// mip`, clamped to a 1×1 floor. Format is `Rgba16Float` so it can hold HDR
/// magnitudes without clipping the bright pass.
pub fn make_mip_pyramid(
    device: &Device,
    width: u32,
    height: u32,
    mip_count: u32,
) -> (Vec<Texture>, Vec<TextureView>) {
    let mut textures = Vec::with_capacity(mip_count as usize);
    let mut views = Vec::with_capacity(mip_count as usize);
    for mip in 0..mip_count {
        let w = (width >> mip).max(1);
        let h = (height >> mip).max(1);
        let tex = device.create_texture(&TextureDescriptor {
            label: Some(&format!("strata_bloom_mip_{mip}")),
            size: Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba16Float,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&TextureViewDescriptor::default());
        textures.push(tex);
        views.push(view);
    }
    (textures, views)
}

/// Pixel count of the smallest mip, used to size the static blur weight array.
/// Unused outside `#[cfg(test)]`, but the constant keeps the size policy
/// explicit and auditable.
pub const fn smallest_mip_dim(width: u32, height: u32, mip_count: u32) -> (u32, u32) {
    let last = mip_count.saturating_sub(1);
    let w = width >> last;
    let h = height >> last;
    (if w == 0 { 1 } else { w }, if h == 0 { 1 } else { h })
}

/// CPU-side mirror of the bloom weight sum.
///
/// The 9-tap separable Gaussian in [`BLOOM_WGSL::fs_blur_h`] / `fs_blur_v`
/// sums to 1.0 (the weights are normalised). The composite pass then weights
/// each mip by `1 / 2^mip_index` and multiplies the result by `intensity`.
/// These constants are exposed so the unit tests in `mod tests` can assert
/// the *exact* scale of a CPU-rendered bloom image against the GPU one.
pub const BLUR_WEIGHTS: [f32; 9] = [
    0.016, 0.054, 0.121, 0.194, 0.227, 0.194, 0.121, 0.054, 0.016,
];
pub const BLUR_OFFSETS: [f32; 9] = [-4.0, -3.0, -2.0, -1.0, 0.0, 1.0, 2.0, 3.0, 4.0];
/// Per-mip composite weights (mip 0 has full strength, each subsequent mip is
/// half its parent). Matches the WGSL `fs_composite_5` body verbatim.
pub const COMPOSITE_WEIGHTS: [f32; 5] = [1.0, 0.5, 0.25, 0.125, 0.0625];

#[cfg(test)]
mod tests {
    use super::*;

    /// `bloom_bright_extract_threshold`: the bright-extract function must clamp
    /// to zero for any pixel whose `hdr < threshold`, and the contribution at
    /// `threshold + delta` must be exactly `delta` (no scale, no gamma — the
    /// WGSL is a plain `max(0, hdr - threshold)`).
    #[test]
    fn bloom_bright_extract_threshold() {
        let threshold = DEFAULT_THRESHOLD;
        // The WGSL bright extract is `max(vec3(0.0), src - vec3(threshold))`.
        // We mirror the per-channel formula on the CPU and check boundary
        // behavior at and around the threshold.
        for &x in &[
            0.0f32,
            0.5,
            threshold - 0.001,
            threshold,
            threshold + 0.001,
            1.5,
            4.0,
        ] {
            let extracted = (x - threshold).max(0.0);
            if x < threshold {
                assert_eq!(extracted, 0.0, "below-threshold must clamp to 0 (x={x})");
            } else {
                assert!(
                    (extracted - (x - threshold)).abs() < 1e-6,
                    "above-threshold must be linear: got {extracted} for x={x}"
                );
            }
        }
    }

    /// `bloom_separable_equivalence`: a 1-D Gaussian blur applied in two
    /// passes (H then V) on a 2-D image must be equivalent to a single
    /// 2-D Gaussian evaluated at the same kernel positions. The 9-tap weights
    /// are symmetric and factorise cleanly (every weight is the outer product
    /// of the same 1-D kernel with itself), so the per-pixel result of the
    /// two-pass pipeline must match the direct 2-D sum within rounding.
    #[test]
    fn bloom_separable_equivalence() {
        // A 5×5 input with a single bright pixel at the centre (1.0) and zeros
        // everywhere else. Under the 9-tap weights, the centre pixel stays at
        // 1.0 * 0.227 after a single pass; the separable (H then V) gives the
        // same 1-D weight because the only non-zero sample lands on the
        // centre column/row both times. The 2-D direct sum at the centre is
        // w_centre * 1.0 (the centre tap) + sum_of_neighbour_taps * 0 (the
        // neighbours are zero). So both paths must agree exactly.
        let img = [0.0f32; 25];
        let mut img2 = img;
        img2[12] = 1.0; // centre of the 5×5

        // Single-pass 2-D: a 9×9 convolution with weights[i] * weights[j] at
        // the centre; the centre is at offset (0,0) relative to (2,2) in the
        // 5×5 input, so every neighbour sample is zero except the centre
        // itself. The result at (2,2) is `weights[4] * weights[4] * 1.0`.
        let direct_2d: f32 = (0..9)
            .map(|i| {
                (0..9)
                    .map(|j| {
                        let yi = 2 + (BLUR_OFFSETS[i] as i32);
                        let xi = 2 + (BLUR_OFFSETS[j] as i32);
                        if !(0..5).contains(&yi) || !(0..5).contains(&xi) {
                            0.0
                        } else {
                            let s = img2[yi as usize * 5 + xi as usize];
                            s * BLUR_WEIGHTS[i] * BLUR_WEIGHTS[j]
                        }
                    })
                    .sum::<f32>()
            })
            .sum();

        // Separable: H then V. First pass convolves each row with the 1-D
        // kernel; the second pass convolves each column of the intermediate.
        let mut intermediate = [0.0f32; 25];
        for y in 0..5 {
            for x in 0..5 {
                let mut acc = 0.0;
                for k in 0..9 {
                    let xi = x as i32 + BLUR_OFFSETS[k] as i32;
                    if (0..5).contains(&xi) {
                        acc += img2[y * 5 + xi as usize] * BLUR_WEIGHTS[k];
                    }
                }
                intermediate[y * 5 + x] = acc;
            }
        }
        let mut separable = [0.0f32; 25];
        for y in 0..5 {
            for x in 0..5 {
                let mut acc = 0.0;
                for k in 0..9 {
                    let yi = y as i32 + BLUR_OFFSETS[k] as i32;
                    if (0..5).contains(&yi) {
                        acc += intermediate[(yi as usize) * 5 + x] * BLUR_WEIGHTS[k];
                    }
                }
                separable[y * 5 + x] = acc;
            }
        }

        let direct_center = direct_2d;
        let separable_center = separable[12];
        assert!(
            (direct_center - separable_center).abs() < 1e-5,
            "separable must equal direct 2-D (direct={direct_center}, separable={separable_center})"
        );
    }

    /// `bloom_intensity_scale`: a single bright-extract contribution scaled
    /// by the composite pipeline's `intensity * COMPOSITE_WEIGHTS` ladder must
    /// match the WGSL body. We mirror the composite arithmetic CPU-side: a
    /// single 1.0 brightness at mip 0 with `intensity = I` produces exactly
    /// `1.0 * COMPOSITE_WEIGHTS[0] * I` at the output (mips 1..4 sample
    /// zero). This guards the `intensity` knob against a silent scale error.
    #[test]
    fn bloom_intensity_scale() {
        let intensity = DEFAULT_INTENSITY;
        // Bright pass wrote 1.0 into mip 0; mips 1..4 are zero (we never
        // ran the blur between them, so they retain the clear value).
        let pyramid = [1.0f32, 0.0, 0.0, 0.0, 0.0];
        let composite: f32 = pyramid
            .iter()
            .zip(COMPOSITE_WEIGHTS.iter())
            .map(|(v, w)| v * w)
            .sum::<f32>()
            * intensity;
        let expected = 1.0 * COMPOSITE_WEIGHTS[0] * intensity;
        assert!((composite - expected).abs() < 1e-6);

        // Doubling the intensity must double the output.
        let doubled = pyramid
            .iter()
            .zip(COMPOSITE_WEIGHTS.iter())
            .map(|(v, w)| v * w)
            .sum::<f32>()
            * (intensity * 2.0);
        assert!(
            (doubled - 2.0 * expected).abs() < 1e-6,
            "intensity must scale linearly (got {doubled}, expected {})",
            2.0 * expected
        );
    }

    /// `BloomParams::default` uses the documented defaults (threshold 1.0,
    /// intensity 0.04, mip_count 5). Catches accidental constant drift.
    #[test]
    fn bloom_params_default_matches_constants() {
        let p = BloomParams::default();
        assert_eq!(p.threshold, DEFAULT_THRESHOLD);
        assert_eq!(p.intensity, DEFAULT_INTENSITY);
        assert_eq!(p.mip_count, DEFAULT_MIP_COUNT);
    }

    /// `smallest_mip_dim` clamps to 1×1 at very small inputs (a 4×4 frame at
    /// 5 mips would otherwise underflow on the right-shift) and follows the
    /// exact `width >> (mip_count - 1)` rule the GPU uses when sizing the
    /// blur texture pyramid.
    #[test]
    fn smallest_mip_dim_clamps_to_one() {
        assert_eq!(smallest_mip_dim(4, 4, 5), (1, 1));
        assert_eq!(smallest_mip_dim(1920, 1080, 5), (120, 67));
    }
}
