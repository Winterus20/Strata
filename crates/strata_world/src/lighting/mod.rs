//! Lighting L0/L1 (M7): packed `LightData`, per-sector `SectorLight` storage,
//! and a `LightEngine` resource that computes sky (L0, column-first) and
//! block-light (L1, Starlight-style dual-queue BFS).
//!
//! CPU-only, deterministic, heap-free result buffers (the `SectorLight` array is
//! a fixed 32³ result buffer — NOT live voxel storage, which stays in the
//! `GlobalBrickPool` per AGENTS.md §7.G). No cross-sector propagation yet
//! (documented deviation): each `compute_sector` is self-contained.

use bevy::ecs::query::Or;
use bevy::math::Vec3;
use bevy::prelude::*;

use crate::plugin::Generated;
use crate::streaming::{StreamingManager, load_priority};

const LIGHTING_BUDGET: usize = 2;

/// Dial 16-bucket queue for BFS lighting propagation (plan 13 §1.3).
///
/// Light levels range 0..=15, so 16 buckets map directly to levels.
/// Dequeue is O(1) amortized via a rotating head pointer; insert is O(1).
struct DialQueue {
    buckets: [Vec<usize>; 16],
    head: usize,
}

impl DialQueue {
    fn new() -> Self {
        Self {
            buckets: Default::default(),
            head: 0,
        }
    }

    #[allow(dead_code)]
    fn clear(&mut self) {
        for b in &mut self.buckets {
            b.clear();
        }
        self.head = 0;
    }

    fn push(&mut self, idx: usize, level: u8) {
        let bucket = (self.head + level as usize) % 16;
        self.buckets[bucket].push(idx);
    }

    fn pop(&mut self) -> Option<(usize, u8)> {
        loop {
            if !self.buckets[self.head].is_empty() {
                let idx = self.buckets[self.head].pop().unwrap();
                let level = self.head as u8;
                return Some((idx, level));
            }
            self.head = (self.head + 1) % 16;
            if self.head == 0 {
                return None;
            }
        }
    }
}

/// Pooled visited bitset for `remove_source` (plan 13 §1.3).
///
/// Generation-stamped so the same allocation is reused across calls without
/// clearing the entire buffer each time.
struct VisitedPool {
    bits: Vec<u64>,
    generation: Vec<u32>,
    current_gen: u32,
}

impl VisitedPool {
    fn new() -> Self {
        let words = SECTOR_VOXELS.div_ceil(64);
        Self {
            bits: vec![0u64; words],
            generation: vec![0u32; words],
            current_gen: 0,
        }
    }

    fn reset(&mut self) {
        self.current_gen = self.current_gen.wrapping_add(1);
    }

    #[inline]
    fn is_visited(&self, idx: usize) -> bool {
        let word = idx >> 6;
        let bit = idx & 63;
        self.generation[word] == self.current_gen && (self.bits[word] & (1u64 << bit)) != 0
    }

    #[inline]
    fn mark(&mut self, idx: usize) {
        let word = idx >> 6;
        let bit = idx & 63;
        self.generation[word] = self.current_gen;
        self.bits[word] |= 1u64 << bit;
    }
}

/// Per-frame timings for L0/L1 light compute (surfaced to client DIAG).
#[derive(Resource, Default)]
pub struct LightingTimers {
    pub apply_us: u64,
    pub applied: usize,
    /// Sky light compute time (µs) — column-first + horizontal BFS.
    pub sky_us: u64,
    /// Block light compute time (µs) — seed scan + BFS propagation.
    pub block_us: u64,
    /// Number of voxels pushed through the sky horizontal BFS.
    pub sky_bfs_pushed: usize,
    /// Number of voxels pushed through the block-light BFS.
    pub block_bfs_pushed: usize,
    /// Number of light sources found during seed scan.
    pub light_sources: usize,
}
use strata_core::prelude::*;

