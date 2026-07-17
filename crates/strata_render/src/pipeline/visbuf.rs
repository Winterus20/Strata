//! CPU-side 64-bit visibility-buffer value type (M4b, plan 10 §1).
//!
//! This is the host representation of the unified visibility buffer entry (Aokana
//! Figure 7 layout). No WGSL / GPU upload happens here yet; M4c builds the storage
//! buffer from these values. All packing is branchless bit arithmetic so the same
//! shifts can be mirrored verbatim in the shader.
//!
//! Bit layout (all 64 bits used, no reserved gap — plan 10 §1):
//! * `bit[0:23]`   voxel_pos  (24 bits, 0 .. 2^24-1)
//! * `bit[24:36]`  sector_id  (13 bits, 0 .. 2^13-1)
//! * `bit[37:39]`  normal     (3 bits, 0 .. 7)
//! * `bit[40:63]`  depth      (24 bits, reversed-Z; clear = max)

use bytemuck::{Pod, Zeroable};

use crate::meshing::{MeshData, PackedQuad};

/// `voxel_pos` occupies the low 24 bits.
pub const VOXEL_POS_MASK: u64 = (1u64 << 24) - 1;
/// `sector_id` occupies 13 bits starting at bit 24 (plan 10 §1: 2^13 sectors).
pub const SECTOR_ID_MASK: u64 = (1u64 << 13) - 1;
/// `normal` occupies 3 bits starting at bit 37.
pub const NORMAL_MASK: u64 = 0x7;
/// `depth` occupies the high 24 bits starting at bit 40 (reversed-Z).
pub const DEPTH_MASK: u64 = (1u64 << 24) - 1;

const SECTOR_ID_SHIFT: u32 = 24;
const NORMAL_SHIFT: u32 = 37;
const DEPTH_SHIFT: u32 = 40;

/// A single visibility-buffer entry stored as a raw `u64`.
///
/// Wrap the raw value so accessors stay branchless and field boundaries are
/// impossible to confuse. `0` is *not* a valid clear value under reversed-Z:
/// use [`VisBufEntry::empty`] for the far/cleared value.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VisBufEntry(pub u64);

impl VisBufEntry {
    /// Pack fields into a single 64-bit entry. Inputs are masked to their field
    /// width, so over-wide values simply truncate without branching.
    #[inline]
    pub fn pack(voxel_pos: u32, sector_id: u32, normal: u8, depth: u32) -> Self {
        let v = (voxel_pos as u64 & VOXEL_POS_MASK)
            | ((sector_id as u64 & SECTOR_ID_MASK) << SECTOR_ID_SHIFT)
            | ((normal as u64 & NORMAL_MASK) << NORMAL_SHIFT)
            | ((depth as u64 & DEPTH_MASK) << DEPTH_SHIFT);
        Self(v)
    }

    /// Cleared/far entry under reversed-Z: depth = max, everything else zero.
    #[inline]
    pub fn empty() -> Self {
        Self(DEPTH_MASK << DEPTH_SHIFT)
    }

    #[inline]
    pub fn raw(&self) -> u64 {
        self.0
    }

    #[inline]
    pub fn voxel_pos(&self) -> u32 {
        (self.0 & VOXEL_POS_MASK) as u32
    }

    #[inline]
    pub fn sector_id(&self) -> u32 {
        ((self.0 >> SECTOR_ID_SHIFT) & SECTOR_ID_MASK) as u32
    }

    #[inline]
    pub fn normal(&self) -> u8 {
        ((self.0 >> NORMAL_SHIFT) & NORMAL_MASK) as u8
    }

    #[inline]
    pub fn depth(&self) -> u32 {
        ((self.0 >> DEPTH_SHIFT) & DEPTH_MASK) as u32
    }
}

/// GPU-ready mirror of [`PackedQuad`]: exactly 8 bytes, `repr(C)`, `Pod`.
///
/// The next part uploads these verbatim into a storage buffer that the
/// vertex-pulling shader reads. Layout is identical to [`PackedQuad::data`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct PackedQuadGpu {
    pub data: [u8; 8],
}

impl PackedQuadGpu {
    /// Copy a CPU [`PackedQuad`] into its GPU-ready form.
    #[inline]
    pub fn from_packed_quad(q: &PackedQuad) -> Self {
        Self { data: q.data }
    }
}

/// Flatten the opaque quads of `mesh` into a single byte buffer of
/// [`PackedQuadGpu`] entries (the transparent batch is returned separately so
/// the consumer can bind two distinct storage buffers / draws).
///
/// Pure CPU work: no wgpu calls. Each quad contributes exactly
/// `size_of::<PackedQuadGpu>()` (8) bytes via `bytemuck` cast.
#[inline]
pub fn meshdata_to_gpu_bytes(mesh: &MeshData) -> (Vec<u8>, Vec<u8>) {
    let opaque = pack_batch(&mesh.opaque);
    let transparent = pack_batch(&mesh.transparent);
    (opaque, transparent)
}

