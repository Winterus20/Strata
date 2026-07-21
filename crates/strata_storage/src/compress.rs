//! Compression wrappers over zstd (plan 15 §1.7).
//!
//! Tier selects the zstd level (WARM=1, DISTANT=3, ARCHIVE=19). `compress`/`decompress`
//! operate on the full payload; per-sector parallelism (rayon / `AsyncComputeTaskPool`)
//! is applied by the caller on the flush path (plan 15 §1.6 / D7), since single-sector
//! zstd multithreading (`nbWorkers`) does not pay off below ~1 MB.
//!
//! Decompression is size-capped ([`MAX_DECOMPRESSED_SECTOR_BYTES`]) to reject
//! compression bombs.
//!
//! On-disk sector records may be legacy **uncompressed** (pre-compress write path).
//! [`decode_stored_payload`] uses the zstd frame magic to decide: magic present →
//! decompress (fail closed on bad frames); otherwise treat as raw after header
//! integrity already verified the stored bytes.

use std::io::Read;

use crate::envelope::{Tier, compress as env_compress};
use crate::error::{StorageError, StorageResult};

/// Upper bound on a single sector's decompressed size.
///
/// A 32³ sector snapshot is far smaller than this; the cap stops malicious
/// zstd frames that expand to gigabytes (compression bomb).
pub const MAX_DECOMPRESSED_SECTOR_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

/// RFC 8878 / zstd frame magic (`0xFD2FB528` little-endian).
pub const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// True when `bytes` begins with a zstd frame descriptor magic.
#[inline]
pub fn is_zstd_frame(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == ZSTD_MAGIC
}

/// Compress `payload` at the tier's zstd level.
#[inline]
pub fn compress(payload: &[u8], tier: Tier) -> StorageResult<Vec<u8>> {
    env_compress(payload, tier)
}

/// Decompress a blob produced by [`compress`], capped at [`MAX_DECOMPRESSED_SECTOR_BYTES`].
#[inline]
pub fn decompress(bytes: &[u8]) -> StorageResult<Vec<u8>> {
    decompress_with_limit(bytes, MAX_DECOMPRESSED_SECTOR_BYTES)
}

/// Decode a region-file payload after header BLAKE3/xxHash verification.
///
/// - zstd magic → decompress (unknown/corrupt frame → [`StorageError::Decompress`])
/// - otherwise → return a copy of `bytes` (legacy uncompressed, intentional)
#[inline]
pub fn decode_stored_payload(bytes: &[u8]) -> StorageResult<Vec<u8>> {
    if is_zstd_frame(bytes) {
        decompress(bytes)
    } else {
        Ok(bytes.to_vec())
    }
}

/// Decompress with an explicit output-size cap. Exceeding `max_bytes` → `Decompress` error.
pub fn decompress_with_limit(bytes: &[u8], max_bytes: usize) -> StorageResult<Vec<u8>> {
    let mut decoder = zstd::stream::read::Decoder::new(bytes)
        .map_err(|e| StorageError::Decompress(e.to_string()))?;
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = decoder
            .read(&mut buf)
            .map_err(|e| StorageError::Decompress(e.to_string()))?;
        if n == 0 {
            break;
        }
        if out.len().saturating_add(n) > max_bytes {
            return Err(StorageError::Decompress(format!(
                "decompressed size exceeds limit ({max_bytes} bytes)"
            )));
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}
