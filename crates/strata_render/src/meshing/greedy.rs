//! Binary greedy mesher (plan 09 §4): per-axis face masks, greedy coplanar
//! merge, neighbor-aware culling, and branchless per-vertex AO.

use crate::meshing::packed_quad::{FLIP_FLAG, PackedQuad};
use crate::meshing::{MeshData, Mesher, NeighborView, SectorLightView};
use strata_core::prelude::*;

const DIM: i32 = 32;
/// Number of voxels in a 32³ sector; the size of one owned mesh snapshot.
pub const SNAPSHOT_LEN: usize = 32 * 32 * 32;

/// Flatten (x,y,z) to a linear index into the 32³ snapshot array.
/// Layout: `x + y*32 + z*1024` (**Z-major** order).
/// NOTE: Different from [`OccupancyScratch::bit_index`] which uses X-major.
#[inline]
fn snap_index(x: u32, y: u32, z: u32) -> usize {
    (x + y * 32 + z * 1024) as usize
}

/// Voxels in one 32×32 sector-boundary plane (a neighbor's shared face).
pub const PLANE_LEN: usize = 32 * 32;

/// One neighbor's 32×32 boundary plane of resolved block ids. Only this plane
/// is ever sampled across a sector border (plan 09 §4.0), so meshing carries it
/// (2 KB) instead of the neighbor's full 64 KB [`VoxelSnapshot`] — this removed
/// the dominant per-sector streaming bandwidth + heap-allocation cost.
pub type BoundaryPlane = [BlockId; PLANE_LEN];

/// Flatten the two in-plane (tangent) axes of a border sample to a
/// [`BoundaryPlane`] index for neighbor direction `idx` (0=+X..5=-Z). The
/// primary (out-of-sector) axis is fixed by `idx`, so only the tangents index
/// the plane. MUST match the write order in [`build_boundary_plane_locked`].
#[inline]
fn plane_index(idx: usize, nx: u32, ny: u32, nz: u32) -> usize {
    match idx {
        0 | 1 => (ny * 32 + nz) as usize,
        2 | 3 => (nx * 32 + nz) as usize,
        _ => (nx * 32 + ny) as usize,
    }
}

/// Owned, `Send + 'static` copy of a sector's resolved block ids, so meshing can
/// run on a background thread without touching the shared `GlobalBrickPool`.
pub type VoxelSnapshot = Box<[BlockId; SNAPSHOT_LEN]>;

/// Snapshot a sector's voxels into a flat owned array (one `get_block` per voxel).
pub fn build_sector_snapshot(
    sector: &XBrickMap,
    pool: &GlobalBrickPool,
    palette: &SectorPalette,
) -> VoxelSnapshot {
    let mut s = Box::new([BlockId::AIR; SNAPSHOT_LEN]);
    for z in 0..32u32 {
        for y in 0..32u32 {
            for x in 0..32u32 {
                s[snap_index(x, y, z)] = sector.get_block(pool, palette, VoxelCoord::new(x, y, z));
            }
        }
    }
    s
}

/// Snapshot a sector's voxels into a flat owned array using a pre-locked pool read guard
/// to avoid repeated RwLock acquisitions in the 32x32x32 loop.
pub fn build_sector_snapshot_locked(
    sector: &XBrickMap,
    pool: &InnerPool,
    palette: &SectorPalette,
) -> VoxelSnapshot {
    let mut s = Box::new([BlockId::AIR; SNAPSHOT_LEN]);
    fill_sector_snapshot_locked(sector, pool, palette, &mut s);
    s
}