/// Voxels per 32³ sector. Mirrors `strata_core::SECTOR_VOXEL_COUNT`.
pub const SECTOR_VOXELS: usize = 32 * 32 * 32;
const SECTOR_DIM: u32 = 32;
/// Maximum light level (4-bit channels are clamped to 0..=15).
pub const MAX_LIGHT: u8 = 15;

/// 6-neighbour offsets used by block-light propagation.
const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Horizontal-only offsets for sky BFS (plan 13 §1.4).
///
/// Vertical sky is handled by the column-first pass at full strength. Mixing
/// ±Y into the attenuation BFS lets light climb through a dug hole onto the
/// floor-surface air layer and paint a Manhattan "glow" on neighbouring tops.
const SKY_HORIZONTAL_OFFSETS: [(i32, i32, i32); 4] = [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)];

/// Packed 16-bit light value.
///
/// M7 representation (simplified single-channel, per plan 10 §1): 4 bits of
/// `sky` (bits 0..4) + 4 bits of `block` (bits 4..8). The full `r,g,b,s` 4×4-bit
/// layout (plan 13) is deferred; `block_r/g/b` all return the single block
/// channel so downstream shading can adopt per-channel later without API churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LightData(pub u16);

impl LightData {
    const SKY_SHIFT: u32 = 0;
    const BLOCK_SHIFT: u32 = 4;
    const CHANNEL_MASK: u16 = 0xF;

    /// Pack sky + block into one `u16`. Both are clamped to 0..=15.
    pub fn pack(sky: u8, block: u8) -> Self {
        LightData(
            ((sky.min(MAX_LIGHT) as u16) << Self::SKY_SHIFT)
                | ((block.min(MAX_LIGHT) as u16) << Self::BLOCK_SHIFT),
        )
    }

    #[inline]
    pub fn sky(&self) -> u8 {
        ((self.0 >> Self::SKY_SHIFT) & Self::CHANNEL_MASK) as u8
    }

    #[inline]
    pub fn block(&self) -> u8 {
        ((self.0 >> Self::BLOCK_SHIFT) & Self::CHANNEL_MASK) as u8
    }

    /// Red block-light channel (M7: identical to [`Self::block`]).
    #[inline]
    pub fn block_r(&self) -> u8 {
        self.block()
    }

    /// Green block-light channel (M7: identical to [`Self::block`]).
    #[inline]
    pub fn block_g(&self) -> u8 {
        self.block()
    }

    /// Blue block-light channel (M7: identical to [`Self::block`]).
    #[inline]
    pub fn block_b(&self) -> u8 {
        self.block()
    }

    #[inline]
    pub fn set_sky(&mut self, v: u8) {
        let v = (v.min(MAX_LIGHT) as u16) << Self::SKY_SHIFT;
        self.0 = (self.0 & !(Self::CHANNEL_MASK << Self::SKY_SHIFT)) | v;
    }

    #[inline]
    pub fn set_block(&mut self, v: u8) {
        let v = (v.min(MAX_LIGHT) as u16) << Self::BLOCK_SHIFT;
        self.0 = (self.0 & !(Self::CHANNEL_MASK << Self::BLOCK_SHIFT)) | v;
    }
}

/// Per-sector light result buffer (fixed 32³ array).
///
/// Light is *derived* data. The buffer is heap-allocated (`Box`) so ECS
/// insert/replace moves a pointer (~8 B) instead of memcpy'ing ~64 KiB when
/// the component is not `Copy`. Indexed by local [`VoxelCoord`].
#[derive(Debug, Clone, PartialEq, Component)]
pub struct SectorLight {
    pub data: Box<[LightData; SECTOR_VOXELS]>,
}

impl Default for SectorLight {
    fn default() -> Self {
        SectorLight {
            data: Box::new([LightData(0); SECTOR_VOXELS]),
        }
    }
}

