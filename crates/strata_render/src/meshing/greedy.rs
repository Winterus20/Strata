//! Binary greedy mesher (plan 09 §4): per-axis face masks, greedy coplanar
//! merge, neighbor-aware culling, and branchless per-vertex AO.

use crate::meshing::packed_quad::PackedQuad;
use crate::meshing::{MeshData, Mesher, NeighborView};
use strata_core::prelude::*;

const DIM: i32 = 32;
/// Number of voxels in a 32³ sector; the size of one owned mesh snapshot.
pub const SNAPSHOT_LEN: usize = 32 * 32 * 32;

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
    for z in 0..32u32 {
        for y in 0..32u32 {
            for x in 0..32u32 {
                out[snap_index(x, y, z)] =
                    sector.get_block_locked(pool, palette, VoxelCoord::new(x, y, z));
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
    let sample =
        |x: u32, y: u32, z: u32| sector.get_block_locked(pool, palette, VoxelCoord::new(x, y, z));
    // The fixed primary-axis coordinate is the neighbor edge facing the owning
    // sector: +X neighbor -> its x=0 face, -X neighbor -> its x=31 face, etc.
    match idx {
        0 => {
            for ny in 0..32u32 {
                for nz in 0..32u32 {
                    out[(ny * 32 + nz) as usize] = sample(0, ny, nz);
                }
            }
        }
        1 => {
            for ny in 0..32u32 {
                for nz in 0..32u32 {
                    out[(ny * 32 + nz) as usize] = sample(31, ny, nz);
                }
            }
        }
        2 => {
            for nx in 0..32u32 {
                for nz in 0..32u32 {
                    out[(nx * 32 + nz) as usize] = sample(nx, 0, nz);
                }
            }
        }
        3 => {
            for nx in 0..32u32 {
                for nz in 0..32u32 {
                    out[(nx * 32 + nz) as usize] = sample(nx, 31, nz);
                }
            }
        }
        4 => {
            for nx in 0..32u32 {
                for ny in 0..32u32 {
                    out[(nx * 32 + ny) as usize] = sample(nx, ny, 0);
                }
            }
        }
        _ => {
            for nx in 0..32u32 {
                for ny in 0..32u32 {
                    out[(nx * 32 + ny) as usize] = sample(nx, ny, 31);
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
    pub fn mesh_sector_planes(
        &self,
        self_vox: &[BlockId; SNAPSHOT_LEN],
        neighbors: &[Option<&[BlockId; PLANE_LEN]>; 6],
        registry: &BlockRegistry,
    ) -> MeshData {
        let sampler = SectorSampler {
            self_vox,
            neighbors,
            unloaded: self.unloaded,
        };
        let mut mesh = MeshData::default();
        for d in 0..3 {
            self.mesh_face_axis(d, 1, &sampler, registry, &mut mesh);
            self.mesh_face_axis(d, -1, &sampler, registry, &mut mesh);
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
        // (plan 09 §4.0), so build that (2 KB) instead of a 64 KB snapshot. Each
        // neighbor may live in its own pool (test setups), so take that pool's
        // guard per neighbor and never nest read locks on the same thread.
        let mut planes: [Option<Box<BoundaryPlane>>; 6] = [const { None }; 6];
        for i in 0..6 {
            if let Some(nsec) = neighbors[i].sector {
                let npal = neighbors[i]
                    .palette
                    .expect("neighbor sector present without palette");
                let g = neighbors[i].pool.read_inner();
                planes[i] = Some(build_boundary_plane_locked(nsec, &g, npal, i));
            }
        }
        let mut plane_refs: [Option<&BoundaryPlane>; 6] = [const { None }; 6];
        for i in 0..6 {
            plane_refs[i] = planes[i].as_deref();
        }
        self.mesh_sector_planes(&self_vox, &plane_refs, registry)
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

    /// AO for the 4 quad corners (min/min, max/min, min/max, max/max).
    #[inline]
    fn compute_ao(&self, base: [i32; 3], u: usize, v: usize) -> [u8; 4] {
        [
            self.ao_at(base, u, v, -1, -1),
            self.ao_at(base, u, v, 1, -1),
            self.ao_at(base, u, v, -1, 1),
            self.ao_at(base, u, v, 1, 1),
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

                    // Greedy width along u.
                    let mut w = 1usize;
                    while i + w < DIM as usize && mask[j * DIM as usize + i + w] == Some(block) {
                        w += 1;
                    }
                    // Greedy height along v.
                    let mut h = 1usize;
                    'outer: while j + h < DIM as usize {
                        for k in 0..w {
                            if mask[(j + h) * DIM as usize + i + k] != Some(block) {
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

                    let ao = sampler.compute_ao(base, u, v);
                    let face_idx = (if s > 0 { 2 * d } else { 2 * d + 1 }) as u8;
                    let quad = PackedQuad::new(
                        base[0] as u32,
                        base[1] as u32,
                        base[2] as u32,
                        w as u32,
                        h as u32,
                        face_idx,
                        block.0 as u8,
                        PackedQuad::pack_ao(ao),
                        0,
                        0,
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
