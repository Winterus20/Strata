//! CPU-side 64-bit visibility-buffer value type (M4b + M12, plan 10 §1).
//!
//! This is the host representation of the unified visibility buffer entry (Aokana
//! Figure 7 layout). No WGSL / GPU upload happens here yet; M4c builds the storage
//! buffer from these values. All packing is branchless bit arithmetic so the same
//! shifts can be mirrored verbatim in the shader.
//!
//! Bit layout (M12 visbuf v6 — 17-bit depth for stable coplanar atomicMax):
//! * `bit[0:15]`   voxel_pos    (15 bits, sector-local 5+5+5; 0..32767)
//! * `bit[15:19]`  block_id     (4 bits, copied from `PackedQuad::data[4]`, 0..15)
//! * `bit[19:27]`  ao_corners   (8 bits, 4 corners x 2 bits — c0[0:2] c1[2:4] c2[4:6] c3[6:8])
//!   0 = most occluded, 3 = fully open. The fragment shader
//!   does bi-linear interpolation across the quad via the UV
//!   coords reconstructed in `get_quad_space_uv`.
//! * `bit[27:43]`  quad_id      (17 bits, global SSBO / lightmap slot; up to 131072)
//! * `bit[44:46]`  normal       (3 bits, 0..7)
//! * `bit[47:63]`  depth        (17 bits, reversed-Z; clear = 0)
//!   (17-bit depth = 131072 levels — stable coplanar atomicMax vs old 13-bit ties)
//!
//! v4 stored a 4-bit `sector_id` and only 16-bit `quad_id`. With streaming AOI
//! caches past ~65K quads, truncated `quad_id` made resolve sample the wrong
//! lightmap byte (sector-aligned dark slabs until a remesh reshuffled slots).
//! `sector_id` was unused in resolve; those bits now extend `quad_id`.
//! The current `BlockRegistry` tops out at 16 blocks, which fits the 4-bit
//! `block_id`; a 17th block will require either a layout bump or a palette
//! mask fallback.

use bytemuck::{Pod, Zeroable};

use crate::meshing::{MeshData, PackedQuad};

/// `voxel_pos` occupies the low 15 bits (5+5+5 packing).
pub const VOXEL_POS_MASK: u64 = (1u64 << 15) - 1;
/// `block_id` occupies 4 bits starting at bit 15.
pub const BLOCK_ID_MASK: u64 = (1u64 << 4) - 1;
/// `ao_corners` occupies 8 bits starting at bit 19 (4 corners x 2 bits).
pub const AO_CORNERS_MASK: u64 = (1u64 << 8) - 1;
/// `quad_id` occupies 17 bits starting at bit 27 (global SSBO slot, up to 131072).
pub const QUAD_ID_MASK: u64 = (1u64 << 17) - 1;
/// `normal` occupies 3 bits starting at bit 44.
pub const NORMAL_MASK: u64 = 0x7;
/// `depth` occupies 17 bits starting at bit 47 (reversed-Z depth precision).
pub const DEPTH_MASK: u64 = (1u64 << 17) - 1;