impl SectorLight {
    #[inline]
    pub fn idx_of(c: VoxelCoord) -> usize {
        ((c.y as usize) * SECTOR_DIM as usize + c.z as usize) * SECTOR_DIM as usize + c.x as usize
    }

    #[inline]
    pub fn get(&self, c: VoxelCoord) -> LightData {
        self.data[Self::idx_of(c)]
    }

    #[inline]
    pub fn set(&mut self, c: VoxelCoord, v: LightData) {
        self.data[Self::idx_of(c)] = v;
    }

    #[inline]
    pub fn sky(&self, c: VoxelCoord) -> u8 {
        self.get(c).sky()
    }

    #[inline]
    pub fn block(&self, c: VoxelCoord) -> u8 {
        self.get(c).block()
    }
}

/// Decodes a flat `SectorLight` index back into a local `(x, y, z)` triple.
/// Layout: `idx = (y * 32 + z) * 32 + x`, packed as `bits[14:10]=y, [9:5]=z, [4:0]=x`.
#[inline]
fn coord_of(idx: usize) -> (u32, u32, u32) {
    let x = (idx & 0x1F) as u32;
    let z = ((idx >> 5) & 0x1F) as u32;
    let y = (idx >> 10) as u32;
    (x, y, z)
}

/// Drives L0 (sky) + L1 (block) lighting for the prototype.
///
/// Stateless per call: `compute_sector` fully recomputes one sector from the
/// live `XBrickMap`. Edits trigger a per-sector recompute (acceptable for M7;
/// see `apply_edit`). `sun_dir` is the directional sun uniform (reserved for
/// shading; sky light is column-first for M7).
#[derive(Debug, Clone, Copy, Resource)]
pub struct LightEngine {
    pub sun_dir: Vec3,
}

impl Default for LightEngine {
    fn default() -> Self {
        LightEngine {
            sun_dir: Vec3::new(0.4, 1.0, 0.3).normalize(),
        }
    }
}

impl LightEngine {
    pub fn new(sun_dir: Vec3) -> Self {
        LightEngine { sun_dir }
    }

    /// Compute sky + block light for one sector. `coord` is accepted for future
    /// cross-sector propagation and ignored in M7 (self-contained sector).
    pub fn compute_sector(
        &self,
        _coord: SectorCoord,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
    ) -> (SectorLight, LightingTimers) {
        let pool_guard = pool.read_inner();
        let mut light = SectorLight::default();
        let mut timers = LightingTimers {
            applied: 1,
            ..Default::default()
        };

        let t0 = std::time::Instant::now();
        let sky_bfs = self.compute_sky(map, &pool_guard, palette, registry, &mut light);
        timers.sky_us = t0.elapsed().as_micros() as u64;
        timers.sky_bfs_pushed = sky_bfs;

        let t1 = std::time::Instant::now();
        let (block_bfs, sources) =
            self.compute_block(map, &pool_guard, palette, registry, &mut light);
        timers.block_us = t1.elapsed().as_micros() as u64;
        timers.block_bfs_pushed = block_bfs;
        timers.light_sources = sources;
        timers.apply_us = timers.sky_us + timers.block_us;

        (light, timers)
    }