/// Fill a caller-owned snapshot buffer (pre-locked pool). Every voxel is written
/// unconditionally, so the buffer needs no clearing and can be reused across
/// sectors (a per-worker scratch), keeping the meshing hot path heap-free
/// (AGENTS.md §7.G / plan 39).
pub fn fill_sector_snapshot_locked(
    sector: &XBrickMap,
    pool: &InnerPool,
    palette: &SectorPalette,
    out: &mut [BlockId; SNAPSHOT_LEN],
) {
    out.fill(BlockId::AIR);
    let mut sector_mask = sector.sector_mask;
    while sector_mask != 0 {
        let bi = sector_mask.trailing_zeros() as usize;
        sector_mask &= sector_mask - 1;
        let Some(handle) = sector.brick_handle_at(bi) else {
            continue;
        };
        let Some(brick) = pool.bricks.get(handle) else {
            continue;
        };
        let bx = (bi % 4) as u32;
        let by = (bi / 16) as u32;
        let bz = ((bi % 16) / 4) as u32;
        let brick_base_x = bx * 8;
        let brick_base_y = by * 8;
        let brick_base_z = bz * 8;

        let mut sub_mask = brick.sub_mask;
        while sub_mask != 0 {
            let si = sub_mask.trailing_zeros() as usize;
            sub_mask &= sub_mask - 1;
            let sub = &brick.subs[si];
            let sx = (si % 4) as u32;
            let sy = (si / 16) as u32;
            let sz = ((si % 16) / 4) as u32;
            let sub_base_x = brick_base_x + sx * 2;
            let sub_base_y = brick_base_y + sy * 2;
            let sub_base_z = brick_base_z + sz * 2;

            let mut voxel_mask = sub.voxel_mask;
            while voxel_mask != 0 {
                let vb = voxel_mask.trailing_zeros() as usize;
                voxel_mask &= voxel_mask - 1;
                let id = palette.resolve(sub.indices[vb]);
                if id != BlockId::AIR {
                    let lx = sub_base_x + (vb as u32 & 1);
                    let ly = sub_base_y + ((vb as u32 >> 2) & 1);
                    let lz = sub_base_z + ((vb as u32 >> 1) & 1);
                    out[snap_index(lx, ly, lz)] = id;
                }
            }
        }
    }
}

/// Extract the single 32×32 boundary plane of `sector` that faces the sector
/// being meshed, for neighbor direction `idx` (0=+X..5=-Z). Only this plane is
/// ever sampled across a sector border (plan 09 §4.0), so building it (2 KB)
/// replaces the full 64 KB neighbor snapshot — the dominant streaming bandwidth
/// cost. Indexed by [`plane_index`] so `SectorSampler::sample_block` reads back
/// the exact voxel the full-snapshot path would have.
pub fn build_boundary_plane_locked(
    sector: &XBrickMap,
    pool: &InnerPool,
    palette: &SectorPalette,
    idx: usize,
) -> Box<BoundaryPlane> {
    let mut plane = Box::new([BlockId::AIR; PLANE_LEN]);
    fill_boundary_plane_locked(sector, pool, palette, idx, &mut plane);
    plane
}

