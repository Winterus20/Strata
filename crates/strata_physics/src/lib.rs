//! Strata physics: Rapier3D voxel colliders and character-controller scaffolding (M6).
//!
//! Builds one `Voxels` collider per generated sector from the sector's occupied
//! voxels, keeps it in sync with block edits via `Voxels::set_voxel`, and provides
//! a CPU branchless ground probe plus a kinematic character-controller resource.

#![allow(ambiguous_glob_reexports)]

pub mod plugin;
pub mod voxel_collider;

#[cfg(test)]
mod tests;

pub mod prelude {
    pub use crate::plugin::*;
    pub use crate::voxel_collider::*;
    pub use bevy_rapier3d::prelude::*;
    pub use strata_core::prelude::*;
}
