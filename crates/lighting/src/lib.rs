use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use strata_ecs::components::ChunkDirty;
use strata_ecs::systems::ChunkStorage;

pub mod block_light;
pub mod propagate;
pub mod sunlight;

pub use propagate::on_block_change;
pub use propagate::propagate_all;

/// Maps `BlockId` → light emission level (0–15).
#[derive(Resource)]
pub struct LightEmissionTable {
    pub table: Vec<u8>,
}

impl Default for LightEmissionTable {
    fn default() -> Self {
        Self {
            table: vec![0u8; 256],
        }
    }
}

impl LightEmissionTable {
    pub fn set(&mut self, block_id: u16, level: u8) {
        if block_id as usize >= self.table.len() {
            self.table.resize(block_id as usize + 1, 0);
        }
        self.table[block_id as usize] = level.min(15);
    }
}

/// Plugin for Bevy ECS lighting integration.
pub struct LightPlugin;

impl Default for LightPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl LightPlugin {
    pub fn new() -> Self {
        Self
    }
}

impl Plugin for LightPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightEmissionTable>();
        app.add_systems(Update, lighting_system);
    }
}

fn lighting_system(
    mut chunk_storage: ResMut<ChunkStorage>,
    mut dirty_query: Query<(Entity, &mut ChunkDirty)>,
    light_emission: Res<LightEmissionTable>,
) {
    let light_dirty_positions: Vec<_> = chunk_storage
        .chunks
        .iter()
        .filter(|(_, chunk)| chunk.light_dirty)
        .map(|(pos, _)| *pos)
        .collect();

    for pos in light_dirty_positions {
        if let Some(chunk) = chunk_storage.chunks.get_mut(&pos) {
            propagate_all(chunk, &light_emission.table);
            chunk.light_dirty = false;
        }
    }

    for (_, mut dirty) in dirty_query.iter_mut() {
        if let Some(chunk) = chunk_storage.chunks.values().next()
            && !chunk.light_dirty
        {
            dirty.needs_light = false;
        }
    }
}
