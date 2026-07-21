//! Packed quad + 4-byte vertex formats for greedy meshing output (plan 06 / 09).
//!
//! [`PackedQuad`] is exactly 8 bytes: a 32-bit geometry word (pos + size + face)
//! plus three 8-bit fields (block type, AO, light) and an 8-bit flags byte. It is
//! the CPU-side representation that M4's vertex-pulling shader will expand.
//!
//! [`PackedVertex4`] is the eventual GPU 4-byte-per-vertex format (pos + normal +
//! uv + color packed into a single `u32`); `PackedQuad::vertex` derives it for a
//! given corner so consumers can validate the layout today.

/// Face direction index. Order is fixed so it matches the greedy mesher's
/// `(axis * 2 + sign)` encoding: +X=0, -X=1, +Y=2, -Y=3, +Z=4, -Z=5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FaceDir {
    PosX = 0,
    NegX = 1,
    PosY = 2,
    NegY = 3,
    PosZ = 4,
    NegZ = 5,
}

impl FaceDir {
    #[inline]
    pub fn from_index(i: u8) -> FaceDir {
        match i {
            0 => FaceDir::PosX,
            1 => FaceDir::NegX,
            2 => FaceDir::PosY,
            3 => FaceDir::NegY,
            4 => FaceDir::PosZ,
            5 => FaceDir::NegZ,
            _ => unreachable!("invalid face index: {i}"),
        }
    }

    /// Axis index (0=X, 1=Y, 2=Z) this face points along.
    #[inline]
    pub fn axis(self) -> usize {
        (self as usize) / 2
    }

    /// +1 for positive faces, -1 for negative faces.
    #[inline]
    pub fn sign(self) -> i32 {
        if (self as usize) & 1 == 0 { 1 } else { -1 }
    }
}

const POS_MASK: u32 = 0x1F; // 5 bits (0..31, sector-local position)
const SIZE_MASK: u32 = 0x3F; // 6 bits (0..63, width/height up to 32)
const FACE_MASK: u32 = 0x7; // 3 bits (0..7, six face directions)

/// Bit 0 of [`PackedQuad::flags`] — when set, the pre-pass vertex shader
/// emits the **flipped** diagonal split for this quad (0fps.net / Exile
/// anisotropy fix). Stored CPU-side so the GPU stays branchless.
///
/// 0fps.net: `if a00 + a11 > a01 + a10` → flipped. The CPU computes the
/// sum-compare in the mesher and stores the result here; the shader
/// only reads the bit.
pub const FLIP_FLAG: u8 = 0x1;

/// An 8-byte packed greedy-meshed quad (plan 09 §3.2).
///
/// Layout (little-endian):
/// * `data[0..4]` — geometry word:
///   `x(5)|y(5)|z(5)|width(6)|height(6)|face(3)` (30 bits used, 2 reserved).
///   NOTE: the constitution sketches `x/y/z(6)|width/height(6)|face(2)`; the
///   implementation uses 5-bit positions (sufficient for 32³ sectors, 0..31)
///   and 3-bit face (required for 6 directions, plan's 2-bit only fits 4).
///   This is a deliberate deviation — 2 reserved bits remain for future use.
/// * `data[4]`    — block type id (low byte; matches the constitution's `u8`)
/// * `data[5]`    — AO: 4 corners x 2 bits (corner0 in bits 0..2, ... corner3 6..8)
/// * `data[6]`    — light (sky<<4 | block, filled by lighting in a later milestone)
/// * `data[7]`    — flags: bit0 = [FLIP_FLAG] (CPU-decided diagonal flip for
///   quad anisotropy correction)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackedQuad {
    pub data: [u8; 8],
}

impl PackedQuad {
    /// True if the quad's 4-corner AO values require a **flipped** diagonal split
    /// to avoid GPU barycentric interpolation seams (0fps.net anisotropy fix,
    /// Exile "Vertex Ambient Occlusion", Andre Blunt "Quadrilateral
    /// Interpolation"). The pre-pass vertex shader uses this bit to pick a
    /// branchless `select` for the diagonal.
    ///
    /// 0fps.net: `if a00 + a11 > a01 + a10` → flipped. The CPU pre-decides the
    /// comparison once per quad so the GPU never needs a divergent branch.
    #[inline]
    pub fn needs_flip(corners: [u8; 4]) -> bool {
        // c0 = a00 (du=0,dv=0), c1 = a01 (du=1,dv=0),
        // c2 = a10 (du=0,dv=1), c3 = a11 (du=1,dv=1) — matches
        // `PackedQuad::ao()` packing order, which the pre-pass shader
        // mirrors verbatim. The same rule works in either orientation
        // because the diagonal choice only depends on the four corner
        // AOs being assigned to the same four vertex positions.
        (corners[0] as u32 + corners[3] as u32) > (corners[1] as u32 + corners[2] as u32)
    }