/// Fill a caller-owned boundary plane (pre-locked pool). Every in-plane cell is
/// written unconditionally, so a stack-allocated buffer can be reused with no
/// clearing — the neighbor path is then fully heap-free too.
pub fn fill_boundary_plane_locked(
    sector: &XBrickMap,
    pool: &InnerPool,
    palette: &SectorPalette,
    idx: usize,
    out: &mut BoundaryPlane,
) {
    out.fill(BlockId::AIR);
    let mut sector_mask = sector.sector_mask;
    while sector_mask != 0 {
        let bi = sector_mask.trailing_zeros() as usize;
        sector_mask &= sector_mask - 1;

        // Filter bricks by face
        let bx = (bi % 4) as u32;
        let by = (bi / 16) as u32;
        let bz = ((bi % 16) / 4) as u32;
        match idx {
            0 if bx != 0 => continue,
            1 if bx != 3 => continue,
            2 if by != 0 => continue,
            3 if by != 3 => continue,
            4 if bz != 0 => continue,
            5 if bz != 3 => continue,
            _ => {}
        }

        let Some(handle) = sector.brick_handle_at(bi) else {
            continue;
        };
        let Some(brick) = pool.bricks.get(handle) else {
            continue;
        };
        let brick_base_x = bx * 8;
        let brick_base_y = by * 8;
        let brick_base_z = bz * 8;

        let mut sub_mask = brick.sub_mask;
        while sub_mask != 0 {
            let si = sub_mask.trailing_zeros() as usize;
            sub_mask &= sub_mask - 1;

            // Filter sub-bricks by face
            let sx = (si % 4) as u32;
            let sy = (si / 16) as u32;
            let sz = ((si % 16) / 4) as u32;
            match idx {
                0 if sx != 0 => continue,
                1 if sx != 3 => continue,
                2 if sy != 0 => continue,
                3 if sy != 3 => continue,
                4 if sz != 0 => continue,
                5 if sz != 3 => continue,
                _ => {}
            }

            let sub = &brick.subs[si];
            let sub_base_x = brick_base_x + sx * 2;
            let sub_base_y = brick_base_y + sy * 2;
            let sub_base_z = brick_base_z + sz * 2;

            let mut voxel_mask = sub.voxel_mask;
            while voxel_mask != 0 {
                let vb = voxel_mask.trailing_zeros() as usize;
                voxel_mask &= voxel_mask - 1;

                // Filter voxels by face
                let lx = vb as u32 & 1;
                let ly = (vb as u32 >> 2) & 1;
                let lz = (vb as u32 >> 1) & 1;
                match idx {
                    0 if lx != 0 => continue,
                    1 if lx != 1 => continue,
                    2 if ly != 0 => continue,
                    3 if ly != 1 => continue,
                    4 if lz != 0 => continue,
                    5 if lz != 1 => continue,
                    _ => {}
                }

                let id = palette.resolve(sub.indices[vb]);
                if id != BlockId::AIR {
                    let rx = sub_base_x + lx;
                    let ry = sub_base_y + ly;
                    let rz = sub_base_z + lz;
                    let p_idx = match idx {
                        0 | 1 => (ry * 32 + rz) as usize,
                        2 | 3 => (rx * 32 + rz) as usize,
                        4 | 5 => (rx * 32 + ry) as usize,
                        _ => unreachable!(),
                    };
                    out[p_idx] = id;
                }
            }
        }
    }
}

/// Conservative-solid sentinel used for unloaded (`None`) neighbors (plan 09 §4.0).
#[derive(Clone, Copy)]
pub struct GreedyMesher {
    unloaded: BlockId,
}

impl GreedyMesher {
    pub fn new(_registry: &BlockRegistry) -> Self {
        // Unloaded (not-yet-generated) neighbors are treated as AIR, not as a
        // conservative solid block. This guarantees every boundary face facing an
        // unloaded sector is emitted; if the neighbor later turns out solid, its
        // nearer front face wins the prepass `atomicMax` and hides the (now
        // interior) boundary face. Treating unloaded as solid caused permanently
        // missing faces until a remesh (see `remesh-on-load` in ecs.rs).
        Self {
            unloaded: BlockId::AIR,
        }
    }

    /// Mesh purely from prebuilt [`VoxelSnapshot`]s — no `GlobalBrickPool` access,
    /// so it is safe to run on a background thread (async streaming meshing,
    /// plan 09 §2 / AGENTS.md §3.A).
    ///
    /// `light` is optional: when present, every quad's `PackedQuad.light` is
    /// filled with the 4-corner average of the corresponding `SectorLight`
    /// samples (M10a.4). When absent (e.g. tests, or before lighting has
    /// caught up), the field is left at zero and the resolve shader darkens
    /// the pixel — correct, but visually "indoor dark" everywhere.
    pub fn mesh_sector_planes(
        &self,
        self_vox: &[BlockId; SNAPSHOT_LEN],
        neighbors: &[Option<&[BlockId; PLANE_LEN]>; 6],
        registry: &BlockRegistry,
        light: Option<&SectorLightView<'_>>,
    ) -> MeshData {
        let sampler = SectorSampler {
            self_vox,
            neighbors,
            unloaded: self.unloaded,
        };
        let mut mesh = MeshData::default();
        for d in 0..3 {
            self.mesh_face_axis(d, 1, &sampler, registry, light, &mut mesh);
            self.mesh_face_axis(d, -1, &sampler, registry, light, &mut mesh);
        }

        // Pre-flatten opaque/transparent quads and pre-calculate AABB on background worker
        let (opaque_gpu, transparent_gpu) = crate::pipeline::visbuf::meshdata_to_gpu_bytes(&mesh);
        mesh.opaque_gpu = opaque_gpu;
        mesh.transparent_gpu = transparent_gpu;
        mesh.aabb = crate::pipeline::cull::aabb_of_mesh(&mesh);

        mesh
    }
}