    /// Mutate `existing_light` after a known edit (block break/place).
    ///
    /// If the caller provides a [`DirtyVoxel`] describing the changed voxel and
    /// its old `BlockId`, this method performs **incremental** lighting:
    ///
    /// | Change | Sky | Block light |
    /// |--------|-----|-------------|
    /// | Break  | Column (x,z) recompute + horizontal BFS re-seed from that column | `remove_source` if old block was a light source |
    /// | Place  | Same column update | Propagate from the placed voxel if it emits light |
    ///
    /// Without a `DirtyVoxel` (e.g. first-generation sector), falls back to a
    /// full [`compute_sector`].
    #[allow(clippy::too_many_arguments)]
    pub fn apply_edit(
        &self,
        coord: SectorCoord,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        existing_light: &mut SectorLight,
        dirty: Option<&DirtyVoxel>,
    ) -> LightingTimers {
        let Some(dv) = dirty else {
            let (light, timers) = self.compute_sector(coord, map, pool, palette, registry);
            *existing_light = light;
            return timers;
        };

        let pool_guard = pool.read_inner();
        let mut timers = LightingTimers {
            applied: 1,
            ..Default::default()
        };

        // ---- Sky: column-only incremental ----
        let t0 = std::time::Instant::now();
        let sky_bfs = self.compute_sky_column(
            map,
            &pool_guard,
            palette,
            registry,
            existing_light,
            dv.voxel.x,
            dv.voxel.z,
        );
        timers.sky_us = t0.elapsed().as_micros() as u64;
        timers.sky_bfs_pushed = sky_bfs;

        // ---- Block: incremental ----
        let t1 = std::time::Instant::now();
        let (block_bfs, sources) =
            self.apply_block_incremental(map, &pool_guard, palette, registry, existing_light, dv);
        timers.block_us = t1.elapsed().as_micros() as u64;
        timers.block_bfs_pushed = block_bfs;
        timers.light_sources = sources;
        timers.apply_us = timers.sky_us + timers.block_us;

        timers
    }

    /// Sky light for a single (x,z) column only, then re-seed the horizontal
    /// BFS from sky-lit voxels in that column. ~32× cheaper than the full
    /// all-column scan for single-voxel edits.
    #[allow(clippy::too_many_arguments)]
    fn compute_sky_column(
        &self,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
        cx: u32,
        cz: u32,
    ) -> usize {
        // Phase 1: recompute the single column top-down.
        let mut blocked = false;
        for y in (0..SECTOR_DIM).rev() {
            let c = VoxelCoord::new(cx, y, cz);
            let id = map.get_block_locked(pool, palette, c);
            let sky = if !registry.is_transparent(id) {
                blocked = true;
                0
            } else if blocked {
                0
            } else {
                MAX_LIGHT
            };
            light.data[SectorLight::idx_of(c)].set_sky(sky);
        }

        // Phase 2: seed horizontal BFS from this column only.
        let mut bfs = DialQueue::new();
        for y in 0..SECTOR_DIM {
            let idx = SectorLight::idx_of(VoxelCoord::new(cx, y, cz));
            if light.data[idx].sky() == 0 {
                continue;
            }
            for (dx, _dy, dz) in SKY_HORIZONTAL_OFFSETS {
                let nx = cx as i32 + dx;
                let nz = cz as i32 + dz;
                if nx < 0 || nz < 0 || nx >= SECTOR_DIM as i32 || nz >= SECTOR_DIM as i32 {
                    continue;
                }
                let nc = VoxelCoord::new(nx as u32, y, nz as u32);
                let nidx = SectorLight::idx_of(nc);
                if light.data[nidx].sky() == 0 {
                    let nid = map.get_block_locked(pool, palette, nc);
                    if registry.is_transparent(nid) {
                        bfs.push(idx, 1);
                        break;
                    }
                }
            }
        }

        // Run the same horizontal BFS as compute_sky.
        let mut sky_bfs_count = 0usize;
        while let Some((idx, level)) = bfs.pop() {
            sky_bfs_count += 1;
            let cur = light.data[idx].sky();
            let new = cur.saturating_sub(1);
            if new == 0 {
                continue;
            }
            let (x, y, z) = coord_of(idx);
            for (dx, _dy, dz) in SKY_HORIZONTAL_OFFSETS {
                let nx = x as i32 + dx;
                let nz = z as i32 + dz;
                if nx < 0 || nz < 0 || nx >= SECTOR_DIM as i32 || nz >= SECTOR_DIM as i32 {
                    continue;
                }
                let nc = VoxelCoord::new(nx as u32, y, nz as u32);
                let nidx = SectorLight::idx_of(nc);
                if light.data[nidx].sky() < new {
                    let nid = map.get_block_locked(pool, palette, nc);
                    if registry.is_transparent(nid) {
                        light.data[nidx].set_sky(new);
                        bfs.push(nidx, level);
                    }
                }
            }
        }
        sky_bfs_count
    }

