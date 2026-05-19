use bevy_app::prelude::*;

pub mod components;
pub mod systems;

pub use components::*;

pub struct EcsPlugin;

impl Plugin for EcsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::WorldState>();
        app.init_resource::<systems::ChunkStorage>();
        app.add_observer(systems::on_block_break);
        app.add_observer(systems::on_block_place);
    }
}
