//! Strata render: wgpu pipeline, meshing, and visibility buffer.
//!
//! Client-only crate. GPU/windowing features (`bevy_render`, `bevy_pbr`,
//! `bevy_winit`, `bevy_audio`) live here and must never leak into the shared
//! crates, so a future headless server can be built without them.

pub mod meshing;
pub mod pipeline;
pub mod prelude {
    pub use strata_core::prelude::*;
}