    /// Incremental block-light update after a single-voxel edit.
    fn apply_block_incremental(
        &self,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
        dv: &DirtyVoxel,
    ) -> (usize, usize) {
        let idx = SectorLight::idx_of(dv.voxel);

        // If the old block was a light source, remove its contribution first.
        let old_emission = registry.light_emission(dv.old_block);
        let mut bfs_count = 0usize;
        if old_emission > 0 {
            self.remove_source(dv.voxel, map, pool, palette, registry, light);
        }

        // If the current block emits light (placed a torch, etc.), seed and propagate.
        let current_id = map.get_block_locked(pool, palette, dv.voxel);
        let current_emission = registry.light_emission(current_id);
        let mut sources = 0usize;
        if current_emission > 0 {
            let e = current_emission.min(MAX_LIGHT);
            light.data[idx].set_block(e);
            let mut increase = DialQueue::new();
            increase.push(idx, e);
            bfs_count = self.propagate_increase(&mut increase, map, pool, palette, registry, light);
            sources = 1;
        }

        (bfs_count, sources)
    }

    /// L0 sky light: column-first top-down, then **horizontal-only** BFS spread.
    ///
    /// Phase 1 (column): sunlight falls straight down at **full strength** — no
    /// per-step attenuation — until an opaque voxel blocks the column.
    /// Phase 2 (horizontal BFS, plan 13 §1.4): sky-lit voxels adjacent to
    /// non-sky-lit transparent voxels seed a BFS that spreads with level-1
    /// decay on ±X/±Z only, stopping at opaque blocks. Vertical attenuation
    /// BFS is intentionally omitted — otherwise light climbs through dug holes
    /// onto floor-surface air and paints a Manhattan glow on neighbour tops.
    fn compute_sky(
        &self,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) -> usize {
        // Phase 1: column-first, top-down.
        for x in 0..SECTOR_DIM {
            for z in 0..SECTOR_DIM {
                let mut blocked = false;
                for y in (0..SECTOR_DIM).rev() {
                    let c = VoxelCoord::new(x, y, z);
                    let id = map.get_block_locked(pool, palette, c);
                    let sky = if !registry.is_transparent(id) {
                        blocked = true;
                        0
                    } else if blocked {
                        0
                    } else {
                        MAX_LIGHT
                    };
                    light.data[SectorLight::idx_of(c)].set_sky(sky);
                }
            }
        }

        // Phase 2: horizontal-only BFS with level-1 decay (plan 13 §1.4).
        // Seed: any sky-lit voxel with a transparent non-sky-lit *horizontal* neighbour.
        let mut bfs = DialQueue::new();
        for i in 0..SECTOR_VOXELS {
            if light.data[i].sky() == 0 {
                continue;
            }
            let (x, y, z) = coord_of(i);
            for (dx, _dy, dz) in SKY_HORIZONTAL_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32;
                let nz = z as i32 + dz;
                if nx < 0
                    || ny < 0
                    || nz < 0
                    || nx >= SECTOR_DIM as i32
                    || ny >= SECTOR_DIM as i32
                    || nz >= SECTOR_DIM as i32
                {
                    continue;
                }
                let nc = VoxelCoord::new(nx as u32, ny as u32, nz as u32);
                let nidx = SectorLight::idx_of(nc);
                if light.data[nidx].sky() == 0 {
                    let nid = map.get_block_locked(pool, palette, nc);
                    if registry.is_transparent(nid) {
                        bfs.push(i, 1);
                        break;
                    }
                }
            }
        }

