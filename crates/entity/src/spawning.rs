use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use bevy_rapier3d::prelude::*;
use bevy_transform::prelude::Transform;
use hashbrown::HashSet;
use rand::Rng;

use crate::components::{AiState, Health, Mob};
use strata_physics::{ENTITY, PLAYER, TERRAIN};

pub fn spawn_mob(commands: &mut Commands, position: Vec3) -> Entity {
    commands
        .spawn((
            Mob,
            Health {
                current: 20,
                max: 20,
            },
            AiState::Idle { timer: 2.0 },
            Transform::from_translation(position),
            RigidBody::KinematicPositionBased,
            Collider::capsule_y(0.4, 0.4),
            KinematicCharacterController {
                filter_groups: Some(CollisionGroups::new(ENTITY, TERRAIN | PLAYER | ENTITY)),
                ..Default::default()
            },
        ))
        .id()
}

#[derive(Resource, Default)]
pub struct SpawnedChunks {
    positions: HashSet<(i32, i32)>,
}

pub fn chunk_mob_spawner(
    mut commands: Commands,
    mut spawned: ResMut<SpawnedChunks>,
    player_query: Query<&Transform, With<crate::components::Mob>>,
) {
    let mut rng = rand::thread_rng();

    let player_pos = match player_query.single() {
        Ok(t) => t.translation,
        Err(_) => return,
    };

    let player_chunk_x = (player_pos.x as i32).div_euclid(16);
    let player_chunk_z = (player_pos.z as i32).div_euclid(16);

    for dx in -2..=2 {
        for dz in -2..=2 {
            let cx = player_chunk_x + dx;
            let cz = player_chunk_z + dz;

            if spawned.positions.contains(&(cx, cz)) {
                continue;
            }

            if rng.gen_bool(0.3) {
                let spawn_x = (cx * 16 + rng.gen_range(0..16)) as f32;
                let spawn_z = (cz * 16 + rng.gen_range(0..16)) as f32;
                let spawn_y = player_pos.y + rng.gen_range(-2.0..5.0);

                spawn_mob(&mut commands, Vec3::new(spawn_x, spawn_y, spawn_z));
                spawned.positions.insert((cx, cz));
            }
        }
    }
}
