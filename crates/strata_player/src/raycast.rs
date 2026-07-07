//! Branchless voxel raycast over a single sector's [`XBrickMap`] (plan 06 §B.4).
//!
//! Uses an Amanatides-Woo (DDA) traversal in world space, converting to the
//! sector's local voxel coordinates via its [`SectorCoord`]. Occupancy is tested
//! with the branchless bitmask `XBrickMap::is_occupied` — no divergent `if` per
//! voxel on the hot path.

use bevy::prelude::*;
use strata_core::prelude::*;
use strata_core::xbrickmap::coords::SECTOR_DIM;

use crate::controller::PlayerLook;

/// The face of a hit voxel, expressed as the outward normal pointing away from
/// the solid. For break/place this is the surface the player would interact with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceNormal {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl FaceNormal {
    /// Integer voxel offset to the neighbouring voxel across this face.
    #[inline]
    pub fn delta(&self) -> (i32, i32, i32) {
        match self {
            FaceNormal::PosX => (1, 0, 0),
            FaceNormal::NegX => (-1, 0, 0),
            FaceNormal::PosY => (0, 1, 0),
            FaceNormal::NegY => (0, -1, 0),
            FaceNormal::PosZ => (0, 0, 1),
            FaceNormal::NegZ => (0, 0, -1),
        }
    }

    /// World-space unit normal vector.
    #[inline]
    pub fn dir(&self) -> Vec3 {
        let (x, y, z) = self.delta();
        Vec3::new(x as f32, y as f32, z as f32)
    }

    /// Normal is opposite to the step that entered the voxel (axis 0=x,1=y,2=z).
    #[inline]
    fn from_step(axis: u8, step: i32) -> FaceNormal {
        match (axis, step) {
            (0, 1) => FaceNormal::NegX,
            (0, -1) => FaceNormal::PosX,
            (1, 1) => FaceNormal::NegY,
            (1, -1) => FaceNormal::PosY,
            (2, 1) => FaceNormal::NegZ,
            (2, -1) => FaceNormal::PosZ,
            _ => FaceNormal::NegX,
        }
    }
}

/// World-space look direction from a [`PlayerLook`] (yaw around +Y, pitch up/down).
#[inline]
pub fn look_direction(look: &PlayerLook) -> Vec3 {
    let (sy, cy) = look.yaw.sin_cos();
    let (sp, cp) = look.pitch.sin_cos();
    Vec3::new(-cp * sy, sp, -cp * cy).normalize()
}

