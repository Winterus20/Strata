//! CPU-side 64-bit visibility-buffer value type (M4b + M12, plan 10 §1).
//!
//! This is the host representation of the unified visibility buffer entry (Aokana
//! Figure 7 layout). No WGSL / GPU upload happens here yet; M4c builds the storage
//! buffer from these values. All packing is branchless bit arithmetic so the same
//! shifts can be mirrored verbatim in the shader.
//!
//! Bit layout (M12 visbuf v4 — 13-bit reversed-Z depth for high precision):
//! * `bit[0:16]`   voxel_pos    (16 bits, sector-local 5+5+5+1 reserved; 0..65535)
//! * `bit[16:20]`  block_id     (4 bits, copied from `PackedQuad::data[4]`, 0..15)
//! * `bit[20:24]`  sector_id    (4 bits; owning sector index in per-frame AOI, 0..15)
//! * `bit[24:32]`  ao_corners   (8 bits, 4 corners x 2 bits — c0[0:2] c1[2:4] c2[4:6] c3[6:8])
//!   0 = most occluded, 3 = fully open. The fragment shader
//!   does bi-linear interpolation across the quad via the UV
//!   coords reconstructed in `get_quad_space_uv`.
//! * `bit[32:48]`  quad_id      (16 bits, global quad index in SSBO; supports up to 65K quads)
//! * `bit[48:51]`  normal       (3 bits, 0..7)
//! * `bit[51:64]`  depth        (13 bits, reversed-Z; clear = 0)
//!   (13-bit depth = 8192 levels, giving ~0.03 m precision at 250 m for high reversed-Z depth precision)
//!
//! The 4-bit `sector_id` lets the resolve shader disambiguate per-sector pixels
//! when a single frame's visbuf receives quads from multiple sectors (AOI
//! overlap). It is decoded for verification only; per-sector palette work
//! comes in a later milestone. The current `BlockRegistry` tops out at 16 blocks,
//! which fits the 4-bit `block_id`; a 17th block will require either a layout
//! bump or a palette mask fallback.

use bytemuck::{Pod, Zeroable};

use crate::meshing::{MeshData, PackedQuad};

/// `voxel_pos` occupies the low 16 bits.
pub const VOXEL_POS_MASK: u64 = (1u64 << 16) - 1;
/// `block_id` occupies 4 bits starting at bit 16.
pub const BLOCK_ID_MASK: u64 = (1u64 << 4) - 1;
/// `sector_id` occupies 4 bits starting at bit 20.
pub const SECTOR_ID_MASK: u64 = (1u64 << 4) - 1;
/// `ao_corners` occupies 8 bits starting at bit 24 (4 corners x 2 bits).
pub const AO_CORNERS_MASK: u64 = (1u64 << 8) - 1;
/// `quad_id` occupies 16 bits starting at bit 32.
pub const QUAD_ID_MASK: u64 = (1u64 << 16) - 1;
/// `normal` occupies 3 bits starting at bit 48.
pub const NORMAL_MASK: u64 = 0x7;
/// `depth` occupies 13 bits starting at bit 51 (reversed-Z depth precision).
pub const DEPTH_MASK: u64 = (1u64 << 13) - 1;

