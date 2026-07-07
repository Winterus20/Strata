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
use strata_core::prelude::*;

/// Voxels per 32³ sector. Mirrors `strata_core::SECTOR_VOXEL_COUNT`.
pub const SECTOR_VOXELS: usize = 32 * 32 * 32;
const SECTOR_DIM: u32 = 32;
/// Maximum light level (4-bit channels are clamped to 0..=15).
pub const MAX_LIGHT: u8 = 15;

/// 6-neighbour offsets used by both sky and block-light propagation.
const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

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

/// Per-sector light result buffer (fixed 32³ array, no per-sector heap).
///
/// Light is *derived* data, so a fixed-size array (heap-free, deterministic) is
/// acceptable; it is never the live voxel store (which lives in the
/// `GlobalBrickPool`). Indexed by local [`VoxelCoord`].
#[derive(Debug, Clone, Copy, Component)]
pub struct SectorLight {
    pub data: [LightData; SECTOR_VOXELS],
}

impl Default for SectorLight {
    fn default() -> Self {
        SectorLight {
            data: [LightData(0); SECTOR_VOXELS],
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
#[inline]
fn coord_of(idx: usize) -> (u32, u32, u32) {
    let x = (idx % SECTOR_DIM as usize) as u32;
    let rest = idx / SECTOR_DIM as usize;
    let z = (rest % SECTOR_DIM as usize) as u32;
    let y = (rest / SECTOR_DIM as usize) as u32;
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
    ) -> SectorLight {
        let mut light = SectorLight::default();
        self.compute_sky(map, pool, palette, registry, &mut light);
        self.compute_block(map, pool, palette, registry, &mut light);
        light
    }

    /// Recompute a sector after an edit (block place/break). Full per-sector
    /// recompute is the documented M7 behaviour (correct, not yet incremental).
    pub fn apply_edit(
        &self,
        coord: SectorCoord,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
    ) -> SectorLight {
        self.compute_sector(coord, map, pool, palette, registry)
    }

    /// L0 sky light: column-first, top-down. A transparent voxel above the local
    /// terrain top receives `MAX`; each step down decreases by 1 until a
    /// non-transparent voxel blocks the column (interior below overhang = dark).
    fn compute_sky(
        &self,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) {
        for x in 0..SECTOR_DIM {
            for z in 0..SECTOR_DIM {
                let mut l: u8 = MAX_LIGHT;
                for y in (0..SECTOR_DIM).rev() {
                    let c = VoxelCoord::new(x, y, z);
                    let id = map.get_block(pool, palette, c);
                    if !registry.is_transparent(id) {
                        l = 0;
                        light.data[SectorLight::idx_of(c)].set_sky(0);
                    } else {
                        light.data[SectorLight::idx_of(c)].set_sky(l);
                        l = l.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// L1 block light: seed `LIGHT_SRC` voxels to their emission, then propagate
    /// with a Starlight-style increase queue (`-1` step, stop at 0). Solids do
    /// not receive block light.
    fn compute_block(
        &self,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) {
        for d in light.data.iter_mut() {
            d.set_block(0);
        }

        let mut increase: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for i in 0..SECTOR_VOXELS {
            let (x, y, z) = coord_of(i);
            let id = map.get_block(pool, palette, VoxelCoord::new(x, y, z));
            let emission = registry.light_emission(id);
            if emission > 0 {
                let e = emission.min(MAX_LIGHT);
                light.data[i].set_block(e);
                increase.push_back(i);
            }
        }

        self.propagate_increase(&mut increase, map, pool, palette, registry, light);
    }

    /// Increase pass: push `current - 1` into transparent neighbours when it
    /// exceeds their stored level. Terminates because levels are bounded/capped.
    fn propagate_increase(
        &self,
        queue: &mut std::collections::VecDeque<usize>,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) {
        while let Some(idx) = queue.pop_front() {
            let cur = light.data[idx].block();
            if cur == 0 {
                continue;
            }
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
                let nid = map.get_block(pool, palette, nc);
                if !registry.is_transparent(nid) {
                    continue;
                }
                let nidx = SectorLight::idx_of(nc);
                let new = cur.saturating_sub(1);
                if new > light.data[nidx].block() {
                    light.data[nidx].set_block(new);
                    queue.push_back(nidx);
                }
            }
        }
    }

    /// Two-phase removal of a light source (Starlight decrease + increase).
    ///
    /// Phase 1 (decrease): clear the source, then lower any voxel whose light
    /// exceeds what its neighbours can still supply. Phase 2 (increase):
    /// re-seed all remaining `LIGHT_SRC` voxels and re-propagate, refilling the
    /// field deterministically.
    pub fn remove_source(
        &self,
        source: VoxelCoord,
        map: &XBrickMap,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        registry: &BlockRegistry,
        light: &mut SectorLight,
    ) {
        let sidx = SectorLight::idx_of(source);
        light.data[sidx].set_block(0);

        let mut decrease: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        decrease.push_back(sidx);
        for n in self.neighbor_indices(sidx) {
            decrease.push_back(n);
        }

        while let Some(idx) = decrease.pop_front() {
            let cur = light.data[idx].block();
            let mut max_from_neighbors = 0u8;
            for n in self.neighbor_indices(idx) {
                let cand = light.data[n].block().saturating_sub(1);
                if cand > max_from_neighbors {
                    max_from_neighbors = cand;
                }
            }
            if cur > max_from_neighbors {
                light.data[idx].set_block(max_from_neighbors);
                for n in self.neighbor_indices(idx) {
                    decrease.push_back(n);
                }
            }
        }

        let mut increase: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for i in 0..SECTOR_VOXELS {
            let (x, y, z) = coord_of(i);
            let id = map.get_block(pool, palette, VoxelCoord::new(x, y, z));
            if registry.light_emission(id) > 0 {
                let e = registry.light_emission(id).min(MAX_LIGHT);
                light.data[i].set_block(e);
                increase.push_back(i);
            }
        }
        self.propagate_increase(&mut increase, map, pool, palette, registry, light);
    }

    #[inline]
    fn neighbor_indices(&self, idx: usize) -> impl Iterator<Item = usize> {
        let (x, y, z) = coord_of(idx);
        NEIGHBOR_OFFSETS
            .iter()
            .filter_map(move |(dx, dy, dz)| {
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
            .collect::<Vec<_>>()
            .into_iter()
    }
}

/// Strata lighting plugin (M7): registers the lighting system in the `Lighting`
/// set. Filter-First — only `Generated` sectors missing `SectorLight` or with a
/// freshly `Added<ChunkDirty>` are (re)computed; `ChunkDirty` is left in place
/// so other consumers (collider/meshing) can also observe it.
pub struct LightingPlugin;

impl StrataPlugin for LightingPlugin {
    fn name(&self) -> &'static str {
        "lighting"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<LightEngine>() {
            app.insert_resource(LightEngine::default());
        }
        app.add_systems(Update, lighting_system.in_set(StrataSet::Lighting));
    }
}

/// Filter-First lighting pass: recompute `SectorLight` for changed/generated
/// sectors and store it as a component on the sector entity.
#[allow(clippy::type_complexity)]
fn lighting_system(
    mut commands: Commands,
    registry: Res<BlockRegistry>,
    pool: Res<GlobalBrickPool>,
    engine: Res<LightEngine>,
    query: Query<
        (
            Entity,
            &SectorCoord,
            &XBrickMap,
            &SectorPalette,
            Option<&ChunkDirty>,
        ),
        (
            With<Generated>,
            Or<(Without<SectorLight>, Added<ChunkDirty>)>,
        ),
    >,
) {
    for (entity, coord, map, palette, _cd) in &query {
        let light = engine.compute_sector(*coord, map, &pool, palette, &registry);
        commands.entity(entity).insert(light);
    }
}

#[cfg(test)]
mod tests;
