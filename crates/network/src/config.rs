use bevy_ecs::prelude::Resource;

#[derive(Debug, Clone, Resource)]
pub struct NetworkConfig {
    pub server_port: u16,
    pub tick_rate: u8,
    pub client_send_rate: u8,
    pub chunk_view_distance: u8,
    pub max_clients: u16,
    pub heartbeat_interval_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            server_port: 27015,
            tick_rate: 20,
            client_send_rate: 30,
            chunk_view_distance: 10,
            max_clients: 1024,
            heartbeat_interval_ms: 1000,
        }
    }
}