#[inline]
fn pack_batch(batch: &[PackedQuad]) -> Vec<u8> {
    let gpu: Vec<PackedQuadGpu> = batch.iter().map(PackedQuadGpu::from_packed_quad).collect();
    bytemuck::cast_slice(&gpu).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_VOXEL_POS: u32 = (1u32 << 24) - 1;
    const MAX_SECTOR_ID: u32 = (1u32 << 13) - 1;
    const MAX_NORMAL: u8 = 0x7;
    const MAX_DEPTH: u32 = (1u32 << 24) - 1;

    #[test]
    fn test_visbuf_round_trip_max_values() {
        let e = VisBufEntry::pack(MAX_VOXEL_POS, MAX_SECTOR_ID, MAX_NORMAL, MAX_DEPTH);
        assert_eq!(e.voxel_pos(), MAX_VOXEL_POS);
        assert_eq!(e.sector_id(), MAX_SECTOR_ID);
        assert_eq!(e.normal(), MAX_NORMAL);
        assert_eq!(e.depth(), MAX_DEPTH);
        // sector_id is now 13 bits (24..=36) and normal starts at 37, so the
        // packed max entry uses all 64 bits with no reserved gap.
    }

    #[test]
    fn test_visbuf_round_trip_zero() {
        let e = VisBufEntry::pack(0, 0, 0, 0);
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.sector_id(), 0);
        assert_eq!(e.normal(), 0);
        assert_eq!(e.depth(), 0);
        assert_eq!(e.raw(), 0);
    }

    #[test]
    fn test_visbuf_round_trip_scattered() {
        let cases: [(u32, u32, u8, u32); 6] = [
            (0xAB_CDEF, 0x05A, 0x3, 0x12_3456),
            (1, 1, 1, 1),
            (MAX_VOXEL_POS, 0, 0, 0),
            (0, MAX_SECTOR_ID, 0, 0),
            (0, 0, MAX_NORMAL, 0),
            (0, 0, 0, MAX_DEPTH),
        ];
        for (vp, sid, n, d) in cases {
            let e = VisBufEntry::pack(vp, sid, n, d);
            assert_eq!(e.voxel_pos(), vp, "voxel_pos round-trip");
            assert_eq!(e.sector_id(), sid, "sector_id round-trip");
            assert_eq!(e.normal(), n, "normal round-trip");
            assert_eq!(e.depth(), d, "depth round-trip");
        }
    }

    #[test]
    fn test_visbuf_field_boundaries_no_bleed() {
        // Each field set to its max independently; only the corresponding bits
        // should be set. Shifting/ORing them together must not overlap.
        let vp = VisBufEntry::pack(MAX_VOXEL_POS, 0, 0, 0).raw();
        let sid = VisBufEntry::pack(0, MAX_SECTOR_ID, 0, 0).raw();
        let n = VisBufEntry::pack(0, 0, MAX_NORMAL, 0).raw();
        let d = VisBufEntry::pack(0, 0, 0, MAX_DEPTH).raw();

        // No two single-field values should overlap when ANDed.
        assert_eq!(vp & sid, 0, "voxel_pos/sector_id overlap");
        assert_eq!(vp & n, 0, "voxel_pos/normal overlap");
        assert_eq!(vp & d, 0, "voxel_pos/depth overlap");
        assert_eq!(sid & n, 0, "sector_id/normal overlap");
        assert_eq!(sid & d, 0, "sector_id/depth overlap");
        assert_eq!(n & d, 0, "normal/depth overlap");

        // Combined OR equals the fully-packed max entry.
        let combined = vp | sid | n | d;
        let full = VisBufEntry::pack(MAX_VOXEL_POS, MAX_SECTOR_ID, MAX_NORMAL, MAX_DEPTH).raw();
        assert_eq!(combined, full, "combined fields differ from packed max");
    }

    #[test]
    fn test_visbuf_empty_is_max_depth() {
        let e = VisBufEntry::empty();
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.sector_id(), 0);
        assert_eq!(e.normal(), 0);
        assert_eq!(
            e.depth(),
            MAX_DEPTH,
            "clear depth must be max for reversed-Z"
        );
        assert_eq!(e.raw(), DEPTH_MASK << DEPTH_SHIFT);
    }

    #[test]
    fn test_visbuf_overflow_masked() {
        // Over-wide inputs must truncate to their field width without bleeding.
        let e = VisBufEntry::pack(u32::MAX, u32::MAX, 0xFF, u32::MAX);
        assert_eq!(e.voxel_pos(), MAX_VOXEL_POS);
        assert_eq!(e.sector_id(), MAX_SECTOR_ID);
        assert_eq!(e.normal(), MAX_NORMAL);
        assert_eq!(e.depth(), MAX_DEPTH);
    }

    #[test]
    fn test_packed_quad_gpu_layout() {
        assert_eq!(std::mem::size_of::<PackedQuadGpu>(), 8);
        assert_eq!(
            std::mem::size_of::<PackedQuadGpu>(),
            std::mem::size_of::<PackedQuad>()
        );
        let q = PackedQuad::new(1, 2, 3, 4, 5, 0, 7, 0, 0, 0);
        let g = PackedQuadGpu::from_packed_quad(&q);
        assert_eq!(g.data, q.data);
    }

    #[test]
    fn test_meshdata_to_gpu_bytes() {
        let mut mesh = MeshData::default();
        let q1 = PackedQuad::new(0, 0, 0, 1, 1, 0, 1, 0, 0, 0);
        let q2 = PackedQuad::new(5, 5, 5, 2, 2, 1, 2, 0, 0, 0);
        let qt = PackedQuad::new(1, 1, 1, 1, 1, 2, 3, 0, 0, 0);
        mesh.opaque.push(q1);
        mesh.opaque.push(q2);
        mesh.transparent.push(qt);

        let (opaque, transparent) = meshdata_to_gpu_bytes(&mesh);
        assert_eq!(opaque.len(), 2 * 8);
        assert_eq!(transparent.len(), 8);

        let cast = bytemuck::cast_slice::<u8, PackedQuadGpu>(&opaque);
        assert_eq!(cast[0].data, q1.data);
        assert_eq!(cast[1].data, q2.data);
        let cast_t = bytemuck::cast_slice::<u8, PackedQuadGpu>(&transparent);
        assert_eq!(cast_t[0].data, qt.data);
    }
}
