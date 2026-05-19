use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use bevy_rapier3d::prelude::*;
use bevy_time::Time;
use bevy_transform::prelude::Transform;
use rand::Rng;

use crate::components::AiState;

pub fn mob_ai_system(
    time: Res<Time>,
    rapier_context: ReadRapierContext,
    mut query: Query<(
        Entity,
        &mut AiState,
        &Transform,
        &mut KinematicCharacterController,
    )>,
) {
    let dt = time.delta_secs();
    let Ok(rapier_context) = rapier_context.single() else {
        return;
    };

    let mut rng = rand::thread_rng();

    for (entity, mut state, transform, mut controller) in query.iter_mut() {
        match *state {
            AiState::Idle { ref mut timer } => {
                *timer -= dt;
                if *timer <= 0.0 {
                    let dir = Vec3::new(rng.gen_range(-1.0..1.0), 0.0, rng.gen_range(-1.0..1.0))
                        .try_normalize()
                        .unwrap_or(Vec3::Z);
                    *state = AiState::Wander {
                        target_dir: dir,
                        timer: rng.gen_range(2.0..5.0),
                    };
                }
                controller.translation = Some(Vec3::ZERO);
            }
            AiState::Wander {
                ref mut target_dir,
                ref mut timer,
            } => {
                *timer -= dt;
                if *timer <= 0.0 {
                    *state = AiState::Idle {
                        timer: rng.gen_range(1.0..3.0),
                    };
                    controller.translation = Some(Vec3::ZERO);
                    continue;
                }

                let speed = 2.0;
                let mut movement = *target_dir * speed * dt;

                let ray_pos = transform.translation + Vec3::Y * 0.1;
                let ray_dir = *target_dir;
                let max_toi = 0.5;
                let solid = true;
                let filter = QueryFilter::new().exclude_rigid_body(entity);

                if let Some((_hit_entity, _toi)) =
                    rapier_context.cast_ray(ray_pos, ray_dir, max_toi, solid, filter)
                {
                    movement.y += 5.0 * dt;
                } else {
                    movement.y -= 9.8 * dt;
                }

                controller.translation = Some(movement);
            }
        }
    }
}