    /// Pack a quad. Positions must be < 32; size < 64; `face` < 6.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        x: u32,
        y: u32,
        z: u32,
        width: u32,
        height: u32,
        face: u8,
        block_type: u8,
        ao: u8,
        light: u8,
        flags: u8,
    ) -> Self {
        let geometry = (x & POS_MASK)
            | ((y & POS_MASK) << 5)
            | ((z & POS_MASK) << 10)
            | ((width & SIZE_MASK) << 15)
            | ((height & SIZE_MASK) << 21)
            | ((face as u32 & FACE_MASK) << 27);
        let mut d = [0u8; 8];
        d[..4].copy_from_slice(&geometry.to_le_bytes());
        d[4] = block_type;
        d[5] = ao;
        d[6] = light;
        d[7] = flags;
        Self { data: d }
    }

    #[inline]
    fn geometry(&self) -> u32 {
        u32::from_le_bytes([self.data[0], self.data[1], self.data[2], self.data[3]])
    }

    #[inline]
    pub fn x(&self) -> u32 {
        self.geometry() & POS_MASK
    }

    #[inline]
    pub fn y(&self) -> u32 {
        (self.geometry() >> 5) & POS_MASK
    }

    #[inline]
    pub fn z(&self) -> u32 {
        (self.geometry() >> 10) & POS_MASK
    }

    #[inline]
    pub fn width(&self) -> u32 {
        (self.geometry() >> 15) & SIZE_MASK
    }

    #[inline]
    pub fn height(&self) -> u32 {
        (self.geometry() >> 21) & SIZE_MASK
    }

    #[inline]
    pub fn face(&self) -> FaceDir {
        FaceDir::from_index(((self.geometry() >> 27) & FACE_MASK) as u8)
    }

    #[inline]
    pub fn block_type(&self) -> u8 {
        self.data[4]
    }

    /// 4 corner AO values (0=occluded .. 3=open), corner0..corner3.
    #[inline]
    pub fn ao(&self) -> [u8; 4] {
        let a = self.data[5];
        [a & 0x3, (a >> 2) & 0x3, (a >> 4) & 0x3, (a >> 6) & 0x3]
    }

    /// Pack 4 corner AO values (each 0..=3) into the 8-bit AO field.
    #[inline]
    pub fn pack_ao(corners: [u8; 4]) -> u8 {
        (corners[0] & 0x3)
            | ((corners[1] & 0x3) << 2)
            | ((corners[2] & 0x3) << 4)
            | ((corners[3] & 0x3) << 6)
    }

    /// Max of the 4 corner AO values (0..=3). Used by the resolve shader's
    /// `mix(0.55, 1.0, ao_max / 3.0)` multiplier (M10a.3).
    #[inline]
    pub fn ao_max(&self) -> u8 {
        let [c0, c1, c2, c3] = self.ao();
        c0.max(c1).max(c2).max(c3)
    }

    /// Pack an 8-bit light value (`sky<<4 | block`) into the quad's light field.
    /// Both nibbles are masked to 4 bits so the result is always a valid
    /// `LightData` nibble pair (M7 `LightData` / M10a.4 layout).
    #[inline]
    pub fn pack_light(sky: u8, block: u8) -> u8 {
        ((sky & 0xF) << 4) | (block & 0xF)
    }

    #[inline]
    pub fn light(&self) -> u8 {
        self.data[6]
    }

    /// Overwrite the 8-bit light field. Used by the mesher when integrating
    /// the `SectorLight` sample for this quad's 4 corners.
    #[inline]
    pub fn set_light(&mut self, light: u8) {
        self.data[6] = light;
    }

    #[inline]
    pub fn flags(&self) -> u8 {
        self.data[7]
    }

    /// Re-pack this quad from its own fields; returns an identical byte array.
    /// Used by the round-trip test (`pack -> unpack -> pack == original`).
    #[inline]
    pub fn repack(&self) -> Self {
        let [c0, c1, c2, c3] = self.ao();
        Self::new(
            self.x(),
            self.y(),
            self.z(),
            self.width(),
            self.height(),
            self.face() as u8,
            self.block_type(),
            PackedQuad::pack_ao([c0, c1, c2, c3]),
            self.light(),
            self.flags(),
        )
    }

    /// Tangent axis indices (u, v) for this quad's face (the in-plane axes).
    #[inline]
    fn tangent_axes(&self) -> (usize, usize) {
        let d = self.face().axis();
        ((d + 1) % 3, (d + 2) % 3)
    }

    /// World-ish (sector-local) position of quad corner `i` (0..4).
    ///
    /// This is the eventual GPU vertex-pulling expansion; exact plane offsetting is
    /// finalized in M4. For M3 it returns a deterministic per-corner anchor so the
    /// 4-byte [`PackedVertex4`] layout can be round-tripped and inspected.
    #[inline]
    pub fn corner_pos(&self, corner: u8) -> [u32; 3] {
        let (u, v) = self.tangent_axes();
        let (x, y, z, w, h, _) = (
            self.x(),
            self.y(),
            self.z(),
            self.width(),
            self.height(),
            self.face() as u8,
        );
        let base = [x, y, z];
        let du = [0u32, 1, 0, 1][corner as usize % 4];
        let dv = [0u32, 0, 1, 1][corner as usize % 4];
        let mut p = base;
        p[u] += du * w;
        p[v] += dv * h;
        p
    }

    /// Expand this quad's `corner` into the 4-byte GPU vertex format.
    #[inline]
    pub fn vertex(&self, corner: u8) -> PackedVertex4 {
        let pos = self.corner_pos(corner);
        let normal = self.face() as u8;
        // UV: 0 at min corner, 1 at the far corner along each tangent.
        let du = [0u8, 1, 0, 1][corner as usize % 4];
        let dv = [0u8, 0, 1, 1][corner as usize % 4];
        let uv_u = if du == 0 { 0 } else { 1 };
        let uv_v = if dv == 0 { 0 } else { 1 };
        let ao = self.ao()[corner as usize % 4];
        let color = self.block_type();
        PackedVertex4::pack(pos, normal, [uv_u, uv_v], ao, color)
    }
}

