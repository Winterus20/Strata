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
/// The body is therefore `2 * PLAYER_HALF_HEIGHT` = 2 blocks tall (matches the
/// Rapier `capsule_y(0.5, 0.5)` collider).
pub const PLAYER_HALF_HEIGHT: f32 = 1.0;
/// Eye height above the player's center. Equal to `PLAYER_HALF_HEIGHT` so the
/// eye sits at the top of the 2-block body — i.e. eye-above-feet == body height
/// (both 2 blocks).
pub const EYE_HEIGHT: f32 = PLAYER_HALF_HEIGHT;
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

/// Small skin so the resolved AABB rests just outside the blocking voxel face
/// (avoids re-colliding on the next frame from floating-point exactness).
const COLLIDE_SKIN: f32 = 1.0e-3;
/// How far below the feet the ground probe looks when deciding `grounded`.
const GROUND_PROBE: f32 = 0.05;
/// Plan 14 §Player Controller: clamp fall speed (world units / s).
const TERMINAL_VELOCITY: f32 = 50.0;
/// Sub-step when `|delta|` exceeds half a voxel (plan 14 collide-and-slide).
const MAX_AXIS_STEP: f32 = 0.5;
/// Cap sub-steps so pathological deltas still terminate (plan 14 max-substeps).
const MAX_AXIS_SUBSTEPS: usize = 32;

/// The player's axis-aligned bounding box for a given capsule *center*.
#[inline]
fn player_aabb(center: Vec3) -> (Vec3, Vec3) {
    let half = Vec3::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT, PLAYER_RADIUS);
    (center - half, center + half)
}

/// Inclusive integer voxel range spanned by `[min, max)` on one axis. The tiny
/// `COLLIDE_SKIN` bias keeps an AABB that merely *touches* a voxel face (max on an
/// integer boundary) from spuriously counting that neighbouring voxel.
#[inline]
fn voxel_span(min: f32, max: f32) -> (i64, i64) {
    (min.floor() as i64, (max - COLLIDE_SKIN).floor() as i64)
}

/// Does the player AABB centered at `center` overlap any solid voxel?
fn aabb_hits_solid(is_solid: &impl Fn(i64, i64, i64) -> bool, center: Vec3) -> bool {
    let (min, max) = player_aabb(center);
    let (x0, x1) = voxel_span(min.x, max.x);
    let (y0, y1) = voxel_span(min.y, max.y);
    let (z0, z1) = voxel_span(min.z, max.z);
    for vx in x0..=x1 {
        for vy in y0..=y1 {
            for vz in z0..=z1 {
                if is_solid(vx, vy, vz) {
                    return true;
                }
            }
        }
    }
    false
}

/// Move the capsule center along a single axis by `delta`, resolving voxel
/// collisions by snapping the AABB to the blocking face. Returns the new center
/// and whether a collision stopped the motion on that axis.
///
/// When `|delta| > MAX_AXIS_STEP`, the move is split into sub-steps so a large
/// `vy*dt` cannot tunnel through a thin floor (plan 14).
///
/// `axis`: 0 = X, 1 = Y, 2 = Z.
fn move_axis(
    is_solid: &impl Fn(i64, i64, i64) -> bool,
    center: Vec3,
    axis: usize,
    delta: f32,
) -> (Vec3, bool) {
    if delta == 0.0 {
        return (center, false);
    }

    let abs = delta.abs();
    if abs <= MAX_AXIS_STEP {
        return move_axis_once(is_solid, center, axis, delta);
    }

    let steps = ((abs / MAX_AXIS_STEP).ceil() as usize).clamp(1, MAX_AXIS_SUBSTEPS);
    let step = delta / steps as f32;
    let mut pos = center;
    let mut hit = false;
    for _ in 0..steps {
        let (p, h) = move_axis_once(is_solid, pos, axis, step);
        pos = p;
        if h {
            hit = true;
            break;
        }
    }
    (pos, hit)
}

