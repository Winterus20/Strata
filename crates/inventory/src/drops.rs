use bevy_ecs::bundle::Bundle;
use bevy_ecs::component::Component;
use bevy_ecs::observer::On;
use bevy_ecs::prelude::*;
use bevy_math::Vec3;
use bevy_rapier3d::dynamics::RigidBody;
use bevy_rapier3d::geometry::Collider;
use bevy_transform::components::Transform;

use crate::components::ItemStack;

#[derive(Component, Clone, Debug)]
pub struct ItemDrop {
    pub item: ItemStack,
}

#[derive(Bundle)]
pub struct ItemDropBundle {
    pub item_drop: ItemDrop,
    pub transform: Transform,
    pub rigid_body: RigidBody,
    pub collider: Collider,
}

impl ItemDropBundle {
    pub fn new(item: ItemStack, position: Vec3) -> Self {
        Self {
            item_drop: ItemDrop { item },
            transform: Transform::from_translation(position),
            rigid_body: RigidBody::Dynamic,
            collider: Collider::ball(0.15),
        }
    }
}

#[derive(Event)]
pub struct ItemDropEvent {
    pub position: Vec3,
    pub item_id: u16,
    pub count: u8,
}

pub fn on_item_drop_spawn(trigger: On<ItemDropEvent>, mut commands: Commands) {
    let event = trigger.event();
    let item = ItemStack::new(event.item_id, event.count);
    commands.spawn(ItemDropBundle::new(item, event.position));
}
