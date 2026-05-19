use bevy_ecs::prelude::*;
use std::collections::HashMap;

pub struct EntityConfig {
    pub max_health: u16,
    pub speed: f32,
}

#[derive(Resource, Default)]
pub struct EntityRegistry {
    configs: HashMap<String, EntityConfig>,
}

impl EntityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: &str, config: EntityConfig) {
        self.configs.insert(id.to_string(), config);
    }

    pub fn get(&self, id: &str) -> Option<&EntityConfig> {
        self.configs.get(id)
    }
}
