use bevy_ecs::prelude::*;
use strata_core::ChunkPos;

#[derive(Component, Debug, Clone, Copy)]
pub struct ChunkPosition(pub ChunkPos);

#[derive(Component, Debug)]
pub struct ChunkDirty {
    pub needs_mesh: bool,
    pub needs_light: bool,
}
