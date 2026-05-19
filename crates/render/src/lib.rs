//! Rendering crate for the Strata voxel engine.
//!
//! Provides GPU abstractions over wgpu: [`RenderEngine`] for window management
//! and frame submission, texture arrays, frustum culling, and chunk mesh management.

pub mod camera;
pub mod chunk_renderer;
pub mod crosshair;
pub mod engine;
pub mod frustum;
pub mod pipeline;
pub mod texture_manager;

pub use camera::Camera;
pub use chunk_renderer::ChunkRenderer;
pub use engine::{RenderEngine, RenderOutput};
pub use frustum::Frustum;
pub use pipeline::RenderPipelineManager;
pub use texture_manager::TextureManager;

use bevy_app::prelude::*;

/// Bevy plugin that registers render-related systems.
/// In Faz 2 this is a no-op shell; systems will be added in later phases
/// when the ECS render integration is complete.
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, _app: &mut App) {
        // RenderPlugin placeholder — ECS integration in future phases
    }
}
