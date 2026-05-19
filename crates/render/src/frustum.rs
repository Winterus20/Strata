use glam::{Mat4, Vec3, Vec4};

/// 6-plane view frustum for chunk visibility testing.
#[derive(Debug, Clone, Copy)]
pub struct Frustum {
    planes: [Vec4; 6],
}

impl Frustum {
    /// Extract frustum planes from view-projection matrix.
    ///
    /// Produces **outward-facing** normals for `test_aabb` (which culls when `dist > radius`).
    /// LH projection: clip z/w ∈ [0,1].
    pub fn from_view_projection(vp: Mat4) -> Self {
        let cols = vp.to_cols_array_2d();

        // `to_cols_array_2d()` returns `result[col][row]`, reconstruct rows:
        let row0 = [cols[0][0], cols[1][0], cols[2][0], cols[3][0]];
        let row1 = [cols[0][1], cols[1][1], cols[2][1], cols[3][1]];
        let row2 = [cols[0][2], cols[1][2], cols[2][2], cols[3][2]];
        let row3 = [cols[0][3], cols[1][3], cols[2][3], cols[3][3]];

        // Clip-space constraints (LH, z/w ∈ [0,1]):
        //   -w ≤ x ≤ w  →  x + w ≥ 0 ∧ w − x ≥ 0
        //   -w ≤ y ≤ w  →  y + w ≥ 0 ∧ w − y ≥ 0
        //    0 ≤ z ≤ w  →  z ≥ 0     ∧ w − z ≥ 0
        //
        // Each row_i · P = clip_i.  Inward = (row3 ± row_i) · P ≥ 0
        // Outward = −Inward (negated) so that dist > radius → culled.
        let left = outward_plane(row3, row0, 1.0);   // −(row3 + row0)
        let right = outward_plane(row3, row0, -1.0);  // row0 − row3
        let bottom = outward_plane(row3, row1, 1.0);  // −(row3 + row1)
        let top = outward_plane(row3, row1, -1.0);    // row1 − row3
        let near = negate(row2);                       // −row2
        let far = outward_plane(row3, row2, -1.0);     // row2 − row3

        let normalize = |p: Vec4| -> Vec4 {
            let len = p.truncate().length();
            if len > 0.0 { p / len } else { p }
        };

        Self {
            planes: [
                normalize(left),
                normalize(right),
                normalize(bottom),
                normalize(top),
                normalize(near),
                normalize(far),
            ],
        }
    }

    /// Test AABB against all 6 planes.
    pub fn test_aabb(&self, center: Vec3, half_extents: Vec3) -> bool {
        for &plane in &self.planes {
            let dist = plane.x * center.x + plane.y * center.y + plane.z * center.z + plane.w;
            let radius = half_extents.x * plane.x.abs()
                + half_extents.y * plane.y.abs()
                + half_extents.z * plane.z.abs();
            if dist > radius {
                return false;
            }
        }
        true
    }

    /// Test chunk visibility using its world position.
    pub fn test_chunk(&self, chunk_world_x: f32, chunk_world_z: f32) -> bool {
        let center = Vec3::new(chunk_world_x + 8.0, 128.0, chunk_world_z + 8.0);
        let half_extents = Vec3::new(8.0, 128.0, 8.0);
        self.test_aabb(center, half_extents)
    }
}

/// Outward-facing plane: `−(row3 + sign * row_i)`.
fn outward_plane(row3: [f32; 4], row_i: [f32; 4], sign: f32) -> Vec4 {
    Vec4::new(
        -row3[0] - sign * row_i[0],
        -row3[1] - sign * row_i[1],
        -row3[2] - sign * row_i[2],
        -row3[3] - sign * row_i[3],
    )
}

fn negate(v: [f32; 4]) -> Vec4 {
    Vec4::new(-v[0], -v[1], -v[2], -v[3])
}

impl Default for Frustum {
    fn default() -> Self {
        Self {
            planes: [Vec4::ZERO; 6],
        }
    }
}
