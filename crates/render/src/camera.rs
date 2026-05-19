use glam::{Mat4, Vec3};

/// Represents a 3D camera with position, yaw/pitch rotation, and projection parameters.
#[derive(Debug, Clone)]
pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::new(0.0, 64.0, 0.0),
            yaw: -core::f32::consts::PI,
            pitch: 0.0,
            fov: 70.0_f32.to_radians(),
            aspect,
            near: 0.1,
            far: 512.0,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let forward = Vec3::new(cos_yaw * cos_pitch, sin_pitch, sin_yaw * cos_pitch);
        let target = self.position + forward;
        Mat4::look_at_lh(self.position, target, Vec3::Y)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        Mat4::perspective_lh(self.fov, self.aspect, self.near, self.far)
    }

    pub fn view_projection_matrix(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Returns the 8 corners of the view frustum in world space.
    pub fn frustum_corners(&self) -> [Vec3; 8] {
        let vp_inv = self.view_projection_matrix().inverse();
        let ndc_corners = [
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        let mut world = [Vec3::ZERO; 8];
        for (i, ndc) in ndc_corners.iter().enumerate() {
            world[i] = vp_inv.project_point3(*ndc);
        }
        world
    }
}

/// Simple camera controller that reads input values and updates the camera.
#[derive(Debug, Clone)]
pub struct CameraController {
    pub move_speed: f32,
    pub mouse_sensitivity: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            move_speed: 10.0,
            mouse_sensitivity: 0.003,
        }
    }
}

/// Raw input deltas for camera update in a single frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct CameraInput {
    pub forward: f32,
    pub strafe: f32,
    pub vertical: f32,
    pub yaw_delta: f32,
    pub pitch_delta: f32,
}

impl CameraController {
    pub fn new(move_speed: f32, mouse_sensitivity: f32) -> Self {
        Self {
            move_speed,
            mouse_sensitivity,
        }
    }

    /// Update camera position/yaw/pitch from raw input deltas.
    pub fn update_camera(&self, camera: &mut Camera, dt: f32, input: CameraInput) {
        camera.yaw -= input.yaw_delta * self.mouse_sensitivity;
        camera.pitch -= input.pitch_delta * self.mouse_sensitivity;
        camera.pitch = camera.pitch.clamp(-1.55, 1.55);

        let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
        let forward_dir = Vec3::new(cos_yaw, 0.0, sin_yaw).normalize();
        let right_dir = Vec3::new(sin_yaw, 0.0, -cos_yaw).normalize();

        let velocity =
            forward_dir * input.forward + right_dir * input.strafe + Vec3::Y * input.vertical;
        if velocity.length_squared() > 0.0 {
            camera.position += velocity.normalize() * self.move_speed * dt;
        }
    }
}
