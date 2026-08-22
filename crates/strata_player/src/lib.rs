//! Strata player: branchless voxel raycast, controller, input mapping,
//! block interaction (break/place), and a minimal hotbar inventory (M8).
//!
//! All logic is self-contained and unit-testable without a window. The raycast,
//! controller integration, and break/place are exposed as pure functions; thin
//! ECS systems drive them from `StrataSet::Input`.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss
)]

pub mod controller;
pub mod input;
pub mod interaction;
pub mod inventory;
pub mod plugin;
pub mod raycast;

pub use controller::{PlayerController, PlayerLook, PlayerState};
pub use input::{InputAction, InputMapper, PlayerInput};
pub use interaction::{
    ChunkMap, PlayerBreak, PlayerPlace, RayHit, SELECTION_OUTLINE_EXPAND, apply_break, apply_place,
    pick_solid_voxel, selection_outline_front_facing, selection_outline_hit_face,
    selection_outline_line_list, voxel_world_min,
};
pub use inventory::{Inventory, ItemStack};
pub use plugin::{PlayerPlugin, spawn_player};
pub use raycast::{FaceNormal, look_direction, raycast_voxel};

pub mod prelude {
    pub use crate::*;
    pub use strata_core::prelude::*;
}
