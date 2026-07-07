//! Binary greedy mesher (plan 09 §4): per-axis face masks, greedy coplanar
//! merge, neighbor-aware culling, and branchless per-vertex AO.

use crate::meshing::occupancy::OccupancyScratch;
use crate::meshing::packed_quad::PackedQuad;
use crate::meshing::{MeshData, Mesher, NeighborView};
use strata_core::prelude::*;

const DIM: i32 = 32;

/// Conservative-solid sentinel used for unloaded (`None`) neighbors (plan 09 §4.0).
#[derive(Clone, Copy)]
pub struct GreedyMesher {
    unloaded: BlockId,
}

impl GreedyMesher {
    pub fn new(registry: &BlockRegistry) -> Self {
        let unloaded = (0..registry.count())
            .map(|i| BlockId(i as u16))
            .find(|b| registry.is_solid(*b))
            .unwrap_or(BlockId::AIR);
        Self { unloaded }
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
        let mut scratch = OccupancyScratch::new();
        fill_scratch_inline(&mut scratch, sector, pool);

        let sampler = SectorSampler {
            scratch: &scratch,
            sector,
            palette,
            pool,
            unloaded: self.unloaded,
            neighbors,
        };

        let mut mesh = MeshData::default();
        for d in 0..3 {
            self.mesh_face_axis(d, 1, &sampler, registry, &mut mesh);
            self.mesh_face_axis(d, -1, &sampler, registry, &mut mesh);
        }
        mesh
    }
}

#[inline]
fn fill_scratch_inline(scratch: &mut OccupancyScratch, sector: &XBrickMap, pool: &GlobalBrickPool) {
    scratch.clear();
    for x in 0..DIM as u32 {
        for y in 0..DIM as u32 {
            for z in 0..DIM as u32 {
                if sector.is_occupied(pool, VoxelCoord::new(x, y, z)) {
                    scratch.set(x, y, z);
                }
            }
        }
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
    scratch: &'a OccupancyScratch,
    sector: &'a XBrickMap,
    palette: &'a SectorPalette,
    pool: &'a GlobalBrickPool,
    unloaded: BlockId,
    neighbors: &'a [NeighborView<'a>; 6],
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
            return self.sector.get_block(
                self.pool,
                self.palette,
                VoxelCoord::new(x as u32, y as u32, z as u32),
            );
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
        let nv = &self.neighbors[idx];
        match nv.sector {
            Some(sector) => {
                let pal = nv.palette.expect("neighbor sector present without palette");
                sector.get_block(
                    nv.pool,
                    pal,
                    VoxelCoord::new(nx as u32, ny as u32, nz as u32),
                )
            }
            None => self.unloaded,
        }
    }

    #[inline]
    fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        if Self::in_range(x, y, z) {
            self.scratch.is_occupied(x as u32, y as u32, z as u32)
        } else {
            self.sample_block(x, y, z) != BlockId::AIR
        }
    }

    /// Branchless per-vertex AO (0=occluded .. 3=open) from the 3 occluder
    /// voxels adjacent to the solid voxel `base` toward `(du,dv)` tangent dirs.
    #[inline]
    fn ao_at(&self, base: [i32; 3], u: usize, v: usize, du: i32, dv: i32) -> u8 {
        let mut c1 = base;
        c1[u] += du;
        let mut c2 = base;
        c2[v] += dv;
        let s1 = self.solid(c1[0], c1[1], c1[2]) as u8;
        let s2 = self.solid(c2[0], c2[1], c2[2]) as u8;
        // 3 - (s1 + s2 + (s1 & s2)) — fully arithmetic, no wavefront-branching.
        3u8 - (s1 + s2 + (s1 & s2))
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
                    eprintln!(
                        "  PUSH d={d} s={s} n={back_layer} i={i} j={j} base={:?} w={w} h={h} face={face_idx}",
                        base
                    );

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
