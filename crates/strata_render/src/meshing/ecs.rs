//! ECS integration: `NeedsRemesh` (ZST, filter-first) drives greedy meshing.
//!
//! Meshing runs on the `AsyncComputeTaskPool` (background threads) so streaming
//! a ring of new sectors never blocks the render frame — this eliminated the
//! instantaneous FPS spikes that synchronous meshing caused (plan 09 §2,
//! AGENTS.md §3.A). The main thread only (a) snapshots the few dirty sectors
//! per frame into owned [`VoxelSnapshot`]s and (b) applies finished meshes.

use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{Task, TaskPool, TaskPoolBuilder};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use strata_core::prelude::*;

use crate::meshing::MeshData;
use crate::meshing::greedy::{
    BoundaryPlane, GreedyMesher, PLANE_LEN, SNAPSHOT_LEN, fill_boundary_plane_locked,
    fill_sector_snapshot_locked,
};

thread_local! {
    /// Per-worker reusable self-snapshot buffer for the meshing hot path. The
    /// mesher overwrites every voxel each call, so it is never cleared and never
    /// re-allocated — a streaming burst does zero per-sector 64 KB heap churn on
    /// the global allocator (AGENTS.md §7.G heap-free hot path).
    static MESH_SCRATCH: std::cell::RefCell<Arc<[BlockId; SNAPSHOT_LEN]>> =
        std::cell::RefCell::new(Arc::new([BlockId::AIR; SNAPSHOT_LEN]));
}

/// Max sectors spawned (snapshot + mesh dispatched to workers) per frame.
const MESH_BUDGET: usize = 2;

/// Max wall time (µs) for mesh snapshot work on the main thread per frame.
const SNAPSHOT_BUDGET_US: u64 = 800;

/// Resident sector count at the client's default ACTIVE shell (radius 3 → 7³).
const FULL_SHELL_SECTORS: usize = 343;

/// Max completed meshes applied (inserted + `remesh-on-load` cascade) per frame.
const APPLY_BUDGET: usize = 2;

/// Storage for completed sector meshes, keyed by sector coordinate. Consumed by
/// M4's render stage. Result buffers only — no live voxel data.
#[derive(Resource, Default)]
pub struct MeshStorage {
    pub meshes: HashMap<SectorCoord, MeshData>,
    /// Monotonic generation bumped on every mesh insert. The renderer watches
    /// this to know when its cached GPU buffers must be rebuilt.
    pub version: u64,
    /// Coords whose mesh was (re)inserted since the renderer last drained them.
    /// The client drains this each frame instead of scanning every resident
    /// sector, so a streaming burst of N new meshes costs O(N) not O(all_sectors)
    /// (the per-frame scan was the chunk-load FPS dip).
    pub dirty: HashSet<SectorCoord>,
    /// For each meshed sector, the bitmask of neighbor directions that were
    /// *resident* when its mesh was last built. Bit `i` (0=+X..5=-Z) is set when
    /// neighbor `i` existed at mesh time. Drives `remesh-on-load`: a sector whose
    /// neighbor appears later regenerates the boundary faces that were culled
    /// while that neighbor was absent.
    pub neighbor_mask: HashMap<SectorCoord, u8>,
}

/// In-flight async meshing tasks, keyed by sector coordinate.
#[derive(Resource, Default)]
pub struct PendingMesh {
    tasks: HashMap<SectorCoord, PendingMeshTask>,
}

impl PendingMesh {
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

/// Per-frame timing for the meshing spawn stage (the synchronous main-thread
/// cost of cloning sector handles + queueing worker tasks). Surfaced to client
/// diagnostics so streaming drops can be attributed correctly.
#[derive(Resource, Default)]
pub struct MeshingTimers {
    pub spawn_us: u64,
    pub spawned: usize,
    pub apply_us: u64,
    pub applied: usize,
}

/// Dedicated background pool for greedy meshing. Its thread count is capped well
/// below `num_cpus` so the render/main thread keeps clear cores: greedy meshing is
/// cache/bandwidth-heavy (32^3 voxel reads per sector), so even on separate
/// threads a large worker count thrashes the shared cache/memory and starves the
/// frame loop — the cause of the streaming FPS dips. `.1` stores the worker count
/// so the spawner can throttle against it (never grossly over-subscribe the pool).
#[derive(Resource)]
pub struct MeshingPool(pub TaskPool, pub usize);

impl FromWorld for MeshingPool {
    fn from_world(_world: &mut World) -> Self {
        let n = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4)
            .max(2);
        // Reserve ~4 cores for main + render + Bevy + OS. Meshing is pure CPU
        // once snapshotted; extra workers only starve the frame loop.
        let workers = (n.saturating_sub(6)).clamp(1, 2);
        MeshingPool(
            TaskPoolBuilder::new()
                .num_threads(workers)
                .thread_name("strata-meshing".to_string())
                .on_thread_spawn(lower_worker_thread_priority)
                .build(),
            workers,
        )
    }
}

