//! Strata core: shared ECS components, math types, and voxel data structures.
//!
//! This crate intentionally has no GPU/windowing dependencies so it can be
//! shared with a future headless server build without pulling in rendering.

pub mod change_detection;
pub mod component;
pub mod core_plugin;
pub mod plugin;
pub mod registry;
pub mod sets;
pub mod xbrickmap;

#[cfg(test)]
mod tests;

pub mod prelude {
    pub use crate::change_detection::*;
    pub use crate::component::*;
    pub use crate::core_plugin::*;
    pub use crate::plugin::*;
    pub use crate::registry::*;
    pub use crate::sets::*;
    pub use crate::xbrickmap::*;
    pub use bevy::prelude::*;
}
