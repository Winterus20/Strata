use bevy_ecs::component::Component;
use serde::{Deserialize, Serialize};

#[derive(Component, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ItemStack {
    pub id: u16,
    pub count: u8,
}

impl ItemStack {
    pub const MAX_STACK: u8 = 64;

    pub fn new(id: u16, count: u8) -> Self {
        Self {
            id,
            count: count.min(Self::MAX_STACK),
        }
    }

    pub fn is_full(&self) -> bool {
        self.count >= Self::MAX_STACK
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

#[derive(Component, Debug, Serialize, Deserialize)]
pub struct Inventory {
    pub slots: Vec<Option<ItemStack>>,
    pub selected_slot: u8,
}

impl Inventory {
    pub const HOTBAR_SIZE: u8 = 9;
    pub const TOTAL_SLOTS: usize = 36;

    pub fn new() -> Self {
        Self {
            slots: vec![None; Self::TOTAL_SLOTS],
            selected_slot: 0,
        }
    }

    pub fn add_item(&mut self, id: u16, count: u8) -> bool {
        let mut remaining = count;

        for slot in self.slots.iter_mut() {
            if remaining == 0 {
                return true;
            }

            match slot {
                Some(stack) if stack.id == id && !stack.is_full() => {
                    let space = ItemStack::MAX_STACK - stack.count;
                    let to_add = remaining.min(space);
                    stack.count += to_add;
                    remaining -= to_add;
                }
                None => {
                    *slot = Some(ItemStack::new(id, remaining));
                    remaining = 0;
                }
                _ => {}
            }
        }

        remaining == 0
    }

    pub fn get_selected(&self) -> Option<&ItemStack> {
        self.slots
            .get(self.selected_slot as usize)
            .and_then(|s| s.as_ref())
    }

    pub fn get_selected_mut(&mut self) -> Option<&mut ItemStack> {
        self.slots
            .get_mut(self.selected_slot as usize)
            .and_then(|s| s.as_mut())
    }

    pub fn set_selected_slot(&mut self, slot: u8) {
        self.selected_slot = slot.min(Self::HOTBAR_SIZE - 1);
    }

    pub fn remove_item(&mut self, slot_index: usize, count: u8) -> Option<ItemStack> {
        let slot = self.slots.get_mut(slot_index)?;
        let stack = slot.as_mut()?;

        let remove_count = count.min(stack.count);
        stack.count -= remove_count;

        let removed = ItemStack::new(stack.id, remove_count);

        if stack.count == 0 {
            *slot = None;
        }

        Some(removed)
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}
