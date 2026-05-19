use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use bevy_rapier3d::prelude::*;
use bevy_transform::components::Transform;

use strata_physics::{ENTITY, PLAYER, TERRAIN};

#[derive(Component, Default)]
pub struct Player;

#[derive(Component, Default)]
pub struct PlayerInput {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sprint: bool,
    pub left_click: bool,
    pub right_click: bool,
    pub look_direction: Vec3,
}

#[derive(Component, Default)]
pub struct Velocity(pub Vec3);

#[derive(Bundle)]
pub struct PlayerBundle {
    pub player: Player,
    pub input: PlayerInput,
    pub velocity: Velocity,
    pub transform: Transform,
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub controller: KinematicCharacterController,
}

impl Default for PlayerBundle {
    fn default() -> Self {
        Self {
            player: Player,
            input: PlayerInput::default(),
            velocity: Velocity::default(),
            transform: Transform::default(),
            rigid_body: RigidBody::KinematicPositionBased,
            collider: Collider::capsule_y(0.5, 0.3),
            controller: KinematicCharacterController {
                filter_groups: Some(CollisionGroups::new(PLAYER, TERRAIN | ENTITY)),
                offset: CharacterLength::Absolute(0.01),
                ..Default::default()
            },
        }
    }
}
