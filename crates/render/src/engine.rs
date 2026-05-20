//! Wgpu render engine — wgpu 27 API
use std::sync::Arc;
use winit::window::Window;

/// Intermediate output from the main render pass, before overlay and submission.
pub struct RenderOutput {
    pub view: wgpu::TextureView,
    pub encoder: wgpu::CommandEncoder,
    pub frame: wgpu::SurfaceTexture,
}
use crate::block_outline::BlockOutline;
use crate::camera::Camera;
use crate::chunk_renderer::ChunkRenderer;
use crate::crosshair::Crosshair;
use crate::frustum::Frustum;
use crate::pipeline::RenderPipelineManager;
use crate::texture_manager::TextureManager;
use strata_core::BlockRegistry;

/// GPU uniform data: view-proj matrix + fog color/density + camera position.
/// Must match `FrameUniforms` in chunk_textured.wgsl.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShaderUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub fog_color_density: [f32; 4],
    pub camera_pos: [f32; 4],
}

pub struct RenderEngine {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub pipeline_manager: RenderPipelineManager,
    pub texture_manager: TextureManager,
    pub chunk_renderer: ChunkRenderer,
    pub camera: Camera,
    pub frustum: Frustum,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    pub depth_texture: wgpu::TextureView,
    pub crosshair: Crosshair,
    pub block_outline: BlockOutline,
    pub selected_block: Option<glam::IVec3>,
    /// Fog density (0 = no fog). Set from main loop based on camera position.
    pub fog_density: f32,
    /// Fog colour (RGB).
    pub fog_color: glam::Vec3,
}

impl RenderEngine {
    pub fn create_depth_texture(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
    ) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub async fn new(window: Arc<Window>, registry: &BlockRegistry) -> anyhow::Result<Self> {
        // wgpu 27: Instance::new takes &InstanceDescriptor
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            ..Default::default()
        });
        let size = window.inner_size();
        let surface = instance.create_surface(window)?;

        // wgpu 27: request_adapter returns Result<Adapter, RequestAdapterError>
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await?;

        // wgpu 27: request_device takes only descriptor (no trace_path arg)
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                trace: wgpu::Trace::default(),
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];

        // Prefer Mailbox (low-latency, no tearing) over Fifo (VSync stutter)
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::AutoNoVsync) {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::Fifo
        };
        tracing::info!("Selected present mode: {:?}", present_mode);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let depth_texture = Self::create_depth_texture(&device, &config);
        let pipeline_manager = RenderPipelineManager::new(&device, format);
        let texture_manager = TextureManager::new(&device, &queue, registry).await;

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &pipeline_manager.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let aspect = size.width as f32 / size.height as f32;
        let camera = Camera::new(aspect);
        let frustum = Frustum::from_view_projection(camera.view_projection_matrix());
        let chunk_renderer = ChunkRenderer::new();
        let crosshair = Crosshair::new(&device, &queue, format);
        let block_outline = BlockOutline::new(&device, &queue, format);

        Ok(Self {
            surface,
            device,
            queue,
            config,
            pipeline_manager,
            texture_manager,
            chunk_renderer,
            camera,
            frustum,
            uniform_buffer,
            uniform_bind_group,
            depth_texture,
            crosshair,
            block_outline,
            selected_block: None,
            fog_density: 0.0,
            fog_color: glam::Vec3::new(0.1, 0.4, 0.7),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_texture = Self::create_depth_texture(&self.device, &self.config);
        self.camera.aspect = width as f32 / height as f32;
    }

    pub fn update_camera(&mut self) {
        self.frustum = Frustum::from_view_projection(self.camera.view_projection_matrix());
        let uniforms = ShaderUniforms {
            view_proj: self.camera.view_projection_matrix().to_cols_array_2d(),
            fog_color_density: [
                self.fog_color.x,
                self.fog_color.y,
                self.fog_color.z,
                self.fog_density,
            ],
            camera_pos: [
                self.camera.position.x,
                self.camera.position.y,
                self.camera.position.z,
                0.0,
            ],
        };
        self.queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );
    }

    /// Run the main chunk render pass and return the output for further overlay rendering.
    /// Caller is responsible for submitting the encoder and presenting the frame.
    pub fn render_frame(&mut self) -> Option<RenderOutput> {
        // wgpu 27: get_current_texture returns Result<SurfaceTexture, SurfaceError>
        // SurfaceTexture is just a struct with .texture field
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost) => {
                self.surface.configure(&self.device, &self.config);
                return None;
            }
            Err(wgpu::SurfaceError::Outdated) => return None,
            Err(e) => {
                tracing::warn!("Surface error: {:?}", e);
                return None;
            }
        };

        // wgpu 24: TextureViewDescriptor (no usage field)
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Surface Texture View"),
            format: None,
            dimension: Some(wgpu::TextureViewDimension::D2),
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: 0,
            array_layer_count: None,
            usage: None,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        {
            // wgpu 27: RenderPassColorAttachment has depth_slice: Option<u32>
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.2,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.pipeline_manager.chunk_pipeline);
            render_pass.set_bind_group(0, Some(&self.uniform_bind_group), &[]);
            render_pass.set_bind_group(1, Some(&self.texture_manager.bind_group), &[]);
            self.chunk_renderer.render(&mut render_pass);

            // Block outline overlay (depth-tested, black line list)
            if let Some(block_pos) = self.selected_block {
                let vp = self.camera.view_projection_matrix();
                self.block_outline.render(
                    &self.queue,
                    &mut render_pass,
                    vp,
                    block_pos,
                );
            }
        }

        // Crosshair overlay (no depth, alpha blended)
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Crosshair Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.crosshair.render(&mut pass);
        }

        Some(RenderOutput {
            view,
            encoder,
            frame,
        })
    }
}