/// Single discrete axis move (no sub-stepping).
fn move_axis_once(
    is_solid: &impl Fn(i64, i64, i64) -> bool,
    center: Vec3,
    axis: usize,
    delta: f32,
) -> (Vec3, bool) {
    if delta == 0.0 {
        return (center, false);
    }
    let mut moved = center;
    moved[axis] += delta;
    if !aabb_hits_solid(is_solid, moved) {
        return (moved, false);
    }

    // Collision: snap to the face of the nearest blocking voxel along this axis.
    let (min, max) = player_aabb(moved);
    let (x0, x1) = voxel_span(min.x, max.x);
    let (y0, y1) = voxel_span(min.y, max.y);
    let (z0, z1) = voxel_span(min.z, max.z);
    let half = if axis == 1 {
        PLAYER_HALF_HEIGHT
    } else {
        PLAYER_RADIUS
    };

    let mut snapped = moved[axis];
    let mut hit = false;
    for vx in x0..=x1 {
        for vy in y0..=y1 {
            for vz in z0..=z1 {
                if !is_solid(vx, vy, vz) {
                    continue;
                }
                hit = true;
                let cell = [vx, vy, vz][axis] as f32;
                if delta > 0.0 {
                    // Moving +: rest the AABB max face against the voxel's min face.
                    snapped = snapped.min(cell - half - COLLIDE_SKIN);
                } else {
                    // Moving -: rest the AABB min face against the voxel's max face.
                    snapped = snapped.max(cell + 1.0 + half + COLLIDE_SKIN);
                }
            }
        }
    }
    if hit {
        moved[axis] = snapped;
    }
    (moved, hit)
}

/// Cheap MTD-style push-out when the AABB already overlaps solids at rest
/// (spawn-in-block / sector pop-in). Tries the shortest axis separation first.
fn depenetrate_if_overlapping(
    is_solid: &impl Fn(i64, i64, i64) -> bool,
    center: Vec3,
) -> (Vec3, bool) {
    if !aabb_hits_solid(is_solid, center) {
        return (center, false);
    }

    let (min, max) = player_aabb(center);
    let (x0, x1) = voxel_span(min.x, max.x);
    let (y0, y1) = voxel_span(min.y, max.y);
    let (z0, z1) = voxel_span(min.z, max.z);

    let mut best: Option<(f32, usize, f32)> = None; // (|pen|, axis, signed push)
    for vx in x0..=x1 {
        for vy in y0..=y1 {
            for vz in z0..=z1 {
                if !is_solid(vx, vy, vz) {
                    continue;
                }
                let cell = [vx as f32, vy as f32, vz as f32];
                // Penetration depth on each axis (how far to push center to clear).
                let push_neg_x = (cell[0] + 1.0 + PLAYER_RADIUS + COLLIDE_SKIN) - center.x;
                let push_pos_x = center.x - (cell[0] - PLAYER_RADIUS - COLLIDE_SKIN);
                let push_neg_y = (cell[1] + 1.0 + PLAYER_HALF_HEIGHT + COLLIDE_SKIN) - center.y;
                let push_pos_y = center.y - (cell[1] - PLAYER_HALF_HEIGHT - COLLIDE_SKIN);
                let push_neg_z = (cell[2] + 1.0 + PLAYER_RADIUS + COLLIDE_SKIN) - center.z;
                let push_pos_z = center.z - (cell[2] - PLAYER_RADIUS - COLLIDE_SKIN);

                let candidates = [
                    (push_neg_x.abs(), 0, push_neg_x),
                    (push_pos_x.abs(), 0, -push_pos_x),
                    (push_neg_y.abs(), 1, push_neg_y),
                    (push_pos_y.abs(), 1, -push_pos_y),
                    (push_neg_z.abs(), 2, push_neg_z),
                    (push_pos_z.abs(), 2, -push_pos_z),
                ];
                for (pen, axis, signed) in candidates {
                    if pen <= 0.0 {
                        continue;
                    }
                    if best.is_none_or(|(b, _, _)| pen < b) {
                        best = Some((pen, axis, signed));
                    }
                }
            }
        }
    }

    if let Some((_, axis, signed)) = best {
        let mut out = center;
        out[axis] += signed;
        // Prefer a resolved pose; if still overlapping, leave the push (caller
        // may clear on a later frame / other axis).
        return (out, true);
    }
    (center, true)
}

/// Is a solid voxel directly beneath the feet (within `GROUND_PROBE`)?
fn grounded_below(is_solid: &impl Fn(i64, i64, i64) -> bool, center: Vec3) -> bool {
    let (min, max) = player_aabb(center);
    let (x0, x1) = voxel_span(min.x, max.x);
    let (z0, z1) = voxel_span(min.z, max.z);
    let fy = (min.y - GROUND_PROBE).floor() as i64;
    for vx in x0..=x1 {
        for vz in z0..=z1 {
            if is_solid(vx, fy, vz) {
                return true;
            }
        }
    }
    false
}

