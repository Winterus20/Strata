//! Screen HUD overlay: Minecraft-style selection wireframe + center crosshair.
//!
//! Drawn onto the swapchain after the HDR present blit (`LoadOp::Load`). The
//! selection box is a `LineList` in world space; when the frame visbuf is
//! bound, each fragment is discarded if it lies behind scene geometry (packed
//! 17-bit reversed-Z, same encoding as the prepass). Crosshair is two NDC
//! quads, always on top (no depth test).

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use wgpu::*;

use crate::pipeline::camera::CameraView;

const LINE_WGSL: &str = r#"
struct CameraView {
    eye: vec4<f32>,
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    width: u32,
    height: u32,
    _pad: vec2<u32>,
}

struct HudParams {
    // 1 = compare against visbuf reversed-Z depth; 0 = always draw.
    depth_test: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> camera: CameraView;
@group(0) @binding(1) var<uniform> hud: HudParams;
@group(0) @binding(2) var<storage, read> visbuf: array<u64>;

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.clip_pos = camera.proj * camera.view * vec4<f32>(position, 1.0);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    if (hud.depth_test != 0u) {
        // Match prepass pixel index + rev-Z depth (larger = nearer).
        let max_pix = camera.width * camera.height - 1u;
        let pix = min(
            u32(floor(in.clip_pos.y) * f32(camera.width) + floor(in.clip_pos.x)),
            max_pix,
        );
        let entry = visbuf[pix];
        let scene_rev = u32((entry >> u32(47)) & u64(0x1FFFFu));
        // Sky / empty pixels keep depth 0 — always draw the outline there.
        if (scene_rev > 0u) {
            let line_rev = u32(clamp(1.0 - in.clip_pos.z, 0.0, 1.0) * 131071.0);
            // Bias so the slightly inflated outline still wins on the hit face
            // despite 17-bit quantization and sub-pixel rasterization.
            let bias = 384u;
            if (line_rev + bias < scene_rev) {
                discard;
            }
        }
    }
    // Near-black selection outline (Minecraft wireframe feel).
    return vec4<f32>(0.02, 0.02, 0.02, 1.0);
}
"#;

const CROSSHAIR_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec4<f32>,
}

struct Vert {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs(v: Vert) -> VsOut {
    var out: VsOut;
    out.clip_pos = vec4<f32>(v.pos, 0.0, 1.0);
    out.color = v.color;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

/// Max vertices for the selection `LineList` (12 edges × 2).
pub const SELECTION_LINE_VERTS: u64 = 24;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineVertex {
    position: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HudParamsGpu {
    depth_test: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CrosshairVertex {
    pos: [f32; 2],
    color: [f32; 4],
}

/// Lazily-built HUD GPU state (one per surface format).
pub struct HudOverlay {
    pub format: TextureFormat,
    line_pipeline: RenderPipeline,
    line_bgl: BindGroupLayout,
    line_bind_group: BindGroup,
    camera_buffer: Buffer,
    hud_params_buffer: Buffer,
    /// Tiny 1-element storage buffer used when no frame visbuf is bound.
    dummy_visbuf: Buffer,
    /// Tracks whether the current bind group points at a real visbuf.
    bound_depth_test: bool,
    line_vertex_buffer: Buffer,
    crosshair_pipeline: RenderPipeline,
    crosshair_vertex_buffer: Buffer,
    crosshair_vertex_count: u32,
}

impl HudOverlay {
    pub fn build(device: &Device, format: TextureFormat, width: u32, height: u32) -> Self {
        let camera_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_hud_camera"),
            size: std::mem::size_of::<CameraView>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let hud_params_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_hud_params"),
            size: std::mem::size_of::<HudParamsGpu>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dummy_visbuf = device.create_buffer(&BufferDescriptor {
            label: Some("strata_hud_dummy_visbuf"),
            size: std::mem::size_of::<u64>() as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let line_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("strata_hud_line_bgl"),
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
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
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
            ],
        });

        let line_pl = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("strata_hud_line_pl"),
            bind_group_layouts: &[Some(&line_bgl)],
            immediate_size: 0,
        });

        let line_module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_hud_line"),
            source: ShaderSource::Wgsl(LINE_WGSL.into()),
        });

        let line_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("strata_hud_line_pipeline"),
            layout: Some(&line_pl),
            vertex: VertexState {
                module: &line_module,
                entry_point: Some("vs"),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: &vertex_attr_array![0 => Float32x3],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &line_module,
                entry_point: Some("fs"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let line_bind_group = make_line_bind_group(
            device,
            &line_bgl,
            &camera_buffer,
            &hud_params_buffer,
            &dummy_visbuf,
        );

        let line_vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_hud_line_vb"),
            size: SELECTION_LINE_VERTS * std::mem::size_of::<LineVertex>() as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cross_module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_hud_crosshair"),
            source: ShaderSource::Wgsl(CROSSHAIR_WGSL.into()),
        });

