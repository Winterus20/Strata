//! `WorldGenPlugin`: drives sector generation through the `WorldGen` set
//! (plan 11 §4 / 08 §4).
//!
//! Sectors lacking a `Generated` marker are queued onto the
//! `AsyncComputeTaskPool`; the heavy [`generate_compressed`] runs off-thread,
//! and results are applied on the **main thread only** (so no two systems race
//! on the shared [`GlobalBrickPool`]). Application happens once per entity,
//! gated by the `Generated` / `Generating` markers — equivalent to a
//! `set_if_neq` guard against re-apply.

use bevy::prelude::*;
use bevy::tasks::futures::check_ready;
use bevy::tasks::{Task, TaskPool, TaskPoolBuilder};
use std::collections::HashMap;
use std::sync::Arc;
use strata_core::prelude::*;

use crate::generator::generate_compressed;
use crate::streaming::{StreamingManager, load_priority};
use strata_core::prelude::CompressedChunkData;

/// Max sectors queued for background generation per frame (async only).
const WORLDGEN_SPAWN_BUDGET: usize = 4;
/// Max synchronous `unpack` calls per frame on the main thread. One unpack can
/// still take several ms; keeping this at 1 avoids stacking with mesh snapshots.
const WORLDGEN_APPLY_BUDGET: usize = 1;

/// Marker: a sector's voxel data has been generated and applied.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct Generated;

/// Marker: generation has been queued this frame but not yet applied. Keeps the
/// `Without<Generated>` world-gen query archetype-filtered (cheap) while the
/// per-frame [`WORLDGEN_BUDGET`] caps how many sectors block the frame.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct Generating;

/// Shareable generated snapshot, held on the sector entity (plan 07 snapshot).
#[derive(Debug, Component, Clone)]
pub struct SectorSnapshot(pub Arc<CompressedChunkData>);

/// In-flight async world generation tasks.
#[derive(Resource, Default)]
pub struct PendingWorldGen {
    pub tasks: HashMap<SectorCoord, Task<Arc<CompressedChunkData>>>,
}

/// Per-frame timings for the world-gen hot path, surfaced to the client
/// diagnostics so streaming FPS drops can be attributed to the right stage.
/// `apply_us` is the synchronous main-thread `unpack` cost (the only stage that
/// can stall a frame during streaming).
#[derive(Resource, Default)]
pub struct WorldGenTimers {
    pub apply_us: u64,
    pub applied: usize,
}

/// Dedicated background pool for world generation.
#[derive(Resource)]
pub struct GeneratorPool(pub TaskPool);

impl FromWorld for GeneratorPool {
    fn from_world(_world: &mut World) -> Self {
        #[cfg(test)]
        let workers = 1;
        #[cfg(not(test))]
        let workers = {
            let n = std::thread::available_parallelism()
                .map(|p| p.get())
                .unwrap_or(4)
                .max(2);
            // Reserve ~4 cores for main + render + Bevy + OS (see MeshingPool).
            (n.saturating_sub(6)).clamp(1, 2)
        };
        GeneratorPool(
            TaskPoolBuilder::new()
                .num_threads(workers)
                .thread_name("strata-worldgen".to_string())
                .on_thread_spawn(lower_worker_thread_priority)
                .build(),
        )
    }
}

/// Drop the calling (worker) thread to below-normal OS priority so the main +
/// render threads always preempt it. The dedicated worldgen + meshing pools
/// otherwise saturate every core during a streaming burst, and the OS then
/// descheduled the render thread for whole scheduler quanta — the `mesh_spawn`
/// wall-clock inflation behind the streaming FPS dip. Below-normal (not idle)
/// keeps the workers progressing on spare cycles, so load stays fast AND smooth.
/// Errors are ignored: priority only affects scheduling, never correctness.
fn lower_worker_thread_priority() {
    use thread_priority::{ThreadPriority, set_current_thread_priority};
    // Crossplatform 20..=39 maps to THREAD_PRIORITY_BELOW_NORMAL on Windows and a
    // correspondingly low priority elsewhere.
    if let Ok(v) = 30u8.try_into() {
        let _ = set_current_thread_priority(ThreadPriority::Crossplatform(v));
    }
}

