use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_time::prelude::*;
use hashbrown::HashMap;
use strata_core::ChunkPos;

use crate::flow::WaterFlow;
use crate::water_level::ChunkWaterLevels;

/// Resource that stores water levels for all loaded chunks.
#[derive(Resource, Default)]
pub struct FluidWorld {
    /// Per-chunk water level data.
    pub chunk_water: HashMap<ChunkPos, ChunkWaterLevels>,
    /// Ticks per second for water simulation.
    pub tick_rate: f64,
    /// Accumulated time since last tick.
    pub tick_accumulator: f64,
}

impl FluidWorld {
    /// Creates a new `FluidWorld` with the given tick rate.
    pub fn new(tick_rate: f64) -> Self {
        Self {
            chunk_water: HashMap::new(),
            tick_rate,
            tick_accumulator: 0.0,
        }
    }

    /// Registers a chunk for fluid simulation.
    pub fn register_chunk(&mut self, pos: ChunkPos) {
        self.chunk_water.entry(pos).or_default();
    }

    /// Removes a chunk from fluid simulation.
    pub fn remove_chunk(&mut self, pos: &ChunkPos) {
        self.chunk_water.remove(pos);
    }

    /// Returns mutable water levels for a chunk.
    pub fn get_water_mut(&mut self, pos: &ChunkPos) -> Option<&mut ChunkWaterLevels> {
        self.chunk_water.get_mut(pos)
    }

    /// Returns shared water levels for a chunk.
    pub fn get_water(&self, pos: &ChunkPos) -> Option<&ChunkWaterLevels> {
        self.chunk_water.get(pos)
    }
}

/// ECS plugin for fluid simulation.
pub struct FluidPlugin;

impl Plugin for FluidPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FluidWorld>()
            .add_systems(Update, fluid_tick_system);
    }
}

/// System that runs water simulation at a fixed tick rate.
fn fluid_tick_system(
    time: Res<Time>,
    mut fluid_world: ResMut<FluidWorld>,
    mut chunk_storage: ResMut<strata_ecs::systems::ChunkStorage>,
) {
    fluid_world.tick_accumulator += time.delta_secs_f64();

    let tick_interval = 1.0 / fluid_world.tick_rate;

    while fluid_world.tick_accumulator >= tick_interval {
        fluid_world.tick_accumulator -= tick_interval;
        tick_all_chunks(&mut fluid_world, &mut chunk_storage);
    }
}

/// Runs one water flow tick for all loaded chunks.
fn tick_all_chunks(
    fluid_world: &mut FluidWorld,
    chunk_storage: &mut strata_ecs::systems::ChunkStorage,
) {
    let chunk_positions: Vec<ChunkPos> = fluid_world.chunk_water.keys().copied().collect();

    for pos in chunk_positions {
        let Some(chunk) = chunk_storage.chunks.get_mut(&pos) else {
            continue;
        };
        let Some(water) = fluid_world.chunk_water.get_mut(&pos) else {
            continue;
        };

        let changed = WaterFlow::tick(chunk, water);

        if changed {
            chunk.dirty = true;
        }

        water.clear_dirty();
    }
}