        let cross_pl = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("strata_hud_crosshair_pl"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });

        let crosshair_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("strata_hud_crosshair_pipeline"),
            layout: Some(&cross_pl),
            vertex: VertexState {
                module: &cross_module,
                entry_point: Some("vs"),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<CrosshairVertex>() as u64,
                    step_mode: VertexStepMode::Vertex,
                    attributes: &vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                }],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &cross_module,
                entry_point: Some("fs"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
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

        let cross_verts = crosshair_verts(width, height);
        let crosshair_vertex_count = cross_verts.len() as u32;
        let crosshair_vertex_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("strata_hud_crosshair_vb"),
                contents: bytemuck::cast_slice(&cross_verts),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            });

        Self {
            format,
            line_pipeline,
            line_bgl,
            line_bind_group,
            camera_buffer,
            hud_params_buffer,
            dummy_visbuf,
            bound_depth_test: false,
            line_vertex_buffer,
            crosshair_pipeline,
            crosshair_vertex_buffer,
            crosshair_vertex_count,
        }
    }

    /// Rebuild crosshair NDC quads after a window resize.
    pub fn resize_crosshair(&mut self, queue: &Queue, width: u32, height: u32) {
        let verts = crosshair_verts(width, height);
        self.crosshair_vertex_count = verts.len() as u32;
        queue.write_buffer(
            &self.crosshair_vertex_buffer,
            0,
            bytemuck::cast_slice(&verts),
        );
    }

    fn bind_visbuf(&mut self, device: &Device, visbuf: Option<&Buffer>) {
        let want_depth = visbuf.is_some();
        if !want_depth {
            if !self.bound_depth_test {
                return;
            }
            self.line_bind_group = make_line_bind_group(
                device,
                &self.line_bgl,
                &self.camera_buffer,
                &self.hud_params_buffer,
                &self.dummy_visbuf,
            );
            self.bound_depth_test = false;
            return;
        }
        // Rebind every frame: visbuf buffer identity changes on resize.
        self.line_bind_group = make_line_bind_group(
            device,
            &self.line_bgl,
            &self.camera_buffer,
            &self.hud_params_buffer,
            visbuf.expect("checked above"),
        );
        self.bound_depth_test = true;
    }
}

fn make_line_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    camera: &Buffer,
    hud_params: &Buffer,
    visbuf: &Buffer,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("strata_hud_line_bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 1,
                resource: hud_params.as_entire_binding(),
            },
            BindGroupEntry {
                binding: 2,
                resource: visbuf.as_entire_binding(),
            },
        ],
    })
}

fn crosshair_verts(width: u32, height: u32) -> Vec<CrosshairVertex> {
    let w = width.max(1) as f32;
    let h = height.max(1) as f32;
    // ~10px arm, ~2px thickness — scales with resolution via NDC.
    let arm = 10.0;
    let thick = 2.0;
    let dx = arm / w;
    let dy = arm / h;
    let tw = (thick * 0.5) / w;
    let th = (thick * 0.5) / h;
    let color = [0.92, 0.92, 0.92, 0.95];

    let mut verts = Vec::with_capacity(12);
    // Horizontal bar.
    push_quad(&mut verts, -dx, -th, dx, th, color);
    // Vertical bar.
    push_quad(&mut verts, -tw, -dy, tw, dy, color);
    verts
}

fn push_quad(out: &mut Vec<CrosshairVertex>, x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]) {
    let a = CrosshairVertex {
        pos: [x0, y0],
        color,
    };
    let b = CrosshairVertex {
        pos: [x1, y0],
        color,
    };
    let c = CrosshairVertex {
        pos: [x1, y1],
        color,
    };
    let d = CrosshairVertex {
        pos: [x0, y1],
        color,
    };
    out.extend_from_slice(&[a, b, c, a, c, d]);
}

/// Upload camera + optional outline lines, then draw outline + crosshair onto
/// the surface view (`LoadOp::Load`).
///
/// When `visbuf` is `Some`, line fragments are occluded by scene geometry using
/// the packed reversed-Z depth (pre-pass encoding). Otherwise lines draw on top
/// (caller should prefer front-facing / hit-face edges in that case).
pub fn draw_hud(
    device: &Device,
    queue: &Queue,
    overlay: &mut HudOverlay,
    surface_view: &TextureView,
    camera: &CameraView,
    outline_lines: Option<&[[f32; 3]]>,
    visbuf: Option<&Buffer>,
) {
    overlay.bind_visbuf(device, visbuf);

    queue.write_buffer(&overlay.camera_buffer, 0, bytemuck::bytes_of(camera));
    let params = HudParamsGpu {
        depth_test: u32::from(visbuf.is_some()),
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    queue.write_buffer(&overlay.hud_params_buffer, 0, bytemuck::bytes_of(&params));

    let line_count = if let Some(lines) = outline_lines {
        let n = lines.len().min(SELECTION_LINE_VERTS as usize);
        let mut verts = Vec::with_capacity(n);
        for p in lines.iter().take(n) {
            verts.push(LineVertex { position: *p });
        }
        queue.write_buffer(&overlay.line_vertex_buffer, 0, bytemuck::cast_slice(&verts));
        n as u32
    } else {
        0
    };

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("strata_hud"),
    });
    {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("strata_hud_pass"),
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

        if line_count > 0 {
            pass.set_pipeline(&overlay.line_pipeline);
            pass.set_bind_group(0, &overlay.line_bind_group, &[]);
            pass.set_vertex_buffer(0, overlay.line_vertex_buffer.slice(..));
            pass.draw(0..line_count, 0..1);
        }

        pass.set_pipeline(&overlay.crosshair_pipeline);
        pass.set_vertex_buffer(0, overlay.crosshair_vertex_buffer.slice(..));
        pass.draw(0..overlay.crosshair_vertex_count, 0..1);
    }
    queue.submit(std::iter::once(encoder.finish()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_line_shader_module_creates() {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("hud_line_shader_module_creates IGNORED: no adapter");
                return;
            }
        };
        if !adapter.features().contains(Features::SHADER_INT64) {
            eprintln!("hud_line_shader_module_creates IGNORED: no SHADER_INT64");
            return;
        }
        let (device, _queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("strata_hud_line_shader_test"),
            required_features: Features::SHADER_INT64,
            ..Default::default()
        }))
        .expect("request_device failed");

        // Validation crash (u64 & u32) surfaces here — must not panic.
        let _module = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("strata_hud_line"),
            source: ShaderSource::Wgsl(LINE_WGSL.into()),
        });
    }
}
