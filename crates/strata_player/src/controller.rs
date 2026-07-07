//! Player controller: state machine (grounded/flying) + gravity/ground-snap
//! movement integration (plan 14 §Player Controller).
//!
//! For M8 the controller uses a simple, headless-testable gravity + `ground_below`
//! snap instead of wiring Rapier's `KinematicCharacterController` movement (which
//! needs a live physics step). The math is exposed as a pure function
//! [`integrate_player`] so it can be unit-tested without a window; the ECS system
//! [`player_controller_system`] simply applies its result to the player's
//! `Transform` (Filter-First: a single player entity, no per-entity `if`).

use bevy::prelude::*;
use strata_core::prelude::*;

use crate::input::PlayerInput;

/// Half the player capsule height in world units (feet sit at `center.y - HALF`).
pub const PLAYER_HALF_HEIGHT: f32 = 1.0;
/// Eye height above the player's center.
pub const EYE_HEIGHT: f32 = 1.6;
/// Horizontal half-extent used for block-placement overlap rejection.
pub const PLAYER_RADIUS: f32 = 0.3;

/// Tunable movement parameters for the player.
#[derive(Debug, Clone, Copy, Component)]
pub struct PlayerController {
    pub speed: f32,
    pub fly_speed: f32,
    pub jump_speed: f32,
    pub gravity: f32,
    pub sprint_mul: f32,
    pub sneak_mul: f32,
}

impl Default for PlayerController {
    fn default() -> Self {
        PlayerController {
            speed: 5.0,
            fly_speed: 9.0,
            jump_speed: 8.0,
            gravity: 24.0,
            sprint_mul: 1.6,
            sneak_mul: 0.4,
        }
    }
}

/// Per-frame player movement state. `set_if_neq`-guarded on write (AGENTS.md §3.A).
#[derive(Debug, Clone, Copy, Component, Default, PartialEq)]
pub struct PlayerState {
    pub grounded: bool,
    pub flying: bool,
    pub vy: f32,
}

/// First-person look orientation (radians). `yaw` rotates around +Y, `pitch` up/down.
#[derive(Debug, Clone, Copy, Component, Default, PartialEq)]
pub struct PlayerLook {
    pub yaw: f32,
    pub pitch: f32,
}

/// Result of one [`integrate_player`] step.
#[derive(Debug, Clone, Copy)]
pub struct PlayerStepResult {
    pub new_position: Vec3,
    pub grounded: bool,
    pub vy: f32,
}

/// Integrate one movement step. Pure: no ECS access, fully unit-testable.
///
/// * Grounded: horizontal wish-dir from yaw + gravity + jump, then a `ground_below`
///   snap so the player rests on the voxel surface.
/// * Flying: free 3D movement along the look direction, gravity disabled.
#[allow(clippy::too_many_arguments)]
pub fn integrate_player(
    ctrl: &PlayerController,
    state: &PlayerState,
    input: &PlayerInput,
    look: &PlayerLook,
    xbrick: &XBrickMap,
    pool: &GlobalBrickPool,
    position: Vec3,
    dt: f32,
) -> PlayerStepResult {
    let forward = Vec3::new(-look.yaw.sin(), 0.0, -look.yaw.cos());
    let right = Vec3::new(look.yaw.cos(), 0.0, -look.yaw.sin());

    let mut speed = ctrl.speed;
    if input.sprint {
        speed *= ctrl.sprint_mul;
    }
    if input.sneak {
        speed *= ctrl.sneak_mul;
    }

    let wish_horizontal = (forward * input.move_z + right * input.move_x).normalize_or_zero();

    let feet_current = position - Vec3::new(0.0, PLAYER_HALF_HEIGHT, 0.0);
    let grounded_now = is_grounded(xbrick, pool, feet_current);

    let mut vy = state.vy;
    let delta;

    if state.flying {
        let pitch = look.pitch;
        let dir3 = Vec3::new(
            -pitch.cos() * look.yaw.sin(),
            pitch.sin(),
            -pitch.cos() * look.yaw.cos(),
        );
        let mut move3 = (dir3 * input.move_z + right * input.move_x).normalize_or_zero();
        if input.jump {
            move3.y += 1.0;
        }
        delta = move3.normalize_or_zero() * ctrl.fly_speed * dt;
        vy = 0.0;
    } else {
        // Horizontal displacement.
        let horiz = wish_horizontal * speed * dt;
        // Vertical: gravity + jump (jump is only allowed when already grounded).
        vy -= ctrl.gravity * dt;
        if input.jump && grounded_now {
            vy = ctrl.jump_speed;
        }
        // Resting on the ground: don't accumulate downward velocity.
        if grounded_now && vy < 0.0 {
            vy = 0.0;
        }
        delta = Vec3::new(horiz.x, vy * dt, horiz.z);
    }

    let mut new_position = position + delta;

    // Ground snap: if grounded and the step would sink the feet below the current
    // surface, clamp vertically so the player rests on top of the supporting voxel.
    if !state.flying && grounded_now {
        let new_feet = new_position - Vec3::new(0.0, PLAYER_HALF_HEIGHT, 0.0);
        if new_feet.y < feet_current.y {
            new_position.y = position.y;
            vy = 0.0;
        }
    }

    let grounded = if state.flying {
        false
    } else {
        let new_feet = new_position - Vec3::new(0.0, PLAYER_HALF_HEIGHT, 0.0);
        is_grounded(xbrick, pool, new_feet)
    };

    PlayerStepResult {
        new_position,
        grounded,
        vy,
    }
}

