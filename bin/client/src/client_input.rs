//! Client-side FPS input: pointer capture + mouse-look (window-only).
//!
//! The `strata_player` crate is deliberately headless (its controller reads a
//! [`PlayerLook`] but never touches a window), so the two window-dependent
//! bits — grabbing/hiding the OS cursor and turning raw mouse motion into
//! yaw/pitch — live here in the client binary.
//!
//! * [`cursor_grab_system`] locks + hides the cursor when the player clicks the
//!   window and releases it on `Escape` (classic FPS capture behaviour).
//! * [`mouse_look_system`] feeds [`AccumulatedMouseMotion`] into the player's
//!   [`PlayerLook`] while the cursor is grabbed.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use strata_player::PlayerLook;

/// Radians of look rotation per pixel of mouse motion (~0.126°/px).
const MOUSE_SENSITIVITY: f32 = 0.0022;
/// Pitch clamp just shy of ±90° so the view never flips over the poles.
const PITCH_LIMIT: f32 = 1.54;

/// Click-to-capture / `Escape`-to-release the OS cursor on the primary window.
///
/// winit 0.30 supports [`CursorGrabMode::Locked`] on Windows, so we lock (rather
/// than merely confine) the cursor and hide it, giving raw relative motion for
/// the look system.
pub fn cursor_grab_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut cursor) = cursor.single_mut() else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) && cursor.grab_mode == CursorGrabMode::None {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    } else if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Turn accumulated mouse motion into yaw/pitch on the player's [`PlayerLook`].
///
/// Only runs while the cursor is grabbed, so releasing with `Escape` freezes the
/// view and lets the pointer move freely over the window.
pub fn mouse_look_system(
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<Ref<CursorOptions>, With<PrimaryWindow>>,
    mut look: Query<&mut PlayerLook>,
) {
    let Ok(cursor) = cursor.single() else {
        return;
    };
    if cursor.grab_mode == CursorGrabMode::None {
        return;
    }
    // The frame we grab, the OS warps the cursor to centre, which can surface as
    // one huge motion delta that would fling the view (often straight down into
    // the terrain -> a gray screen). Skip look on any frame the grab state just
    // toggled so that jump is discarded.
    if cursor.is_changed() {
        return;
    }

    let delta = motion.delta;
    if delta == Vec2::ZERO {
        return;
    }

    for mut look in &mut look {
        // Mouse right (delta.x > 0) turns the view right; mouse up (delta.y < 0)
        // pitches the view up. See `build_camera` for the yaw/pitch→forward map.
        look.yaw -= delta.x * MOUSE_SENSITIVITY;
        look.pitch = (look.pitch - delta.y * MOUSE_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
}
