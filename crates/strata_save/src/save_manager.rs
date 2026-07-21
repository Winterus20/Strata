//! Save manager: atomic world/player persistence (plan 15 §38 §5).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bevy::prelude::Resource;

use crate::envelope::SaveEnvelope;
use crate::player_save_data::PlayerSaveData;
use crate::world_metadata::WorldMetadata;
use strata_storage::error::StorageResult;
use strata_storage::metadata::MetadataStore;

/// Atomic world/player save orchestrator (plan 15 §38 §5).
///
/// Holds the durable `MetadataStore` (used by the storage side) and exposes
/// sync atomic save/load over the versioned `SaveEnvelope`. Auto-save
/// cadence is driven by the `SavePlugin`, not here.
#[derive(Clone, Resource)]
pub struct SaveManager {
    /// Durable metadata store (plan 15 §1.5).
    pub metadata: Arc<dyn MetadataStore>,
    /// Maximum staleness before a forced auto-save.
    pub auto_save_interval: Duration,
}

impl SaveManager {
    /// Build a save manager around a metadata store and auto-save interval.
    pub fn new(metadata: Arc<dyn MetadataStore>, auto_save_interval: Duration) -> Self {
        Self {
            metadata,
            auto_save_interval,
        }
    }

    /// Atomically write `WorldMetadata` to `path`.
    pub fn save_world(&self, path: &Path, meta: &WorldMetadata) -> StorageResult<()> {
        let payload = postcard::to_allocvec(meta).map_err(|e| {
            strata_storage::error::StorageError::Envelope(format!("serialize: {e}"))
        })?;
        let env = SaveEnvelope::pack(
            crate::migration::CURRENT_SAVE_VERSION,
            meta.generator_version,
            &payload,
        )?;
        env.save(path)
    }

    /// Atomically load `WorldMetadata` from `path`, migrating if needed.
    pub fn load_world(&self, path: &Path) -> StorageResult<WorldMetadata> {
        let env = SaveEnvelope::open(path)?;
        let env = crate::migration::migrate(&env)?;
        env.decode()
    }

    /// Atomically write `PlayerSaveData` to `path`.
    pub fn save_player(&self, path: &Path, data: &PlayerSaveData) -> StorageResult<()> {
        let payload = postcard::to_allocvec(data).map_err(|e| {
            strata_storage::error::StorageError::Envelope(format!("serialize: {e}"))
        })?;
        let env = SaveEnvelope::pack(crate::migration::CURRENT_SAVE_VERSION, 0, &payload)?;
        env.save(path)
    }

    /// Atomically load `PlayerSaveData` from `path`, migrating if needed.
    pub fn load_player(&self, path: &Path) -> StorageResult<PlayerSaveData> {
        let env = SaveEnvelope::open(path)?;
        let env = crate::migration::migrate(&env)?;
        env.decode()
    }
}