/// Strata world-generation plugin (M5).
///
/// Generation runs asynchronously on background worker threads (`GeneratorPool`)
/// to prevent main-thread CPU stutters.
pub struct WorldGenPlugin;

impl StrataPlugin for WorldGenPlugin {
    fn name(&self) -> &'static str {
        "world_gen"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GlobalBrickPool>() {
            app.insert_resource(GlobalBrickPool::new());
        }
        app.init_resource::<PendingWorldGen>();
        app.init_resource::<GeneratorPool>();
        app.init_resource::<WorldGenTimers>();
        app.add_systems(
            Update,
            (spawn_world_gen_tasks, apply_world_gen_tasks)
                .chain()
                .in_set(StrataSet::WorldGen),
        );
    }
}

#[allow(clippy::type_complexity)]
fn spawn_world_gen_tasks(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    mut pending: ResMut<PendingWorldGen>,
    generator_pool: Res<GeneratorPool>,
    streaming: Option<Res<StreamingManager>>,
    q_new: Query<(Entity, &SectorCoord), (Without<Generated>, Without<Generating>)>,
) {
    let mut budget = WORLDGEN_SPAWN_BUDGET;
    let pool = &generator_pool.0;
    let registry_arc = Arc::new(registry.clone());
    let player = streaming
        .as_ref()
        .map(|s| s.player_sector)
        .unwrap_or(SectorCoord(0, 0, 0));
    let move_dir = streaming
        .as_ref()
        .map(|s| s.move_dir)
        .unwrap_or(SectorCoord(0, 0, 0));

    let mut work: Vec<(Entity, SectorCoord)> = q_new.iter().map(|(e, c)| (e, *c)).collect();
    work.sort_by_key(|(_, c)| load_priority(player, move_dir, *c));

    for (e, coord) in work {
        if budget == 0 {
            break;
        }
        if pending.tasks.contains_key(&coord) {
            continue;
        }

        let coord_val = coord;
        let reg = registry_arc.clone();
        let task = pool.spawn(async move { generate_compressed(coord_val, &reg) });

        pending.tasks.insert(coord_val, task);
        commands.entity(e).insert(Generating);
        budget -= 1;
    }
}

#[allow(clippy::type_complexity)]
fn apply_world_gen_tasks(
    mut commands: Commands,
    mut pending: ResMut<PendingWorldGen>,
    mut pool: ResMut<GlobalBrickPool>,
    mut timers: ResMut<WorldGenTimers>,
    q_generating: Query<(Entity, &SectorCoord), With<Generating>>,
) {
    timers.apply_us = 0;
    timers.applied = 0;
    if pending.tasks.is_empty() {
        return;
    }

    let mut gen_entities = HashMap::new();
    for (e, coord) in &q_generating {
        gen_entities.insert(*coord, e);
    }

    let coords: Vec<SectorCoord> = pending.tasks.keys().copied().collect();
    // Cap synchronous unpacks per frame. `unpack` is the heaviest main-thread
    // job in the stream (a 32³ sector written into the shared pool); an uncapped
    // loop applies every *ready* sector in one frame, so a shell crossing (~49
    // sectors) or the initial spawn (~343) blocks the frame into an FPS hitch.
    // A small budget spreads that cost across frames (meshing is budgeted too).
    let mut apply_budget = WORLDGEN_APPLY_BUDGET;
    for coord in coords {
        if apply_budget == 0 {
            break;
        }
        let mut task = pending.tasks.remove(&coord).unwrap();
        let ready = check_ready(&mut task);
        if let Some(data) = ready {
            if let Some(&e) = gen_entities.get(&coord) {
                let t0 = std::time::Instant::now();
                let (map, palette) = data.unpack(&mut pool);
                timers.apply_us += t0.elapsed().as_micros() as u64;
                commands
                    .entity(e)
                    .insert(map)
                    .insert(palette)
                    .insert(SectorSnapshot(data))
                    .insert(Generated)
                    .remove::<Generating>();
                timers.applied += 1;
                apply_budget -= 1;
            }
        } else {
            pending.tasks.insert(coord, task);
        }
    }
}
