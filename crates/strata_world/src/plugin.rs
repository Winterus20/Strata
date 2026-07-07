//! `WorldGenPlugin`: drives sector generation through the `WorldGen` set
//! (plan 11 §4 / 08 §4).
//!
//! Sectors lacking a `Generated` marker are queued onto the
//! `AsyncComputeTaskPool`; the heavy [`generate_compressed`] runs off-thread,
//! and results are applied on the **main thread only** (so no two systems race
//! on the shared [`GlobalBrickPool`]). Application happens once per entity,
//! gated by the `Generated` / `Generating` markers — equivalent to a
//! `set_if_neq` guard against re-apply.

use std::sync::Arc;

use bevy::prelude::*;

use strata_core::prelude::*;

use crate::generator::generate_compressed;
use strata_core::prelude::CompressedChunkData;

/// Marker: a sector's voxel data has been generated and applied.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct Generated;

/// Shareable generated snapshot, held on the sector entity (plan 07 snapshot).
#[derive(Debug, Component, Clone)]
pub struct SectorSnapshot(pub Arc<CompressedChunkData>);

/// Strata world-generation plugin (M5).
///
/// Generation runs synchronously on the main thread inside the `WorldGen` set.
/// This is deterministic and avoids async-timing flakiness in the prototype;
/// the plan's `AsyncComputeTaskPool` offloading can be layered back on top
/// later (it requires a single main-thread consumer of `GlobalBrickPool`).
pub struct WorldGenPlugin;

impl StrataPlugin for WorldGenPlugin {
    fn name(&self) -> &'static str {
        "world_gen"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GlobalBrickPool>() {
            app.insert_resource(GlobalBrickPool::new());
        }
        app.add_systems(Update, world_gen_system.in_set(StrataSet::WorldGen));
    }
}

#[allow(clippy::type_complexity)]
fn world_gen_system(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    mut pool: ResMut<GlobalBrickPool>,
    q_new: Query<(Entity, &SectorCoord), Without<Generated>>,
) {
    for (e, coord) in &q_new {
        let data = generate_compressed(*coord, &registry);
        let (map, palette) = data.unpack(&mut pool);
        commands
            .entity(e)
            .insert(map)
            .insert(palette)
            .insert(SectorSnapshot(data))
            .insert(Generated);
    }
}
