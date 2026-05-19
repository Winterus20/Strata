use bevy_ecs::prelude::*;
use bevy_math::{Vec3, Vec3Swizzles};
use bevy_rapier3d::prelude::*;
use bevy_time::Time;

use crate::components::{Player, PlayerInput, Velocity};
use strata_physics::GRAVITY;

const MOVEMENT_SPEED: f32 = 4.0;
const SPRINT_SPEED: f32 = 6.0;
const JUMP_VELOCITY: f32 = 8.0;
const FRICTION: f32 = 10.0;

pub fn player_controller_system(
    time: Res<Time>,
    mut query: Query<
        (
            &mut Velocity,
            &PlayerInput,
            &mut KinematicCharacterController,
            Option<&KinematicCharacterControllerOutput>,
        ),
        With<Player>,
    >,
) {
    let dt = time.delta_secs();

    for (mut velocity, input, mut controller, output) in query.iter_mut() {
        let is_grounded = output.map(|o| o.grounded).unwrap_or(false);

        let mut direction = Vec3::ZERO;
        if input.up {
            direction.z -= 1.0;
        }
        if input.down {
            direction.z += 1.0;
        }
        if input.left {
            direction.x -= 1.0;
        }
        if input.right {
            direction.x += 1.0;
        }

        if direction.length_squared() > 0.0 {
            direction = direction.normalize();
        }

        let yaw = input
            .look_direction
            .xz()
            .try_normalize()
            .map(|d| d.to_angle())
            .unwrap_or(0.0);
        let cos = yaw.cos();
        let sin = yaw.sin();
        let rotated = Vec3::new(
            direction.x * cos - direction.z * sin,
            0.0,
            direction.x * sin + direction.z * cos,
        );

        let speed = if input.sprint {
            SPRINT_SPEED
        } else {
            MOVEMENT_SPEED
        };
        let target_vel = rotated * speed;

        let current_xz = Vec3::new(velocity.0.x, 0.0, velocity.0.z);
        let diff = target_vel - current_xz;

        let accel = FRICTION * dt;
        if diff.length() < accel {
            velocity.0.x = target_vel.x;
            velocity.0.z = target_vel.z;
        } else {
            let step = diff.normalize() * accel;
            velocity.0.x += step.x;
            velocity.0.z += step.z;
        }

        if is_grounded {
            velocity.0.y = velocity.0.y.max(-0.5);
            if input.jump {
                velocity.0.y = JUMP_VELOCITY;
            }
        } else {
            velocity.0.y += GRAVITY * dt;
        }

        controller.translation = Some(velocity.0 * dt);
    }
}
