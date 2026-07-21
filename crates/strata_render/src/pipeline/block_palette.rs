//! Block-palette SSBO (M10a.2).
//!
//! Uploads the [`BlockRegistry::base_color`] table (currently 16 blocks in the
//! prototype, 48 bytes of `vec3<f32>`) as a single read-only storage buffer
//! bound at `@group(0) @binding(2]` of the resolve shader. The shader indexes
//! the array with the 8-bit `block_id` decoded from the visbuf and multiplies
//! the per-face tint by the result.
//!
//! The buffer is sized for the full registered block count plus 1 slot for the
//! implicit AIR entry the registry always exports; out-of-range `block_id`
//! lookups in the shader are masked to zero so the read stays in-bounds and
//! AIR (vec3(0)) is returned as the safe fallback.
//!
//! Pure CPU; no wgpu calls outside the explicit [`BlockPalette::upload`] helper
//! so the binding is a one-liner at the call site.

use bytemuck::{Pod, Zeroable};
use std::collections::HashMap;
use strata_core::registry::{BlockId, BlockRegistry};
use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

/// Linearised `vec3<f32>` color + textures per registered block (`count + 1` entries,
/// index 0 = AIR). The resolve shader reads `block_colors[block_id]`.
/// Size: 48 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct BlockColorGpu {
    pub rgb: [f32; 3],
    pub _pad: u32,
    pub textures: [u32; 6],
    pub use_quad_uv: u32,
    pub _pad2: u32,
}

/// Build a GPU-ready color + texture mapping table from the registry.
pub fn build_block_colors(
    registry: &BlockRegistry,
    texture_mapping: &HashMap<String, u32>,
) -> Vec<BlockColorGpu> {
    let n = registry.count().max(1);
    let cap = n.next_power_of_two().max(2);
    let mut out = vec![BlockColorGpu::default(); cap];
    for (i, c) in registry.base_color.iter().enumerate() {
        out[i].rgb = [
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
        ];
        let id = registry.id[i];
        let mut layers = [0u32; 6];
        if id != BlockId::AIR {
            for face in 0..6 {
                let name = &registry.textures(id)[face];
                layers[face] = *texture_mapping.get(name).unwrap_or(&0);
            }
        }
        out[i].textures = layers;
        out[i].use_quad_uv = if registry.use_quad_uv(id) { 1 } else { 0 };
    }
    out
}

/// Lazily-allocated, identity-stable block-palette storage buffer.
#[derive(Debug)]
pub struct BlockPalette {
    buffer: Buffer,
    /// Number of valid `BlockColorGpu` entries (always a power of two, >= 2).
    /// The resolve shader uses `count - 1` as the bit-mask for `block_id`.
    capacity: u32,
}

impl BlockPalette {
    /// Build the storage buffer for `registry` and immediately upload it to
    /// `queue`. The buffer is sized for a power-of-two `count` so the shader
    /// can mask `block_id & (capacity - 1)` for a safe lookup.
    pub fn upload(
        device: &Device,
        queue: &Queue,
        registry: &BlockRegistry,
        texture_mapping: &HashMap<String, u32>,
    ) -> Self {
        let colors = build_block_colors(registry, texture_mapping);
        let capacity = colors.len() as u32;
        let size = (capacity as u64) * std::mem::size_of::<BlockColorGpu>() as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_block_palette"),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&colors));
        Self { buffer, capacity }
    }

    /// Build a minimum-size palette (one AIR slot, all-zero color) for use as
    /// a pre-pass placeholder before the real registry is installed.
    pub fn empty(device: &Device, queue: &Queue) -> Self {
        let capacity = 2u32;
        let size = (capacity as u64) * std::mem::size_of::<BlockColorGpu>() as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_block_palette_empty"),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("strata_block_palette_empty_init"),
        });
        encoder.clear_buffer(&buffer, 0, None);
        queue.submit(std::iter::once(encoder.finish()));
        Self { buffer, capacity }
    }

    /// Power-of-two length of the underlying storage buffer; the shader masks
    /// `block_id` with `capacity - 1` so the read never escapes the array.
    #[inline]
    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    /// Borrow the underlying wgpu buffer (used by the resolve bind group).
    #[inline]
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::registry::load_block_registry;

    #[test]
    fn build_block_colors_covers_registry_ids() {
        let reg = load_block_registry();
        let mapping = HashMap::new();
        let colors = build_block_colors(&reg, &mapping);
        // Length is a power of two and >= count() so any in-range block_id
        // (0..count()) is a valid index.
        assert!(colors.len() >= reg.count());
        assert!(colors.len().is_power_of_two());
        for (i, expected) in reg.base_color.iter().enumerate() {
            assert_eq!(
                colors[i].rgb,
                [
                    expected[0] as f32 / 255.0,
                    expected[1] as f32 / 255.0,
                    expected[2] as f32 / 255.0,
                ],
                "block {i} color round-trip"
            );
        }
        // AIR (id 0) is the registry's first entry; the SoA invariant in
        // `load_block_registry` guarantees this.
        assert_eq!(reg.id[0], strata_core::registry::BlockId::AIR);
    }

    #[test]
    fn build_block_colors_pads_to_power_of_two() {
        let mapping = HashMap::new();
        let colors = build_block_colors(&load_block_registry(), &mapping);
        // 16 blocks in the prototype => next power of two is 16 (no pad).
        assert_eq!(colors.len(), 16);
    }

    #[test]
    fn empty_registry_still_has_two_slots() {
        let reg = BlockRegistry::default();
        let mapping = HashMap::new();
        let colors = build_block_colors(&reg, &mapping);
        assert!(colors.len() >= 2);
        assert!(colors.len().is_power_of_two());
        for c in &colors {
            assert_eq!(c.rgb, [0.0; 3]);
        }
    }

    #[test]
    fn block_palette_byte_layout_is_pod() {
        assert_eq!(std::mem::size_of::<BlockColorGpu>(), 48);
        let mut c = BlockColorGpu::default();
        c.rgb = [0.25, 0.5, 0.75];
        c.textures = [1, 2, 3, 4, 5, 6];
        c.use_quad_uv = 1;
        let bytes = bytemuck::bytes_of(&c);
        assert_eq!(bytes.len(), 48);
        let back: BlockColorGpu = *bytemuck::from_bytes(bytes);
        assert_eq!(back, c);
    }

    #[test]
    fn empty_palette_buffer_is_zeroed() {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(_) => {
                    eprintln!("empty_palette_buffer_is_zeroed IGNORED: no adapter");
                    return;
                }
            };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("strata_test_device"),
            ..Default::default()
        }))
        .expect("request_device failed");
        let palette = BlockPalette::empty(&device, &queue);
        let size = (palette.capacity() as u64) * std::mem::size_of::<BlockColorGpu>() as u64;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strata_test_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("strata_test_copy"),
        });
        encoder.copy_buffer_to_buffer(palette.buffer(), 0, &staging, 0, size);
        queue.submit(std::iter::once(encoder.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().expect("map signal").expect("map");
        let mapped = slice.get_mapped_range();
        let bytes = bytemuck::cast_slice::<u8, BlockColorGpu>(&mapped);
        assert!(
            bytes.iter().all(|b| *b == BlockColorGpu::default()),
            "empty palette buffer must be zeroed"
        );
        drop(mapped);
        staging.unmap();
    }
}
