//! Sector voxel colliders and character-controller scaffolding (M6).
//!
//! Each 32³ sector gets a single Rapier `Voxels` collider built from the world-space
//! centers of its solid voxels via [`Collider::voxels_from_points`]. Block edits are
//! mirrored into the live collider with `O(1)` [`Voxels::set_voxel`] calls instead of
//! a full rebuild.
//!
//! Fresh sector builds run on a dedicated background pool (same pattern as meshing):
//! the main thread only snapshots solid-voxel centers (~0.2 ms); Rapier
//! `voxels_from_points` (~4 ms for dense terrain) never blocks the frame loop.

use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use strata_core::prelude::*;
use strata_core::xbrickmap::coords::SECTOR_DIM;
use strata_world::plugin::Generated;
use strata_world::streaming::{StreamingManager, StreamingTimers, load_priority};

const COLLIDER_SPAWN_BUDGET: usize = 1;
/// One Rapier collider insert per frame — `init_colliders` in PostUpdate is costly.
const COLLIDER_APPLY_BUDGET: usize = 1;

/// Per-frame timings for sector collider work (surfaced to client DIAG).
/// `build_*` = fresh `Generated` sectors; `sync_*` = `ChunkDirty` edits.
#[derive(Resource, Default)]
pub struct PhysicsTimers {
    pub build_us: u64,
    pub sort_us: u64,
    pub queue_us: u64,
    pub built: usize,
    /// Voxel sample collection (bitmask walk, main thread).
    pub collect_us: u64,
    /// Rapier `voxels_from_points` (background workers).
    pub rapier_us: u64,
    /// Main-thread ECS insert of a finished collider (commands queue).
    pub apply_us: u64,
    pub sync_us: u64,
    pub synced: usize,
    pub pending: usize,
}

/// Edge length (world units) of one voxel. Kept at `1.0` so that the Rapier
/// `Voxels` grid key of a local voxel `(lx, ly, lz)` equals `(lx, ly, lz)` when
/// sample centers are `(lx + 0.5, ly + 0.5, lz + 0.5)`.
pub const VOXEL_SIZE: f32 = 1.0;

/// Marker attached to a sector entity once its static voxel collider is built.
/// Prevents the build system from re-running (Filter-First: `Without<SectorCollider>`).
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct SectorCollider;

/// In-flight async collider build. Removed when the collider is applied or the
/// sector is unloaded.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct BuildingSectorCollider;

/// Shared Rapier kinematic character controller (M8 wires this to player input).
/// Created once by [`crate::plugin::PhysicsPlugin`].
#[derive(Resource, Default)]
pub struct CharacterController {
    pub controller: KinematicCharacterController,
}

pub struct VoxelColliderRequest {
    pub entity: Entity,
    pub coord: SectorCoord,
    pub origin: Vec3,
    pub samples: Vec<Vec3>,
}

pub struct VoxelColliderResponse {
    pub entity: Entity,
    pub coord: SectorCoord,
    pub origin: Vec3,
    pub collider: Collider,
    pub rapier_us: u64,
}

#[derive(Resource)]
pub struct PhysicsWorkerChannels {
    pub tx_request: Sender<VoxelColliderRequest>,
    pub rx_response: std::sync::Mutex<Receiver<VoxelColliderResponse>>,
}

struct PendingColliderTask {
    entity: Entity,
    origin: Vec3,
}

/// In-flight async collider builds, keyed by sector coordinate.
#[derive(Resource, Default)]
pub struct PendingCollider {
    tasks: HashMap<SectorCoord, PendingColliderTask>,
}