        let mut sky_bfs_count = 0usize;
        while let Some((idx, level)) = bfs.pop() {
            sky_bfs_count += 1;
            let cur = light.data[idx].sky();
            let new = cur.saturating_sub(1);
            if new == 0 {
                continue;
            }
            let (x, y, z) = coord_of(idx);
            for (dx, _dy, dz) in SKY_HORIZONTAL_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32;
                let nz = z as i32 + dz;
                if nx < 0
                    || ny < 0
                    || nz < 0
                    || nx >= SECTOR_DIM as i32
                    || ny >= SECTOR_DIM as i32
                    || nz >= SECTOR_DIM as i32
                {
                    continue;
                }
                let nc = VoxelCoord::new(nx as u32, ny as u32, nz as u32);
                let nidx = SectorLight::idx_of(nc);
                if light.data[nidx].sky() < new {
                    let nid = map.get_block_locked(pool, palette, nc);
                    if registry.is_transparent(nid) {
                        light.data[nidx].set_sky(new);
                        bfs.push(nidx, level);
                    }
                }
            }
        }
        sky_bfs_count
    }

    /// L1 block light: seed `LIGHT_SRC` voxels to their emission, then propagate
    /// with a Starlight-style increase queue (`-1` step, stop at 0). Solids do
    /// not receive block light.
    fn compute_block(
        &self,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) -> (usize, usize) {
        for d in light.data.iter_mut() {
            d.set_block(0);
        }

        let mut increase = DialQueue::new();
        let mut sources = 0usize;
        for i in 0..SECTOR_VOXELS {
            let (x, y, z) = coord_of(i);
            let id = map.get_block_locked(pool, palette, VoxelCoord::new(x, y, z));
            let emission = registry.light_emission(id);
            if emission > 0 {
                let e = emission.min(MAX_LIGHT);
                light.data[i].set_block(e);
                increase.push(i, e);
                sources += 1;
            }
        }

        let bfs_count = self.propagate_increase(&mut increase, map, pool, palette, registry, light);
        (bfs_count, sources)
    }

    /// Increase pass: push `current - 1` into transparent neighbours when it
    /// exceeds their stored level. Terminates because levels are bounded/capped.
    fn propagate_increase(
        &self,
        queue: &mut DialQueue,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) -> usize {
        let mut count = 0usize;
        while let Some((idx, _level)) = queue.pop() {
            count += 1;
            let cur = light.data[idx].block();
            let (x, y, z) = coord_of(idx);
            for (dx, dy, dz) in NEIGHBOR_OFFSETS {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                let nz = z as i32 + dz;
                if nx < 0
                    || ny < 0
                    || nz < 0
                    || nx >= SECTOR_DIM as i32
                    || ny >= SECTOR_DIM as i32
                    || nz >= SECTOR_DIM as i32
                {
                    continue;
                }
                let nc = VoxelCoord::new(nx as u32, ny as u32, nz as u32);
                let nid = map.get_block_locked(pool, palette, nc);
                if !registry.is_transparent(nid) {
                    continue;
                }
                let nidx = SectorLight::idx_of(nc);
                let new = cur.saturating_sub(1);
                if new > light.data[nidx].block() {
                    light.data[nidx].set_block(new);
                    queue.push(nidx, new);
                }
            }
        }
        count
    }

    /// Two-phase removal of a light source (canonical push-model decrease + boundary re-propagation).
    ///
    /// Phase 1 (push decrease): BFS outward from the removed source. For each
    /// node, check if its light level is less than the source's old level minus
    /// distance (i.e. it was dependent on this source). If dependent: zero it
    /// and enqueue. If not dependent: it's a boundary source — save for Phase 2.
    /// Phase 2 (increase): re-propagate from boundary sources only (not all
    /// emitters), restoring coverage from remaining lights.
    ///
    /// Uses a pooled, generation-stamped visited set (plan 13 §1.3) to ensure
    /// each node is processed at most once, bringing complexity from O(15N) to O(N).
    pub fn remove_source(
        &self,
        source: VoxelCoord,
        map: &XBrickMap,
        pool: &InnerPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) {
        let sidx = SectorLight::idx_of(source);

        // Save the source's old level before zeroing.
        let old_level = light.data[sidx].block();
        light.data[sidx].set_block(0);

        // Phase 1: push-model BFS from removed source.
        // Each entry stores (index, level_at_source) — the level the removed
        // source contributed at that distance.
        //
        // Visited set: pooled, generation-stamped bitset — no per-call allocation.
        let mut visited = VisitedPool::new();
        visited.reset();
        visited.mark(sidx);

        let mut decrease = DialQueue::new();
        // Seed neighbors of the source with level = old_level - 1.
        let seed_level = old_level.saturating_sub(1);
        for n in self.neighbor_indices(sidx) {
            if !visited.is_visited(n) {
                visited.mark(n);
                decrease.push(n, seed_level);
            }
        }

        let mut boundary_sources: Vec<usize> = Vec::new();

        while let Some((idx, src_contrib)) = decrease.pop() {
            let cur = light.data[idx].block();
            if cur == 0 {
                // Already dark — nothing to propagate through.
                continue;
            }

            if cur <= src_contrib {
                // Dependent on the removed source: zero and propagate.
                light.data[idx].set_block(0);
                let next_level = src_contrib.saturating_sub(1);
                if next_level > 0 {
                    for n in self.neighbor_indices(idx) {
                        if !visited.is_visited(n) {
                            visited.mark(n);
                            decrease.push(n, next_level);
                        }
                    }
                }
            } else {
                // Not dependent (>= what the removed source contributed):
                // this voxel is sustained by another source — it's a boundary
                // for Phase 2 re-propagation.
                boundary_sources.push(idx);
            }
        }

        // Phase 2: re-propagate from boundary sources only.
        let mut increase = DialQueue::new();
        for &idx in &boundary_sources {
            let lvl = light.data[idx].block();
            increase.push(idx, lvl);
        }
        self.propagate_increase(&mut increase, map, pool, palette, registry, light);
    }

    #[inline]
    fn neighbor_indices(&self, idx: usize) -> impl Iterator<Item = usize> {
        let (x, y, z) = coord_of(idx);
        // Return the filter_map iterator directly (no `collect::<Vec>()`): it
        // captures the owned `(x,y,z)` and borrows only the `'static` offset
        // table, so no heap allocation happens per call — `remove_source` calls
        // this thousands of times.
        NEIGHBOR_OFFSETS.iter().filter_map(move |(dx, dy, dz)| {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            let nz = z as i32 + dz;
            if nx < 0
                || ny < 0
                || nz < 0
                || nx >= SECTOR_DIM as i32
                || ny >= SECTOR_DIM as i32
                || nz >= SECTOR_DIM as i32
            {
                None
            } else {
                Some(SectorLight::idx_of(VoxelCoord::new(
                    nx as u32, ny as u32, nz as u32,
                )))
            }
        })
    }
}

