//! Input mapping (plan 14 §InputMapper): maps Bevy key/mouse state to a frame
//! [`PlayerInput`] snapshot that the controller and interaction systems consume.
//!
//! [`InputMapper::resolve`] is a pure function so it can be tested by injecting a
//! `ButtonInput<KeyCode>`/`ButtonInput<MouseButton>` directly — no window needed.

use bevy::input::keyboard::KeyCode;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;

use crate::interaction::{PlayerBreak, PlayerPlace};

/// High-level gameplay actions the player can perform (plan 11 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputAction {
    MoveX,
    MoveZ,
    Jump,
    Break,
    Place,
    HotbarNext,
}

/// Per-frame input snapshot produced by [`InputMapper::resolve`]. Stored as a
/// `Resource` so the controller/interaction systems read a single shared state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Resource)]
pub struct PlayerInput {
    /// Strafe axis: -1 left, +1 right.
    pub move_x: f32,
    /// Forward axis: -1 back, +1 forward.
    pub move_z: f32,
    pub jump: bool,
    pub sprint: bool,
    pub sneak: bool,
    pub break_block: bool,
    pub place_block: bool,
    pub hotbar_next: bool,
}

/// Resolves raw Bevy button state into a [`PlayerInput`].
pub struct InputMapper;

impl InputMapper {
    /// Pure mapping: keyboard WASD + Space + mouse buttons + Q (hotbar). Works on
    /// any `ButtonInput` state, so it is trivially unit-testable.
    pub fn resolve(keys: &ButtonInput<KeyCode>, mouse: &ButtonInput<MouseButton>) -> PlayerInput {
        let fwd = keys.pressed(KeyCode::KeyW) as i32 - keys.pressed(KeyCode::KeyS) as i32;
        let strafe = keys.pressed(KeyCode::KeyD) as i32 - keys.pressed(KeyCode::KeyA) as i32;
        PlayerInput {
            move_x: strafe as f32,
            move_z: fwd as f32,
            jump: keys.pressed(KeyCode::Space),
            sprint: keys.pressed(KeyCode::ShiftLeft),
            sneak: keys.pressed(KeyCode::ControlLeft),
            break_block: mouse.pressed(MouseButton::Left),
            place_block: mouse.pressed(MouseButton::Right),
            hotbar_next: keys.just_pressed(KeyCode::KeyQ),
        }
    }
}

/// ECS system: sample buttons and write the shared [`PlayerInput`], then emit
/// break/place events for the interaction systems to consume.
#[allow(clippy::too_many_arguments)]
pub fn input_mapper_system(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut input: ResMut<PlayerInput>,
    mut break_writer: MessageWriter<PlayerBreak>,
    mut place_writer: MessageWriter<PlayerPlace>,
) {
    let resolved = InputMapper::resolve(&keys, &mouse);
    // Change-detection guard: only flag the resource when it actually changed.
    input.set_if_neq(resolved);
    if resolved.break_block {
        break_writer.write(PlayerBreak);
    }
    if resolved.place_block {
        place_writer.write(PlayerPlace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_forward_and_strafe() {
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyW);
        keys.press(KeyCode::KeyD);
        let input = InputMapper::resolve(&keys, &mouse);
        assert_eq!(input.move_z, 1.0);
        assert_eq!(input.move_x, 1.0);
        assert!(!input.jump);
    }

    #[test]
    fn resolve_jump_and_break() {
        let mut keys = ButtonInput::<KeyCode>::default();
        let mut mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::Space);
        mouse.press(MouseButton::Left);
        let input = InputMapper::resolve(&keys, &mouse);
        assert!(input.jump);
        assert!(input.break_block);
        assert!(!input.place_block);
    }

    #[test]
    fn resolve_hotbar_next_edge() {
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyQ);
        let input = InputMapper::resolve(&keys, &mouse);
        assert!(input.hotbar_next);
    }
}
