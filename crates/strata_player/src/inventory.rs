//! Minimal hotbar inventory (plan 14 §Inventory): a 9-slot hotbar with one
//! active slot, cycled by `HotbarNext` (scroll/Q). Kept tiny for the prototype.

use bevy::prelude::*;
use strata_core::prelude::*;

use crate::controller::PlayerController;
use crate::input::PlayerInput;

/// A stack of identical blocks. Prototype stores only the `BlockId` + count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemStack {
    pub block: BlockId,
    pub count: u32,
}

/// Player hotbar: 9 slots, one active. `active` is change-guarded on write.
#[derive(Debug, Clone, Component)]
pub struct Inventory {
    pub hotbar: [ItemStack; 9],
    pub active: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            hotbar: [ItemStack {
                block: BlockId(1),
                count: 64,
            }; 9],
            active: 0,
        }
    }
}

impl Inventory {
    /// The currently-selected stack.
    #[inline]
    pub fn active_slot(&self) -> ItemStack {
        self.hotbar[self.active]
    }

    /// The `BlockId` selected for placement.
    #[inline]
    pub fn active_block(&self) -> BlockId {
        self.hotbar[self.active].block
    }

    /// Select a specific slot (ignored if out of range).
    pub fn select(&mut self, slot: usize) {
        if slot < 9 {
            self.active = slot;
        }
    }

    /// Cycle to the next slot (scroll down).
    pub fn scroll_next(&mut self) {
        self.active = (self.active + 1) % 9;
    }

    /// Cycle to the previous slot (scroll up).
    pub fn scroll_prev(&mut self) {
        self.active = (self.active + 8) % 9;
    }
}

/// ECS system: advance the hotbar when `PlayerInput::hotbar_next` is set, then
/// consume the flag so it fires once per press.
pub fn hotbar_system(
    mut input: ResMut<PlayerInput>,
    mut inv: Query<&mut Inventory, With<PlayerController>>,
) {
    if !input.hotbar_next {
        return;
    }
    if let Ok(mut i) = inv.single_mut() {
        let prev = i.active;
        i.scroll_next();
        let _ = prev; // change-detection handled by slot rollover
    }
    input.hotbar_next = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_active_is_slot_zero() {
        let inv = Inventory::default();
        assert_eq!(inv.active, 0);
        assert_eq!(inv.active_block(), BlockId(1));
    }

    #[test]
    fn scroll_next_cycles_through_all_slots_and_wraps() {
        let mut inv = Inventory::default();
        for i in 1..9 {
            inv.scroll_next();
            assert_eq!(inv.active, i, "slot after {i} scrolls");
        }
        inv.scroll_next();
        assert_eq!(inv.active, 0, "wraps to 0 after last slot");
    }

    #[test]
    fn select_sets_active_slot() {
        let mut inv = Inventory::default();
        inv.select(4);
        assert_eq!(inv.active, 4);
        inv.select(99); // out of range -> ignored
        assert_eq!(inv.active, 4);
    }
}
