use bevy_ecs::prelude::*;
use bevy_input::ButtonInput;
use bevy_input::keyboard::KeyCode;
use bevy_input::mouse::AccumulatedMouseScroll;
use bevy_transform::components::Transform;

use crate::components::Inventory;
use crate::drops::ItemDrop;

pub fn hotbar_selection_system(
    mut inventory_query: Query<&mut Inventory>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut mouse_scroll: ResMut<AccumulatedMouseScroll>,
) {
    let mut inv = match inventory_query.single_mut() {
        Ok(i) => i,
        Err(_) => return,
    };

    let digit_keys = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];

    for (i, &key) in digit_keys.iter().enumerate() {
        if keyboard_input.just_pressed(key) {
            inv.set_selected_slot(i as u8);
            mouse_scroll.delta = bevy_math::Vec2::ZERO;
            return;
        }
    }

    if mouse_scroll.delta.y > 0.0 {
        let next = if inv.selected_slot == 0 {
            Inventory::HOTBAR_SIZE - 1
        } else {
            inv.selected_slot - 1
        };
        inv.set_selected_slot(next);
    } else if mouse_scroll.delta.y < 0.0 {
        let next = (inv.selected_slot + 1) % Inventory::HOTBAR_SIZE;
        inv.set_selected_slot(next);
    }

    mouse_scroll.delta = bevy_math::Vec2::ZERO;
}

pub fn item_pickup_system(
    mut commands: Commands,
    mut player_query: Query<(&Transform, &mut Inventory)>,
    drop_query: Query<(Entity, &Transform, &ItemDrop)>,
) {
    let (player_pos, mut inventory) = match player_query.single_mut() {
        Ok((t, i)) => (t.translation, i),
        Err(_) => return,
    };

    const PICKUP_RANGE: f32 = 1.5;

    for (entity, drop_transform, item_drop) in drop_query.iter() {
        let distance = player_pos.distance(drop_transform.translation);
        if distance < PICKUP_RANGE && inventory.add_item(item_drop.item.id, item_drop.item.count) {
            commands.entity(entity).despawn();
        }
    }
}