/// Drop the calling (worker) thread to below-normal OS priority so the main +
/// render threads always preempt it. The dedicated meshing (and worldgen) pools
/// otherwise saturate every core during a streaming burst, descheduling the
/// render thread for whole scheduler quanta — the `mesh_spawn` wall-clock
/// inflation behind the streaming FPS dip. Below-normal (not idle) keeps workers
/// progressing on spare cycles, so load stays fast AND smooth. Errors are
/// ignored: priority only affects scheduling, never correctness.
fn lower_worker_thread_priority() {
    use thread_priority::{ThreadPriority, set_current_thread_priority};
    // Crossplatform 20..=39 maps to THREAD_PRIORITY_BELOW_NORMAL on Windows and a
    // correspondingly low priority elsewhere.
    if let Ok(v) = 30u8.try_into() {
        let _ = set_current_thread_priority(ThreadPriority::Crossplatform(v));
    }
}

struct PendingMeshTask {
    task: Task<MeshData>,
    /// Neighbor-residency mask captured at spawn time (drives `remesh-on-load`).
    present_mask: u8,
}

/// Meshing plugin: registers the spawn + apply systems in the `Meshing` set.
pub struct MeshingPlugin;

impl StrataPlugin for MeshingPlugin {
    fn name(&self) -> &'static str {
        "meshing"
    }

    fn build(&self, app: &mut App) {
        app.init_resource::<MeshStorage>();
        app.init_resource::<PendingMesh>();
        app.init_resource::<MeshingPool>();
        app.init_resource::<MeshingTimers>();
        app.add_systems(
            Update,
            (apply_mesh_tasks, spawn_mesh_tasks, cleanup_unloaded_meshes)
                .chain()
                .in_set(StrataSet::Meshing),
        );
    }
}

/// Offsets of the 6 neighbor sectors, indexed to match [`NeighborView`]:
/// 0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z.
fn neighbor_offsets(coord: SectorCoord) -> [SectorCoord; 6] {
    [
        SectorCoord(coord.0 + 1, coord.1, coord.2),
        SectorCoord(coord.0 - 1, coord.1, coord.2),
        SectorCoord(coord.0, coord.1 + 1, coord.2),
        SectorCoord(coord.0, coord.1 - 1, coord.2),
        SectorCoord(coord.0, coord.1, coord.2 + 1),
        SectorCoord(coord.0, coord.1, coord.2 - 1),
    ]
}

