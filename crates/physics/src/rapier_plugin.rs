use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_rapier3d::prelude::*;

pub const GRAVITY: f32 = -20.0;

/// Plugin that initialises the Rapier3D physics backend.
pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());
        app.insert_resource(PhysicsConfig { gravity: GRAVITY });
    }
}

/// Global physics configuration.
#[derive(Resource)]
pub struct PhysicsConfig {
    pub gravity: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self { gravity: GRAVITY }
    }
}
