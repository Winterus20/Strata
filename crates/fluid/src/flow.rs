use strata_core::{BlockId, CHUNK_DEPTH, CHUNK_HEIGHT, CHUNK_WIDTH, Chunk};

use crate::water_level::{ChunkWaterLevels, WaterLevel};

/// Work queue entry for BFS water flow propagation.
#[derive(Debug, Clone, Copy)]
struct FlowWork {
    /// Flat index in the chunk array.
    idx: usize,
    /// Water level at this position.
    level: u8,
}

/// Minecraft-style water flow simulation.
///
/// Flow rules:
/// - Water flows DOWN first (gravity)
/// - Water flows HORIZONTALLY to adjacent air blocks
/// - Water NEVER flows UP
/// - Water level decreases by 1 per horizontal step from source
/// - Source blocks (level 15) maintain their level
/// - Flowing water (levels 1–14) spreads until it reaches level 0
pub struct WaterFlow;

impl WaterFlow {
    /// Simulates one tick of water flow for a chunk.
    ///
    /// This performs a BFS flood-fill from all source blocks,
    /// propagating water downward and horizontally.
    ///
    /// Returns `true` if any water levels changed.
    pub fn tick(chunk: &Chunk, water: &mut ChunkWaterLevels) -> bool {
        if !water.has_water() {
            return false;
        }

        let mut work_queue: Vec<FlowWork> = Vec::with_capacity(4096);

        // Phase 1: Collect all source blocks as starting points
        Self::collect_sources(chunk, water, &mut work_queue);

        // Phase 2: BFS propagation from sources
        let mut changed = Self::propagate(chunk, water, &mut work_queue);

        // Phase 3: Fill downward (gravity) for all water columns
        changed |= Self::fill_downward(chunk, water);

        changed
    }