impl Mesher for GreedyMesher {
    fn mesh_sector(
        &self,
        sector: &XBrickMap,
        palette: &SectorPalette,
        pool: &GlobalBrickPool,
        registry: &BlockRegistry,
        neighbors: &[NeighborView<'_>; 6],
    ) -> MeshData {
        // Self voxels: full owned snapshot (each axis pass reads it back).
        let self_vox = {
            let g = pool.read_inner();
            build_sector_snapshot_locked(sector, &g, palette)
        };
        // Neighbors: only the 32×32 boundary plane facing us is ever sampled
        // (plan 09 §4.0). Use stack-allocated planes (12 KB total) via
        // `fill_boundary_plane_locked` to avoid 6 heap allocations per mesh
        // (AGENTS.md §7.G / plan 39 — heap-free hot path).
        let mut planes: [BoundaryPlane; 6] = [[BlockId::AIR; PLANE_LEN]; 6];
        let mut plane_some: [bool; 6] = [false; 6];
        for i in 0..6 {
            if let Some(nsec) = neighbors[i].sector {
                let npal = neighbors[i]
                    .palette
                    .expect("neighbor sector present without palette");
                let g = neighbors[i].pool.read_inner();
                fill_boundary_plane_locked(nsec, &g, npal, i, &mut planes[i]);
                plane_some[i] = true;
            }
        }
        let mut plane_refs: [Option<&BoundaryPlane>; 6] = [const { None }; 6];
        for i in 0..6 {
            if plane_some[i] {
                plane_refs[i] = Some(&planes[i]);
            }
        }
        self.mesh_sector_planes(&self_vox, &plane_refs, registry, None)
    }
}

/// Decide whether `a`'s outward face is visible given the block `b` on the
/// other side. `a` owns the face (it is the solid-side voxel).
#[inline]
fn face_visible(a: BlockId, b: BlockId, reg: &BlockRegistry) -> bool {
    if a == BlockId::AIR {
        return false;
    }
    if b == BlockId::AIR {
        return true;
    }
    let ta = reg.is_transparent(a);
    let tb = reg.is_transparent(b);
    if ta != tb {
        return true; // opaque vs transparent: always show the boundary
    }
    if ta && tb {
        return a != b; // two different transparent blocks expose each other
    }
    false // both opaque: never draw between them
}

struct SectorSampler<'a> {
    self_vox: &'a [BlockId; SNAPSHOT_LEN],
    neighbors: &'a [Option<&'a [BlockId; PLANE_LEN]>; 6],
    unloaded: BlockId,
}

impl<'a> SectorSampler<'a> {
    #[inline]
    fn in_range(x: i32, y: i32, z: i32) -> bool {
        (0..DIM).contains(&x) && (0..DIM).contains(&y) && (0..DIM).contains(&z)
    }

