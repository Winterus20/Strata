use bevy_ecs::prelude::*;
use hashbrown::HashSet;
use strata_core::ChunkPos;

#[derive(Resource, Default)]
pub struct WorldState {
    pub loaded_chunks: HashSet<ChunkPos>,
    pub render_distance: u32,
}