impl PendingCollider {
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
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
/// caller's scratch `Vec`. Walks only occupied bricks/sub-bricks/voxels via the
/// sector bitmask — never scans all 32³ cells.
fn collect_solid_samples(
    map: &XBrickMap,
    pool: &InnerPool,
    palette: &SectorPalette,
    registry: &BlockRegistry,
    samples: &mut Vec<Vec3>,
) {
    samples.clear();
    let mut sector_mask = map.sector_mask;
    while sector_mask != 0 {
        let bi = sector_mask.trailing_zeros() as usize;
        sector_mask &= sector_mask - 1;
        let Some(handle) = map.brick_handle_at(bi) else {
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
                if id != BlockId::AIR && registry.is_solid(id) {
                    let lx = sub_base_x + (vb as u32 & 1);
                    let ly = sub_base_y + ((vb as u32 >> 2) & 1);
                    let lz = sub_base_z + ((vb as u32 >> 1) & 1);
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

fn queue_collider_build(
    commands: &mut Commands,
    channels: &PhysicsWorkerChannels,
    pending: &mut PendingCollider,
    entity: Entity,
    coord: SectorCoord,
    samples: Vec<Vec3>,
) {
    let origin = sector_world_origin(coord);

    let t_send = std::time::Instant::now();
    let _ = channels.tx_request.send(VoxelColliderRequest {
        entity,
        coord,
        origin,
        samples,
    });
    let d_send = t_send.elapsed().as_micros();

    let t_insert = std::time::Instant::now();
    pending
        .tasks
        .insert(coord, PendingColliderTask { entity, origin });
    let d_insert = t_insert.elapsed().as_micros();

    let t_cmd = std::time::Instant::now();
    commands.entity(entity).insert(BuildingSectorCollider);
    let d_cmd = t_cmd.elapsed().as_micros();

    trace!(
        "QUEUE_COLLIDER_BUILD_DIAG coord={:?} send={} us, insert={} us, cmd={} us",
        coord, d_send, d_insert, d_cmd
    );
}

/// Snapshot solid voxels and dispatch Rapier builds to the physics worker pool.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn spawn_sector_collider_tasks(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    pool: Res<GlobalBrickPool>,
    channels: Res<PhysicsWorkerChannels>,
    streaming: Option<Res<StreamingManager>>,
    stream_timers: Option<Res<StreamingTimers>>,
    mut pending: ResMut<PendingCollider>,
    mut timers: ResMut<PhysicsTimers>,
    new_sectors: Query<
        (Entity, &SectorCoord, &XBrickMap, &SectorPalette),
        (
            With<Generated>,
            Without<SectorCollider>,
            Without<BuildingSectorCollider>,
        ),
    >,
    revived: Query<
        (Entity, &SectorCoord, &XBrickMap, &SectorPalette),
        (
            With<ChunkDirty>,
            With<SectorCollider>,
            Without<Collider>,
            Without<BuildingSectorCollider>,
        ),
    >,
) {
    timers.build_us = 0;
    timers.sort_us = 0;
    timers.queue_us = 0;
    timers.collect_us = 0;
    timers.pending = pending.len();
    if new_sectors.is_empty() && revived.is_empty() {
        return;
    }

    // Spread collider queue work away from the same frame as sector unloads.
    if stream_timers.is_some_and(|t| t.unloaded > 0) {
        return;
    }

    let max_inflight = 3; // 1 background worker thread + 2 tasks buffer limit
    if pending.len() >= max_inflight {
        return;
    }

    let t0 = std::time::Instant::now();
    let player = streaming
        .as_ref()
        .map(|s| s.player_sector)
        .unwrap_or(SectorCoord(0, 0, 0));
    let move_dir = streaming
        .as_ref()
        .map(|s| s.move_dir)
        .unwrap_or(SectorCoord(0, 0, 0));

    let t_sort = std::time::Instant::now();
    let mut work: Vec<_> = new_sectors.iter().chain(revived.iter()).collect();
    work.sort_by_key(|(_, c, _, _)| load_priority(player, move_dir, **c));
    timers.sort_us = t_sort.elapsed().as_micros() as u64;

    let mut samples: Vec<Vec3> = Vec::new();
    let mut budget = COLLIDER_SPAWN_BUDGET;
    timers.queue_us = 0;
    for (entity, coord, map, palette) in work {
        if budget == 0 {
            break;
        }
        if pending.tasks.contains_key(coord) {
            continue;
        }
        let tc = std::time::Instant::now();
        let owned = {
            let pool_guard = pool.read_inner();
            collect_solid_samples(map, &pool_guard, palette, &registry, &mut samples);
            std::mem::take(&mut samples)
        };
        timers.collect_us += tc.elapsed().as_micros() as u64;
        if owned.is_empty() {
            let t_q = std::time::Instant::now();
            commands.entity(entity).insert(SectorCollider);
            timers.queue_us += t_q.elapsed().as_micros() as u64;
            budget -= 1;
            continue;
        }
        let t_q = std::time::Instant::now();
        queue_collider_build(
            &mut commands,
            &channels,
            &mut pending,
            entity,
            *coord,
            owned,
        );
        timers.queue_us += t_q.elapsed().as_micros() as u64;
        budget -= 1;
    }
    timers.build_us = t0.elapsed().as_micros() as u64;
    timers.pending = pending.len();
}

/// Apply completed background collider builds on the main thread.
pub fn apply_sector_collider_tasks(
    mut commands: Commands,
    mut pending: ResMut<PendingCollider>,
    channels: Res<PhysicsWorkerChannels>,
    mut timers: ResMut<PhysicsTimers>,
    entities: Query<Entity, With<SectorCoord>>,
) {
    timers.apply_us = 0;
    timers.rapier_us = 0;
    timers.built = 0;

    let mut apply_budget = COLLIDER_APPLY_BUDGET;
    while apply_budget > 0 {
        let Ok(res) = channels.rx_response.lock().unwrap().try_recv() else {
            break;
        };

        // Stale / cleaned: require entity match so an old response for a recycled
        // sector coord cannot consume the newer pending task (or attach the wrong
        // collider). Leave pending in place so the matching response can apply.
        let Some(pt) = pending.tasks.get(&res.coord) else {
            continue;
        };
        if pt.entity != res.entity {
            continue;
        }
        let pt = pending.tasks.remove(&res.coord).unwrap();

        apply_budget -= 1;
        timers.built += 1;
        timers.rapier_us += res.rapier_us;

        if entities.get(pt.entity).is_err() {
            continue;
        }

        let ta = std::time::Instant::now();
        commands
            .entity(pt.entity)
            .insert(RigidBody::Fixed)
            .insert(res.collider)
            .insert(Transform::from_translation(pt.origin))
            .insert(GlobalTransform::from_translation(pt.origin))
            .insert(SectorCollider)
            .remove::<BuildingSectorCollider>();
        timers.apply_us += ta.elapsed().as_micros() as u64;
    }
    timers.pending = pending.len();
}

/// Drop pending collider tasks for sectors that were unloaded.
pub fn cleanup_pending_colliders(
    mut pending: ResMut<PendingCollider>,
    streaming: Option<Res<StreamingManager>>,
) {
    let Some(sm) = streaming else { return };
    pending.tasks.retain(|c, _| sm.is_resident(c));
}

/// Mirror `ChunkDirty` sectors from the `XBrickMap` into their live `Voxels` collider.
///
/// Runs in the `Physics` set. The collider is rebuilt voxel-by-voxel through the
/// `O(1)` [`Voxels::set_voxel`] path, so individual block edits stay cheap. The
/// `ChunkDirty` marker is intentionally *not* removed here — it is owned by its
/// authoritative consumer (meshing / block-change events), and physics only reflects
/// the current occupancy each time it is marked.
///
/// Sectors that were processed as empty (and so carry a `SectorCollider` but no
/// `Collider`) are handled by [`spawn_sector_collider_tasks`] once an edit makes
/// them non-empty.
#[allow(clippy::type_complexity)]
pub fn sync_dirty_sector_colliders(
    pool: Res<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    mut timers: ResMut<PhysicsTimers>,
    mut existing: Query<(&XBrickMap, &SectorPalette, &mut Collider), With<ChunkDirty>>,
) {
    timers.sync_us = 0;
    timers.synced = 0;
    if existing.is_empty() {
        return;
    }
    let t0 = std::time::Instant::now();
    let pool_guard = pool.read_inner();
    for (map, palette, mut collider) in &mut existing {
        let Some(mut voxels) = collider.as_voxels_mut() else {
            continue;
        };
        for bi in 0..64 {
            let bx = (bi % 4) as u32 * 8;
            let by = (bi / 16) as u32 * 8;
            let bz = ((bi % 16) / 4) as u32 * 8;

            let handle = match map.brick_handle_at(bi) {
                Some(h) => h,
                None => {
                    for lx in 0..8 {
                        for ly in 0..8 {
                            for lz in 0..8 {
                                let key = IVect::new(
                                    (bx + lx) as i32,
                                    (by + ly) as i32,
                                    (bz + lz) as i32,
                                );
                                voxels.set_voxel(key, false);
                            }
                        }
                    }
                    continue;
                }
            };

            let brick = match pool_guard.bricks.get(handle) {
                Some(b) => b,
                None => {
                    for lx in 0..8 {
                        for ly in 0..8 {
                            for lz in 0..8 {
                                let key = IVect::new(
                                    (bx + lx) as i32,
                                    (by + ly) as i32,
                                    (bz + lz) as i32,
                                );
                                voxels.set_voxel(key, false);
                            }
                        }
                    }
                    continue;
                }
            };

            for si in 0..64 {
                let sx = (si % 4) as u32 * 2;
                let sy = (si / 16) as u32 * 2;
                let sz = ((si % 16) / 4) as u32 * 2;

                if (brick.sub_mask >> si) & 1 == 0 {
                    for lx in 0..2 {
                        for ly in 0..2 {
                            for lz in 0..2 {
                                let key = IVect::new(
                                    (bx + sx + lx) as i32,
                                    (by + sy + ly) as i32,
                                    (bz + sz + lz) as i32,
                                );
                                voxels.set_voxel(key, false);
                            }
                        }
                    }
                    continue;
                }

                let sub = &brick.subs[si];
                for vb in 0..8 {
                    let lx = bx + sx + (vb as u32 & 1);
                    let ly = by + sy + ((vb as u32 >> 2) & 1);
                    let lz = bz + sz + ((vb as u32 >> 1) & 1);
                    let key = IVect::new(lx as i32, ly as i32, lz as i32);

                    let occupied = if (sub.voxel_mask >> vb) & 1 != 0 {
                        let id = palette.resolve(sub.indices[vb]);
                        id != BlockId::AIR && registry.is_solid(id)
                    } else {
                        false
                    };
                    voxels.set_voxel(key, occupied);
                }
            }
        }
        timers.synced += 1;
    }
    timers.sync_us = t0.elapsed().as_micros() as u64;
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

#[cfg(test)]
mod entity_match_tests {
    use super::*;
    use bevy::ecs::system::SystemState;
    use std::sync::mpsc;

    #[test]
    fn stale_collider_response_skips_entity_mismatch() {
        let mut app = App::new();
        app.init_resource::<PhysicsTimers>();
        app.init_resource::<PendingCollider>();

        let (tx_req, _rx_req) = mpsc::channel::<VoxelColliderRequest>();
        let (tx_res, rx_res) = mpsc::channel::<VoxelColliderResponse>();
        app.insert_resource(PhysicsWorkerChannels {
            tx_request: tx_req,
            rx_response: std::sync::Mutex::new(rx_res),
        });

        let coord = SectorCoord(0, 0, 0);
        let live = app.world_mut().spawn(coord).id();
        let stale = Entity::from_bits(0xDEAD_BEEF);

        app.world_mut()
            .resource_mut::<PendingCollider>()
            .tasks
            .insert(
                coord,
                PendingColliderTask {
                    entity: live,
                    origin: Vec3::ZERO,
                },
            );

        // Old worker result for a previous entity at the same sector coord.
        tx_res
            .send(VoxelColliderResponse {
                entity: stale,
                coord,
                origin: Vec3::ZERO,
                collider: Collider::ball(0.1),
                rapier_us: 1,
            })
            .unwrap();

        {
            let mut state: SystemState<(
                Commands,
                ResMut<PendingCollider>,
                Res<PhysicsWorkerChannels>,
                ResMut<PhysicsTimers>,
                Query<Entity, With<SectorCoord>>,
            )> = SystemState::new(app.world_mut());
            let (commands, pending, channels, timers, entities) = state.get_mut(app.world_mut());
            apply_sector_collider_tasks(commands, pending, channels, timers, entities);
            state.apply(app.world_mut());
        }

        let pending = app.world().resource::<PendingCollider>();
        assert!(
            pending.tasks.contains_key(&coord),
            "pending task for live entity must survive a stale response"
        );
        assert_eq!(
            pending.tasks.get(&coord).unwrap().entity,
            live,
            "pending entity must remain the live sector"
        );
        assert!(
            app.world().get::<SectorCollider>(live).is_none(),
            "stale response must not insert SectorCollider on the live entity"
        );
    }
}
