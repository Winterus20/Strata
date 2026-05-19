use crate::mesher::{BoundingBox, MeshData, Mesher, Vertex};
use glam::Vec3;
use hashbrown::HashMap;
use strata_core::{BlockFace, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

/// Classic greedy meshing algorithm.
///
/// Iterates every slice along each axis, builds a 2D mask of visible faces
/// grouped by block type, merges adjacent same-type faces into larger quads,
/// and emits vertices with correct winding for backface culling.
pub struct ClassicGreedyMesher;

impl ClassicGreedyMesher {
    /// Returns `true` if the face of the block at `(x, y, z)` on the given
    /// `axis`/`dir` side should be rendered (i.e. the neighbor is air or OOB).
    #[inline]
    fn should_render_face(
        chunk: &Chunk,
        x: usize,
        y: usize,
        z: usize,
        axis: usize,
        dir: i32,
    ) -> bool {
        let (nx, ny, nz) = match (axis, dir) {
            (0, -1) => (x.wrapping_sub(1), y, z),
            (0, 1) => (x + 1, y, z),
            (1, -1) => (x, y.wrapping_sub(1), z),
            (1, 1) => (x, y + 1, z),
            (2, -1) => (x, y, z.wrapping_sub(1)),
            (2, 1) => (x, y, z + 1),
            _ => unreachable!(),
        };

        if nx >= CHUNK_WIDTH || ny >= CHUNK_HEIGHT || nz >= CHUNK_WIDTH {
            return true;
        }

        let neighbor = chunk.get_block(nx, ny, nz);
        neighbor.is_air()
    }

    /// Greedy-merges a 2D boolean mask into a minimal set of rectangles.
    fn greedy_merge(
        mask: &[Vec<bool>],
        width: usize,
        height: usize,
    ) -> Vec<(usize, usize, usize, usize)> {
        let mut rects = Vec::new();
        let mut visited = vec![vec![false; height]; width];

        for y in 0..height {
            for x in 0..width {
                if visited[x][y] || !mask[x][y] {
                    continue;
                }

                let mut w = 1;
                while x + w < width && mask[x + w][y] && !visited[x + w][y] {
                    w += 1;
                }

                let mut h = 1;
                let mut can_extend = true;
                while y + h < height && can_extend {
                    for dx in 0..w {
                        if !mask[x + dx][y + h] || visited[x + dx][y + h] {
                            can_extend = false;
                            break;
                        }
                    }
                    if can_extend {
                        h += 1;
                    }
                }

                for dy in 0..h {
                    for dx in 0..w {
                        visited[x + dx][y + dy] = true;
                    }
                }

                rects.push((x, y, w, h));
            }
        }

        rects
    }
}

impl Mesher for ClassicGreedyMesher {
    fn generate_mesh(&self, chunk: &Chunk) -> MeshData {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut vertex_offset = 0u32;

        let world_x = chunk.position.world_x() as f32;
        let world_z = chunk.position.world_z() as f32;

        if chunk.is_empty() {
            return MeshData::empty();
        }

        let y_start = chunk
            .heightmap_bottom
            .iter()
            .filter(|&&h| h > 0)
            .min()
            .copied()
            .unwrap_or(0) as usize;
        let y_end = chunk.heightmap_top.iter().max().copied().unwrap_or(0) as usize;

        for axis in 0..3 {
            for dir in [-1i32, 1] {
                // Main-axis range and the two slice dimensions (u, v).
                //   axis 0 (X): main = X(0..16),  u = Y(0..256), v = Z(0..16)
                //   axis 1 (Y): main = Y(0..256), u = X(0..16),  v = Z(0..16)
                //   axis 2 (Z): main = Z(0..16),  u = X(0..16),  v = Y(0..256)
                let (dim_main, dim_u, dim_v) = match axis {
                    0 => (CHUNK_WIDTH, CHUNK_HEIGHT, CHUNK_WIDTH),
                    1 => (CHUNK_HEIGHT, CHUNK_WIDTH, CHUNK_WIDTH),
                    2 => (CHUNK_WIDTH, CHUNK_WIDTH, CHUNK_HEIGHT),
                    _ => unreachable!(),
                };

                // Iterate every slice along the main axis.
                for d in 0..dim_main {
                    // Heightmap optimization: skip empty Y slices
                    if axis == 1 && (d < y_start || d > y_end) {
                        continue;
                    }
                    // Key: (block_id, face_id) so each face gets its own texture
                let mut masks: HashMap<(u16, u8), Vec<Vec<bool>>> = HashMap::new();

                for u in 0..dim_u {
                    for v in 0..dim_v {
                        let (x, y, z) = match axis {
                            0 => (d, u, v),
                            1 => (u, d, v),
                            2 => (u, v, d),
                            _ => unreachable!(),
                        };

                        if chunk.get_block(x, y, z).is_air() {
                            continue;
                        }

                        if !Self::should_render_face(chunk, x, y, z, axis, dir) {
                            continue;
                        }

                        let block_id = chunk.get_block(x, y, z).0;
                        let face = BlockFace::from_axis_dir(axis, dir);
                        let key = (block_id, face.id());
                        masks
                            .entry(key)
                            .or_insert_with(|| vec![vec![false; dim_v]; dim_u])[u][v] = true;
                    }
                }

                for ((block_id, face_id), mask) in &masks {
                        let rects = Self::greedy_merge(mask, dim_u, dim_v);

                        for (u_start, v_start, w, h) in rects {
                            // The face sits between block d and its neighbor.
                            // dir=+1 → face at d+1; dir=-1 → face at d.
                            let face_d = if dir == 1 { (d + 1) as f32 } else { d as f32 };

                            // Map (u, v) back to world-space positions.
                            // Y is absolute (0..256); X and Z need chunk offset.
                            let (p0, p1, p2, p3) = match axis {
                                0 => {
                                    // X-face in the YZ plane - CCW winding from outside
                                    let fx = world_x + face_d;
                                    let y0 = u_start as f32;
                                    let z0 = world_z + v_start as f32;
                                    (
                                        [fx, y0, z0],
                                        [fx, y0 + w as f32, z0],
                                        [fx, y0 + w as f32, z0 + h as f32],
                                        [fx, y0, z0 + h as f32],
                                    )
                                }
                                1 => {
                                    // Y-face in the XZ plane
                                    let fy = face_d;
                                    let x0 = world_x + u_start as f32;
                                    let z0 = world_z + v_start as f32;
                                    (
                                        [x0, fy, z0],
                                        [x0 + w as f32, fy, z0],
                                        [x0 + w as f32, fy, z0 + h as f32],
                                        [x0, fy, z0 + h as f32],
                                    )
                                }
                                2 => {
                                    // Z-face in the XY plane
                                    let fz = world_z + face_d;
                                    let x0 = world_x + u_start as f32;
                                    let y0 = v_start as f32;
                                    (
                                        [x0, y0, fz],
                                        [x0, y0 + h as f32, fz],
                                        [x0 + w as f32, y0 + h as f32, fz],
                                        [x0 + w as f32, y0, fz],
                                    )
                                }
                                _ => unreachable!(),
                            };

                            let normal = match axis {
                                0 => [dir as f32, 0.0, 0.0],
                                1 => [0.0, dir as f32, 0.0],
                                2 => [0.0, 0.0, dir as f32],
                                _ => unreachable!(),
                            };

                            let ao = 1.0;
                            let tex = get_texture_id(*block_id, *face_id);

                            vertices.push(Vertex {
                                position: p0,
                                normal,
                                uv: [0.0, 0.0],
                                ao,
                                texture_id: tex,
                                _padding: 0,
                            });
                            vertices.push(Vertex {
                                position: p1,
                                normal,
                                uv: [0.0, 1.0],
                                ao,
                                texture_id: tex,
                                _padding: 0,
                            });
                            vertices.push(Vertex {
                                position: p2,
                                normal,
                                uv: [1.0, 1.0],
                                ao,
                                texture_id: tex,
                                _padding: 0,
                            });
                            vertices.push(Vertex {
                                position: p3,
                                normal,
                                uv: [1.0, 0.0],
                                ao,
                                texture_id: tex,
                                _padding: 0,
                            });

                            // Y-axis (axis 1) winding is reversed because the vertex
                            // order in the XZ plane (X right, Z down) is CW when it
                            // needs to be CCW from above.  Axes 0 (YZ plane) and
                            // 2 (XY plane) use standard winding.
                            let rev = axis == 1;
                            if dir == 1 {
                                if rev {
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset + 1);
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 3);
                                    indices.push(vertex_offset + 2);
                                } else {
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 1);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset + 3);
                                }
                            } else {
                                if rev {
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 1);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset + 3);
                                } else {
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 2);
                                    indices.push(vertex_offset + 1);
                                    indices.push(vertex_offset);
                                    indices.push(vertex_offset + 3);
                                    indices.push(vertex_offset + 2);
                                }
                            }

                            vertex_offset += 4;
                        }
                    }
                }
            }
        }

        let bounds = BoundingBox {
            min: Vec3::new(world_x, 0.0, world_z),
            max: Vec3::new(
                world_x + CHUNK_WIDTH as f32,
                CHUNK_HEIGHT as f32,
                world_z + CHUNK_WIDTH as f32,
            ),
        };

        MeshData {
            vertex_count: vertices.len(),
            index_count: indices.len(),
            vertices,
            indices,
            bounds,
        }
    }

    fn name(&self) -> &str {
        "classic_greedy"
    }
}

/// Maps block ID and face ID to the corresponding 0-based texture layer index.
fn get_texture_id(block_id: u16, face_id: u8) -> u16 {
    match block_id {
        1 => 0, // STONE -> stone.png (layer 0)
        2 => 1, // DIRT -> dirt.png (layer 1)
        3 => {  // GRASS
            match face_id {
                3 => 1, // NegativeY (bottom) -> dirt.png (layer 1)
                4 => 2, // PositiveY (top) -> grass_top.png (layer 2)
                _ => 8, // sides -> grass_side.png (layer 8)
            }
        }
        4 => 3, // BEDROCK -> bedrock.png (layer 3)
        _ => 0,
    }
}
