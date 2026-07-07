//! GPU pipeline bootstrap for Strata (M4 — wgpu bootstrap + headless offscreen clear).
//!
//! M4a built the offscreen HDR clear target. M4c adds the GPU depth pre-pass: a
//! vertex-pulling pipeline that rasterizes greedy quads into a 64-bit visibility
//! buffer (`visbuf`) via `atomicMax`, so the nearest fragment (reversed-Z) wins
//! each pixel. The pre-pass is driven by [`Renderer::render_prepass`], and the
//! result can be read back with [`Renderer::read_visbuf`].

pub mod camera;
pub mod cull;
pub mod prepass;
pub mod resolve;
pub mod visbuf;

pub use camera::CameraView;
pub use cull::{Aabb, cull_visible};
pub use visbuf::{PackedQuadGpu, VisBufEntry, meshdata_to_gpu_bytes};

use std::sync::Arc;

use wgpu::*;

use crate::meshing::MeshData;
use crate::pipeline::cull::aabb_of_mesh;
use crate::pipeline::prepass::{
    PREPASS_COLOR_FORMAT, make_quad_buffer, prepass_bind_group_layout, prepass_pipeline,
};
use crate::pipeline::resolve::{ResolveParams, resolve_bind_group_layout, resolve_pipeline};

/// `rgba16float` uses 8 bytes per texel (4 channels × half-float).
const BYTES_PER_PIXEL: u32 = 8;

/// Fullscreen-triangle blit that samples the offscreen HDR target and writes it
/// to the window surface (M9b). Linear color is written directly (no tonemap);
/// the surface format performs the final encoding conversion.
const BLIT_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

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

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, in.uv);
}
"#;

/// Device features required to build/run the depth pre-pass (`u64` types and
/// `atomicMax` on `u64`). Native-only; the pre-pass is skipped on devices that
/// lack them (M4a offscreen clear still works).
fn prepass_features() -> Features {
    Features::SHADER_INT64 | Features::SHADER_INT64_ATOMIC_MIN_MAX
}

/// Offscreen HDR renderer + GPU depth pre-pass driver (M4a + M4c).
///
/// Holds the GPU device/queue and the M4a HDR clear target. The M4c pre-pass
/// resources are created lazily on first use (see [`Renderer::ensure_prepass`])
/// and only when the device exposes the required `u64`-atomic features.
pub struct Renderer {
    device: Device,
    queue: Queue,
    width: u32,
    height: u32,
    offscreen: Texture,
    offscreen_view: TextureView,
    staging: Buffer,
    bytes_per_row: u32,
    prepass: Option<Prepass>,
    /// Lazily-built fullscreen blit used to present the offscreen HDR target to a
    /// window surface (M9b). One per surface format.
    blit: Option<Blit>,
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
    bind_group: BindGroup,
    visbuf: Buffer,
    visbuf_pixels: u32,
    visbuf_staging: Buffer,
    dummy_color: TextureView,
    resolve_pipeline: RenderPipeline,
    resolve_bgl: BindGroupLayout,
    resolve_params: Buffer,
    resolve_bind_group: BindGroup,
}

