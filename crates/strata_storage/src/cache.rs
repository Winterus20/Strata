//! WARM L2 cache (plan 15 §1.1.1 / D5).
//!
//! Stores decompressed `Bytes` blobs, byte-weighted, with S3-FIFO/TinyLFU eviction
//! built into `moka`. The byte budget (default 512 MB) is enforced through moka's
//! weighted `max_capacity` with a weigher that returns the blob's length. No manual
//! eviction code is needed.

use bytes::Bytes;

use strata_core::component::SectorCoord;

/// Default byte budget for the WARM cache (plan 15 §1.1.1): 512 MB.
pub const DEFAULT_BYTE_BUDGET: u64 = 512 * 1024 * 1024;

/// Byte-weighted, S3-FIFO WARM L2 cache of decompressed sector blobs.
pub struct WarmCache {
    inner: moka::future::Cache<SectorCoord, Bytes>,
    byte_budget: u64,
}

impl WarmCache {
    /// Build a cache with the given `byte_budget` (bytes). moka's `max_capacity` acts
    /// as the weighted budget when a `weigher` is installed, so we pass the byte
    /// budget directly and weigh each blob by `len()`. S3-FIFO eviction is built in.
    pub fn new(byte_budget: u64) -> Self {
        let inner = moka::future::Cache::builder()
            .max_capacity(byte_budget)
            .weigher(|_key: &SectorCoord, value: &Bytes| value.len() as u32)
            .build();
        WarmCache { inner, byte_budget }
    }

    /// Look up a sector's decompressed blob.
    pub async fn get(&self, coord: SectorCoord) -> Option<Bytes> {
        self.inner.get(&coord).await
    }

    /// Insert/replace a sector's decompressed blob.
    pub async fn put(&self, coord: SectorCoord, bytes: Bytes) {
        self.inner.insert(coord, bytes).await;
    }

    /// Current total weight (bytes) across all cached blobs.
    pub async fn byte_usage(&self) -> u64 {
        self.inner.weighted_size()
    }

    /// Drive moka's background eviction/housekeeping synchronously (test helper).
    pub async fn run_pending_tasks(&self) {
        self.inner.run_pending_tasks().await;
    }

    /// Configured byte budget.
    pub fn budget(&self) -> u64 {
        self.byte_budget
    }
}
