use strata_core::CHUNK_VOLUME;
use strata_core::light::LightData;

/// Block light propagation (torch, lava, glowstone).
pub struct BlockLightPropagator;

impl BlockLightPropagator {
    /// Initialize block light from emitting blocks.
    pub fn init(blocks: &[u16], light: &mut LightData, light_emission: &[u8]) {
        let mut sources = Vec::new();
        for (idx, &block_id) in blocks.iter().enumerate().take(CHUNK_VOLUME) {
            let emission = light_emission.get(block_id as usize).copied().unwrap_or(0);
            if emission > 0 {
                sources.push((idx, emission));
            }
        }
        Self::propagate_bfs(blocks, light, &sources);
    }

    /// BFS flood fill from light sources.
    pub fn propagate_bfs(blocks: &[u16], light: &mut LightData, sources: &[(usize, u8)]) {
        use std::collections::BinaryHeap;
        let mut heap = BinaryHeap::new();
        let mut visited = vec![false; CHUNK_VOLUME];

        for &(idx, level) in sources {
            if level > 0 && !visited[idx] {
                heap.push((level, idx));
                visited[idx] = true;
            }
        }

        while let Some((level, idx)) = heap.pop() {
            light.set_block(idx, level);
            if level <= 1 {
                continue;
            }

            let x = idx % 16;
            let z = (idx / 16) % 16;
            let y = idx / 256;

            let neighbors = [
                (x.wrapping_sub(1), y, z),
                (x + 1, y, z),
                (x, y.wrapping_sub(1), z),
                (x, y + 1, z),
                (x, y, z.wrapping_sub(1)),
                (x, y, z + 1),
            ];

            for (nx, ny, nz) in neighbors {
                if nx < 16 && ny < 256 && nz < 16 {
                    let nidx = nx + nz * 16 + ny * 256;
                    if !visited[nidx] {
                        let opacity = if blocks[nidx] == 0 { 1 } else { 15 };
                        let new_level = level.saturating_sub(opacity);
                        if new_level > 0 {
                            visited[nidx] = true;
                            heap.push((new_level, nidx));
                        }
                    }
                }
            }
        }
    }

    /// Re-initialize after block change (full re-propagation for now).
    pub fn on_block_change(blocks: &[u16], light: &mut LightData, light_emission: &[u8]) {
        Self::init(blocks, light, light_emission);
    }
}
