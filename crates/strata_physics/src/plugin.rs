//! `PhysicsPlugin`: wires Rapier3D into the Strata pipeline (M6).

use bevy::app::App;
use bevy::prelude::*;
use bevy_rapier3d::prelude::*;
use strata_core::prelude::*;

use crate::voxel_collider::{
    CharacterController, PendingCollider, PhysicsPool, PhysicsTimers, apply_sector_collider_tasks,
    cleanup_pending_colliders, spawn_sector_collider_tasks, sync_dirty_sector_colliders,
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
        app.init_resource::<PhysicsPool>();
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
