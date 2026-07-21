//! Sector envelope: the on-disk `SectorHeader` (40B), tier→compression mapping, and
//! the zstd compress/decompress wrappers (plan 15 §1.2 / §1.7).
//!
//! Design notes (plan 15 audit):
//! - `content_hash` is BLAKE3 over the **compressed** payload (§1.3) — identical air
//!   sectors collapse to the same blob, and dedup captures it.
//! - `frame_checksum` is a *separate* xxHash64 over the compressed payload — an
//!   independent accidental-bitrot detector (§1.2 / D14). The two algorithms are
//!   deliberately distinct so a BLAKE3 flaw cannot mask an xxHash64 flaw.
//! - `coord` is `i32` per axis (plan 15 §1.2) for unlimited-Y addressing.

use bytemuck::{Pod, Zeroable};
use strata_core::component::SectorCoord;

use crate::error::{StorageError, StorageResult};

/// Streaming tier a sector is persisted at. Drives the zstd level (plan 15 §1.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Tier {
    /// Fast cache tier. zstd-1 (speed > size).
    Warm = 0,
    /// Mid tier. zstd-3 (balanced).
    Distant = 1,
    /// Write-once / read-rarely. zstd-19 (size > speed).
    Archive = 2,
}

impl Tier {
    /// zstd compression level for this tier (plan 15 §1.7).
    pub const fn zstd_level(self) -> i32 {
        match self {
            Tier::Warm => 1,
            Tier::Distant => 3,
            Tier::Archive => 19,
        }
    }

    /// Decode a `u8` stored in the header. Unknown values fall back to `Distant`.
    pub const fn from_u8(v: u8) -> Self {
        match v {
            0 => Tier::Warm,
            2 => Tier::Archive,
            _ => Tier::Distant,
        }
    }
}

/// Sector header, fixed layout, Pod-safe (plan 15 §1.2).
///
/// `repr(C)` + `Pod` guarantees the byte image is stable across platforms; we never
/// rely on Rust struct layout for the on-disk format. An explicit `_pad3` keeps the
/// struct 8-byte aligned so `frame_checksum: u64` has no implicit trailing padding
/// (which would make `bytemuck::Pod` reject the derive).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SectorHeader {
    /// `"STSV"` magic to reject foreign blobs.
    pub magic: [u8; 4],
    /// Save format version.
    pub version: u16,
    pub _pad: u16,
    /// Sector coordinate (i32 per axis, unlimited Y — plan 15 §1.2).
    pub coord: [i32; 3],
    /// Tier (0=Warm, 1=Distant, 2=Archive).
    pub tier: u8,
    /// zstd level actually used (redundant with tier, aids recovery).
    pub compression_level: i8,
    pub _pad2: [u8; 2],
    /// BLAKE3 over the compressed payload (dedup key + integrity).
    pub payload_hash: [u8; 32],
    /// Compressed payload length in bytes.
    pub payload_size: u32,
    /// Explicit pad to 8-byte align `frame_checksum` (no implicit trailing padding).
    pub _pad3: [u8; 4],
    /// xxHash64 over the compressed payload (bitrot detector).
    pub frame_checksum: u64,
}

impl SectorHeader {
    /// Magic bytes for a Strata sector envelope.
    pub const MAGIC: [u8; 4] = *b"STSV";
    /// Current on-disk format version.
    pub const VERSION: u16 = 1;

    /// Build a header for `payload` (already compressed) belonging to `coord`/`tier`.
    pub fn new(coord: SectorCoord, tier: Tier, payload: &[u8]) -> Self {
        let hash = blake3::hash(payload);
        let checksum = xxhash_rust::xxh64::xxh64(payload, crate::envelope::XXH_SEED);
        SectorHeader {
            magic: Self::MAGIC,
            version: Self::VERSION,
            _pad: 0,
            coord: [coord.0, coord.1, coord.2],
            tier: tier as u8,
            compression_level: tier.zstd_level() as i8,
            _pad2: [0; 2],
            payload_hash: hash.into(),
            payload_size: payload.len() as u32,
            _pad3: [0; 4],
            frame_checksum: checksum,
        }
    }

    /// Verify the header's stored hash/checksum against `payload` (compressed).
    /// Returns `CorruptPayload` on mismatch (plan 15 §1.2.1).
    pub fn verify(&self, payload: &[u8]) -> StorageResult<()> {
        if self.magic != Self::MAGIC {
            return Err(StorageError::Envelope("bad sector magic".into()));
        }
        let hash: [u8; 32] = blake3::hash(payload).into();
        if hash != self.payload_hash {
            return Err(StorageError::CorruptPayload {
                coord: SectorCoord(self.coord[0], self.coord[1], self.coord[2]),
            });
        }
        let checksum = xxhash_rust::xxh64::xxh64(payload, crate::envelope::XXH_SEED);
        if checksum != self.frame_checksum {
            return Err(StorageError::CorruptPayload {
                coord: SectorCoord(self.coord[0], self.coord[1], self.coord[2]),
            });
        }
        Ok(())
    }
}

/// xxHash64 seed — fixed so checksums are reproducible across runs.
pub const XXH_SEED: u64 = 0x5472_a3d0_5f1a_b1e2;

/// Compress `payload` for the given tier (plan 15 §1.7).
pub fn compress(payload: &[u8], tier: Tier) -> StorageResult<Vec<u8>> {
    zstd::stream::encode_all(payload, tier.zstd_level())
        .map_err(|e| StorageError::Compress(e.to_string()))
}

/// Decompress `bytes` produced by [`compress`].
pub fn decompress(bytes: &[u8]) -> StorageResult<Vec<u8>> {
    zstd::stream::decode_all(bytes).map_err(|e| StorageError::Decompress(e.to_string()))
}

/// BLAKE3 over a compressed payload — the dedup key (plan 15 §1.3).
pub fn compute_hash(bytes: &[u8]) -> [u8; 32] {
    blake3::hash(bytes).into()
}

/// xxHash64 over a compressed payload — independent bitrot checksum (plan 15 §1.2).
pub fn compute_frame_checksum(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh64::xxh64(bytes, XXH_SEED)
}
