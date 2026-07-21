//! Compression wrappers over zstd (plan 15 §1.7).
//!
//! Tier selects the zstd level (WARM=1, DISTANT=3, ARCHIVE=19). `compress`/`decompress`
//! operate on the full payload; per-sector parallelism (rayon / `AsyncComputeTaskPool`)
//! is applied by the caller on the flush path (plan 15 §1.6 / D7), since single-sector
//! zstd multithreading (`nbWorkers`) does not pay off below ~1 MB.

use crate::envelope::{Tier, compress as env_compress, decompress as env_decompress};
use crate::error::StorageResult;

/// Compress `payload` at the tier's zstd level.
#[inline]
pub fn compress(payload: &[u8], tier: Tier) -> StorageResult<Vec<u8>> {
    env_compress(payload, tier)
}

/// Decompress a blob produced by [`compress`].
#[inline]
pub fn decompress(bytes: &[u8]) -> StorageResult<Vec<u8>> {
    env_decompress(bytes)
}