/// 4-byte packed GPU vertex (plan 06 §B.4): pos(6|6|6) | normal(3) | uv(2|2) |
/// ao(2) | color(3) — fits in 32 bits for vertex pulling.
///
/// **Limitation:** The `color` field is 3 bits (0..7), which only supports 8
/// distinct block types. Production rendering uses [`PackedQuad`] vertex pulling
/// where `block_type` is a full 8-bit field — `PackedVertex4` is a CPU-side
/// validation/debug format only (see `meshdata_to_gpu_bytes` in the pipeline).
/// If this format is ever promoted to production, the bit layout must be
/// redesigned to accommodate the full block type range.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PackedVertex4 {
    pub data: u32,
}

const VTX_MASK6: u32 = 0x3F;

impl PackedVertex4 {
    #[inline]
    pub fn pack(pos: [u32; 3], normal: u8, uv: [u8; 2], ao: u8, color: u8) -> Self {
        let g = (pos[0] & VTX_MASK6)
            | ((pos[1] & VTX_MASK6) << 6)
            | ((pos[2] & VTX_MASK6) << 12)
            | ((normal as u32 & 0x7) << 18)
            | ((uv[0] as u32 & 0x3) << 21)
            | ((uv[1] as u32 & 0x3) << 23)
            | ((ao as u32 & 0x3) << 25)
            | ((color as u32 & 0x7) << 27);
        Self { data: g }
    }

    #[inline]
    pub fn unpack(&self) -> ([u32; 3], u8, [u8; 2], u8, u8) {
        let g = self.data;
        (
            [g & VTX_MASK6, (g >> 6) & VTX_MASK6, (g >> 12) & VTX_MASK6],
            ((g >> 18) & 0x7) as u8,
            [((g >> 21) & 0x3) as u8, ((g >> 23) & 0x3) as u8],
            ((g >> 25) & 0x3) as u8,
            ((g >> 27) & 0x7) as u8,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "invalid face index")]
    fn from_index_rejects_6() {
        FaceDir::from_index(6);
    }

    #[test]
    #[should_panic(expected = "invalid face index")]
    fn from_index_rejects_7() {
        FaceDir::from_index(7);
    }
}
