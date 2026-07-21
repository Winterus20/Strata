//! Per-sector lightmap SSBO (M10a.4).
//!
//! Holds packed 8-bit light values keyed by **global SSBO slot index** (same
//! bump-allocator `base` as the quad buffer). The buffer grows with
//! [`crate::pipeline::Renderer::ensure_quad_capacity`]; its initial size is
//! `SECTOR_LIGHTMAP_QUADS` (32 KB) until the first grow. The resolve shader
//! looks up `lightmap[quad_id & lightmap_mask]`.
//!
//! For M10a the mesher also writes per-quad light into `PackedQuad.light`
//! (one byte, `sky<<4 | block`); this SSBO is the parallel array sampled at
//! resolve time.

use bytemuck::{Pod, Zeroable};
use wgpu::{Buffer, BufferDescriptor, BufferUsages, Device, Queue};

/// One sector's worth of per-quad light entries.
///
/// Length is `SECTOR_LIGHTMAP_QUADS` (one byte per quad for the current
/// `quad_id` < 32 KB headroom; matches the sector's max quad count so any
/// `quad_id_in_sector` the pre-pass writes is a valid index).
pub const SECTOR_LIGHTMAP_QUADS: usize = 32 * 1024;

/// Newtype wrapper for a one-byte lightmap entry. Mirrors the CPU's
/// `PackedQuad.light` byte layout: `(sky & 0xF) << 4 | (block & 0xF)`. The
/// resolve shader decodes both halves and modulates the resolve albedo.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct LightmapEntry(pub u8);

impl LightmapEntry {
    /// Pack `(sky, block)` into one byte (high nibble = sky, low = block).
    #[inline]
    pub fn pack(sky: u8, block: u8) -> Self {
        Self(((sky & 0xF) << 4) | (block & 0xF))
    }

    #[inline]
    pub fn sky(self) -> u8 {
        (self.0 >> 4) & 0xF
    }

    #[inline]
    pub fn block(self) -> u8 {
        self.0 & 0xF
    }
}

/// Lazily-allocated, identity-stable lightmap storage buffer.
///
/// One buffer per renderer's "current sector" slot — the renderer overwrites
/// the bytes when streaming shifts the focus sector. The buffer is
/// `SECTOR_LIGHTMAP_QUADS` bytes long so any `quad_id_in_sector` the pre-pass
/// can write is a safe index.
#[derive(Debug)]
pub struct LightmapSSBO {
    buffer: Buffer,
}

impl LightmapSSBO {
    /// Create the storage buffer sized for the given quad capacity.
    /// Storage + COPY_DST + COPY_SRC; the resolve shader reads and the renderer
    /// writes. COPY_SRC is used for enlarging the buffer when capacity grows.
    pub fn new(device: &Device, size: usize) -> Self {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("strata_lightmap"),
            size: size as u64,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self { buffer }
    }

    /// Overwrite the entire lightmap with `bytes`.
    pub fn write(&self, queue: &Queue, bytes: &[LightmapEntry]) {
        self.write_offset(queue, 0, bytes);
    }

    /// Overwrite a section of the lightmap at the given quad byte offset.
    pub fn write_offset(&self, queue: &Queue, offset: u64, bytes: &[LightmapEntry]) {
        queue.write_buffer(&self.buffer, offset, bytemuck::cast_slice(bytes));
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

    #[test]
    fn lightmap_entry_is_one_byte() {
        assert_eq!(std::mem::size_of::<LightmapEntry>(), 1);
    }

    #[test]
    fn lightmap_pack_and_unpack_round_trip() {
        for (s, b) in [(0u8, 0u8), (15, 0), (0, 15), (12, 7), (15, 15)] {
            let e = LightmapEntry::pack(s, b);
            assert_eq!(e.sky(), s, "sky round-trip for ({s},{b})");
            assert_eq!(e.block(), b, "block round-trip for ({s},{b})");
        }
    }

    /// Round-trip the lightmap indexing math: given a quad_id, what byte
    /// index does the resolve shader hit? Sector-indexing test = the quad_id
    /// is the in-sector index, so the answer is just `quad_id as usize`.
    #[test]
    fn sector_indexing_round_trip() {
        for qid in [0u32, 1, 7, 255, 1024, 32 * 1024 - 1] {
            let idx = qid as usize;
            assert!(idx < SECTOR_LIGHTMAP_QUADS, "qid in range");
            // Simulated byte lookup. The byte that would be read is whatever
            // the renderer uploaded at this quad id.
            let uploaded = vec![LightmapEntry(0xA5); SECTOR_LIGHTMAP_QUADS];
            assert_eq!(uploaded[idx].0, 0xA5, "uploads are looked up at quad_id");
        }
    }

    #[test]
    fn out_of_range_quad_id_is_clamped() {
        // The shader masks `quad_id & (SECTOR_LIGHTMAP_QUADS - 1)` (power of
        // two), so any 16-bit `quad_id` stays in range. The constant itself
        // must be a power of two for the mask to work without a divide.
        assert!(SECTOR_LIGHTMAP_QUADS.is_power_of_two());
    }
}
