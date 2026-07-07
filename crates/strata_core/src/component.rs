//! Core ECS value types and marker components for Strata sectors.
//!
//! Per the Filter-First discipline (AGENTS.md §3.A), transient state is encoded
//! as zero-sized marker components (`ChunkDirty`, `NeedsRemesh`, `NeedsBake`)
//! and queried with archetype-level `With<T>`/`Without<T>` filters — never a
//! per-entity `if option.is_some()` check.

use bevy::prelude::*;

/// A 32³ sector coordinate in sector-space (not voxel-space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Component)]
pub struct SectorCoord(pub i32, pub i32, pub i32);

/// Authoritative per-sector transform state. `tier` is the live streaming tier.
#[derive(Debug, Clone, Copy, Component)]
pub struct SectorTransform {
    pub coord: SectorCoord,
    pub tier: Tier,
}

/// Spawn-only immutable sector entity. Carries no mutable streaming state.
#[derive(Debug, Clone, Copy, Component)]
pub struct SectorEntity {
    pub coord: SectorCoord,
}

/// Streaming tier. The prototype only models the `Active` tier (`08` minimal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    Active,
}

/// Marker: sector data changed and needs downstream processing.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct ChunkDirty;

/// Marker: sector mesh must be rebuilt.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct NeedsRemesh;

/// Marker: sector needs an SVDAG bake.
#[derive(Debug, Component)]
#[component(storage = "SparseSet")]
pub struct NeedsBake;

/// Diagnostic counter written by the filter-first demo system.
#[derive(Debug, Resource, Default)]
pub struct DirtySectorCount(pub u32);

/// Internal test component used to prove the change-detection guard.
#[cfg(test)]
#[derive(Debug, Component, PartialEq, Clone)]
pub(crate) struct Counter(pub u32);

/// Filter-first demonstration: counts only sectors flagged `ChunkDirty`.
///
/// No per-entity `if` check — the `With<ChunkDirty>` filter excludes
/// non-dirty archetypes at the archetype level (AGENTS.md §3.A).
pub fn count_dirty_sectors(
    dirty: Query<&SectorCoord, With<ChunkDirty>>,
    mut count: ResMut<DirtySectorCount>,
) {
    count.0 = dirty.iter().count() as u32;
}
