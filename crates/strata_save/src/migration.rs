//! Save migration chain (plan 15 §38 §2).
//!
//! On load, an envelope is migrated forward pure-function-style (no I/O, no
//! globals) until its `save_version` matches `CURRENT_SAVE_VERSION`.

use crate::envelope::SaveEnvelope;
use crate::world_metadata::WorldMetadata;
use strata_storage::error::{StorageError, StorageResult};

/// The current on-disk save format version.
pub const CURRENT_SAVE_VERSION: u32 = 1;

/// A single forward migration step (plan 15 §38 §2).
pub struct MigrationChain {
    /// Version this step migrates *from*.
    pub from: u32,
    /// Version this step migrates *to*.
    pub to: u32,
    /// Pure transform applied to the inner `WorldMetadata` (no I/O, no globals).
    pub transform: Box<dyn Fn(WorldMetadata) -> WorldMetadata>,
}

/// Build the ordered migration chain. For v1 the chain is empty (identity).
pub fn chain() -> Vec<MigrationChain> {
    Vec::new()
}

/// Run the from→to chain on `envelope` until `save_version == CURRENT_SAVE_VERSION`.
///
/// Each step: decode `WorldMetadata`, apply the matching transform, re-pack the
/// envelope at the new `save_version`. An unknown version gap is an error.
pub fn migrate(envelope: &SaveEnvelope) -> StorageResult<SaveEnvelope> {
    let mut current = envelope.clone();
    let steps = chain();
    while current.save_version < CURRENT_SAVE_VERSION {
        let meta: WorldMetadata = current.decode()?;
        let step = steps
            .iter()
            .find(|s| s.from == current.save_version)
            .ok_or_else(|| {
                StorageError::Envelope(format!(
                    "no migration step from save_version {}",
                    current.save_version
                ))
            })?;
        let migrated = (step.transform)(meta);
        current = SaveEnvelope::pack(
            step.to,
            current.generator_version,
            &postcard::to_allocvec(&migrated)
                .map_err(|e| StorageError::Envelope(format!("serialize: {e}")))?,
        )?;
    }
    Ok(current)
}