const BLOCK_ID_SHIFT: u32 = 15;
const AO_CORNERS_SHIFT: u32 = 19;
const QUAD_ID_SHIFT: u32 = 27;
const NORMAL_SHIFT: u32 = 44;
const DEPTH_SHIFT: u32 = 47;

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
        ao_corners: u8,
        quad_id: u32,
        normal: u8,
        depth: u32,
    ) -> Self {
        let v = (voxel_pos as u64 & VOXEL_POS_MASK)
            | ((block_id as u64 & BLOCK_ID_MASK) << BLOCK_ID_SHIFT)
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

/// Bytes uploaded for the single-draw depth prepass.
///
/// Contract: every non-empty mesh batch that should appear on screen must be
/// present here. Until a dedicated blended transparent pass exists, transparent
/// quads are appended after opaque so water/leaf/ice/glass stay visible
/// (drawn as solid; alpha blending comes later).
#[inline]
pub fn mesh_prepass_bytes(mesh: &MeshData) -> Vec<u8> {
    let mut out = Vec::with_capacity(mesh.opaque_gpu.len() + mesh.transparent_gpu.len());
    out.extend_from_slice(&mesh.opaque_gpu);
    out.extend_from_slice(&mesh.transparent_gpu);
    out
}

#[inline]
fn pack_batch(batch: &[PackedQuad]) -> Vec<u8> {
    let gpu: Vec<PackedQuadGpu> = batch.iter().map(PackedQuadGpu::from_packed_quad).collect();
    bytemuck::cast_slice(&gpu).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_VOXEL_POS: u32 = (1u32 << 15) - 1;
    const MAX_BLOCK_ID: u32 = (1u32 << 4) - 1;
    const MAX_AO_CORNERS: u8 = 0xFF;
    const MAX_QUAD_ID: u32 = (1u32 << 17) - 1;
    const MAX_NORMAL: u8 = 0x7;
    const MAX_DEPTH: u32 = (1u32 << 17) - 1;

    fn pack_max_all() -> VisBufEntry {
        VisBufEntry::pack(
            MAX_VOXEL_POS,
            MAX_BLOCK_ID,
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
        let e = VisBufEntry::pack(0, 0, 0, 0, 0, 0);
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.block_id(), 0);
        assert_eq!(e.ao_corners(), [0, 0, 0, 0]);
        assert_eq!(e.ao_max(), 0);
        assert_eq!(e.quad_id(), 0);
        assert_eq!(e.normal(), 0);
        assert_eq!(e.depth(), 0);
        assert_eq!(e.raw(), 0);
    }

    #[test]
    fn test_visbuf_round_trip_scattered() {
        // Each tuple: (voxel_pos, block_id, ao_corners byte, quad_id, normal, depth)
        let cases: [(u32, u32, u8, u32, u8, u32); 8] = [
            (0x3BCD, 0x5, 0xFF, 0x1234, 0x3, 0x1FFFF),
            (1, 1, 0x5A, 1, 1, 1),
            (MAX_VOXEL_POS, 0, 0, 0, 0, 0),
            (0, MAX_BLOCK_ID, 0, 0, 0, 0),
            (0, 0, MAX_AO_CORNERS, 0, 0, 0),
            (0, 0, 0, MAX_QUAD_ID, 0, 0),
            (0, 0, 0, 0, MAX_NORMAL, 0),
            (0, 0, 0, 0, 0, MAX_DEPTH),
        ];
        for (vp, bid, ao, qid, n, d) in cases {
            let e = VisBufEntry::pack(vp, bid, ao, qid, n, d);
            assert_eq!(e.voxel_pos(), vp, "voxel_pos round-trip");
            assert_eq!(e.block_id(), bid, "block_id round-trip");
            assert_eq!(e.ao_corners(), {
                let a = ao;
                [a & 0x3, (a >> 2) & 0x3, (a >> 4) & 0x3, (a >> 6) & 0x3]
            });
            assert_eq!(e.quad_id(), qid, "quad_id round-trip");
            assert_eq!(e.normal(), n, "normal round-trip");
            assert_eq!(e.depth(), d, "depth round-trip");
        }
    }

    #[test]
    fn test_visbuf_field_boundaries_no_bleed() {
        let vp = VisBufEntry::pack(MAX_VOXEL_POS, 0, 0, 0, 0, 0).raw();
        let bid = VisBufEntry::pack(0, MAX_BLOCK_ID, 0, 0, 0, 0).raw();
        let ao = VisBufEntry::pack(0, 0, MAX_AO_CORNERS, 0, 0, 0).raw();
        let qid = VisBufEntry::pack(0, 0, 0, MAX_QUAD_ID, 0, 0).raw();
        let n = VisBufEntry::pack(0, 0, 0, 0, MAX_NORMAL, 0).raw();
        let d = VisBufEntry::pack(0, 0, 0, 0, 0, MAX_DEPTH).raw();
        assert_eq!(vp & bid, 0, "voxel_pos/block_id overlap");
        assert_eq!(vp & ao, 0, "voxel_pos/ao_corners overlap");
        assert_eq!(vp & qid, 0, "voxel_pos/quad_id overlap");
        assert_eq!(vp & n, 0, "voxel_pos/normal overlap");
        assert_eq!(vp & d, 0, "voxel_pos/depth overlap");
        assert_eq!(bid & ao, 0, "block_id/ao_corners overlap");
        assert_eq!(bid & qid, 0, "block_id/quad_id overlap");
        assert_eq!(bid & n, 0, "block_id/normal overlap");
        assert_eq!(bid & d, 0, "block_id/depth overlap");
        assert_eq!(ao & qid, 0, "ao_corners/quad_id overlap");
        assert_eq!(ao & n, 0, "ao_corners/normal overlap");
        assert_eq!(ao & d, 0, "ao_corners/depth overlap");
        assert_eq!(qid & n, 0, "quad_id/normal overlap");
        assert_eq!(qid & d, 0, "quad_id/depth overlap");
        assert_eq!(n & d, 0, "normal/depth overlap");
    }

    #[test]
    fn test_visbuf_empty_is_far_depth() {
        let e = VisBufEntry::empty();
        assert_eq!(e.depth(), MAX_DEPTH);
        assert_eq!(e.voxel_pos(), 0);
        assert_eq!(e.block_id(), 0);
        assert_eq!(e.ao_corners(), [0, 0, 0, 0]);
        assert_eq!(e.quad_id(), 0);
        assert_eq!(e.normal(), 0);
    }

    #[test]
    fn test_visbuf_masks_overwide_inputs() {
        let e = VisBufEntry::pack(u32::MAX, u32::MAX, 0xFF, u32::MAX, 0xFF, u32::MAX);
        assert_eq!(e.voxel_pos(), MAX_VOXEL_POS);
        assert_eq!(e.block_id(), MAX_BLOCK_ID);
        assert_eq!(e.quad_id(), MAX_QUAD_ID);
        assert_eq!(e.normal(), MAX_NORMAL);
        assert_eq!(e.depth(), MAX_DEPTH);
    }

    /// Regression: global SSBO slots past 64K must survive packing. v4's 16-bit
    /// `quad_id` truncated these; v6 keeps 17 bits (131072 slots).
    #[test]
    fn test_visbuf_quad_id_past_64k_round_trips() {
        for qid in [65_536u32, 100_000, 118_259, MAX_QUAD_ID] {
            let e = VisBufEntry::pack(0x1234, 0xC, 0x5A, qid, 5, 0x1FFFF);
            assert_eq!(e.quad_id(), qid, "quad_id={qid} must not truncate");
            assert_eq!(e.voxel_pos(), 0x1234);
            assert_eq!(e.block_id(), 0xC);
            assert_eq!(e.depth(), 0x1FFFF);
        }
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

    /// Regression: dropping the transparent batch makes water/leaf/ice/glass
    /// invisible in-game (meshed, never uploaded).
    #[test]
    fn prepass_bytes_include_transparent_batch() {
        let mut mesh = MeshData::default();
        let qo = PackedQuad::new(0, 0, 0, 1, 1, 0, 1, 0, 0, 0);
        let qt = PackedQuad::new(1, 1, 1, 1, 1, 2, 3, 0, 0, 0);
        mesh.opaque.push(qo);
        mesh.transparent.push(qt);
        let (opaque_gpu, transparent_gpu) = meshdata_to_gpu_bytes(&mesh);
        mesh.opaque_gpu = opaque_gpu;
        mesh.transparent_gpu = transparent_gpu;

        let upload = mesh_prepass_bytes(&mesh);
        assert_eq!(
            upload.len(),
            mesh.opaque_gpu.len() + mesh.transparent_gpu.len(),
            "transparent quads must reach the prepass upload"
        );

        let mut transparent_only = MeshData::default();
        transparent_only.transparent_gpu = mesh.transparent_gpu.clone();
        assert_eq!(
            mesh_prepass_bytes(&transparent_only).len(),
            8,
            "transparent-only sector (e.g. leaf/water) must not upload empty"
        );
    }

    #[test]
    fn test_visbuf_17bit_depth_precision() {
        assert_eq!(DEPTH_SHIFT, 47, "depth shift must start at bit 47");
        assert_eq!(QUAD_ID_MASK, (1 << 17) - 1, "17-bit quad_id mask");
        let entry = VisBufEntry::pack(0, 0, 0, 0, 0, 131_071);
        assert_eq!(entry.depth(), 131_071);
    }

    /// Verify the CPU bit layout matches the GPU packing in prepass.rs.
    #[test]
    fn test_visbuf_cpu_gpu_layout_consistency() {
        assert_eq!(BLOCK_ID_SHIFT, 15, "block_id shift must be 15 (GPU << 15u)");
        assert_eq!(
            AO_CORNERS_SHIFT, 19,
            "ao_corners shift must be 19 (GPU << 19u)"
        );
        assert_eq!(QUAD_ID_SHIFT, 27, "quad_id shift must be 27 (GPU << 27u)");
        assert_eq!(NORMAL_SHIFT, 44, "normal shift must be 44 (GPU << 44u)");
        assert_eq!(DEPTH_SHIFT, 47, "depth shift must be 47 (GPU << 47u)");

        assert_eq!(
            VOXEL_POS_MASK, 0x7FFF,
            "voxel_pos mask must be 0x7FFF (GPU)"
        );
        assert_eq!(BLOCK_ID_MASK, 0xF, "block_id mask must be 0xF (GPU)");
        assert_eq!(AO_CORNERS_MASK, 0xFF, "ao_corners mask must be 0xFF (GPU)");
        assert_eq!(QUAD_ID_MASK, 0x1_FFFF, "quad_id mask must be 0x1FFFF (GPU)");
        assert_eq!(NORMAL_MASK, 0x7, "normal mask must be 0x7 (GPU)");
        assert_eq!(DEPTH_MASK, 0x1_FFFF, "depth mask must be 0x1FFFF (GPU)");

        let e = VisBufEntry::pack(0x3BCD, 0xC, 0x5A, 0x1_4242, 5, 0x1FFFF);
        let raw = e.raw();
        assert_eq!(raw & 0x7FFF, 0x3BCD, "voxel_pos at bit[0:15]");
        assert_eq!((raw >> 15) & 0xF, 0xC, "block_id at bit[15:19]");
        assert_eq!((raw >> 19) & 0xFF, 0x5A, "ao_corners at bit[19:27]");
        assert_eq!((raw >> 27) & 0x1_FFFF, 0x1_4242, "quad_id at bit[27:43]");
        assert_eq!((raw >> 44) & 0x7, 5, "normal at bit[44:46]");
        assert_eq!((raw >> 47) & 0x1_FFFF, 0x1FFFF, "depth at bit[47:63]");
    }
}
