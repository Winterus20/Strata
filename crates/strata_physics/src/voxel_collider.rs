//! Sector voxel colliders and character-controller scaffolding (M6).
//!
//! Each 32³ sector gets a single Rapier `Voxels` collider built from the world-space
//! centers of its solid voxels via [`Collider::voxels_from_points`]. Block edits are
//! mirrored into the live collider with `O(1)` [`Voxels::set_voxel`] calls instead of
//! a full rebuild.

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use strata_core::prelude::*;
use strata_core::xbrickmap::coords::SECTOR_DIM;
use strata_world::plugin::Generated;

/// Edge length (world units) of one voxel. Kept at `1.0` so that the Rapier
/// `Voxels` grid key of a local voxel `(lx, ly, lz)` equals `(lx, ly, lz)` when
/// sample centers are `(lx + 0.5, ly + 0.5, lz + 0.5)`.
pub const VOXEL_SIZE: f32 = 1.0;

/// Marker attached to a sector entity once its static voxel collider is built.
/// Prevents the build system from re-running (Filter-First: `Without<SectorCollider>`).
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct SectorCollider;

/// Shared Rapier kinematic character controller (M8 wires this to player input).
/// Created once by [`crate::plugin::PhysicsPlugin`].
#[derive(Resource, Default)]
pub struct CharacterController {
    pub controller: KinematicCharacterController,
}

/// World-space translation of a sector's origin (minimum corner of the 32³ grid).
#[inline]
pub fn sector_world_origin(coord: SectorCoord) -> Vec3 {
    Vec3::new(
        coord.0 as f32 * SECTOR_DIM as f32,
        coord.1 as f32 * SECTOR_DIM as f32,
        coord.2 as f32 * SECTOR_DIM as f32,
    ) * VOXEL_SIZE
}

/// Push the local-space centers of every solid voxel into `samples`, reusing the
/// caller's scratch `Vec` (heap-friendly; no per-sector voxel `Vec` is retained).
fn collect_solid_samples(
    map: &XBrickMap,
    pool: &GlobalBrickPool,
    palette: &SectorPalette,
    registry: &BlockRegistry,
    samples: &mut Vec<Vec3>,
) {
    samples.clear();
    for lx in 0..SECTOR_DIM {
        for ly in 0..SECTOR_DIM {
            for lz in 0..SECTOR_DIM {
                let coord = VoxelCoord::new(lx, ly, lz);
                let id = map.get_block(pool, palette, coord);
                if id != BlockId::AIR && registry.is_solid(id) {
                    samples.push(Vec3::new(
                        (lx as f32 + 0.5) * VOXEL_SIZE,
                        (ly as f32 + 0.5) * VOXEL_SIZE,
                        (lz as f32 + 0.5) * VOXEL_SIZE,
                    ));
                }
            }
        }
    }
}

/// Build a `Voxels` collider from world-space voxel centers (local to the sector).
#[inline]
pub fn build_voxels_collider(samples: &[Vec3]) -> Collider {
    Collider::voxels_from_points(Vect::splat(VOXEL_SIZE), samples)
}

/// Build a static `Voxels` collider for every freshly-generated sector that lacks
/// one. Filter-First: only `Generated` sectors `Without<SectorCollider>`.
#[allow(clippy::type_complexity)]
pub fn build_sector_colliders(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    pool: Res<GlobalBrickPool>,
    query: Query<
        (Entity, &SectorCoord, &XBrickMap, &SectorPalette),
        (With<Generated>, Without<SectorCollider>),
    >,
) {
    let mut samples: Vec<Vec3> = Vec::new();
    for (entity, coord, map, palette) in &query {
        collect_solid_samples(map, &pool, palette, &registry, &mut samples);
        if samples.is_empty() {
            continue;
        }
        let origin = sector_world_origin(*coord);
        commands
            .entity(entity)
            .insert(RigidBody::Fixed)
            .insert(build_voxels_collider(&samples))
            .insert(Transform::from_translation(origin))
            .insert(GlobalTransform::from_translation(origin))
            .insert(SectorCollider);
    }
}

/// Mirror `ChunkDirty` sectors from the `XBrickMap` into their live `Voxels` collider.
///
/// Runs in the `Physics` set. The collider is rebuilt voxel-by-voxel through the
/// `O(1)` [`Voxels::set_voxel`] path, so individual block edits stay cheap. The
/// `ChunkDirty` marker is intentionally *not* removed here — it is owned by its
/// authoritative consumer (meshing / block-change events), and physics only reflects
/// the current occupancy each time it is marked.
#[allow(clippy::type_complexity)]
pub fn sync_dirty_sector_colliders(
    pool: Res<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    mut query: Query<(&XBrickMap, &SectorPalette, &mut Collider), With<ChunkDirty>>,
) {
    for (map, palette, mut collider) in &mut query {
        let Some(mut voxels) = collider.as_voxels_mut() else {
            continue;
        };
        for lx in 0..SECTOR_DIM {
            for ly in 0..SECTOR_DIM {
                for lz in 0..SECTOR_DIM {
                    let coord = VoxelCoord::new(lx, ly, lz);
                    let id = map.get_block(&pool, palette, coord);
                    let occupied = id != BlockId::AIR && registry.is_solid(id);
                    let key = IVect::new(lx as i32, ly as i32, lz as i32);
                    voxels.set_voxel(key, occupied);
                }
            }
        }
    }
}

/// Apply a single voxel's occupancy to a sector `Voxels` collider (O(1) partial
/// rebuild). Used directly by block-edit callers that already know the changed
/// coordinate, avoiding a full 32³ rescan.
pub fn set_sector_voxel(collider: &mut Collider, local: VoxelCoord, occupied: bool) {
    if let Some(mut voxels) = collider.as_voxels_mut() {
        let key = IVect::new(local.x as i32, local.y as i32, local.z as i32);
        voxels.set_voxel(key, occupied);
    }
}

/// Spawn a kinematic (M8 player) capsule body wired to the character controller.
pub fn spawn_kinematic_player(commands: &mut Commands, position: Vec3) {
    commands.spawn((
        RigidBody::KinematicPositionBased,
        Collider::capsule_y(0.5, 0.5),
        KinematicCharacterController::default(),
        Transform::from_translation(position),
        GlobalTransform::from_translation(position),
    ));
}

/// Branchless CPU ground probe: is the voxel directly below `pos` occupied?
///
/// `pos` is a world position; the sector's own [`SectorCoord`] is used to derive the
/// local voxel coordinate, so out-of-sector probes safely report `false` (no Rapier
/// ray needed — pure bitmask `is_occupied`).
pub fn ground_below(xbrick: &XBrickMap, pool: &GlobalBrickPool, pos: Vec3) -> bool {
    let wx = pos.x.floor() as i64;
    let wy = pos.y.floor() as i64;
    let wz = pos.z.floor() as i64;
    if wy < 0 {
        return false;
    }
    let ox = xbrick.coord.0 as i64 * SECTOR_DIM as i64;
    let oy = xbrick.coord.1 as i64 * SECTOR_DIM as i64;
    let oz = xbrick.coord.2 as i64 * SECTOR_DIM as i64;
    let lx = wx - ox;
    let ly = wy - oy - 1;
    let lz = wz - oz;
    if lx < 0
        || lx >= SECTOR_DIM as i64
        || ly < 0
        || ly >= SECTOR_DIM as i64
        || lz < 0
        || lz >= SECTOR_DIM as i64
    {
        return false;
    }
    xbrick.is_occupied(pool, VoxelCoord::new(lx as u32, ly as u32, lz as u32))
}
