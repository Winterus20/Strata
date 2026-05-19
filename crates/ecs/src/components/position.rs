use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position(pub Vec3);

#[derive(Component, Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Velocity(pub Vec3);
