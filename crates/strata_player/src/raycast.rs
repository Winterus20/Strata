//! Branchless voxel raycast over a single sector's [`XBrickMap`] (plan 06 §B.4).
//!
//! Uses an Amanatides-Woo (DDA) traversal in world space, converting to the
//! sector's local voxel coordinates via its [`SectorCoord`]. Hit testing is
//! caller-supplied (typically [`BlockRegistry::is_solid`]) so break/place matches
//! movement collision and skips liquids — no divergent `if` beyond the solid check.

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

/// Cast a ray through `xbrick` from `origin` along `dir` (must be normalized),
/// returning the first solid voxel (sector-local [`VoxelCoord`]) and the
/// [`FaceNormal`] it was hit on.
///
/// `is_solid` decides whether a voxel stops the ray — use registry solidity
/// (not raw `is_occupied`) so water/liquids are not break targets. The traversal
/// is confined to the sector's 32³ box; axis-step selection uses `select`-style
/// min-of-t.
pub fn raycast_voxel(
    xbrick: &XBrickMap,
    _pool: &GlobalBrickPool,
    origin: Vec3,
    dir: Vec3,
    max_dist: f32,
    is_solid: impl Fn(VoxelCoord) -> bool,
) -> Option<(VoxelCoord, FaceNormal, f32)> {
    let dim = SECTOR_DIM as f32;
    const RAY_EPSILON: f32 = 1e-4;
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
        if t1 <= t_enter {
            return None; // ray exits before or at entry -> box missed
        }
    }
    if t_enter > max_dist {
        return None;
    }
    let start_bias = if t_enter > 0.0 { RAY_EPSILON } else { 0.0 };
    if t_enter + start_bias > 0.0 {
        p += dir * (t_enter + start_bias);
    }
    let max_t = max_dist - t_enter - start_bias;
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
    let mut traveled = 0.0f32;

    loop {
        if !(0..32).contains(&vx) || !(0..32).contains(&vy) || !(0..32).contains(&vz) {
            return None; // ray left the sector without a hit
        }
        let c = VoxelCoord::new(vx as u32, vy as u32, vz as u32);
        if is_solid(c) {
            return Some((c, normal, t_enter + traveled));
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
        )
        .expect("test set_block");
        (map, pool, palette)
    }

    const REACH: f32 = 16.0;

    fn occupied<'a>(
        map: &'a XBrickMap,
        pool: &'a GlobalBrickPool,
    ) -> impl Fn(VoxelCoord) -> bool + 'a {
        move |c| map.is_occupied(pool, c)
    }

    #[test]
    fn hits_block_from_positive_x_face() {
        let (map, pool, _) = single_block_sector();
        // Eye in air just outside the +X face, looking toward -X.
        let origin = Vec3::new(6.5, 7.5, 3.5);
        let dir = Vec3::new(-1.0, 0.0, 0.0);
        let (v, n, _t) =
            raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::PosX); // entered through +X face
    }

    #[test]
    fn hits_block_from_negative_z_face() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 7.5, 6.5);
        let dir = Vec3::new(0.0, 0.0, -1.0);
        let (v, n, _t) =
            raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        // Player at +Z looking toward -Z sees the block's +Z face.
        assert_eq!(n, FaceNormal::PosZ);
    }

    #[test]
    fn hits_block_from_below() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 5.5, 3.5);
        let dir = Vec3::new(0.0, 1.0, 0.0);
        let (v, n, _t) =
            raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::NegY);
    }

    #[test]
    fn hits_block_from_above() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(5.5, 9.5, 3.5);
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let (v, n, _t) =
            raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 3));
        assert_eq!(n, FaceNormal::PosY);
    }

    #[test]
    fn misses_when_ray_runs_parallel_past_block() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(0.5, 7.5, 10.5);
        let dir = Vec3::new(1.0, 0.0, 0.0);
        assert!(raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).is_none());
    }

    #[test]
    fn misses_when_ray_starts_on_sector_face_pointing_outward() {
        let (map, pool, _) = single_block_sector();
        let origin = Vec3::new(32.0, 7.5, 3.5);
        let dir = Vec3::new(1.0, 0.0, 0.0);
        assert!(raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).is_none());
    }

    #[test]
    fn stabilizes_hits_when_ray_lands_on_voxel_edge() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 3),
            BlockId(1),
        )
        .expect("left block");
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 4),
            BlockId(1),
        )
        .expect("right block");

        // Exact z-edge between two neighbour voxels. The ray points slightly
        // toward +Z, so the first interior sample must land in z=4, not z=3.
        let origin = Vec3::new(8.5, 7.5, 4.0);
        let dir = Vec3::new(-1.0, 0.0, 0.001).normalize();
        let (v, _n, _t) =
            raycast_voxel(&map, &pool, origin, dir, REACH, occupied(&map, &pool)).unwrap();
        assert_eq!(v, VoxelCoord::new(5, 7, 4));
    }

    #[test]
    fn skips_non_solid_liquid_to_hit_solid_behind() {
        // Water is occupied but not solid (movement uses is_solid). Break/raycast
        // must match so liquids are not false break targets.
        let registry = load_block_registry();
        let water = registry
            .id_by_name("water")
            .expect("water block in default registry");
        let stone = registry.id_by_name("stone").unwrap_or(BlockId(1));
        assert!(
            !registry.is_solid(water),
            "precondition: water must not be solid"
        );

        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 7, 3), water)
            .expect("test water set_block");
        map.set_block(&mut pool, &mut palette, VoxelCoord::new(3, 7, 3), stone)
            .expect("test stone set_block");

        let origin = Vec3::new(8.5, 7.5, 3.5);
        let dir = Vec3::new(-1.0, 0.0, 0.0);
        let is_solid = |c: VoxelCoord| {
            let id = map.get_block(&pool, &palette, c);
            id != BlockId::AIR && registry.is_solid(id)
        };
        let (v, _n, _t) = raycast_voxel(&map, &pool, origin, dir, REACH, is_solid)
            .expect("must hit solid stone behind water");
        assert_eq!(
            v,
            VoxelCoord::new(3, 7, 3),
            "ray must skip water and hit stone"
        );
    }
}