/// Grounded test: the feet voxel (or the voxel directly below it) is solid.
/// Extends physics' `ground_below` to also catch the frame where gravity has
/// pushed the feet slightly into the supporting voxel.
fn is_grounded(xbrick: &XBrickMap, pool: &GlobalBrickPool, feet: Vec3) -> bool {
    let wx = feet.x.floor() as i64;
    let wy = feet.y.floor() as i64;
    let wz = feet.z.floor() as i64;
    let ox = xbrick.coord.0 as i64 * 32;
    let oz = xbrick.coord.2 as i64 * 32;
    let lx = wx - ox;
    let lz = wz - oz;
    if !(0..32).contains(&lx) || !(0..32).contains(&lz) {
        return false;
    }
    if (0..32).contains(&wy)
        && xbrick.is_occupied(pool, VoxelCoord::new(lx as u32, wy as u32, lz as u32))
    {
        return true;
    }
    let wyb = wy - 1;
    if (0..32).contains(&wyb)
        && xbrick.is_occupied(pool, VoxelCoord::new(lx as u32, wyb as u32, lz as u32))
    {
        return true;
    }
    false
}

/// ECS system: apply [`integrate_player`] to the player entity and write back the
/// new `Transform` with a change-detection guard on [`PlayerState`].
#[allow(clippy::type_complexity)]
pub fn player_controller_system(
    input: Res<PlayerInput>,
    pool: Res<GlobalBrickPool>,
    sectors: Query<(&SectorCoord, &XBrickMap)>,
    mut player: Query<(
        &mut Transform,
        &PlayerController,
        &mut PlayerState,
        &PlayerLook,
    )>,
) {
    for (mut tf, ctrl, mut st, look) in &mut player {
        let feet = tf.translation - Vec3::new(0.0, PLAYER_HALF_HEIGHT, 0.0);
        let sc = SectorCoord(
            (feet.x / 32.0).floor() as i32,
            (feet.y / 32.0).floor() as i32,
            (feet.z / 32.0).floor() as i32,
        );
        let xbrick = sectors.iter().find(|(c, _)| **c == sc).map(|(_, m)| m);

        let result = match xbrick {
            Some(m) => integrate_player(
                ctrl,
                &st,
                &input,
                look,
                m,
                &pool,
                tf.translation,
                1.0 / 60.0,
            ),
            None => PlayerStepResult {
                new_position: tf.translation,
                grounded: false,
                vy: st.vy,
            },
        };

        tf.translation = result.new_position;
        st.set_if_neq(PlayerState {
            grounded: result.grounded,
            flying: st.flying,
            vy: result.vy,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::PlayerInput;
    use strata_physics::voxel_collider::sector_world_origin;

    fn floor_sector() -> (XBrickMap, GlobalBrickPool, SectorPalette) {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        // Solid floor at local y=0 across the sector.
        for x in 0..32u32 {
            for z in 0..32u32 {
                map.set_block(
                    &mut pool,
                    &mut palette,
                    VoxelCoord::new(x, 0, z),
                    BlockId(1),
                );
            }
        }
        (map, pool, palette)
    }

    #[test]
    fn grounded_true_when_floor_below() {
        let (map, pool, _) = floor_sector();
        let ctrl = PlayerController::default();
        let state = PlayerState::default();
        let look = PlayerLook::default();
        let o = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 2.0, 5.0);
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &look,
            &map,
            &pool,
            o,
            1.0 / 60.0,
        );
        assert!(r.grounded, "player should be grounded on the floor");
    }

    #[test]
    fn not_grounded_over_void() {
        let pool = GlobalBrickPool::new();
        let _palette = SectorPalette::new();
        let map = XBrickMap::new(SectorCoord(0, 0, 0));
        let ctrl = PlayerController::default();
        let state = PlayerState::default();
        let look = PlayerLook::default();
        let o = Vec3::new(5.0, 20.0, 5.0);
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &look,
            &map,
            &pool,
            o,
            1.0 / 60.0,
        );
        assert!(!r.grounded, "no floor -> not grounded");
        assert!(r.vy < 0.0, "gravity should pull the player down");
    }

    #[test]
    fn jump_leaves_ground() {
        let (map, pool, _) = floor_sector();
        let ctrl = PlayerController::default();
        let state = PlayerState {
            grounded: true,
            flying: false,
            vy: 0.0,
        };
        let look = PlayerLook::default();
        let o = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 1.0, 5.0);
        let jump = PlayerInput {
            jump: true,
            ..Default::default()
        };
        let r = integrate_player(&ctrl, &state, &jump, &look, &map, &pool, o, 1.0 / 60.0);
        assert!(r.vy > 0.0, "jump should impart upward velocity");
    }
}