/// Integrate one movement step. Pure: no ECS access, fully unit-testable.
///
/// `is_solid(wx, wy, wz)` reports whether the world-space voxel at integer
/// coordinates is solid; the caller wires this to (possibly several) sector
/// `XBrickMap`s so collision works seamlessly across sector boundaries.
///
/// * Grounded: horizontal wish-dir from yaw + gravity + jump, with full
///   axis-separated AABB collision (walls, floors, ceilings).
/// * Flying: free 3D movement along the look direction, gravity/collision off.
#[allow(clippy::too_many_arguments)]
pub fn integrate_player(
    ctrl: &PlayerController,
    state: &PlayerState,
    input: &PlayerInput,
    look: &PlayerLook,
    is_solid: impl Fn(i64, i64, i64) -> bool,
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

    if state.flying {
        // Noclip-style free flight: no gravity, no collision.
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
        let delta = move3.normalize_or_zero() * ctrl.fly_speed * dt;
        return PlayerStepResult {
            new_position: position + delta,
            grounded: false,
            vy: 0.0,
        };
    }

    let grounded_now = grounded_below(&is_solid, position);

    // Vertical velocity: gravity, then jump (only from the ground).
    let mut vy = (state.vy - ctrl.gravity * dt).max(-TERMINAL_VELOCITY);
    if grounded_now && vy < 0.0 {
        vy = 0.0;
    }
    if input.jump && grounded_now {
        vy = ctrl.jump_speed;
    }

    let horiz = (forward * input.move_z + right * input.move_x).normalize_or_zero() * speed * dt;

    // Axis-separated sweep: resolve X, then Z, then Y independently so the player
    // slides along walls instead of sticking, and lands cleanly on floors.
    // Depenetrate first when already overlapping (spawn / sector pop-in).
    let (mut pos, _) = depenetrate_if_overlapping(&is_solid, position);
    let (p, _) = move_axis(&is_solid, pos, 0, horiz.x);
    pos = p;
    let (p, _) = move_axis(&is_solid, pos, 2, horiz.z);
    pos = p;
    let (p, hit_y) = move_axis(&is_solid, pos, 1, vy * dt);
    pos = p;

    // A vertical collision cancels vertical velocity (floor or ceiling).
    if hit_y {
        vy = 0.0;
    }

    let grounded = grounded_below(&is_solid, pos);

    PlayerStepResult {
        new_position: pos,
        grounded,
        vy,
    }
}

