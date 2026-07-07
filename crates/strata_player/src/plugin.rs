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
        app.add_systems(
            Update,
            (
                input_mapper_system,
                hotbar_system,
                player_controller_system,
                player_break_system,
                player_place_system,
            )
                .chain()
                .in_set(StrataSet::Input),
        );
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
        Transform::from_translation(position),
        GlobalTransform::from_translation(position),
        Name::new("player"),
    ));
}
