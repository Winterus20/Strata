use bevy_app::prelude::*;
use bevy_replicon::prelude::RepliconPlugins;
use bevy_replicon_renet2::RepliconRenetPlugins;

mod config;
mod events;
mod protocol;
mod server;
mod client;
mod chunk_sync;
mod visibility;

pub use config::NetworkConfig;
pub use events::*;
pub use chunk_sync::*;
pub use visibility::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Server,
    Client,
    SinglePlayer,
}

pub struct NetworkPlugin {
    pub config: NetworkConfig,
    pub mode: NetworkMode,
}

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.config.clone())
           .add_plugins((RepliconPlugins, RepliconRenetPlugins));

        match self.mode {
            NetworkMode::Server => {
                app.add_plugins(server::ServerPlugin { _config: self.config.clone() });
            }
            NetworkMode::Client => {
                app.add_plugins(client::ClientPlugin { _config: self.config.clone() });
            }
            NetworkMode::SinglePlayer => {}
        }

        app.add_plugins(protocol::NetworkProtocolPlugin);
    }
}

#[cfg(test)]
mod tests {
    use crate::chunk_sync::{ChunkSnapshot, compress_chunk, decompress_chunk, ChunkRequestManager};
    use crate::visibility::{SpatialGrid, EntityId, calculate_player_interest};
    use glam::IVec2;
    use strata_core::chunk::{Chunk, ChunkPos, CHUNK_VOLUME};

    #[test]
    fn test_chunk_compression_decompression() {
        let mut chunk = Chunk::new(ChunkPos(IVec2::new(0, 0)));
        for i in 0..CHUNK_VOLUME {
            chunk.blocks[i] = (i % 5) as u16;
        }
        let snapshot = ChunkSnapshot::from_chunk(&chunk);
        let compressed = compress_chunk(&snapshot).unwrap();
        let decompressed = decompress_chunk(&compressed).unwrap();

        assert_eq!(snapshot.blocks, decompressed.blocks);
        assert_eq!(snapshot.x, decompressed.x);
        assert_eq!(snapshot.z, decompressed.z);
        assert!(compressed.len() < snapshot.blocks.len() * 2);
    }

    #[test]
    fn test_chunk_snapshot_roundtrip() {
        let chunk = Chunk::new(ChunkPos(IVec2::new(5, -3)));
        let snapshot = ChunkSnapshot::from_chunk(&chunk);
        let restored = snapshot.clone().into_chunk();

        assert_eq!(restored.position.0.x, 5);
        assert_eq!(restored.position.0.y, -3);
        assert_eq!(restored.blocks, chunk.blocks);
        assert_eq!(restored.heightmap_top, chunk.heightmap_top);
        assert_eq!(restored.heightmap_bottom, chunk.heightmap_bottom);
    }

    #[test]
    fn test_spatial_grid_nearby() {
        let mut grid = SpatialGrid::default();
        grid.insert(EntityId(1), IVec2::new(0, 0));
        grid.insert(EntityId(2), IVec2::new(16, 0));
        grid.insert(EntityId(3), IVec2::new(100, 100));

        let nearby = grid.get_nearby(IVec2::new(0, 0), 1);
        assert!(nearby.contains(&EntityId(1)));
        assert!(nearby.contains(&EntityId(2)));
        assert!(!nearby.contains(&EntityId(3)));
    }

    #[test]
    fn test_spatial_grid_update() {
        let mut grid = SpatialGrid::default();
        grid.insert(EntityId(1), IVec2::new(0, 0));
        grid.update(EntityId(1), IVec2::new(0, 0), IVec2::new(32, 0));

        let nearby_old = grid.get_nearby(IVec2::new(0, 0), 1);
        assert!(!nearby_old.contains(&EntityId(1)));

        let nearby_new = grid.get_nearby(IVec2::new(32, 0), 1);
        assert!(nearby_new.contains(&EntityId(1)));
    }

    #[test]
    fn test_chunk_request_manager() {
        let mut mgr = ChunkRequestManager::new(3);
        mgr.update_view(IVec2::new(0, 0));
        let requests = mgr.poll_requests();
        assert!(!requests.is_empty());

        mgr.mark_received(0, 0);
        mgr.update_view(IVec2::new(0, 0));
        let requests2 = mgr.poll_requests();
        assert!(requests2.iter().all(|r| !(r.chunk_x == 0 && r.chunk_z == 0)));
    }

    #[test]
    fn test_calculate_player_interest() {
        let interest = calculate_player_interest(IVec2::new(0, 0), 2);

        let in_view = interest.full_sync.iter().any(|pos| *pos == IVec2::new(0, 0));
        assert!(in_view, "chunk (0,0) should be in full sync view distance");

        let far_away = interest.full_sync.iter().any(|pos| *pos == IVec2::new(10, 10));
        assert!(!far_away, "chunk (10,10) should be outside view distance");
    }
}