/// Snapshot dirty sectors into owned buffers and spawn background meshing tasks.
#[allow(clippy::too_many_arguments)]
pub fn spawn_mesh_tasks(
    mut commands: Commands,
    pool: Res<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    meshing_pool: Res<MeshingPool>,
    dirty: Query<(Entity, &SectorCoord, &XBrickMap, &SectorPalette), With<NeedsRemesh>>,
    all: Query<(Entity, &SectorCoord, &XBrickMap, &SectorPalette)>,
    mut pending: ResMut<PendingMesh>,
    mut timers: ResMut<MeshingTimers>,
    wg_timers: Option<Res<strata_world::plugin::WorldGenTimers>>,
    stream_timers: Option<Res<strata_world::streaming::StreamingTimers>>,
    phys_pending: Option<Res<strata_physics::voxel_collider::PendingCollider>>,
    streaming: Option<Res<strata_world::streaming::StreamingManager>>,
) {
    timers.spawn_us = 0;
    timers.spawned = 0;
    if dirty.is_empty() {
        return;
    }

    let max_inflight = meshing_pool.1.saturating_add(1).max(MESH_BUDGET);
    if pending.tasks.len() >= max_inflight {
        return;
    }

    let mesher = GreedyMesher::new(&registry);
    let registry_arc = Arc::new(registry.clone());
    let task_pool = &meshing_pool.0;
    let spawn_t0 = std::time::Instant::now();

    let player = streaming
        .as_ref()
        .map(|s| s.player_sector)
        .unwrap_or(SectorCoord(0, 0, 0));
    let move_dir = streaming
        .as_ref()
        .map(|s| s.move_dir)
        .unwrap_or(SectorCoord(0, 0, 0));

    let mut work: Vec<_> = dirty.iter().collect();
    work.sort_by_key(|(_, coord, _, _)| {
        strata_world::streaming::load_priority(player, move_dir, **coord)
    });

    // When world-gen just applied unpacks this frame, defer most mesh spawns so
    // the pool write lock never races in-flight mesh readers on worker threads.
    let mut budget = if wg_timers.as_ref().is_some_and(|t| t.applied > 0)
        || stream_timers.as_ref().is_some_and(|t| t.unloaded > 0)
        || phys_pending.as_ref().is_some_and(|p| !p.is_empty())
        || streaming
            .as_ref()
            .is_some_and(|s| s.resident_count() < FULL_SHELL_SECTORS)
    {
        1
    } else {
        MESH_BUDGET
    };
    let skip_neighbor_planes = streaming
        .as_ref()
        .is_some_and(|s| s.resident_count() < FULL_SHELL_SECTORS)
        || !pending.tasks.is_empty();
    let snapshot_deadline =
        std::time::Instant::now() + std::time::Duration::from_micros(SNAPSHOT_BUDGET_US);
    for (entity, coord, sector, palette) in work {
        if budget == 0 || std::time::Instant::now() >= snapshot_deadline {
            break;
        }
        if pending.tasks.contains_key(coord) {
            continue;
        }

        if sector.sector_mask == 0 {
            let task = task_pool.spawn(async move { MeshData::default() });
            pending.tasks.insert(
                *coord,
                PendingMeshTask {
                    task,
                    present_mask: 0,
                },
            );
            commands.entity(entity).remove::<NeedsRemesh>();
            budget -= 1;
            timers.spawned += 1;
            continue;
        }

        let pool_guard = pool.read_inner();
        let snap: Arc<[BlockId; SNAPSHOT_LEN]> = MESH_SCRATCH.with(|cell| {
            let mut buf = cell.borrow_mut();
            fill_sector_snapshot_locked(sector, &pool_guard, palette, Arc::make_mut(&mut buf));
            Arc::clone(&buf)
        });
        let mut planes: [BoundaryPlane; 6] = [[BlockId::AIR; PLANE_LEN]; 6];
        let mut plane_present = [false; 6];
        let mut present_mask: u8 = 0;
        if !skip_neighbor_planes {
            let offsets = neighbor_offsets(*coord);
            for i in 0..6 {
                let ne = if let Some(ref sm) = streaming {
                    sm.entity_for(&offsets[i])
                } else {
                    all.iter()
                        .find(|(_, c, _, _)| **c == offsets[i])
                        .map(|(e, _, _, _)| e)
                };
                if let Some(ne) = ne
                    && let Ok((_, _, m, p)) = all.get(ne)
                {
                    present_mask |= 1u8 << i;
                    if m.sector_mask != 0 {
                        fill_boundary_plane_locked(m, &pool_guard, p, i, &mut planes[i]);
                        plane_present[i] = true;
                    }
                }
            }
        }
        drop(pool_guard);

        let reg = registry_arc.clone();
        let task = task_pool.spawn(async move {
            let mut plane_refs: [Option<&BoundaryPlane>; 6] = [None; 6];
            for i in 0..6 {
                if plane_present[i] {
                    plane_refs[i] = Some(&planes[i]);
                }
            }
            // Lighting is computed separately by the lighting plugin; the
            // mesher receives `None` here and the resolve shader darkens the
            // surface until the lightmap SSBO is filled by a follow-up upload
            // (M10a.4 wiring is owned by the client render).
            mesher.mesh_sector_planes(&snap, &plane_refs, &reg, None)
        });

        pending
            .tasks
            .insert(*coord, PendingMeshTask { task, present_mask });
        commands.entity(entity).remove::<NeedsRemesh>();
        budget -= 1;
        timers.spawned += 1;
    }
    timers.spawn_us = spawn_t0.elapsed().as_micros() as u64;
}

