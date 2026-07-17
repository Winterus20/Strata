//! `PlayerPlugin`: registers M8 systems in `StrataSet::Input` (input mapper ->
//! hotbar -> controller -> break -> place) and provides [`spawn_player`].

use bevy::app::App;
use bevy::prelude::*;
use strata_core::prelude::*;

use crate::controller::{PlayerController, PlayerLook, PlayerState, player_controller_system};
use crate::input::{PlayerInput, input_mapper_system};
use crate::interaction::{PlayerBreak, PlayerPlace, player_break_system, player_place_system};
use crate::inventory::{Inventory, hotbar_system};

/// Strata player plugin (M8).
pub struct PlayerPlugin;

impl StrataPlugin for PlayerPlugin {
    fn name(&self) -> &'static str {
        "player"
    }

    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<GlobalBrickPool>() {
            app.insert_resource(GlobalBrickPool::new());
        }
        app.insert_resource(PlayerInput::default());
        app.add_message::<PlayerBreak>();
        app.add_message::<PlayerPlace>();
        // Input sampling + block interaction stay at render rate in the `Input`
        // set. Movement is intentionally NOT here (see FixedUpdate below).
        app.add_systems(
            Update,
            (
                input_mapper_system,
                hotbar_system,
                player_break_system,
                player_place_system,
            )
                .chain()
                .in_set(StrataSet::Input),
        );
        // Movement integration runs on the fixed timestep (framerate-independent,
        // deterministic — plan 14 §D3). It reads the `PlayerInput` sampled by the
        // Update-rate input mapper and writes the player `Transform`; break/place
        // (Update, after FixedMain) then raycast from the fresh position.
        app.add_systems(FixedUpdate, player_controller_system);
    }
}

/// Spawn the player entity with default controller/state/look/inventory at `position`.
///
/// NOTE: M8 uses a headless, Transform-based movement integration (`player_controller_system`)
/// rather than wiring Rapier's `KinematicCharacterController` for actual collision, so we do
/// not also spawn the physics capsule (which would be a separate entity). The gravity + ground-snap
/// logic lives in `controller::integrate_player` and is unit-tested directly.
pub fn spawn_player(commands: &mut Commands, position: Vec3) {
    commands.spawn((
        PlayerController::default(),
        PlayerState::default(),
        PlayerLook::default(),
        Inventory::default(),
        // Drives sector streaming (only this entity's Transform is tracked).
        StreamingAnchor,
        Transform::from_translation(position),
        GlobalTransform::from_translation(position),
        Name::new("player"),
    ));
}
