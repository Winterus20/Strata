//! Content-addressable deduplication (plan 15 §1.3 / D8).
//!
//! The dedup key is BLAKE3 over the **compressed** payload (so identical air sectors
//! collapse to one blob). The durable source of truth for ref-counts lives in the
//! metadata store (transaction-scoped); this in-RAM `HashMap` is only a fast cache of
//! `hash -> region_offset`, populated during a session. It is never the authority.

use std::collections::HashMap;

use strata_core::component::SectorCoord;

use crate::envelope::compute_hash;
use crate::error::StorageResult;

/// In-memory dedup index: `hash -> (region_offset, size)`.
///
/// `ref_count` is tracked in the metadata store, not here. This table answers
/// "have I seen this compressed blob before?" without a store round-trip.
#[derive(Default)]
pub struct DedupTable {
    index: HashMap<[u8; 32], (u64, u32)>,
}

impl DedupTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `hash` lives at `offset` with `size` bytes in the region file.
    pub fn insert(&mut self, hash: [u8; 32], offset: u64, size: u32) {
        self.index.insert(hash, (offset, size));
    }

    /// Look up a previously stored blob. Returns `(offset, size)` if present.
    pub fn get(&self, hash: &[u8; 32]) -> Option<(u64, u32)> {
        self.index.get(hash).copied()
    }

    /// Hash `payload` (compressed) for use as a dedup key.
    pub fn hash_of(payload: &[u8]) -> [u8; 32] {
        compute_hash(payload)
    }

    /// True if `coord`'s compressed payload already exists in the table (caller
    /// supplies the precomputed hash).
    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.index.contains_key(hash)
    }
}

/// Convenience: hash a sector's compressed payload for storage-side dedup bookkeeping.
pub fn dedup_hash(payload: &[u8]) -> [u8; 32] {
    DedupTable::hash_of(payload)
}

/// Placeholder for the store-backed refcount transaction. The actual refcount lives
/// in the metadata store; this keeps `DedupTable` free of store coupling.
pub type DedupRecord = (SectorCoord, [u8; 32]);

/// Best-effort reconciliation: drop cache entries whose offset/size no longer match
/// the store. Called after a store reconcile pass (plan 15 §1.5.1).
pub fn reconcile(
    table: &mut DedupTable,
    live: &HashMap<[u8; 32], (u64, u32)>,
) -> StorageResult<()> {
    table.index.retain(|k, v| live.get(k) == Some(v));
    Ok(())
}