impl Renderer {
    /// Build a [`Renderer`] around an existing device/queue, sizing the
    /// offscreen target to `width`×`height`. The M4c pre-pass resources are
    /// created on first [`Renderer::render_prepass`] call.
    pub fn new(device: Device, queue: Queue, width: u32, height: u32) -> Self {
        assert!(
            width > 0 && height > 0,
            "offscreen target must be non-empty"
        );

        let offscreen = device.create_texture(&TextureDescriptor {
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
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
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

        Self {
            device,
            queue,
            width,
            height,
            offscreen,
            offscreen_view,
            staging,
            bytes_per_row,
            prepass: None,
            blit: None,
        }
    }

    /// Borrow the GPU device (used by the client to configure its surface).
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Lazily build the pre-pass GPU resources (pipeline, buffers, bind group)
    /// if the device supports the required `u64`-atomic features. No-op if the
    /// resources already exist or the features are unavailable.
    fn ensure_prepass(&mut self) {
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
        let bind_group =
            Self::make_bind_group(&self.device, &bgl, &camera_buffer, &quad_buffer, &visbuf);

        // M4d color-resolve resources.
        let resolve_bgl = resolve_bind_group_layout(&self.device);
        let resolve_pipeline = resolve_pipeline(&self.device, &resolve_bgl);
        let resolve_params = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_resolve_params"),
            size: std::mem::size_of::<ResolveParams>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let resolve_bind_group = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_resolve_bg"),
            layout: &resolve_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: resolve_params.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: visbuf.as_entire_binding(),
                },
            ],
        });

        // Upload the (constant) resolve params once.
        self.queue.write_buffer(
            &resolve_params,
            0,
            bytemuck::bytes_of(&ResolveParams::new(self.width, self.height)),
        );

        self.prepass = Some(Prepass {
            pipeline,
            bgl,
            camera_buffer,
            quad_buffer,
            quad_capacity,
            bind_group,
            visbuf,
            visbuf_pixels,
            visbuf_staging,
            dummy_color: dummy_color_view,
            resolve_pipeline,
            resolve_bgl,
            resolve_params,
            resolve_bind_group,
        });
    }

    fn make_bind_group(
        device: &Device,
        bgl: &BindGroupLayout,
        camera_buffer: &Buffer,
        quad_buffer: &Buffer,
        visbuf: &Buffer,
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
    /// Flattens the opaque quads of every `MeshData` into one SSBO, clears the
    /// visibility buffer to the smallest stored value (reversed-Z: far/no-data =
    /// 0), uploads the camera, then runs a single vertex-pulling render pass that
    /// writes the 64-bit visbuf via `atomicMax` (nearest fragment wins).
    ///
    /// `transparent` quads are intentionally ignored here (consumed later by a
    /// separate draw with blending). No-op when the device lacks the `u64`-atomic
    /// features required by the pre-pass.
    pub fn render_prepass(&mut self, meshes: &[MeshData], camera: &CameraView) {
        let refs: Vec<&MeshData> = meshes.iter().collect();
        self.run_prepass(&refs, camera);
    }

    /// Shared pre-pass body used by both [`Renderer::render_prepass`] and
    /// [`Renderer::render_frame`]: flatten the opaque quads, upload them +
    /// camera, record the vertex-pulling pass into `encoder`, and submit.
    fn run_prepass(&mut self, meshes: &[&MeshData], camera: &CameraView) {
        self.ensure_prepass();
        let prepass = match self.prepass.as_mut() {
            Some(p) => p,
            None => return,
        };

        // Flatten opaque quads across all meshes into one byte buffer.
        let mut bytes: Vec<u8> = Vec::new();
        for m in meshes {
            let (opaque, _transparent) = meshdata_to_gpu_bytes(m);
            bytes.extend_from_slice(&opaque);
        }
        let quad_count = bytes.len() / std::mem::size_of::<PackedQuadGpu>();

        // Grow the quad SSBO + bind group if the batch no longer fits.
        if quad_count > prepass.quad_capacity {
            prepass.quad_buffer = make_quad_buffer(&self.device, quad_count);
            prepass.quad_capacity = quad_count;
            prepass.bind_group = Self::make_bind_group(
                &self.device,
                &prepass.bgl,
                &prepass.camera_buffer,
                &prepass.quad_buffer,
                &prepass.visbuf,
            );
        }

        if quad_count > 0 {
            self.queue.write_buffer(&prepass.quad_buffer, 0, &bytes);
        }

        // Clear the visbuf to the smallest stored value (0) under reversed-Z, where
        // a nearer fragment carries a LARGER value. With `atomicMax` this makes the
        // nearest fragment win every pixel. `VisBufEntry::empty()` (max-depth) is the
        // CPU-side far sentinel and is intentionally NOT the GPU clear value here.
        let clear: Vec<u64> = vec![VisBufEntry(0).raw(); prepass.visbuf_pixels as usize];
        self.queue
            .write_buffer(&prepass.visbuf, 0, bytemuck::cast_slice(&clear));

        // Upload the camera uniform.
        self.queue
            .write_buffer(&prepass.camera_buffer, 0, bytemuck::bytes_of(camera));

        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_prepass"),
            });

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
    pub fn render_frame(&mut self, meshes: &[MeshData], camera: &CameraView) {
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

        // Frustum-cull whole meshes (per-sector AABBs); render visible only, but
        // fall back to all meshes if culling drops everything (defensive).
        let boxes: Vec<Aabb> = meshes.iter().map(aabb_of_mesh).collect();
        let visible = cull_visible(&boxes, camera);
        let to_render: Vec<&MeshData> = if visible.is_empty() {
            meshes.iter().collect()
        } else {
            visible.iter().map(|&i| &meshes[i]).collect()
        };

        // Flatten + upload prepass data (visbuf clear, quad SSBO, camera).
        let prepass = self.prepass.as_mut().expect("prepass ensured above");
        let mut bytes: Vec<u8> = Vec::new();
        for m in &to_render {
            let (opaque, _transparent) = meshdata_to_gpu_bytes(m);
            bytes.extend_from_slice(&opaque);
        }
        let quad_count = bytes.len() / std::mem::size_of::<PackedQuadGpu>();
        if quad_count > prepass.quad_capacity {
            prepass.quad_buffer = make_quad_buffer(&self.device, quad_count);
            prepass.quad_capacity = quad_count;
            prepass.bind_group = Self::make_bind_group(
                &self.device,
                &prepass.bgl,
                &prepass.camera_buffer,
                &prepass.quad_buffer,
                &prepass.visbuf,
            );
        }
        if quad_count > 0 {
            self.queue.write_buffer(&prepass.quad_buffer, 0, &bytes);
        }
        let clear: Vec<u64> = vec![VisBufEntry(0).raw(); prepass.visbuf_pixels as usize];
        self.queue
            .write_buffer(&prepass.visbuf, 0, bytemuck::cast_slice(&clear));
        self.queue
            .write_buffer(&prepass.camera_buffer, 0, bytemuck::bytes_of(camera));

        // Single encoder: pre-pass writes visbuf, resolve reads it (WebGPU
        // inserts the storage barrier between the two passes).
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("strata_frame"),
            });

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
        assert!(width > 0 && height > 0, "offscreen target must be non-empty");
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
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.offscreen_view = self.offscreen.create_view(&TextureViewDescriptor::default());

        self.bytes_per_row = (width * BYTES_PER_PIXEL).div_ceil(256) * 256;
        self.staging = self.device.create_buffer(&BufferDescriptor {
            label: Some("strata_offscreen_staging"),
            size: (self.bytes_per_row * height) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        self.prepass = None;
        self.blit = None;
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
            pass.set_pipeline(&blit.pipeline);
            pass.set_bind_group(0, &blit.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Build the present blit pipeline + bind group for `surface_format`. The bind
    /// group samples our own `offscreen_view`, so it must be rebuilt whenever the
    /// offscreen texture is recreated (resize).
    fn build_blit(&self, surface_format: TextureFormat) -> Blit {
        let layout = self.device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("strata_present_bgl"),
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
            ],
        });

        let pipeline_layout =
            self.device
                .create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("strata_present_pl"),
                    bind_group_layouts: &[&layout],
                    push_constant_ranges: &[],
                });

        let module = self.device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_present"),
            source: ShaderSource::Wgsl(BLIT_WGSL.into()),
        });

        let pipeline = self.device.create_render_pipeline(&RenderPipelineDescriptor {
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
            multiview: None,
            cache: None,
        });

        let sampler = self.device.create_sampler(&SamplerDescriptor {
            label: Some("strata_present_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = self.device.create_bind_group(&BindGroupDescriptor {
            label: Some("strata_present_bg"),
            layout: &layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&self.offscreen_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
            ],
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

#[cfg(test)]
mod tests {
    use super::*;

    use strata_core::prelude::*;
    use strata_core::registry::load_block_registry;

    use crate::meshing::{GreedyMesher, Mesher, NeighborView};
    use crate::pipeline::camera::{look_at_rh, perspective_rh_zo};

    #[test]
    fn test_offscreen_clear() {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });

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
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });

        // Search all adapters for one that exposes the `u64`-atomic features the
        // pre-pass requires, so the test runs on capable GPUs.
        let adapter = match instance
            .enumerate_adapters(Backends::all())
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
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });

        let adapter = match instance
            .enumerate_adapters(Backends::all())
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

        renderer.render_frame(std::slice::from_ref(&mesh), &cam);

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
}
