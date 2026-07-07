//! ECS integration: `NeedsRemesh` (ZST, filter-first) drives greedy meshing,
//! results land in the [`MeshStorage`] resource (plan 09 §10, AGENTS.md §3.A).

use bevy::prelude::*;
use std::collections::HashMap;

use strata_core::prelude::*;

use crate::meshing::greedy::GreedyMesher;
use crate::meshing::{MeshData, Mesher, NeighborView};

/// Storage for completed sector meshes, keyed by sector coordinate. Consumed by
/// M4's render stage. Result buffers only — no live voxel data.
#[derive(Resource, Default)]
pub struct MeshStorage {
    pub meshes: HashMap<SectorCoord, MeshData>,
}

/// Meshing plugin: registers the incremental remesh system in the `Meshing` set.
pub struct MeshingPlugin;

impl StrataPlugin for MeshingPlugin {
    fn name(&self) -> &'static str {
        "meshing"
    }

    fn build(&self, app: &mut App) {
        app.init_resource::<MeshStorage>();
        app.add_systems(Update, mesh_dirty_sectors.in_set(StrataSet::Meshing));
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

/// Filter-first: only sectors flagged `With<NeedsRemesh>` are meshed (no
/// per-entity `if` check). Runs synchronously on the main thread for M3; an
/// `AsyncComputeTaskPool` port with main-thread-only apply is deferred to M4.
pub fn mesh_dirty_sectors(
    mut commands: Commands,
    pool: Res<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    dirty: Query<(Entity, &SectorCoord, &XBrickMap, &SectorPalette), With<NeedsRemesh>>,
    all: Query<(Entity, &SectorCoord, &XBrickMap, &SectorPalette)>,
    mut storage: ResMut<MeshStorage>,
) {
    let mut neighbor_map: HashMap<SectorCoord, (&XBrickMap, &SectorPalette)> = HashMap::new();
    for (_, coord, map, pal) in all.iter() {
        neighbor_map.insert(*coord, (map, pal));
    }

    let mesher = GreedyMesher::new(&registry);

    for (entity, coord, sector, palette) in dirty.iter() {
        let offsets = neighbor_offsets(*coord);
        let mut views: [NeighborView<'_>; 6] = [NeighborView {
            sector: None,
            palette: None,
            pool: &pool,
        }; 6];
        for i in 0..6 {
            if let Some((m, p)) = neighbor_map.get(&offsets[i]) {
                views[i].sector = Some(m);
                views[i].palette = Some(p);
            }
        }

        let mesh = mesher.mesh_sector(sector, palette, &pool, &registry, &views);
        storage.meshes.insert(*coord, mesh);
        commands.entity(entity).remove::<NeedsRemesh>();
    }
}
