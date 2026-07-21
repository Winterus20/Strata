//! Core voxel pipeline ordering sets (prototype skeleton).
//!
//! Enforces `Streaming -> Input -> WorldGen -> Meshing -> Physics -> Lighting
//! -> RenderUpdate`. Streaming runs first (pre-Input) so freshly spawned sector
//! entities are available to `WorldGen` in the same frame; it is an addition to
//! the plan's canonical `Input -> WorldGen -> Meshing -> Physics -> Lighting ->
//! RenderUpdate` chain for entity lifecycle management.

use bevy::prelude::*;

/// Core scheduling sets for the single-player prototype pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, SystemSet)]
pub enum StrataSet {
    /// Sector load/unload around the player (M9). Runs before `WorldGen` so the
    /// newly spawned sector entities are generated in the same frame.
    Streaming,
    /// Input sampling + write-back from SubApps.
    Input,
    /// Generate sector data.
    WorldGen,
    /// Build mesh (async apply).
    Meshing,
    /// Physics step.
    Physics,
    /// L0/L1 lighting.
    Lighting,
    /// Upload to GPU.
    RenderUpdate,
}