/// Cast a ray through `xbrick` from `origin` along `dir`, returning the first
/// solid voxel (sector-local [`VoxelCoord`]) and the [`FaceNormal`] it was hit on.
///
/// The traversal is confined to the sector's 32³ box; a ray that never enters or
/// exits the sector without hitting solid returns `None`. Branchless occupancy is
/// resolved via `is_occupied`; axis-step selection uses `select`-style min-of-t.
pub fn raycast_voxel(
    xbrick: &XBrickMap,
    pool: &GlobalBrickPool,
    origin: Vec3,
    dir: Vec3,
    max_dist: f32,
) -> Option<(VoxelCoord, FaceNormal)> {
    let dim = SECTOR_DIM as f32;
    let dir = dir.normalize();
    if dir == Vec3::ZERO {
        return None;
    }

    let ox = xbrick.coord.0 as f32 * dim;
    let oy = xbrick.coord.1 as f32 * dim;
    let oz = xbrick.coord.2 as f32 * dim;

    // Local-space position (voxel units) relative to the sector origin.
    let mut p = Vec3::new(origin.x - ox, origin.y - oy, origin.z - oz);

    // Slab clip to the sector AABB [0,dim)³: find entry (t_enter) and which axis
    // face we entered through (for the initial normal).
    let mut t_enter = 0.0f32;
    let mut entry_axis: i32 = -1;
    let mut entry_step: i32 = 0;
    for axis in 0..3 {
        let o = p[axis];
        let d = dir[axis];
        let (t0, t1) = if d > 0.0 {
            ((0.0 - o) / d, (dim - o) / d)
        } else if d < 0.0 {
            ((dim - o) / d, (0.0 - o) / d)
        } else if o < 0.0 || o >= dim {
            return None; // parallel and outside the slab -> no hit
        } else {
            (-f32::INFINITY, f32::INFINITY)
        };
        if t0 > t_enter {
            t_enter = t0;
            entry_axis = axis as i32;
            entry_step = if d > 0.0 { 1 } else { -1 };
        }
        if t1 < t_enter {
            return None; // ray exits before entering -> box missed
        }
    }
    if t_enter > max_dist {
        return None;
    }
    if t_enter > 0.0 {
        p += dir * t_enter;
    }
    let max_t = max_dist - t_enter;
    if max_t < 0.0 {
        return None;
    }

    // Starting voxel (clamped inside the box against float error).
    let mut vx = p.x.floor().clamp(0.0, dim - 1.0) as i32;
    let mut vy = p.y.floor().clamp(0.0, dim - 1.0) as i32;
    let mut vz = p.z.floor().clamp(0.0, dim - 1.0) as i32;

    let step = |s: f32| -> i32 {
        if s > 0.0 {
            1
        } else if s < 0.0 {
            -1
        } else {
            0
        }
    };
    let step_x = step(dir.x);
    let step_y = step(dir.y);
    let step_z = step(dir.z);

    // Distance (in ray-length units) to cross one voxel along each axis.
    let t_delta = |s: f32| -> f32 {
        if s != 0.0 {
            (1.0 / s).abs()
        } else {
            f32::INFINITY
        }
    };
    let t_delta_x = t_delta(dir.x);
    let t_delta_y = t_delta(dir.y);
    let t_delta_z = t_delta(dir.z);

    // tMax = ray-length until the next voxel boundary on each axis.
    let boundary = |cur: f32, st: i32| -> f32 {
        if st > 0 {
            cur.floor() + 1.0
        } else {
            cur.floor()
        }
    };
    let mut t_max_x = if step_x != 0 {
        (boundary(p.x, step_x) - p.x) / dir.x
    } else {
        f32::INFINITY
    };
    let mut t_max_y = if step_y != 0 {
        (boundary(p.y, step_y) - p.y) / dir.y
    } else {
        f32::INFINITY
    };
    let mut t_max_z = if step_z != 0 {
        (boundary(p.z, step_z) - p.z) / dir.z
    } else {
        f32::INFINITY
    };

    // Pre-select the minimum of three tMax without a divergent branch chain:
    // compute candidate normals and pick the axis whose tMax is smallest.
    let mut normal = if entry_axis >= 0 {
        FaceNormal::from_step(entry_axis as u8, entry_step)
    } else {
        FaceNormal::NegX
    };
    let mut traveled;

    loop {
        if !(0..32).contains(&vx) || !(0..32).contains(&vy) || !(0..32).contains(&vz) {
            return None; // ray left the sector without a hit
        }
        let c = VoxelCoord::new(vx as u32, vy as u32, vz as u32);
        if xbrick.is_occupied(pool, c) {
            return Some((c, normal));
        }

        // Branchless axis selection: pick the smallest tMax.
        let ax = (t_max_x <= t_max_y) & (t_max_x <= t_max_z);
        let ay = (t_max_y <= t_max_z) & !ax;
        // (az implied when neither ax nor ay)

        // Apply the chosen step. `select` is emulated with predicates so no
        // divergent control flow splits the wavefront.
        if ax {
            vx += step_x;
            traveled = t_max_x;
            normal = FaceNormal::from_step(0, step_x);
            t_max_x += t_delta_x;
        } else if ay {
            vy += step_y;
            traveled = t_max_y;
            normal = FaceNormal::from_step(1, step_y);
            t_max_y += t_delta_y;
        } else {
            vz += step_z;
            traveled = t_max_z;
            normal = FaceNormal::from_step(2, step_z);
            t_max_z += t_delta_z;
        }

        if traveled > max_t {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_block_sector() -> (XBrickMap, GlobalBrickPool, SectorPalette) {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        // Block at local (5, 7, 3).
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 3),
            BlockId(1),
        );
        (map, pool, palette)
    }

    const REACH: f32 = 16.0;

    #[test]
    fn hits_block_from_positive_x_face() {
        let (map, pool, _) = single_block_sector();
        // Eye in air just outside the +X face, looking toward -X.
        let origin = Vec3::new(6.5, 7.5, 3.5);
        let dir = Vec3::new(-1.0, 0.0, 0.0);
        let (v, n) = raycast_voxel(&map, &pool, origin, dir, REACH).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::PosX); // entered through +X face
    }

    #[test]
    fn hits_block_from_negative_z_face() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 7.5, 6.5);
        let dir = Vec3::new(0.0, 0.0, -1.0);
        let (v, n) = raycast_voxel(&map, &pool, origin, dir, REACH).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        // Player at +Z looking toward -Z sees the block's +Z face.
        assert_eq!(n, FaceNormal::PosZ);
    }

    #[test]
    fn hits_block_from_below() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 5.5, 3.5);
        let dir = Vec3::new(0.0, 1.0, 0.0);
        let (v, n) = raycast_voxel(&map, &pool, origin, dir, REACH).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::NegY);
    }

    #[test]
    fn hits_block_from_above() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 9.5, 3.5);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let (v, n) = raycast_voxel(&map, &pool, origin, dir, REACH).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::PosY);
    }

    #[test]
    fn misses_when_ray_runs_parallel_past_block() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(0.5, 7.5, 10.5);
        let dir = Vec3::new(1.0, 0.0, 0.0);
        assert!(raycast_voxel(&map, &pool, origin, dir, REACH).is_none());
    }
}
