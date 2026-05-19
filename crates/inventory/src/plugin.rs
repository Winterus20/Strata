use bevy_app::{App, Plugin, Update};
use bevy_ecs::observer::On;
use bevy_ecs::prelude::*;
use strata_ecs::components::interaction::BlockBreakEvent;

use crate::drops::{ItemDropEvent, on_item_drop_spawn};
use crate::systems::{hotbar_selection_system, item_pickup_system};

pub struct InventoryPlugin;

impl Plugin for InventoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (hotbar_selection_system, item_pickup_system));
        app.add_observer(on_block_break_spawn_drop);
        app.add_observer(on_item_drop_spawn);
    }
}

fn on_block_break_spawn_drop(trigger: On<BlockBreakEvent>, mut commands: Commands) {
    let pos = trigger.event().0;
    let drop_pos = bevy_math::Vec3::new(
        pos.0.x as f32 + 0.5,
        pos.0.y as f32 + 0.5,
        pos.0.z as f32 + 0.5,
    );
    commands.trigger(ItemDropEvent {
        position: drop_pos,
        item_id: 1,
        count: 1,
    });
}
