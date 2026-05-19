use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_time::common_conditions::on_timer;
use std::time::Duration;

use crate::ai::mob_ai_system;
use crate::registry::EntityRegistry;
use crate::spawning::{SpawnedChunks, chunk_mob_spawner};

pub struct EntityPlugin;

impl Plugin for EntityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EntityRegistry>()
            .init_resource::<SpawnedChunks>()
            .add_systems(
                Update,
                mob_ai_system.run_if(on_timer(Duration::from_secs_f64(1.0 / 60.0))),
            )
            .add_systems(Update, chunk_mob_spawner);
    }
}
