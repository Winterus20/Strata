use bevy_app::prelude::*;
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_time::common_conditions::on_timer;
use std::time::Duration;

use crate::interaction::block_interaction_system;
use crate::systems::player_controller_system;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            player_controller_system.run_if(on_timer(Duration::from_secs_f64(1.0 / 60.0))),
        );
        app.add_systems(Update, block_interaction_system);
    }
}
