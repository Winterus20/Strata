//! World-level metadata persisted in a save (plan 15 §38 §4).

use serde::{Deserialize, Serialize};

/// Durable world metadata (plan 15 §38 §4). Avoids `Vec3` so postcard stays
/// clean across platforms; uses three explicit `f32` fields.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WorldMetadata {
    /// World seed (terrain generator input).
    pub seed: u64,
    /// Spawn point in world space (x, y, z).
    pub spawn_point: [f32; 3],
    /// Total seconds played.
    pub time_played: u64,
    /// World format/genesis version.
    pub world_version: u32,
    /// Terrain generator version that produced this world.
    pub generator_version: u32,
    /// Unix epoch ms of last modification.
    pub last_modified: i64,
}
