//! Versioned save envelope (plan 15 §38 / §2).
//!
//! The envelope wraps a postcard-serialized payload (a `WorldMetadata` or
//! `PlayerSaveData`) with a BLAKE3 integrity hash and a 5-step atomic
//! write/read path (tmp → fsync → .bak → rename → corrupt-detect).
//!
//! # Hash domain
//!
//! - **v2+ (`SAVE_FORMAT_VERSION`)**: BLAKE3 over canonical header fields ‖ payload
//!   (`save_version ‖ generator_version ‖ payload_size ‖ payload`), domain-separated.
//! - **v1 legacy**: BLAKE3 over payload only — still accepted on `verify`/`open` so
//!   old prototype saves load; re-`pack` upgrades to the v2 domain.

use serde::{Deserialize, Serialize};

use strata_storage::error::{StorageError, StorageResult};

/// Magic bytes for a Strata save envelope.
pub const MAGIC: [u8; 4] = *b"STSV";
/// Current save on-disk format version. Bumped only when the byte layout / hash domain changes.
pub const SAVE_FORMAT_VERSION: u32 = 2;

/// Domain separation for the v2+ header‖payload hash.
const HASH_DOMAIN_V2: &[u8] = b"STSV-SAVE-ENVELOPE-V2";

/// A versioned, integrity-checked save container (plan 15 §38 §2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveEnvelope {
    /// Always `"STSV"`.
    pub magic: [u8; 4],
    /// Save format version (`SAVE_FORMAT_VERSION`), distinct from `generator_version`.
    pub save_version: u32,
    /// World/terrain generator version.
    pub generator_version: u32,
    /// BLAKE3 integrity hash (see module docs for domain).
    pub payload_hash: [u8; 32],
    /// Length of `payload` in bytes.
    pub payload_size: u32,
    /// Ed25519-style signature placeholder. Zeroed by `pack`; filled by signers.
    /// Not included in the integrity hash (so signing can fill it after pack).
    pub signature: [u8; 32],
    /// Postcard-serialized inner data (`WorldMetadata` / `PlayerSaveData`).
    pub payload: Vec<u8>,
}

impl SaveEnvelope {
    /// BLAKE3 over `save_version ‖ generator_version ‖ payload_size ‖ payload` (v2+).
    pub fn hash_canonical(
        save_version: u32,
        generator_version: u32,
        payload_size: u32,
        payload: &[u8],
    ) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(HASH_DOMAIN_V2);
        hasher.update(&save_version.to_le_bytes());
        hasher.update(&generator_version.to_le_bytes());
        hasher.update(&payload_size.to_le_bytes());
        hasher.update(payload);
        *hasher.finalize().as_bytes()
    }

    /// Build an envelope from `payload`.
    ///
    /// `save_version >= 2` uses the header‖payload hash domain; `save_version == 1`
    /// keeps the legacy payload-only hash so hand-built v1 fixtures stay valid.
    pub fn pack(save_version: u32, generator_version: u32, payload: &[u8]) -> StorageResult<Self> {
        let payload_size = payload.len() as u32;
        let hash = if save_version >= 2 {
            Self::hash_canonical(save_version, generator_version, payload_size, payload)
        } else {
            blake3::hash(payload).into()
        };
        Ok(Self {
            magic: MAGIC,
            save_version,
            generator_version,
            payload_hash: hash,
            payload_size,
            signature: [0u8; 32],
            payload: payload.to_vec(),
        })
    }

    /// Verify the stored BLAKE3 hash.
    ///
    /// - `save_version >= 2`: header‖payload canonical hash.
    /// - `save_version == 1`: legacy payload-only hash (prototype compatibility).
    pub fn verify(&self) -> StorageResult<()> {
        if self.magic != MAGIC {
            return Err(StorageError::Envelope("bad save magic".into()));
        }
        if self.payload_size as usize != self.payload.len() {
            return Err(StorageError::Envelope("payload_size mismatch".into()));
        }
        let expected = if self.save_version >= 2 {
            Self::hash_canonical(
                self.save_version,
                self.generator_version,
                self.payload_size,
                &self.payload,
            )
        } else {
            // Legacy v1: payload-only.
            blake3::hash(&self.payload).into()
        };
        if expected != self.payload_hash {
            return Err(StorageError::Envelope("payload hash mismatch".into()));
        }
        Ok(())
    }

    /// Postcard-deserialize the inner payload into `T`.
    pub fn decode<T: for<'de> Deserialize<'de>>(&self) -> StorageResult<T> {
        self.verify()?;
        postcard::from_bytes(&self.payload)
            .map_err(|e| StorageError::Envelope(format!("decode: {e}")))
    }

    /// Write the envelope to `path` via the atomic 5-step order (plan 15 §38 §5).
    pub fn save(&self, path: &std::path::Path) -> StorageResult<()> {
        let bytes = postcard::to_allocvec(self)
            .map_err(|e| StorageError::Envelope(format!("serialize: {e}")))?;
        let dir = path
            .parent()
            .ok_or_else(|| StorageError::Envelope("save path has no parent dir".into()))?;
        let final_name = path
            .file_name()
            .ok_or_else(|| StorageError::Envelope("save path has no file name".into()))?;

        let tmp_name = format!("._tmp.{}", final_name.to_string_lossy());
        let tmp_path = dir.join(&tmp_name);
        let bak_name = format!("{}.bak", final_name.to_string_lossy());
        let bak_path = dir.join(&bak_name);

        // (1) write tmp, (2) fsync tmp.
        {
            let mut f = std::fs::File::create(&tmp_path)?;
            std::io::Write::write_all(&mut f, &bytes)?;
            f.sync_all()?;
        }
        // (3) back up the current good file.
        if path.exists() {
            std::fs::copy(path, &bak_path)?;
        }
        // (4) atomic rename tmp → final.
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Read an envelope from `path`, recovering from `.bak` on corruption
    /// (plan 15 §38 §5 step (e): corrupt → restore `.bak`).
    pub fn open(path: &std::path::Path) -> StorageResult<Self> {
        match Self::read_try(path) {
            Ok(env) => Ok(env),
            Err(err) => {
                let dir = path
                    .parent()
                    .ok_or_else(|| StorageError::Envelope("open path has no parent dir".into()))?;
                let file_name = path
                    .file_name()
                    .ok_or_else(|| StorageError::Envelope("open path has no file name".into()))?;
                let bak_name = format!("{}.bak", file_name.to_string_lossy());
                let bak_path = dir.join(&bak_name);
                if bak_path.exists() {
                    let env = Self::read_try(&bak_path)?;
                    // Promote the good backup to the primary location.
                    std::fs::copy(&bak_path, path).ok();
                    Ok(env)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Internal: read + verify a single envelope file.
    fn read_try(path: &std::path::Path) -> StorageResult<Self> {
        let bytes = std::fs::read(path)?;
        let env: SaveEnvelope = postcard::from_bytes(&bytes)
            .map_err(|e| StorageError::Envelope(format!("deserialize: {e}")))?;
        env.verify()?;
        Ok(env)
    }
}
