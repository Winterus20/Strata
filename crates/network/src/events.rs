use bevy_ecs::prelude::Event;
use serde::{Deserialize, Serialize};
use glam::{IVec3, Vec3};

#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct PlayerInputEvent {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sprint: bool,
    pub look_delta: (f32, f32),
}

#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct BlockInteractEvent {
    pub block_pos: IVec3,
    pub face: u8,
    pub is_break: bool,
    pub block_id: Option<u16>,
}

#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessageEvent {
    pub message: String,
}

#[derive(Event, Serialize, Deserialize, Clone, Debug)]
pub struct EntitySpawnEvent {
    pub entity_id: u32,
    pub entity_type: u16,
    pub position: Vec3,
}
