use std::collections::HashMap;
use glam::IVec2;

const CELL_SIZE: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityId(pub u32);

#[derive(Default)]
pub struct SpatialGrid {
    cells: HashMap<IVec2, Vec<EntityId>>,
}

impl SpatialGrid {
    pub fn insert(&mut self, entity: EntityId, position: IVec2) {
        let cell = position / CELL_SIZE;
        self.cells.entry(cell).or_default().push(entity);
    }

    pub fn remove(&mut self, entity: EntityId, position: IVec2) {
        let cell = position / CELL_SIZE;
        if let Some(entities) = self.cells.get_mut(&cell) {
            entities.retain(|&e| e != entity);
        }
    }

    pub fn get_nearby(&self, position: IVec2, radius: i32) -> Vec<EntityId> {
        let center = position / CELL_SIZE;
        let mut result = Vec::new();

        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let cell = center + IVec2::new(dx, dz);
                if let Some(entities) = self.cells.get(&cell) {
                    result.extend(entities.iter().copied());
                }
            }
        }

        result
    }

    pub fn update(&mut self, entity: EntityId, old_pos: IVec2, new_pos: IVec2) {
        self.remove(entity, old_pos);
        self.insert(entity, new_pos);
    }
}

pub struct PlayerInterest {
    pub full_sync: Vec<IVec2>,
    pub position_only: Vec<IVec2>,
}

pub fn calculate_player_interest(player_pos: IVec2, view_distance: u8) -> PlayerInterest {
    let mut full_sync = Vec::new();
    let mut position_only = Vec::new();
    let radius = view_distance as i32;
    let buffer = 2;

    for dx in -(radius + buffer)..=(radius + buffer) {
        for dz in -(radius + buffer)..=(radius + buffer) {
            let dist = dx.abs().max(dz.abs());
            let chunk = IVec2::new(player_pos.x + dx, player_pos.y + dz);
            if dist <= radius {
                full_sync.push(chunk);
            } else if dist <= radius + buffer {
                position_only.push(chunk);
            }
        }
    }

    PlayerInterest { full_sync, position_only }
}
