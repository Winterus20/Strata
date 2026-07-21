//! Player save data (plan 15 §38 §3).

use serde::{Deserialize, Serialize};

/// A stack of identical blocks in a player's inventory (plan 15 §38 §3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ItemStack {
    /// Block registry id.
    pub block_id: u32,
    /// Count of identical blocks in the stack.
    pub count: u32,
}

/// Durable player save data (plan 15 §38 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerSaveData {
    /// World-space position (x, y, z).
    pub position: [f32; 3],
    /// View rotation: (yaw, pitch).
    pub rotation: [f32; 2],
    /// Current health (clamped ≥ 0 on load).
    pub health: f32,
    /// Current hunger (0..=20 typically).
    pub hunger: f32,
    /// Accumulated experience points.
    pub xp: u32,
    /// Selected hotbar slot index.
    pub hotbar_index: u8,
    /// Hotbar + inventory slots; `None` marks an empty slot.
    pub inventory: Vec<Option<ItemStack>>,
}