    /// Collects all source blocks (natural water) into the work queue.
    fn collect_sources(chunk: &Chunk, water: &ChunkWaterLevels, work_queue: &mut Vec<FlowWork>) {
        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_DEPTH {
                for y in 0..CHUNK_HEIGHT {
                    let idx = Chunk::index(x, y, z);
                    let block = chunk.blocks[idx];
                    let wl = water.get_at(idx);

                    // Source blocks: natural water (level 15)
                    if block == BlockId::WATER.0 && wl.level() == WaterLevel::MAX {
                        work_queue.push(FlowWork {
                            idx,
                            level: WaterLevel::MAX,
                        });
                    }
                }
            }
        }
    }

    /// BFS propagation from source blocks.
    fn propagate(
        chunk: &Chunk,
        water: &mut ChunkWaterLevels,
        work_queue: &mut Vec<FlowWork>,
    ) -> bool {
        let mut changed = false;
        let mut visited = vec![false; CHUNK_WIDTH * CHUNK_HEIGHT * CHUNK_DEPTH];
        let mut head = 0;

        // Mark initial sources as visited
        for work in work_queue.iter() {
            visited[work.idx] = true;
        }

        while head < work_queue.len() {
            let current = work_queue[head];
            head += 1;

            if current.level <= 1 {
                continue;
            }

            let new_level = current.level - 1;
            let x = current.idx % CHUNK_WIDTH;
            let z = (current.idx / CHUNK_WIDTH) % CHUNK_DEPTH;
            let y = current.idx / (CHUNK_WIDTH * CHUNK_DEPTH);

            // Check 4 horizontal neighbors
            let neighbors = [
                (x.wrapping_sub(1), y, z),
                (x + 1, y, z),
                (x, y, z.wrapping_sub(1)),
                (x, y, z + 1),
            ];

            for (nx, ny, nz) in neighbors {
                if nx < CHUNK_WIDTH && ny < CHUNK_HEIGHT && nz < CHUNK_DEPTH {
                    let nidx = Chunk::index(nx, ny, nz);
                    if !visited[nidx] {
                        let neighbor_block = chunk.blocks[nidx];
                        let neighbor_water = water.get_at(nidx);

                        // Flow into air blocks or lower water levels
                        if (neighbor_block == BlockId::AIR.0 || neighbor_block == BlockId::WATER.0)
                            && neighbor_water.level() < new_level
                        {
                            water.set_at(nidx, WaterLevel::from_raw(new_level));
                            changed = true;
                            visited[nidx] = true;
                            work_queue.push(FlowWork {
                                idx: nidx,
                                level: new_level,
                            });
                        }
                    }
                }
            }
        }

        changed
    }

    /// Fills water downward in each column (gravity simulation).
    ///
    /// For each column, water flows down to fill empty spaces below it.
    fn fill_downward(chunk: &Chunk, water: &mut ChunkWaterLevels) -> bool {
        let mut changed = false;

        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_DEPTH {
                let col = Chunk::column_index(x, z);
                let top = chunk.heightmap_top[col] as usize;
                if top == 0 {
                    continue;
                }

                // Scan from bottom to top, filling water downward
                for y in (0..=top.min(CHUNK_HEIGHT - 1)).rev() {
                    let idx = Chunk::index(x, y, z);
                    let wl = water.get_at(idx);

                    // If this block has water and there's air below, fill it
                    if wl.level() > 0 && y > 0 {
                        let below_idx = Chunk::index(x, y - 1, z);
                        let below_block = chunk.blocks[below_idx];
                        let below_water = water.get_at(below_idx);

                        if below_block == BlockId::AIR.0 && below_water.is_empty() {
                            water.set_at(below_idx, WaterLevel::from_raw(wl.level()));
                            changed = true;
                        }
                    }
                }
            }
        }

        changed
    }

    /// Syncs water levels at chunk borders with neighbor chunks.
    ///
    /// This ensures water flows correctly across chunk boundaries.
    pub fn sync_borders(
        _chunk: &mut Chunk,
        water: &mut ChunkWaterLevels,
        _neighbor: &Chunk,
        neighbor_water: &ChunkWaterLevels,
        face: usize,
    ) {
        match face {
            // Our -X border = neighbor's +X face (x=15)
            0 => {
                for z in 0..CHUNK_DEPTH {
                    for y in 0..CHUNK_HEIGHT {
                        let our_x = 0;
                        let their_x = CHUNK_WIDTH - 1;
                        let our_idx = Chunk::index(our_x, y, z);
                        let their_idx = Chunk::index(their_x, y, z);

                        if neighbor_water.get_at(their_idx).level() > water.get_at(our_idx).level()
                        {
                            water.set_at(our_idx, neighbor_water.get_at(their_idx));
                        }
                    }
                }
            }
            // Our +X border = neighbor's -X face (x=0)
            1 => {
                for z in 0..CHUNK_DEPTH {
                    for y in 0..CHUNK_HEIGHT {
                        let our_x = CHUNK_WIDTH - 1;
                        let their_x = 0;
                        let our_idx = Chunk::index(our_x, y, z);
                        let their_idx = Chunk::index(their_x, y, z);

                        if neighbor_water.get_at(their_idx).level() > water.get_at(our_idx).level()
                        {
                            water.set_at(our_idx, neighbor_water.get_at(their_idx));
                        }
                    }
                }
            }
            // Our -Z border = neighbor's +Z face (z=15)
            2 => {
                for x in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let our_z = 0;
                        let their_z = CHUNK_DEPTH - 1;
                        let our_idx = Chunk::index(x, y, our_z);
                        let their_idx = Chunk::index(x, y, their_z);

                        if neighbor_water.get_at(their_idx).level() > water.get_at(our_idx).level()
                        {
                            water.set_at(our_idx, neighbor_water.get_at(their_idx));
                        }
                    }
                }
            }
            // Our +Z border = neighbor's -Z face (z=0)
            3 => {
                for x in 0..CHUNK_WIDTH {
                    for y in 0..CHUNK_HEIGHT {
                        let our_z = CHUNK_DEPTH - 1;
                        let their_z = 0;
                        let our_idx = Chunk::index(x, y, our_z);
                        let their_idx = Chunk::index(x, y, their_z);

                        if neighbor_water.get_at(their_idx).level() > water.get_at(our_idx).level()
                        {
                            water.set_at(our_idx, neighbor_water.get_at(their_idx));
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
    }
}
