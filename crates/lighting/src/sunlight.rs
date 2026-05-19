use strata_core::light::LightData;
use strata_core::{CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

/// Sky light propagation using BFS from top (Minecraft-style).
pub struct SunlightPropagator;

impl SunlightPropagator {
    /// Initialize sunlight for a newly generated chunk.
    pub fn init(blocks: &[u16], heightmap_top: &[u16; 256], light: &mut LightData) {
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let col = Chunk::column_index(x, z);
                let top = heightmap_top[col] as usize;

                for y in (top + 1)..CHUNK_HEIGHT {
                    let idx = Chunk::index(x, y, z);
                    light.set_sky(idx, 15);
                }

                if top > 0 {
                    Self::propagate_column(blocks, light, x, z, top);
                }
            }
        }
    }

    fn propagate_column(blocks: &[u16], light: &mut LightData, x: usize, z: usize, start_y: usize) {
        let mut current_light = 15u8;
        for y in (0..=start_y).rev() {
            let idx = Chunk::index(x, y, z);
            let block = blocks[idx];
            let is_air = block == 0;
            if is_air {
                light.set_sky(idx, current_light);
                current_light = current_light.saturating_sub(1);
            } else {
                light.set_sky(idx, 0);
                if current_light > 0 {
                    current_light = current_light.saturating_sub(1);
                }
            }
            if current_light == 0 {
                break;
            }
        }
    }

    /// BFS flood-fill propagation after a block change.
    pub fn propagate_bfs(blocks: &[u16], light: &mut LightData) {
        let mut queue = Vec::new();
        let mut visited = vec![false; 65536];

        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let y = CHUNK_HEIGHT - 1;
                let idx = Chunk::index(x, y, z);
                queue.push((x, y, z, 15u8));
                visited[idx] = true;
            }
        }

        while let Some((x, y, z, level)) = queue.pop() {
            let idx = Chunk::index(x, y, z);
            light.set_sky(idx, level);

            if level == 0 {
                continue;
            }

            for (dx, dy, dz) in &[
                (0, -1, 0),
                (0, 1, 0),
                (-1, 0, 0),
                (1, 0, 0),
                (0, 0, -1),
                (0, 0, 1),
            ] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let nz = z as i32 + dz;

                if nx < 0
                    || nx >= CHUNK_WIDTH as i32
                    || ny < 0
                    || ny >= CHUNK_HEIGHT as i32
                    || nz < 0
                    || nz >= CHUNK_WIDTH as i32
                {
                    continue;
                }

                let nidx = Chunk::index(nx as usize, ny as usize, nz as usize);
                if visited[nidx] {
                    continue;
                }

                let neighbor_block = blocks[nidx];
                let opacity = if neighbor_block == 0 { 1 } else { 15 };

                if level > opacity {
                    let new_level = level - opacity;
                    queue.push((nx as usize, ny as usize, nz as usize, new_level));
                    visited[nidx] = true;
                }
            }
        }
    }
}
