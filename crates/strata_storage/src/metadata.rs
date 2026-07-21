//! Metadata store (plan 15 §1.5 / D1).
//!
//! The metadata store is the *authoritative durable index* over the region files
//! (the region trailer is only crash-recovery, per §1.2.1). It records per-sector
//! `SectorMetadata` (offset, hash, size, tier, dirty flag) so that pristine
//! sectors can be skipped on disk (§1.1.4) and dirty sectors can be flushed.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use fjall::{Config, Keyspace, PartitionCreateOptions};
use serde::{Deserialize, Serialize};

use strata_core::component::SectorCoord;

use crate::error::{StorageError, StorageResult};

/// Per-sector durable metadata (plan 15 §1.5 / D1).
///
/// `dirty` is the durable recovery source for the dirty-queue bitset (§1.1.3):
/// the RAM sticky flag is only a signal, but after a crash the metadata store's
/// `dirty` column decides what must be re-flushed.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SectorMetadata {
    /// Sector coordinate (unlimited-Y).
    pub coord: SectorCoord,
    /// BLAKE3 content hash of the compressed payload (dedup key + integrity).
    pub hash: [u8; 32],
    /// Compressed payload length in bytes.
    pub size: u64,
    /// Last-modified time (unix epoch ms) for GC / reconciliation ordering.
    pub mtime: i64,
    /// Streaming/storage tier (0=Warm, 1=Distant, 2=Archive).
    pub tier: u8,
    /// On-disk format / schema version of this metadata row.
    pub version: u32,
    /// True if the sector has unsaved edits (must reach disk before eviction).
    pub dirty: bool,
}

impl SectorMetadata {
    /// Key string for `coord` in the metadata partition: `sector:<x>,<y>,<z>`.
    pub fn key(coord: SectorCoord) -> String {
        format!("sector:{},{},{}", coord.0, coord.1, coord.2)
    }
}

/// Durable, async metadata index over the sector metadata rows (plan 15 §1.5).
#[async_trait]
pub trait MetadataStore: Send + Sync {
    /// Fetch the metadata for `coord`, or `None` if it has never been persisted.
    async fn get(&self, coord: SectorCoord) -> StorageResult<Option<SectorMetadata>>;
    /// Upsert metadata for a single sector.
    async fn put(&self, meta: SectorMetadata) -> StorageResult<()>;
    /// Remove metadata for a sector.
    async fn delete(&self, coord: SectorCoord) -> StorageResult<()>;
    /// Return every sector currently flagged dirty (the re-flush set on startup).
    async fn list_dirty(&self) -> StorageResult<Vec<SectorMetadata>>;
    /// Atomic-ish multi-put: all rows land together via a fjall batch.
    async fn batch_write(&self, metas: Vec<SectorMetadata>) -> StorageResult<()>;
}

/// Primary fjall-backed metadata store (plan 15 §1.5: fjall birincil).
///
/// One partition holds postcard-serialized `SectorMetadata` keyed by
/// `sector:<x>,<y>,<z>`. `list_dirty` scans the partition filtering `dirty`.
pub struct FjallMetadata {
    _keyspace: Keyspace,
    partition: fjall::Partition,
}

impl FjallMetadata {
    /// Open (or create) a metadata store rooted at `path`.
    pub fn open(path: &std::path::Path) -> StorageResult<Self> {
        std::fs::create_dir_all(path).map_err(StorageError::Io)?;
        let keyspace = Config::new(path)
            .open()
            .map_err(|e| StorageError::Metadata(format!("fjall open: {e}")))?;
        let partition = keyspace
            .open_partition("sector_metadata", PartitionCreateOptions::default())
            .map_err(|e| StorageError::Metadata(format!("open partition: {e}")))?;
        Ok(Self {
            _keyspace: keyspace,
            partition,
        })
    }
}

#[async_trait]
impl MetadataStore for FjallMetadata {
    async fn get(&self, coord: SectorCoord) -> StorageResult<Option<SectorMetadata>> {
        let raw = self
            .partition
            .get(SectorMetadata::key(coord))
            .map_err(|e| StorageError::Metadata(format!("get: {e}")))?;
        match raw {
            Some(bytes) => {
                let meta: SectorMetadata = postcard::from_bytes(&bytes)
                    .map_err(|e| StorageError::Metadata(format!("deserialize: {e}")))?;
                Ok(Some(meta))
            }
            None => Ok(None),
        }
    }

    async fn put(&self, meta: SectorMetadata) -> StorageResult<()> {
        let bytes = postcard::to_allocvec(&meta)
            .map_err(|e| StorageError::Metadata(format!("serialize: {e}")))?;
        self.partition
            .insert(SectorMetadata::key(meta.coord), bytes)
            .map_err(|e| StorageError::Metadata(format!("insert: {e}")))?;
        Ok(())
    }

    async fn delete(&self, coord: SectorCoord) -> StorageResult<()> {
        self.partition
            .remove(SectorMetadata::key(coord))
            .map_err(|e| StorageError::Metadata(format!("remove: {e}")))?;
        Ok(())
    }

    async fn list_dirty(&self) -> StorageResult<Vec<SectorMetadata>> {
        let mut dirty = Vec::new();
        for entry in self.partition.iter() {
            let (_, value) = entry.map_err(|e| StorageError::Metadata(format!("scan: {e}")))?;
            let meta: SectorMetadata = postcard::from_bytes(&value)
                .map_err(|e| StorageError::Metadata(format!("deserialize: {e}")))?;
            if meta.dirty {
                dirty.push(meta);
            }
        }
        Ok(dirty)
    }

    async fn batch_write(&self, metas: Vec<SectorMetadata>) -> StorageResult<()> {
        let mut batch = self._keyspace.batch();
        for meta in &metas {
            let bytes = postcard::to_allocvec(meta)
                .map_err(|e| StorageError::Metadata(format!("serialize: {e}")))?;
            batch.insert(&self.partition, SectorMetadata::key(meta.coord), bytes);
        }
        batch
            .commit()
            .map_err(|e| StorageError::Metadata(format!("batch commit: {e}")))?;
        Ok(())
    }
}

/// In-memory metadata store for tests and dev runs (no disk).
pub struct InMemoryMetadata {
    map: Mutex<HashMap<SectorCoord, SectorMetadata>>,
}

impl InMemoryMetadata {
    /// Build an empty in-memory metadata store.
    pub fn new() -> Self {
        Self {
            map: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetadataStore for InMemoryMetadata {
    async fn get(&self, coord: SectorCoord) -> StorageResult<Option<SectorMetadata>> {
        Ok(self.map.lock().unwrap().get(&coord).copied())
    }

    async fn put(&self, meta: SectorMetadata) -> StorageResult<()> {
        self.map.lock().unwrap().insert(meta.coord, meta);
        Ok(())
    }

    async fn delete(&self, coord: SectorCoord) -> StorageResult<()> {
        self.map.lock().unwrap().remove(&coord);
        Ok(())
    }

    async fn list_dirty(&self) -> StorageResult<Vec<SectorMetadata>> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .values()
            .copied()
            .filter(|m| m.dirty)
            .collect())
    }

    async fn batch_write(&self, metas: Vec<SectorMetadata>) -> StorageResult<()> {
        let mut map = self.map.lock().unwrap();
        for m in metas {
            map.insert(m.coord, m);
        }
        Ok(())
    }
}
