//! Camera view uniform for the GPU depth pre-pass (M4c, plan 10 §2).
//!
//! A single `Pod` uniform holding the camera origin, the world->view matrix and
//! the view->clip projection matrix (both column-major) plus the framebuffer
//! dimensions needed by the fragment shader to linearise the visbuf index.

use bytemuck::{Pod, Zeroable};

/// Host mirror of the WGSL `CameraView` uniform (see `prepass.rs`).
///
/// Offsets (column-major `f32`):
/// * `0`   — `eye`      : `vec4<f32>` (xyz used, w padding)
/// * `16`  — `view`     : `mat4x4<f32>` (64 bytes)
/// * `80`  — `proj`     : `mat4x4<f32>` (64 bytes)
/// * `144` — `width`    : `u32`
/// * `148` — `height`   : `u32`
/// * `152` — `_pad`     : `u32` x2 (16-byte alignment)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct CameraView {
    pub eye: [f32; 4],
    pub view: [f32; 16],
    pub proj: [f32; 16],
    pub width: u32,
    pub height: u32,
    pub _pad: [u32; 2],
}

impl CameraView {
    /// Pack the matrices + framebuffer size into the GPU uniform struct.
    #[inline]
    pub fn new(eye: [f32; 3], view: [f32; 16], proj: [f32; 16], width: u32, height: u32) -> Self {
        Self {
            eye: [eye[0], eye[1], eye[2], 1.0],
            view,
            proj,
            width,
            height,
            _pad: [0; 2],
        }
    }
}

/// Right-handed perspective projection with a zero-to-one depth range, matching
/// WebGPU's clip-space convention (NDC z in `[0,1]`, 0 = near, 1 = far).
///
/// Column-major `f32` array (16 elements) consumed directly by WGSL `mat4x4`.
#[inline]
pub fn perspective_rh_zo(fovy_rad: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
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