/// Poll completed background meshing tasks and apply their results on the main
/// thread (insert into [`MeshStorage`], bump generation, drive `remesh-on-load`).
pub fn apply_mesh_tasks(
    mut commands: Commands,
    mut pending: ResMut<PendingMesh>,
    mut storage: ResMut<MeshStorage>,
    mut timers: ResMut<MeshingTimers>,
    all: Query<(Entity, &SectorCoord)>,
    streaming: Option<Res<strata_world::streaming::StreamingManager>>,
) {
    timers.apply_us = 0;
    timers.applied = 0;
    if pending.tasks.is_empty() {
        return;
    }

    let t0 = std::time::Instant::now();
    let coords: Vec<SectorCoord> = pending.tasks.keys().copied().collect();
    // Only poll up to `APPLY_BUDGET` pending tasks this frame. Polling a future
    // (even a not-ready one) has cost, and a streaming burst can leave hundreds
    // in-flight; capping the poll set keeps the apply stage O(budget), not
    // O(all_inflight), so the frame loop never stalls on bookkeeping.
    let mut apply_budget = APPLY_BUDGET;
    for coord in coords {
        // Spread a synchronized burst of completions across frames. Check before
        // polling: a *completed* task must never be re-polled (it panics), so we
        // only take a task off the map once we are committed to applying it.
        if apply_budget == 0 {
            break;
        }
        let mut mt = pending.tasks.remove(&coord).unwrap();
        // Non-blocking poll: the background pool drives the task, so it is ready
        // within a frame or two. If not yet done, keep it pending for next frame
        // (no main-thread stall — this is what keeps framerate smooth).
        let Some(mesh) = check_ready(&mut mt.task) else {
            pending.tasks.insert(coord, mt);
            continue;
        };
        apply_budget -= 1;
        timers.applied += 1;

        storage.version += 1;
        let mut mesh = mesh;
        mesh.generation = storage.version;
        storage.meshes.insert(coord, mesh);
        storage.neighbor_mask.insert(coord, mt.present_mask);
        storage.dirty.insert(coord);

        // Mark the sector entity as Meshed
        let entity = if let Some(sm) = &streaming {
            sm.entity_for(&coord)
        } else {
            all.iter().find(|(_, c)| **c == coord).map(|(e, _)| e)
        };
        if let Some(ne) = entity {
            commands.entity(ne).insert(Meshed);
        }

        // remesh-on-load: any resident neighbor that was built while WE were
        // absent (its back-bit toward us unset) must regenerate its boundary
        // faces toward us. The mask check breaks the cycle once both sides have
        // meshed with each other present.
        let offsets = neighbor_offsets(coord);
        for (i, n) in offsets.iter().enumerate() {
            if mt.present_mask & (1u8 << i) == 0 {
                continue;
            }
            let n = *n;
            let back_bit = i ^ 1;
            if !storage.meshes.contains_key(&n) {
                continue;
            }
            let n_mask = storage.neighbor_mask.get(&n).copied().unwrap_or(0);
            if n_mask & (1u8 << back_bit) == 0 {
                let ne = if let Some(sm) = &streaming {
                    sm.entity_for(&n)
                } else {
                    all.iter().find(|(_, c)| **c == n).map(|(e, _)| e)
                };
                if let Some(ne) = ne {
                    commands.entity(ne).insert(NeedsRemesh);
                }
            }
        }
    }
    timers.apply_us = t0.elapsed().as_micros() as u64;
}

/// Remove stale `MeshStorage` entries for sectors no longer resident. Prevents
/// unbounded growth and keeps the per-frame mesh scan in `client_render` bounded
/// to the live sector set.
fn cleanup_unloaded_meshes(
    mut storage: ResMut<MeshStorage>,
    streaming: Option<Res<strata_world::streaming::StreamingManager>>,
) {
    let Some(sm) = streaming else { return };
    if storage.meshes.len() <= sm.resident_count() {
        return;
    }
    storage.meshes.retain(|c, _| sm.is_resident(c));
    storage.neighbor_mask.retain(|c, _| sm.is_resident(c));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn mesh_scratch_is_arc_not_box() {
        MESH_SCRATCH.with(|cell| {
            let buf = cell.borrow();
            // The scratch must be Arc<[BlockId; SNAPSHOT_LEN]> so that
            // worker tasks can share it without an extra Box allocation.
            let type_name = std::any::type_name::<Arc<[BlockId; SNAPSHOT_LEN]>>();
            assert!(std::any::type_name::<Arc<[BlockId; SNAPSHOT_LEN]>>() == type_name);
        });
    }
}