/// ECS system: apply [`integrate_player`] to the player entity and write back the
/// new `Transform` with a change-detection guard on [`PlayerState`].
#[allow(clippy::type_complexity)]
pub fn player_controller_system(
    time: Res<Time<Fixed>>,
    input: Res<PlayerInput>,
    pool: Res<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    sectors: Query<(Entity, &SectorCoord, &XBrickMap, &SectorPalette)>,
    mut player: Query<(
        &mut Transform,
        &PlayerController,
        &mut PlayerState,
        &PlayerLook,
    )>,
    mut sector_index: Local<std::collections::HashMap<SectorCoord, Entity>>,
) {
    // Re-use the Local HashMap buffer across ticks to eliminate per-tick heap allocations.
    sector_index.clear();
    for (entity, coord, _, _) in sectors.iter() {
        sector_index.insert(*coord, entity);
    }

    // Collision is against *solid* blocks only — liquids (water) and other
    // non-solid voxels must not stop the player, otherwise it stands on the sea
    // surface everywhere (a single flat Y level) and never touches real terrain.
    let is_solid = |wx: i64, wy: i64, wz: i64| -> bool {
        let sc = SectorCoord(
            wx.div_euclid(32) as i32,
            wy.div_euclid(32) as i32,
            wz.div_euclid(32) as i32,
        );
        if let Some(&entity) = sector_index.get(&sc)
            && let Ok((_, _, m, palette)) = sectors.get(entity)
        {
            let lx = wx.rem_euclid(32) as u32;
            let ly = wy.rem_euclid(32) as u32;
            let lz = wz.rem_euclid(32) as u32;
            let id = m.get_block(&pool, palette, VoxelCoord::new(lx, ly, lz));
            return id != BlockId::AIR && registry.is_solid(id);
        }
        false
    };

    for (mut tf, ctrl, mut st, look) in &mut player {
        // Don't free-fall through terrain that hasn't streamed in yet. On spawn
        // (and during heavy initial load) the sector under the player may not be
        // generated for several frames; falling meanwhile would drop the player
        // below the surface and bury it once the sector finally loads. While the
        // ground sector (just below the feet) is not resident, hold position and
        // resume the moment it appears — so the player always lands on the
        // surface. Flying ignores this (no gravity / no ground needed).
        if !st.flying {
            let feet_y = tf.translation.y - PLAYER_HALF_HEIGHT;
            let ground_sec = SectorCoord(
                (tf.translation.x / 32.0).floor() as i32,
                ((feet_y - 1.0) / 32.0).floor() as i32,
                (tf.translation.z / 32.0).floor() as i32,
            );
            if !sector_index.contains_key(&ground_sec) {
                st.set_if_neq(PlayerState {
                    grounded: false,
                    flying: st.flying,
                    vy: 0.0,
                });
                continue;
            }
        }

        let result = integrate_player(
            ctrl,
            &st,
            &input,
            look,
            is_solid,
            tf.translation,
            // Fixed timestep (default 64 Hz): movement is framerate-independent
            // and deterministic (plan 14 §D3), so tunneling/jitter that a
            // variable render dt would cause is avoided.
            time.delta_secs(),
        );

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
                )
                .expect("test floor set_block");
            }
        }
        (map, pool, palette)
    }

    /// Build a world-space occupancy probe from a single sector `XBrickMap`
    /// (mirrors the cross-sector closure used by `player_controller_system`).
    fn world_solid<'a>(
        map: &'a XBrickMap,
        pool: &'a GlobalBrickPool,
    ) -> impl Fn(i64, i64, i64) -> bool + 'a {
        let ox = map.coord.0 as i64 * 32;
        let oy = map.coord.1 as i64 * 32;
        let oz = map.coord.2 as i64 * 32;
        move |wx, wy, wz| {
            let lx = wx - ox;
            let ly = wy - oy;
            let lz = wz - oz;
            if (0..32).contains(&lx) && (0..32).contains(&ly) && (0..32).contains(&lz) {
                map.is_occupied(pool, VoxelCoord::new(lx as u32, ly as u32, lz as u32))
            } else {
                false
            }
        }
    }

    #[test]
    fn grounded_true_when_floor_below() {
        let (map, pool, _) = floor_sector();
        let ctrl = PlayerController::default();
        let state = PlayerState::default();
        let look = PlayerLook::default();
        // Feet rest on the top face of the floor voxel (y=1): center at y=2.
        let o = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 2.0, 5.0);
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &look,
            world_solid(&map, &pool),
            o,
            1.0 / 60.0,
        );
        assert!(r.grounded, "player should be grounded on the floor");
    }

    #[test]
    fn grounded_true_in_nonzero_y_sector() {
        // A floor in a sector with coord.1 != 0 must still register as ground.
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 1, 0));
        for x in 0..32u32 {
            for z in 0..32u32 {
                map.set_block(
                    &mut pool,
                    &mut palette,
                    VoxelCoord::new(x, 0, z),
                    BlockId(1),
                )
                .expect("test floor set_block");
            }
        }
        let ctrl = PlayerController::default();
        let state = PlayerState::default();
        let look = PlayerLook::default();
        let o = sector_world_origin(SectorCoord(0, 1, 0)) + Vec3::new(5.0, 2.0, 5.0);
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &look,
            world_solid(&map, &pool),
            o,
            1.0 / 60.0,
        );
        assert!(
            r.grounded,
            "player must be grounded on a floor in a Y=1 sector"
        );
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
            world_solid(&map, &pool),
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
        // Resting on top of the floor (feet at y=1, center at y=2).
        let o = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 2.0, 5.0);
        let jump = PlayerInput {
            jump: true,
            ..Default::default()
        };
        let r = integrate_player(
            &ctrl,
            &state,
            &jump,
            &look,
            world_solid(&map, &pool),
            o,
            1.0 / 60.0,
        );
        assert!(r.vy > 0.0, "jump should impart upward velocity");
        assert!(
            r.new_position.y > o.y,
            "jump should lift the player this frame"
        );
    }

    #[test]
    fn falls_and_rests_on_floor_surface() {
        // Drop the player from mid-air and integrate many frames; it must settle
        // resting exactly on top of the floor voxel (feet.y == 1) instead of
        // bouncing at a single level or sinking through.
        let (map, pool, _) = floor_sector();
        let is_solid = world_solid(&map, &pool);
        let ctrl = PlayerController::default();
        let look = PlayerLook::default();
        let mut state = PlayerState::default();
        let mut pos = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 20.0, 5.0);
        for _ in 0..600 {
            let r = integrate_player(
                &ctrl,
                &state,
                &PlayerInput::default(),
                &look,
                &is_solid,
                pos,
                1.0 / 60.0,
            );
            pos = r.new_position;
            state.vy = r.vy;
            state.grounded = r.grounded;
        }
        let feet = pos.y - PLAYER_HALF_HEIGHT;
        assert!(state.grounded, "player must settle grounded on the floor");
        assert!(
            (feet - 1.0).abs() < 0.05,
            "feet should rest on the floor top (y=1), got feet={feet}"
        );
    }

    #[test]
    fn horizontal_wall_blocks_movement() {
        // Floor plus a solid wall column at x=8 across the full height. Walking
        // +x into it must not let the player pass through the wall.
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        for x in 0..32u32 {
            for z in 0..32u32 {
                map.set_block(
                    &mut pool,
                    &mut palette,
                    VoxelCoord::new(x, 0, z),
                    BlockId(1),
                )
                .expect("test floor set_block");
            }
        }
        for y in 1..4u32 {
            for z in 0..32u32 {
                map.set_block(
                    &mut pool,
                    &mut palette,
                    VoxelCoord::new(8, y, z),
                    BlockId(1),
                )
                .expect("test wall set_block");
            }
        }
        let is_solid = world_solid(&map, &pool);
        let ctrl = PlayerController::default();
        // Face +x (yaw so that forward = +x). forward = (-sin(yaw), 0, -cos(yaw));
        // move_z=1 with yaw=-PI/2 gives forward=(+1,0,0).
        let look = PlayerLook {
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        };
        let input = PlayerInput {
            move_z: 1.0,
            ..Default::default()
        };
        let mut state = PlayerState {
            grounded: true,
            ..Default::default()
        };
        // Start standing on the floor at x=6, well left of the wall.
        let mut pos = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(6.0, 2.0, 5.0);
        for _ in 0..240 {
            let r = integrate_player(&ctrl, &state, &input, &look, &is_solid, pos, 1.0 / 60.0);
            pos = r.new_position;
            state.vy = r.vy;
            state.grounded = r.grounded;
        }
        // Wall occupies voxel x=8 (world [8,9)); the player's +x face (center + radius)
        // must stop before x=8.
        assert!(
            pos.x + PLAYER_RADIUS <= 8.0 + 1.0e-2,
            "player must be stopped by the wall, got x={}",
            pos.x
        );
    }

    #[test]
    fn terminal_velocity_is_clamped() {
        let pool = GlobalBrickPool::new();
        let map = XBrickMap::new(SectorCoord(0, 0, 0));
        let ctrl = PlayerController::default();
        let state = PlayerState {
            grounded: false,
            flying: false,
            vy: -200.0,
        };
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &PlayerLook::default(),
            world_solid(&map, &pool),
            Vec3::new(5.0, 40.0, 5.0),
            1.0 / 60.0,
        );
        assert!(
            r.vy >= -50.0 - 1.0e-3,
            "vy must be clamped to terminal velocity (−50), got {}",
            r.vy
        );
    }

    #[test]
    fn large_fall_delta_does_not_tunnel_through_floor() {
        // One-frame |vy*dt| ≫ 0.5 voxel: without sub-stepping the AABB leaps past
        // the floor cell and never overlaps it (classic discrete tunneling).
        let (map, pool, _) = floor_sector();
        let ctrl = PlayerController::default();
        let state = PlayerState {
            grounded: false,
            flying: false,
            vy: -80.0,
        };
        // Feet just above the floor top (y=1); center at ~2.2.
        let start = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 2.2, 5.0);
        let dt = 0.25; // |vy*dt| = 20 ≫ MAX_AXIS_STEP
        let r = integrate_player(
            &ctrl,
            &state,
            &PlayerInput::default(),
            &PlayerLook::default(),
            world_solid(&map, &pool),
            start,
            dt,
        );
        let feet = r.new_position.y - PLAYER_HALF_HEIGHT;
        assert!(
            feet >= 1.0 - 0.05,
            "must not tunnel below floor top (y=1), got feet={feet} (pos.y={})",
            r.new_position.y
        );
        assert!(
            r.grounded || r.vy.abs() < 1.0e-3,
            "fall should land/stop on the floor"
        );
    }

    #[test]
    fn depenetrates_when_spawned_inside_block() {
        let (map, pool, _) = floor_sector();
        // Center inside the floor voxel at y=0 → AABB overlaps solid.
        let buried = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(5.0, 0.5, 5.0);
        assert!(
            world_solid(&map, &pool)(
                buried.x.floor() as i64,
                buried.y.floor() as i64,
                buried.z.floor() as i64
            ),
            "precondition: spawn inside solid floor"
        );
        let r = integrate_player(
            &PlayerController::default(),
            &PlayerState::default(),
            &PlayerInput::default(),
            &PlayerLook::default(),
            world_solid(&map, &pool),
            buried,
            1.0 / 60.0,
        );
        let feet = r.new_position.y - PLAYER_HALF_HEIGHT;
        assert!(
            feet >= 1.0 - 0.05,
            "depenetration must push above the floor, got feet={feet}"
        );
    }
}
