//! Camera view uniform for the GPU depth pre-pass (M4c, plan 10 §2).
//!
//! A single `Pod` uniform holding the camera origin, the world->view matrix and
//! the view->clip projection matrix (both column-major) plus the framebuffer
//! dimensions needed by the fragment shader to linearise the visbuf index.

use bytemuck::{Pod, Zeroable};

/// Host mirror of the WGSL `CameraView` uniform (see `prepass.rs`).
///
/// Offsets (column-major `f32`):
/// * `0`   — `eye`          : `vec4<f32>` (xyz used, w padding, 16 bytes)
/// * `16`  — `view`         : `mat4x4<f32>` (64 bytes)
/// * `80`  — `proj`         : `mat4x4<f32>` (64 bytes)
/// * `144` — `inv_view_proj`: `mat4x4<f32>` (64 bytes)
/// * `208` — `width`        : `u32` (4 bytes)
/// * `212` — `height`       : `u32` (4 bytes)
/// * `216` — `_pad`         : `u32` x2 (8 bytes; pads total to 224, WGSL 16-byte aligned)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct CameraView {
    pub eye: [f32; 4],
    pub view: [f32; 16],
    pub proj: [f32; 16],
    pub inv_view_proj: [f32; 16],
    pub width: u32,
    pub height: u32,
    pub _pad: [u32; 2],
}

impl CameraView {
    /// Pack the matrices + framebuffer size into the GPU uniform struct.
    #[inline]
    pub fn new(eye: [f32; 3], view: [f32; 16], proj: [f32; 16], width: u32, height: u32) -> Self {
        let view_proj = mul_mat4(&proj, &view);
        let inv_view_proj = invert_mat4(&view_proj).unwrap_or_else(|| {
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ]
        });
        Self {
            eye: [eye[0], eye[1], eye[2], 1.0],
            view,
            proj,
            inv_view_proj,
            width,
            height,
            _pad: [0; 2],
        }
    }
}

fn mul_mat4(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[row + k * 4] * b[k + col * 4];
            }
            out[row + col * 4] = sum;
        }
    }
    out
}

fn invert_mat4(m: &[f32; 16]) -> Option<[f32; 16]> {
    let mut inv = [0.0; 16];
    let s0 = m[0] * m[5] - m[1] * m[4];
    let s1 = m[0] * m[6] - m[2] * m[4];
    let s2 = m[0] * m[7] - m[3] * m[4];
    let s3 = m[1] * m[6] - m[2] * m[5];
    let s4 = m[1] * m[7] - m[3] * m[5];
    let s5 = m[2] * m[7] - m[3] * m[6];

    let c0 = m[8] * m[13] - m[9] * m[12];
    let c1 = m[8] * m[14] - m[10] * m[12];
    let c2 = m[8] * m[15] - m[11] * m[12];
    let c3 = m[9] * m[14] - m[10] * m[13];
    let c4 = m[9] * m[15] - m[11] * m[13];
    let c5 = m[10] * m[15] - m[11] * m[14];

    let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;
    if !det.is_finite() || det.abs() < 1e-8 {
        return None;
    }
    let inv_det = 1.0 / det;

    inv[0] = (m[5] * c5 - m[6] * c4 + m[7] * c3) * inv_det;
    inv[1] = (-m[1] * c5 + m[2] * c4 - m[3] * c3) * inv_det;
    inv[2] = (m[13] * s5 - m[14] * s4 + m[15] * s3) * inv_det;
    inv[3] = (-m[9] * s5 + m[10] * s4 - m[11] * s3) * inv_det;

    inv[4] = (-m[4] * c5 + m[6] * c2 - m[7] * c1) * inv_det;
    inv[5] = (m[0] * c5 - m[2] * c2 + m[3] * c1) * inv_det;
    inv[6] = (-m[12] * s5 + m[14] * s2 - m[15] * s1) * inv_det;
    inv[7] = (m[8] * s5 - m[10] * s2 + m[11] * s1) * inv_det;

    inv[8] = (m[4] * c4 - m[5] * c2 + m[7] * c0) * inv_det;
    inv[9] = (-m[0] * c4 + m[1] * c2 - m[3] * c0) * inv_det;
    inv[10] = (m[12] * s4 - m[13] * s2 + m[15] * s0) * inv_det;
    inv[11] = (-m[8] * s4 + m[9] * s2 - m[11] * s0) * inv_det;

    inv[12] = (-m[4] * c3 + m[5] * c1 - m[6] * c0) * inv_det;
    inv[13] = (m[0] * c3 - m[1] * c1 + m[2] * c0) * inv_det;
    inv[14] = (-m[12] * s3 + m[13] * s1 - m[14] * s0) * inv_det;
    inv[15] = (m[8] * s3 - m[9] * s1 + m[10] * s0) * inv_det;

    Some(inv)
}

/// Right-handed perspective projection with a zero-to-one depth range, matching
/// WebGPU's clip-space convention (NDC z in `[0,1]`, 0 = near, 1 = far).
///
/// Column-major `f32` array (16 elements) consumed directly by WGSL `mat4x4`.
#[inline]
pub fn perspective_rh_zo(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    debug_assert!(far > near, "perspective: far must be > near");
    let f = 1.0 / (fovy_rad * 0.5).tan();
    let mut m = [0f32; 16];
    m[0] = f / aspect;
    m[5] = f;
    m[10] = far / (near - far);
    m[11] = -1.0;
    m[14] = (far * near) / (near - far);
    m
}

/// Right-handed `look_at` view matrix (WebGPU convention: looking down +z after
/// transform). Returns a column-major `f32` array.
#[inline]
pub fn look_at_rh(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> [f32; 16] {
    let f = normalize(sub(center, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);

    [
        s[0],
        u[0],
        -f[0],
        0.0,
        s[1],
        u[1],
        -f[1],
        0.0,
        s[2],
        u[2],
        -f[2],
        0.0,
        -dot(s, eye),
        -dot(u, eye),
        dot(f, eye),
        1.0,
    ]
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= 1e-8 {
        v
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invert_mat4_singular_returns_none() {
        let singular = [1.0; 16];
        assert!(invert_mat4(&singular).is_none());
    }

    #[test]
    fn invert_mat4_nan_returns_none() {
        let nan_input = [
            f32::NAN,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        assert!(invert_mat4(&nan_input).is_none());

        let inf_input = [
            f32::INFINITY,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        assert!(invert_mat4(&inf_input).is_none());

        let neg_inf_input = [
            f32::NEG_INFINITY,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ];
        assert!(invert_mat4(&neg_inf_input).is_none());
    }

    #[test]
    fn camera_view_handles_singular_matrix() {
        let view = [1.0; 16];
        let proj = [1.0; 16];
        let cv = CameraView::new([0.0, 0.0, 0.0], view, proj, 64, 64);
        assert_eq!(
            cv.inv_view_proj,
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0
            ]
        );
    }
}