/// Strata lighting plugin (M7): registers the lighting system in the `Lighting`
/// set. Filter-First — only `Generated` sectors missing `SectorLight` or with
/// `ChunkDirty` are (re)computed. `ChunkDirty` is cleared here after a successful
/// pass: Physics (earlier in the `StrataSet` chain) and save (`Added<ChunkDirty>`)
/// already observed it this frame. Leaving it forever re-lit every frame.
///
/// Does **not** insert `NeedsRemesh`: light-only updates refresh via
/// `Changed<SectorLight>` → client lightmap re-upload. Geometry remesh is owned
/// by interaction / `mark_generated_for_remesh` (Meshing runs before Lighting).
pub struct LightingPlugin;

impl StrataPlugin for LightingPlugin {
    fn name(&self) -> &'static str {
        "lighting"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<LightEngine>() {
            app.insert_resource(LightEngine::default());
        }
        app.init_resource::<LightingTimers>();
        app.add_systems(Update, lighting_system.in_set(StrataSet::Lighting));
    }
}

/// Filter-First lighting pass: recompute `SectorLight` for changed/generated
/// sectors and store it as a component on the sector entity.
///
/// For sectors carrying a [`DirtyVoxel`] (single-voxel edit), uses incremental
/// column-only sky + single-source block light via [`LightEngine::apply_edit`].
/// First-generation sectors fall back to full [`LightEngine::compute_sector`].
#[allow(clippy::type_complexity)]
fn lighting_system(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    pool: Res<GlobalBrickPool>,
    engine: Res<LightEngine>,
    streaming: Option<Res<StreamingManager>>,
    mut timers: ResMut<LightingTimers>,
    mut query: Query<
        (
            Entity,
            &SectorCoord,
            &XBrickMap,
            &SectorPalette,
            Option<&mut SectorLight>,
            Option<&DirtyVoxel>,
        ),
        (
            With<Generated>,
            Or<(Without<SectorLight>, With<ChunkDirty>)>,
        ),
    >,
) {
    timers.apply_us = 0;
    timers.applied = 0;
    timers.sky_us = 0;
    timers.block_us = 0;
    timers.sky_bfs_pushed = 0;
    timers.block_bfs_pushed = 0;
    timers.light_sources = 0;
    let player = streaming
        .as_ref()
        .map(|s| s.player_sector)
        .unwrap_or(SectorCoord(0, 0, 0));
    let move_dir = streaming
        .as_ref()
        .map(|s| s.move_dir)
        .unwrap_or(SectorCoord(0, 0, 0));

    let t0 = std::time::Instant::now();
    let mut pending: Vec<(
        Entity,
        SectorCoord,
        &XBrickMap,
        &SectorPalette,
        Option<Mut<SectorLight>>,
        Option<&DirtyVoxel>,
    )> = Vec::new();
    for (entity, coord, map, palette, light, dv) in &mut query {
        pending.push((entity, *coord, map, palette, light, dv));
    }
    pending.sort_by_key(|(_, c, _, _, _, _)| load_priority(player, move_dir, *c));
    let mut budget = LIGHTING_BUDGET;
    for (entity, coord, map, palette, existing_light, dirty) in pending {
        if budget == 0 {
            break;
        }

        let t;
        if let Some(mut current_light) = existing_light {
            // Already has SectorLight — use incremental path.
            // Clone to compute the new value, then use set_if_neq to only
            // trigger Changed<SectorLight> when the value actually differs.
            // This avoids the explicit O(32K) comparison in user code —
            // set_if_neq handles it internally and conditionally updates the
            // change tick, preventing GPU lightmap re-dirty on no-op updates.
            let mut new_light = current_light.clone();
            t = engine.apply_edit(coord, map, &pool, palette, &registry, &mut new_light, dirty);
            current_light.set_if_neq(new_light);
            commands
                .entity(entity)
                .remove::<ChunkDirty>()
                .remove::<DirtyVoxel>();
        } else {
            // First generation — full compute.
            let (light, full_t) = engine.compute_sector(coord, map, &pool, palette, &registry);
            t = full_t;
            commands
                .entity(entity)
                .insert(light)
                .remove::<ChunkDirty>()
                .remove::<DirtyVoxel>();
        }

        budget -= 1;
        timers.applied += 1;
        timers.sky_us += t.sky_us;
        timers.block_us += t.block_us;
        timers.sky_bfs_pushed += t.sky_bfs_pushed;
        timers.block_bfs_pushed += t.block_bfs_pushed;
        timers.light_sources += t.light_sources;
    }
    if timers.applied > 0 {
        timers.apply_us = t0.elapsed().as_micros() as u64;
    }
}

#[cfg(test)]
mod tests;
