use bevy_ecs::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Component, Debug, Serialize, Deserialize)]
pub struct Player {
    pub selected_slot: u8,
}