    /// Block id at a (possibly out-of-range) coordinate, sampling neighbors or
    /// falling back to the conservative-solid sentinel.
    #[inline]
    fn sample_block(&self, x: i32, y: i32, z: i32) -> BlockId {
        if Self::in_range(x, y, z) {
            return self.self_vox[snap_index(x as u32, y as u32, z as u32)];
        }
        let (nx, ny, nz, idx) = if x < 0 {
            (31, y, z, 1)
        } else if x > DIM - 1 {
            (0, y, z, 0)
        } else if y < 0 {
            (x, 31, z, 3)
        } else if y > DIM - 1 {
            (x, 0, z, 2)
        } else if z < 0 {
            (x, y, 31, 5)
        } else {
            (x, y, 0, 4)
        };
        // AO sampling can push a *second* axis out of range at a sector edge
        // (e.g. depth `sd == 32` plus a tangent offset of -1/32). The branch above
        // only remaps the primary out-of-range axis; wrap the remaining axes into
        // 0..31 so the neighbour lookup stays in-sector. `& 31` maps -1 -> 31 and
        // 32 -> 0, which is exactly the neighbour-local coordinate for an edge
        // voxel crossing the boundary.
        //
        // NOTE: At sector *corners* (both tangent axes out of range), this wraps
        // to the opposite side of the primary neighbour rather than sampling the
        // diagonal neighbour (which is not available). The wrapped value is a
        // reasonable approximation — the visual impact is limited to corner
        // vertices, and full accuracy would require 26 neighbour boundary planes
        // instead of 6 (Spacefarer "Adding AO to Our Voxel Mesher").
        let nx = nx & 31;
        let ny = ny & 31;
        let nz = nz & 31;
        match self.neighbors[idx] {
            Some(plane) => plane[plane_index(idx, nx as u32, ny as u32, nz as u32)],
            None => self.unloaded,
        }
    }

    #[inline]
    fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        if Self::in_range(x, y, z) {
            self.self_vox[snap_index(x as u32, y as u32, z as u32)] != BlockId::AIR
        } else {
            self.sample_block(x, y, z) != BlockId::AIR
        }
    }

    /// Per-vertex AO (0=occluded .. 3=open) from the 3 occluders adjacent to the
    /// solid voxel `base` toward `(du,dv)`: the two edge voxels **and the true
    /// diagonal corner** (0fps canonical `vertexAO`). Sampling the real corner —
    /// not the `s1 & s2` proxy — is what makes a lone diagonal occluder read AO=2
    /// instead of a wrong AO=3.
    #[inline]
    fn ao_at(&self, base: [i32; 3], u: usize, v: usize, du: i32, dv: i32) -> u8 {
        let mut c1 = base;
        c1[u] += du;
        let mut c2 = base;
        c2[v] += dv;
        let mut c3 = base;
        c3[u] += du;
        c3[v] += dv;
        let s1 = self.solid(c1[0], c1[1], c1[2]) as u8;
        let s2 = self.solid(c2[0], c2[1], c2[2]) as u8;
        let corner = self.solid(c3[0], c3[1], c3[2]) as u8;
        // https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/
        if s1 == 1 && s2 == 1 {
            0
        } else {
            3 - (s1 + s2 + corner)
        }
    }

    /// AO signature (4 corner values) for a single voxel's face, computed from
    /// the 3 neighbors per corner (two edges + diagonal). Used by AO-safe greedy
    /// merge to prevent merging cells whose AO patterns differ — without this,
    /// greedy meshing produces quads that span AO boundaries, causing incorrect
    /// GPU interpolation (0fps.net, `block-mesh-bgm` ao_safe).
    #[inline]
    fn ao_signature(&self, base: [i32; 3], d: usize, u: usize, v: usize, s: i32) -> [u8; 4] {
        let mut side_base = base;
        side_base[d] += s;
        [
            self.ao_at(side_base, u, v, -1, -1),
            self.ao_at(side_base, u, v, 1, -1),
            self.ao_at(side_base, u, v, -1, 1),
            self.ao_at(side_base, u, v, 1, 1),
        ]
    }

    /// AO for the 4 quad corners (min/min, max/min, min/max, max/max).
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn compute_ao(
        &self,
        base: [i32; 3],
        d: usize,
        u: usize,
        v: usize,
        s: i32,
        w: i32,
        h: i32,
    ) -> [u8; 4] {
        let mut side_base = base;
        side_base[d] += s;

        let c0 = side_base;
        let mut c1 = side_base;
        c1[u] += w - 1;
        let mut c2 = side_base;
        c2[v] += h - 1;
        let mut c3 = side_base;
        c3[u] += w - 1;
        c3[v] += h - 1;

        [
            self.ao_at(c0, u, v, -1, -1),
            self.ao_at(c1, u, v, 1, -1),
            self.ao_at(c2, u, v, -1, 1),
            self.ao_at(c3, u, v, 1, 1),
        ]
    }
}

