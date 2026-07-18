//! `PhysicsPlugin`: wires Rapier3D into the Strata pipeline (M6).

use bevy::app::App;
use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use strata_core::prelude::*;

use crate::voxel_collider::{
    CharacterController, PendingCollider, PhysicsTimers, PhysicsWorkerChannels,
    apply_sector_collider_tasks, build_voxels_collider, cleanup_pending_colliders,
    spawn_sector_collider_tasks, sync_dirty_sector_colliders,
};

/// Strata physics plugin (M6): Rapier3D voxel colliders + character controller.
pub struct PhysicsPlugin;

impl StrataPlugin for PhysicsPlugin {
    fn name(&self) -> &'static str {
        "physics"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GlobalBrickPool>() {
            app.insert_resource(GlobalBrickPool::new());
        }
        // Headless-safe: no window/render features are enabled (workspace dep has
        // `default-features = false`). The physics pipeline runs in PostUpdate.
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());
        app.insert_resource(CharacterController::default());

        let (tx_request, rx_request) =
            std::sync::mpsc::channel::<crate::voxel_collider::VoxelColliderRequest>();
        let (tx_response, rx_response) =
            std::sync::mpsc::channel::<crate::voxel_collider::VoxelColliderResponse>();

        std::thread::Builder::new()
            .name("strata-physics-worker".to_string())
            .spawn(move || {
                while let Ok(req) = rx_request.recv() {
                    let t0 = std::time::Instant::now();
                    let collider = build_voxels_collider(&req.samples);
                    let elapsed = t0.elapsed().as_micros() as u64;
                    let _ = tx_response.send(crate::voxel_collider::VoxelColliderResponse {
                        entity: req.entity,
                        coord: req.coord,
                        origin: req.origin,
                        collider,
                        rapier_us: elapsed,
                    });
                }
            })
            .expect("Failed to spawn strata-physics-worker thread");

        app.insert_resource(PhysicsWorkerChannels {
            tx_request,
            rx_response: std::sync::Mutex::new(rx_response),
        });

        app.init_resource::<PendingCollider>();
        app.init_resource::<PhysicsTimers>();
        app.add_systems(
            Update,
            (
                apply_sector_collider_tasks,
                spawn_sector_collider_tasks,
                cleanup_pending_colliders,
                sync_dirty_sector_colliders,
            )
                .chain()
                .in_set(StrataSet::Physics),
        );
    }
}
