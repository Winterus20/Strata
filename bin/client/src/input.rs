use winit::event::ElementState;
use winit::keyboard::{Key, NamedKey};

#[derive(Default)]
pub struct InputState {
    pub move_forward: bool,
    pub move_backward: bool,
    pub move_left: bool,
    pub move_right: bool,
    pub jump: bool,
    pub sprint: bool,
    mouse_dx: f64,
    mouse_dy: f64,
}

impl InputState {
    /// Returns net forward direction: positive = forward, negative = backward.
    pub fn forward(&self) -> i8 {
        (self.move_forward as i8) - (self.move_backward as i8)
    }

    /// Returns net strafe direction: positive = right, negative = left.
    pub fn strafe(&self) -> i8 {
        (self.move_right as i8) - (self.move_left as i8)
    }

    /// Returns accumulated cursor delta since last update, then resets.
    pub fn cursor_delta(&mut self) -> (f32, f32) {
        let dx = self.mouse_dx as f32;
        let dy = self.mouse_dy as f32;
        (dx, dy)
    }

    pub fn handle_keyboard_input(&mut self, key: &Key, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        match key {
            Key::Character(c) => match c.as_str() {
                "w" | "W" => self.move_forward = pressed,
                "s" | "S" => self.move_backward = pressed,
                "a" | "A" => self.move_left = pressed,
                "d" | "D" => self.move_right = pressed,
                " " | "\u{00a0}" => self.jump = pressed,
                _ => {}
            },
            Key::Named(NamedKey::Space) => self.jump = pressed,
            Key::Named(NamedKey::Shift) => self.sprint = pressed,
            _ => {}
        }
    }

    pub fn handle_mouse_motion(&mut self, delta: winit::dpi::PhysicalPosition<f64>) {
        self.mouse_dx += delta.x;
        self.mouse_dy += delta.y;
    }

    pub fn update(&mut self) {
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
    }
}
