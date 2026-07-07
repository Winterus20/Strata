//! CPU frustum culling (M4d) — no GPU needed.
//!
//! [`cull_visible`] extracts the six frustum planes from `camera.proj *
//! camera.view` and tests each [`Aabb`] with the positive-vertex (p-vertex)
//! method, returning the indices of boxes that intersect the frustum.

use bytemuck::{Pod, Zeroable};

use crate::pipeline::CameraView;

/// Axis-aligned bounding box, `Pod` so it can be uploaded if ever needed.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    /// A box spanning the 32³ sector at world origin (used by the test).
    pub fn sector32() -> Self {
        Self {
            min: [0.0, 0.0, 0.0],
            max: [32.0, 32.0, 32.0],
        }
    }

    /// Merge `other` into this box (union). Used to fold a mesh's quads.
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        }
    }
}

/// Column-major `4x4` multiply (`(a * b)`), both inputs column-major. Result is
/// also column-major: `out[col*4 + row] = sum_k a[k*4 + row] * b[col*4 + k]`.
#[inline]
fn mat4_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0f32;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = s;
        }
    }
    out
}

/// Extract a single row `r` (`r` in `0..4`) from a column-major matrix.
#[inline]
fn mat4_row(m: &[f32; 16], r: usize) -> [f32; 4] {
    [m[r], m[4 + r], m[8 + r], m[12 + r]]
}

/// Gribb-Hartmann frustum planes from `proj * view` for a zero-to-one depth
/// (WebGPU/D3D) clip volume. Each plane is `(a, b, c, d)` normalized, with the
/// half-space `a*x + b*y + c*z + d >= 0` meaning "inside".
#[inline]
fn frustum_planes(view_proj: &[f32; 16]) -> [[f32; 4]; 6] {
    let r0 = mat4_row(view_proj, 0);
    let r1 = mat4_row(view_proj, 1);
    let r2 = mat4_row(view_proj, 2);
    let r3 = mat4_row(view_proj, 3);

    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];

    let raw = [
        add(r3, r0), // left
        sub(r3, r0), // right
        add(r3, r1), // bottom
        sub(r3, r1), // top
        r2,          // near (z >= 0 in [0,1])
        sub(r3, r2), // far
    ];

    let mut planes = [[0f32; 4]; 6];
    for (dst, p) in planes.iter_mut().zip(raw.iter()) {
        let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        if len > 1e-8 {
            *dst = [p[0] / len, p[1] / len, p[2] / len, p[3] / len];
        } else {
            *dst = *p;
        }
    }
    planes
}

/// Return the indices of all `boxes` that intersect the camera frustum.
///
/// A box is culled if, for any plane, its positive vertex lies outside (the
/// p-vertex distance is negative). Branchless per-component via `select`.
pub fn cull_visible(boxes: &[Aabb], camera: &CameraView) -> Vec<usize> {
    let view_proj = mat4_mul(&camera.proj, &camera.view);
    let planes = frustum_planes(&view_proj);

    let mut out = Vec::new();
    for (i, b) in boxes.iter().enumerate() {
        let mut inside = true;
        for p in planes.iter() {
            // Positive vertex: take max on the +axis side, min on the -axis side.
            let vx = if p[0] >= 0.0 { b.max[0] } else { b.min[0] };
            let vy = if p[1] >= 0.0 { b.max[1] } else { b.min[1] };
            let vz = if p[2] >= 0.0 { b.max[2] } else { b.min[2] };
            let dist = p[0] * vx + p[1] * vy + p[2] * vz + p[3];
            if dist < 0.0 {
                inside = false;
                break;
            }
        }
        if inside {
            out.push(i);
        }
    }
    out
}

/// Bounding box of a mesh's quads (sector-local coords treated as world coords).
/// Used by [`crate::pipeline::Renderer::render_frame`] to frustum-cull sectors.
pub fn aabb_of_mesh(mesh: &crate::meshing::MeshData) -> Aabb {
    let mut acc = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    let mut any = false;
    for q in mesh.opaque.iter().chain(mesh.transparent.iter()) {
        for corner in 0..4u8 {
            let c = q.corner_pos(corner);
            let p = [c[0] as f32, c[1] as f32, c[2] as f32];
            acc = acc.union(&Aabb { min: p, max: p });
            any = true;
        }
    }
    if !any {
        return Aabb::default();
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::camera::{look_at_rh, perspective_rh_zo};

    #[test]
    fn test_aabb_pod() {
        assert_eq!(std::mem::size_of::<Aabb>(), 24);
        // Compile-time Pod guarantee via the derive; this just documents intent.
        fn _assert_pod<T: bytemuck::Pod>() {}
        _assert_pod::<Aabb>();
    }

    #[test]
    fn test_cull_keeps_target_box() {
        let aspect = 1.0f32;
        let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_3, aspect, 0.1, 100.0);
        let eye = [0.0, 0.0, 5.0];
        let view = look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let cam = CameraView::new(eye, view, proj, 64, 64);

        // Box at the look-at target: clearly inside.
        let target = Aabb {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };
        // Box far behind the camera (negative z in view => behind near plane).
        let behind = Aabb {
            min: [-1.0, -1.0, 10.0],
            max: [1.0, 1.0, 12.0],
        };
        let boxes = [target, behind];
        let vis = cull_visible(&boxes, &cam);
        assert!(vis.contains(&0), "target box must be visible");
        assert!(!vis.contains(&1), "box behind camera must be culled");
    }
}
