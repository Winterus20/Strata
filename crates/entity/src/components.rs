use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Component, Serialize, Deserialize)]
pub struct Health {
    pub current: u16,
    pub max: u16,
}

#[derive(Component, Serialize, Deserialize)]
pub enum AiState {
    Idle { timer: f32 },
    Wander { target_dir: Vec3, timer: f32 },
}

#[derive(Component, Serialize, Deserialize)]
pub struct Mob;