const BLOCK_ID_SHIFT: u32 = 16;
const SECTOR_ID_SHIFT: u32 = 20;
const AO_CORNERS_SHIFT: u32 = 24;
const QUAD_ID_SHIFT: u32 = 32;
const NORMAL_SHIFT: u32 = 48;
const DEPTH_SHIFT: u32 = 51;

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
    ///
    /// `ao_corners` is the 4-corner AO byte (c0|c1|c2|c3 packed as 2 bits each,
    /// same layout as `PackedQuad::data[5]`). The fragment shader does bi-linear
    /// interpolation across the quad using the UV coords it reconstructs; this
    /// gives the Exile smooth-shading result with no per-vertex work.
    #[inline]
    pub fn pack(
        voxel_pos: u32,
        block_id: u32,
        sector_id: u32,
        ao_corners: u8,
        quad_id: u32,
        normal: u8,
        depth: u32,
    ) -> Self {
        let v = (voxel_pos as u64 & VOXEL_POS_MASK)
            | ((block_id as u64 & BLOCK_ID_MASK) << BLOCK_ID_SHIFT)
            | ((sector_id as u64 & SECTOR_ID_MASK) << SECTOR_ID_SHIFT)
            | ((ao_corners as u64 & AO_CORNERS_MASK) << AO_CORNERS_SHIFT)
            | ((quad_id as u64 & QUAD_ID_MASK) << QUAD_ID_SHIFT)
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
    pub fn block_id(&self) -> u32 {
        ((self.0 >> BLOCK_ID_SHIFT) & BLOCK_ID_MASK) as u32
    }

    #[inline]
    pub fn sector_id(&self) -> u32 {
        ((self.0 >> SECTOR_ID_SHIFT) & SECTOR_ID_MASK) as u32
    }

    /// 4 corner AO values (0=occluded .. 3=open), c0..c3.
    #[inline]
    pub fn ao_corners(&self) -> [u8; 4] {
        let a = ((self.0 >> AO_CORNERS_SHIFT) & AO_CORNERS_MASK) as u8;
        [a & 0x3, (a >> 2) & 0x3, (a >> 4) & 0x3, (a >> 6) & 0x3]
    }

    /// Max of the 4 corner AO values (0..=3). Kept for compatibility with
    /// fall-back paths and tests; the resolve shader uses `ao_corners()`
    /// directly for bi-linear interpolation.
    #[inline]
    pub fn ao_max(&self) -> u8 {
        let [c0, c1, c2, c3] = self.ao_corners();
        c0.max(c1).max(c2).max(c3)
    }

    #[inline]
    pub fn quad_id(&self) -> u32 {
        ((self.0 >> QUAD_ID_SHIFT) & QUAD_ID_MASK) as u32
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

    const MAX_VOXEL_POS: u32 = (1u32 << 16) - 1;
    const MAX_BLOCK_ID: u32 = (1u32 << 4) - 1;
    const MAX_SECTOR_ID: u32 = (1u32 << 4) - 1;
    const MAX_AO_CORNERS: u8 = 0xFF;
    const MAX_QUAD_ID: u32 = (1u32 << 16) - 1;
    const MAX_NORMAL: u8 = 0x7;
    const MAX_DEPTH: u32 = (1u32 << 13) - 1;

    fn pack_max_all() -> VisBufEntry {
        VisBufEntry::pack(
            MAX_VOXEL_POS,
            MAX_BLOCK_ID,
            MAX_SECTOR_ID,
            MAX_AO_CORNERS,
            MAX_QUAD_ID,
            MAX_NORMAL,
            MAX_DEPTH,
        )
    }

    #[test]
    fn test_visbuf_round_trip_max_values() {
        let e = pack_max_all();
        assert_eq!(e.voxel_pos(), MAX_VOXEL_POS);
        assert_eq!(e.block_id(), MAX_BLOCK_ID);
        assert_eq!(e.sector_id(), MAX_SECTOR_ID);
        // 4 corner AO values; each is max (3), so ao_corners() returns [3,3,3,3].
        let ao = e.ao_corners();
        assert_eq!(ao, [3, 3, 3, 3]);
        assert_eq!(e.ao_max(), 3);
        assert_eq!(e.quad_id(), MAX_QUAD_ID);
        assert_eq!(e.normal(), MAX_NORMAL);
        assert_eq!(e.depth(), MAX_DEPTH);
        // All fields used, no reserved gap.
    }

    #[test]
    fn test_visbuf_round_trip_zero() {
        let e = VisBufEntry::pack(0, 0, 0, 0, 0, 0, 0);
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.block_id(), 0);
        assert_eq!(e.sector_id(), 0);
        assert_eq!(e.ao_corners(), [0, 0, 0, 0]);
        assert_eq!(e.ao_max(), 0);
        assert_eq!(e.quad_id(), 0);
        assert_eq!(e.normal(), 0);
        assert_eq!(e.depth(), 0);
        assert_eq!(e.raw(), 0);
    }

    #[test]
    fn test_visbuf_round_trip_scattered() {
        // Each tuple: (voxel_pos, block_id, sector_id, ao_corners byte, quad_id, normal, depth)
        let cases: [(u32, u32, u32, u8, u32, u8, u32); 8] = [
            (0xABCD, 0x5, 0xA, 0xFF, 0x1234, 0x3, 0x1FFF),
            (1, 1, 1, 0x5A, 1, 1, 1),
            (MAX_VOXEL_POS, 0, 0, 0, 0, 0, 0),
            (0, MAX_BLOCK_ID, 0, 0, 0, 0, 0),
            (0, 0, MAX_SECTOR_ID, 0, 0, 0, 0),
            (0, 0, 0, MAX_AO_CORNERS, 0, 0, 0),
            (0, 0, 0, 0, MAX_QUAD_ID, 0, 0),
            (0, 0, 0, 0, 0, MAX_NORMAL, 0),
        ];
        for (vp, bid, sid, ao, qid, n, d) in cases {
            let e = VisBufEntry::pack(vp, bid, sid, ao, qid, n, d);
            assert_eq!(e.voxel_pos(), vp, "voxel_pos round-trip");
            assert_eq!(e.block_id(), bid, "block_id round-trip");
            assert_eq!(e.sector_id(), sid, "sector_id round-trip");
            // Round-trip the AO corners: extract each 2-bit field and compare.
            let ao_byte = e.ao_corners();
            let recovered =
                (ao_byte[0]) | (ao_byte[1] << 2) | (ao_byte[2] << 4) | (ao_byte[3] << 6);
            assert_eq!(recovered, ao, "ao_corners round-trip");
            assert_eq!(e.quad_id(), qid, "quad_id round-trip");
            assert_eq!(e.normal(), n, "normal round-trip");
            assert_eq!(e.depth(), d, "depth round-trip");
        }
    }

    #[test]
    fn test_visbuf_field_boundaries_no_bleed() {
        // Each field set to its max independently; only the corresponding bits
        // should be set. Shifting/ORing them together must not overlap.
        let vp = VisBufEntry::pack(MAX_VOXEL_POS, 0, 0, 0, 0, 0, 0).raw();
        let bid = VisBufEntry::pack(0, MAX_BLOCK_ID, 0, 0, 0, 0, 0).raw();
        let sid = VisBufEntry::pack(0, 0, MAX_SECTOR_ID, 0, 0, 0, 0).raw();
        let ao = VisBufEntry::pack(0, 0, 0, MAX_AO_CORNERS, 0, 0, 0).raw();
        let qid = VisBufEntry::pack(0, 0, 0, 0, MAX_QUAD_ID, 0, 0).raw();
        let n = VisBufEntry::pack(0, 0, 0, 0, 0, MAX_NORMAL, 0).raw();
        let d = VisBufEntry::pack(0, 0, 0, 0, 0, 0, MAX_DEPTH).raw();

        // No two single-field values should overlap when ANDed.
        assert_eq!(vp & bid, 0, "voxel_pos/block_id overlap");
        assert_eq!(vp & sid, 0, "voxel_pos/sector_id overlap");
        assert_eq!(vp & ao, 0, "voxel_pos/ao_corners overlap");
        assert_eq!(vp & qid, 0, "voxel_pos/quad_id overlap");
        assert_eq!(vp & n, 0, "voxel_pos/normal overlap");
        assert_eq!(vp & d, 0, "voxel_pos/depth overlap");
        assert_eq!(bid & sid, 0, "block_id/sector_id overlap");
        assert_eq!(bid & ao, 0, "block_id/ao_corners overlap");
        assert_eq!(bid & qid, 0, "block_id/quad_id overlap");
        assert_eq!(bid & n, 0, "block_id/normal overlap");
        assert_eq!(bid & d, 0, "block_id/depth overlap");
        assert_eq!(sid & ao, 0, "sector_id/ao_corners overlap");
        assert_eq!(sid & qid, 0, "sector_id/quad_id overlap");
        assert_eq!(sid & n, 0, "sector_id/normal overlap");
        assert_eq!(sid & d, 0, "sector_id/depth overlap");
        assert_eq!(ao & qid, 0, "ao_corners/quad_id overlap");
        assert_eq!(ao & n, 0, "ao_corners/normal overlap");
        assert_eq!(ao & d, 0, "ao_corners/depth overlap");
        assert_eq!(qid & n, 0, "quad_id/normal overlap");
        assert_eq!(qid & d, 0, "quad_id/depth overlap");
        assert_eq!(n & d, 0, "normal/depth overlap");

        // Combined OR equals the fully-packed max entry.
        let combined = vp | bid | sid | ao | qid | n | d;
        let full = pack_max_all().raw();
        assert_eq!(combined, full, "combined fields differ from packed max");
    }

    #[test]
    fn test_visbuf_empty_is_max_depth() {
        let e = VisBufEntry::empty();
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.block_id(), 0);
        assert_eq!(e.sector_id(), 0);
        assert_eq!(e.ao_corners(), [0, 0, 0, 0]);
        assert_eq!(e.ao_max(), 0);
        assert_eq!(e.quad_id(), 0);
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
        let e = VisBufEntry::pack(u32::MAX, u32::MAX, u32::MAX, 0xFF, u32::MAX, 0xFF, u32::MAX);
        assert_eq!(e.voxel_pos(), MAX_VOXEL_POS);
        assert_eq!(e.block_id(), MAX_BLOCK_ID);
        assert_eq!(e.sector_id(), MAX_SECTOR_ID);
        assert_eq!(e.ao_corners(), [3, 3, 3, 3]);
        assert_eq!(e.ao_max(), 3);
        assert_eq!(e.quad_id(), MAX_QUAD_ID);
        assert_eq!(e.normal(), MAX_NORMAL);
        assert_eq!(e.depth(), MAX_DEPTH);
    }

    #[test]
    fn test_visbuf_sector_id_independent_field() {
        // Regression for the M11 visbuf v2 layout: sector_id must occupy its
        // own 4-bit field at bit 20, independent of block_id (bit 16..20) and
        // ao_corners (bit 24..32). Back-to-back max values must round-trip
        // without bleeding into either neighbour.
        for sid in [0u32, 1, 5, 0xA, MAX_SECTOR_ID] {
            let e = VisBufEntry::pack(0x1234, 0xC, sid, 0x5A, 0x4242, 5, 0x1FFF);
            assert_eq!(e.sector_id(), sid, "sector_id={sid} round-trip");
            assert_eq!(e.block_id(), 0xC, "block_id drift on sector_id={sid}");
            assert_eq!(
                e.ao_corners(),
                [2, 2, 1, 1],
                "ao_corners drift on sector_id={sid}"
            );
            assert_eq!(e.voxel_pos(), 0x1234, "voxel_pos drift on sector_id={sid}");
            assert_eq!(e.quad_id(), 0x4242, "quad_id drift on sector_id={sid}");
            assert_eq!(e.normal(), 5, "normal drift on sector_id={sid}");
            assert_eq!(e.depth(), 0x1FFF, "depth drift on sector_id={sid}");
        }
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

    #[test]
    fn test_visbuf_13bit_depth_precision() {
        assert_eq!(DEPTH_MASK, 8191, "13-bit depth mask must be 8191");
        assert_eq!(DEPTH_SHIFT, 51, "depth shift must start at bit 51");
        assert_eq!(QUAD_ID_MASK, 65535, "16-bit quad_id mask must be 65535");
        let entry = VisBufEntry::pack(0, 0, 0, 0, 0, 0, 8191);
        assert_eq!(entry.depth(), 8191);
    }
}
