//! Simple 2D crosshair overlay rendered as two rectangles in a + shape.
//! Uses indexed triangle list for correct rendering.

const CROSSHAIR_VERTICES: [f32; 16] = [
    // Horizontal bar: TL, TR, BL, BR
    -0.018, -0.002, // 0
     0.018, -0.002, // 1
    -0.018,  0.002, // 2
     0.018,  0.002, // 3
    // Vertical bar: TL, TR, BL, BR
    -0.002, -0.018, // 4
     0.002, -0.018, // 5
    -0.002,  0.018, // 6
     0.002,  0.018, // 7
];

const CROSSHAIR_INDICES: [u16; 12] = [
    0, 1, 2, 1, 3, 2, // horizontal rect
    4, 5, 6, 5, 7, 6, // vertical rect
];

const CROSSHAIR_SHADER: &str = r"
struct VsOut {
    @builtin(position) position: vec4f,
};

struct VertexInput {
    @location(0) pos: vec2f,
};

@vertex
fn vs_main(in: VertexInput) -> VsOut {
    var out: VsOut;
    out.position = vec4f(in.pos, 0.0, 1.0);
    return out;
}

@fragment
fn fs_main() -> @location(0) vec4f {
    return vec4f(1.0, 1.0, 1.0, 0.9);
}
";

pub struct Crosshair {
    pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

impl Crosshair {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Crosshair Shader"),
            source: wgpu::ShaderSource::Wgsl(CROSSHAIR_SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Crosshair Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Crosshair Vertex Buffer"),
            size: std::mem::size_of_val(&CROSSHAIR_VERTICES) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(&CROSSHAIR_VERTICES));

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Crosshair Index Buffer"),
            size: std::mem::size_of_val(&CROSSHAIR_INDICES) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&CROSSHAIR_INDICES));

        let vertex_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: 8, // 2 f32
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            }],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Crosshair Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[vertex_buffer_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            vertex_buffer,
            index_buffer,
            index_count: CROSSHAIR_INDICES.len() as u32,
        }
    }

    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}
