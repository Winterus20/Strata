//! Texture2DArray manager — wgpu 27 API
//!
//! Loads block textures from `assets/textures/` directory as a Texture2DArray.
//! Falls back to colored placeholders when files are missing.

use std::collections::HashMap;
use strata_core::BlockRegistry;
use wgpu::{BindGroup, BindGroupLayout, Device, Queue, Sampler, Texture, TextureView};

pub struct TextureManager {
    pub texture: Texture,
    pub texture_view: TextureView,
    pub sampler: Sampler,
    pub bind_group: BindGroup,
    pub bind_group_layout: BindGroupLayout,
    pub texture_count: u32,
    pub texture_size: u32,
    /// Maps block_id → texture array layer index.
    pub block_to_layer: HashMap<u16, u32>,
}

impl TextureManager {
    pub fn create_bind_group_layout(device: &Device) -> BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Array Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }

    /// Load block textures from `assets/textures/` directory.
    /// Falls back to colored placeholders for missing files.
    pub async fn new(device: &Device, queue: &Queue, _registry: &BlockRegistry) -> Self {
        let texture_size: u32 = 16;
        let tex_dir = std::path::Path::new("assets/textures");

        // Block ID → texture filename mapping with fallback color.
        // Layer order MUST match get_texture_id() in classic_greedy.rs
        const TEXTURE_ENTRIES: &[(u16, &str, [u8; 3])] = &[
            (1,  "stone.png",       [120, 120, 120]), // layer 0 — STONE
            (2,  "dirt.png",        [134, 96, 67]),   // layer 1 — DIRT
            (3,  "grass_top.png",   [100, 160, 70]),  // layer 2 — GRASS top
            (4,  "bedrock.png",     [50, 50, 55]),    // layer 3 — BEDROCK
            (5,  "wood.png",        [160, 120, 70]),  // layer 4 — WOOD
            (6,  "leaves.png",      [60, 140, 50]),   // layer 5 — LEAVES
            (7,  "sand.png",        [210, 195, 140]), // layer 6 — SAND
            (9,  "water.png",       [30, 80, 170]),   // layer 7 — WATER
            (3,  "grass_side.png",  [130, 110, 70]),  // layer 8 — GRASS sides
            (8,  "gravel.png",      [140, 135, 125]), // layer 9 — GRAVEL
            (10, "snow.png",        [235, 238, 245]), // layer 10 — SNOW
        ];

        // Load or generate each texture
        let mut layers: Vec<Vec<u8>> = Vec::new();
        let mut block_to_layer: HashMap<u16, u32> = HashMap::new();

        for (block_id, filename, fallback) in TEXTURE_ENTRIES {
            let filepath = tex_dir.join(filename);
            let pixels: Vec<u8> = if filepath.exists() {
                match image::open(&filepath) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        let (w, h) = rgba.dimensions();
                        // Scale to 16x16 if needed
                        if w == texture_size && h == texture_size {
                            rgba.into_raw()
                        } else {
                            let scaled = image::imageops::resize(
                                &rgba,
                                texture_size,
                                texture_size,
                                image::imageops::Nearest,
                            );
                            scaled.into_raw()
                        }
                    }
                    Err(_) => {
                        generate_placeholder(texture_size, fallback)
                    }
                }
            } else {
                generate_placeholder(texture_size, fallback)
            };

            block_to_layer.insert(*block_id, layers.len() as u32);
            layers.push(pixels);
        }

        let layer_count = layers.len() as u32;

        // Interleave all layers into a single flat RGBA buffer
        let mut all_data =
            Vec::with_capacity((texture_size * texture_size * 4 * layer_count) as usize);
        for layer in &layers {
            all_data.extend_from_slice(layer);
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Block Texture Array"),
            size: wgpu::Extent3d {
                width: texture_size,
                height: texture_size,
                depth_or_array_layers: layer_count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let bytes_per_row = texture_size * 4;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &all_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(texture_size),
            },
            wgpu::Extent3d {
                width: texture_size,
                height: texture_size,
                depth_or_array_layers: layer_count,
            },
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Block Texture Array View"),
            format: None,
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            aspect: wgpu::TextureAspect::All,
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: 0,
            array_layer_count: Some(layer_count),
            usage: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Block Texture Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group_layout = Self::create_bind_group_layout(device);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Block Texture Array Bind Group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            texture,
            texture_view,
            sampler,
            bind_group,
            bind_group_layout,
            texture_count: layer_count,
            texture_size,
            block_to_layer,
        }
    }
}

fn generate_placeholder(size: u32, color: &[u8; 3]) -> Vec<u8> {
    let count = (size * size) as usize;
    let mut data = Vec::with_capacity(count * 4);
    for _ in 0..count {
        data.push(color[0]);
        data.push(color[1]);
        data.push(color[2]);
        data.push(255);
    }
    data
}
