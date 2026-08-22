//! Bridge types that let `strata_render` borrow Bevy's `RenderDevice`/`RenderQueue`
//! without taking a hard `bevy_render` dependency (which would create a circular
//! crate dependency: `strata_render` is used by `bevy_render`'s plugin).
//!
//! The client provides concrete Bevy-backed implementations of [`RenderDeviceRef`]
//! and [`RenderQueueRef`] via the `strata_render_bevy` feature.

use wgpu::{Device, Queue};

/// Borrow of Bevy's `RenderDevice` — exposes the underlying wgpu `Device`.
pub trait RenderDeviceRef {
    fn wgpu_device(&self) -> &Device;
}

/// Borrow of Bevy's `RenderQueue` — exposes the underlying wgpu `Queue`.
pub trait RenderQueueRef {
    fn wgpu_queue(&self) -> &Queue;
}