impl GreedyMesher {
    fn mesh_face_axis(
        &self,
        d: usize,
        s: i32,
        sampler: &SectorSampler<'_>,
        reg: &BlockRegistry,
        light: Option<&SectorLightView<'_>>,
        out: &mut MeshData,
    ) {
        let u = (d + 1) % 3;
        let v = (d + 2) % 3;
        let mut mask: [Option<BlockId>; (DIM * DIM) as usize] = [None; (DIM * DIM) as usize];

        // Only emit faces owned by an in-sector voxel. For +d faces the owning
        // voxel is `back` (d=n); skip n=-1 (out-of-sector / unloaded neighbor).
        // For -d faces the owner is `front` (d=n+1); skip n=31 (out-of-sector).
        let (n_start, n_end) = if s > 0 { (0, DIM) } else { (-1, DIM - 1) };
        let mut n = n_start;
        while n < n_end {
            // Build the face mask for the plane between layer n and n+1.
            for j in 0..DIM as usize {
                for i in 0..DIM as usize {
                    let mut back = [0i32; 3];
                    back[u] = i as i32;
                    back[v] = j as i32;
                    back[d] = n;
                    let mut front = back;
                    front[d] = n + 1;
                    let bblock = sampler.sample_block(back[0], back[1], back[2]);
                    let fblock = sampler.sample_block(front[0], front[1], front[2]);
                    let entry = if s > 0 {
                        if face_visible(bblock, fblock, reg) {
                            Some(bblock)
                        } else {
                            None
                        }
                    } else if face_visible(fblock, bblock, reg) {
                        Some(fblock)
                    } else {
                        None
                    };
                    mask[j * DIM as usize + i] = entry;
                }
            }

            // Pre-compute AO signatures for AO-safe greedy merge (0fps.net,
            // `block-mesh-bgm` ao_safe).  Only cells whose block type AND
            // 4-corner AO signature match can be merged into a single quad.
            // This prevents merging across AO boundaries where the per-vertex
            // occlusion differs, which would produce incorrect GPU interpolation.
            let mut ao_sig: [[u8; 4]; (DIM * DIM) as usize] = [[0; 4]; (DIM * DIM) as usize];
            for j in 0..DIM as usize {
                for i in 0..DIM as usize {
                    if mask[j * DIM as usize + i].is_some() {
                        // Use the owning voxel coordinate (sd), not the layer
                        // index (n). For -d faces the owner is at n+1, for +d
                        // faces at n — same as the `sd` computation below.
                        let sd = if s > 0 { n } else { n + 1 };
                        let mut base = [0i32; 3];
                        base[d] = sd;
                        base[u] = i as i32;
                        base[v] = j as i32;
                        ao_sig[j * DIM as usize + i] = sampler.ao_signature(base, d, u, v, s);
                    }
                }
            }

            let back_layer = n;
            for j in 0..DIM as usize {
                let mut i = 0;
                while i < DIM as usize {
                    let block = match mask[j * DIM as usize + i] {
                        Some(b) => b,
                        None => {
                            i += 1;
                            continue;
                        }
                    };

                    // Greedy width along u — block type AND AO signature must match.
                    let base_sig = ao_sig[j * DIM as usize + i];
                    let mut w = 1usize;
                    while i + w < DIM as usize
                        && mask[j * DIM as usize + i + w] == Some(block)
                        && ao_sig[j * DIM as usize + i + w] == base_sig
                    {
                        w += 1;
                    }
                    // Greedy height along v — same check for every cell in
                    // the candidate row across the full width.
                    let mut h = 1usize;
                    'outer: while j + h < DIM as usize {
                        for k in 0..w {
                            let idx = (j + h) * DIM as usize + i + k;
                            if mask[idx] != Some(block) || ao_sig[idx] != base_sig {
                                break 'outer;
                            }
                        }
                        h += 1;
                    }

                    // The face between layer `back_layer` (n) and `n+1` always sits
                    // on the plane d = n+1, regardless of normal direction — only
                    // the owning voxel and the normal differ. Using `back_layer`
                    // for +d faces placed the quad one voxel too low (inside the
                    // owning voxel), hiding top faces behind the block body.
                    // `sd` is the *owning voxel* coordinate (always 0..31), NOT the
                    // face plane. The face plane is `sd` for -d faces and `sd + 1`
                    // for +d faces. We pack the owning voxel (0..31) so the 5-bit
                    // position field never overflows at the sector boundary (where
                    // the +d plane would be 32 and wrap to 0, hiding the face).
                    // The vertex shader adds the +1 for +d faces in float space.
                    let sd = if s > 0 { back_layer } else { back_layer + 1 };
                    let mut base = [0i32; 3];
                    base[d] = sd;
                    base[u] = i as i32;
                    base[v] = j as i32;

                    let ao = sampler.compute_ao(base, d, u, v, s, w as i32, h as i32);
                    let face_idx = (if s > 0 { 2 * d } else { 2 * d + 1 }) as u8;
                    // 0fps.net anisotropy fix (also Exile, Andre Blunt "Quad
                    // Interpolation"): the GPU interpolates per-triangle, so
                    // when the four corner AOs are not coplanar the seam between
                    // the two triangles shifts based on diagonal choice. The
                    // 0fps rule `if a00 + a11 > a01 + a10 → flipped` keeps the
                    // dark corner pair on the same triangle. We decide once per
                    // quad on the CPU and store the result in `PackedQuad.flags`
                    // so the GPU pre-pass stays branchless (`select` on a single
                    // bit) — no divergent wavefront.
                    let flip_bit = if PackedQuad::needs_flip(ao) {
                        FLIP_FLAG
                    } else {
                        0
                    };
                    // Sample the 4 corners of this quad in the SectorLight
                    // array (if available) and pack (sky<<4 | block) into the
                    // 8-bit PackedQuad.light. The resolve shader indexes the
                    // lightmap SSBO with `quad_id` and applies the byte.
                    let light_byte = light
                        .map(|lv| {
                            let mk = |du: i32, dv: i32| {
                                let mut c = base;
                                c[u] += du;
                                c[v] += dv;
                                let x = c[0].clamp(0, 31) as u32;
                                let y = c[1].clamp(0, 31) as u32;
                                let z = c[2].clamp(0, 31) as u32;
                                lv.get(VoxelCoord::new(x, y, z))
                            };
                            let s0 = mk(0, 0);
                            let s1 = mk(w as i32, 0);
                            let s2 = mk(0, h as i32);
                            let s3 = mk(w as i32, h as i32);
                            let sky_avg =
                                ((s0.sky as u16 + s1.sky as u16 + s2.sky as u16 + s3.sky as u16)
                                    / 4) as u8;
                            let block_avg = ((s0.block as u16
                                + s1.block as u16
                                + s2.block as u16
                                + s3.block as u16)
                                / 4) as u8;
                            PackedQuad::pack_light(sky_avg, block_avg)
                        })
                        .unwrap_or(0);
                    let quad = PackedQuad::new(
                        base[0] as u32,
                        base[1] as u32,
                        base[2] as u32,
                        w as u32,
                        h as u32,
                        face_idx,
                        (block.0 & 0xFF) as u8,
                        PackedQuad::pack_ao(ao),
                        light_byte,
                        flip_bit,
                    );
                    if reg.is_transparent(block) {
                        out.transparent.push(quad);
                    } else {
                        out.opaque.push(quad);
                    }

                    for dj in 0..h {
                        for di in 0..w {
                            mask[(j + dj) * DIM as usize + i + di] = None;
                        }
                    }
                    i += w;
                }
            }

            n += 1;
        }
    }
}
