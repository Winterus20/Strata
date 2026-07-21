//! Storage error type and result alias (plan 15).

use thiserror::Error;

/// All durable-storage failures. Read paths surface `CorruptPayload` so callers can
/// fall back to `.bak` recovery (plan 15 §1.2.1); write paths surface `Io`/`Serialize`.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialize(#[from] postcard::Error),

    #[error("zstd compression error: {0}")]
    Compress(String),

    #[error("zstd decompression error: {0}")]
    Decompress(String),

    #[error("corrupt payload detected (blake3 mismatch) at coord {coord:?}")]
    CorruptPayload {
        coord: strata_core::component::SectorCoord,
    },

    #[error("sector not found: {0:?}")]
    NotFound(strata_core::component::SectorCoord),

    #[error("metadata store error: {0}")]
    Metadata(String),

    #[error("region file error: {0}")]
    Region(String),

    #[error("save envelope error: {0}")]
    Envelope(String),
}

pub type StorageResult<T> = Result<T, StorageError>;
