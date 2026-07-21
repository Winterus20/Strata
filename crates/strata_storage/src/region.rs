//! Region file format (plan 15 §1.2 / D9 / D10).
//!
//! A region is a 32×32×32 = 32768-sector cube. The on-disk file is *not* the
//! authoritative index (that is the metadata store, M11d); the region trailer here
//! exists only for crash recovery / verification (plan 15 §1.2.1).
//!
//! Layout (compact header, no 768 KB `[u64; 32768]` arrays):
//!
//! ```text
//! [ compact header : 16B ]
//! [ dense slot table : 8B * present_count ]
//! [ payload data : variable ]
//! [ trailer : 4096B presence bitmap + 16B footer ]
//! ```
//!
//! Each slot is `(payload_offset: u32, payload_size: u32)` for ONLY the present
//! sectors. The trailer carries a 32768-bit presence bitmap so a crash mid-append
//! can be detected/reconciled against the metadata store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use strata_core::component::SectorCoord;

use crate::envelope::SectorHeader;
use crate::error::{StorageError, StorageResult};

/// Sectors per axis of a region (plan 15 §1.2).
pub const REGION_DIM: i32 = 32;
/// Total sectors in a region = 32³ = 32768.
pub const REGION_SECTOR_COUNT: usize =
    (REGION_DIM as usize) * (REGION_DIM as usize) * (REGION_DIM as usize);
/// Region magic, distinct from the sector envelope magic.
pub const REGION_MAGIC: [u8; 4] = *b"STRG";
/// Current region format version.
pub const REGION_VERSION: u16 = 1;
/// Size of the presence bitmap trailer (32768 bits = 4096 bytes).
const TRAILER_BITMAP_BYTES: usize = REGION_SECTOR_COUNT.div_ceil(8);
/// Trailer = bitmap + 4B magic + 2B version + 2B pad + 8B payload tail hash.
const TRAILER_FOOTER: usize = 16;
const TRAILER_TOTAL: usize = TRAILER_BITMAP_BYTES + TRAILER_FOOTER;
/// Byte length of the fixed compact header.
const HEADER_LEN: usize = 20;
/// Byte length of one slot entry.
const SLOT_LEN: usize = 8;
/// Byte length of a sector envelope header on disk.
const SECTOR_HEADER_LEN: usize = std::mem::size_of::<SectorHeader>();

/// 3D region coordinate. `rx = floor(sector.x / 32)` etc. (plan 15 §1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl RegionCoord {
    /// Map a sector coordinate to its containing region.
    pub fn from_sector(coord: SectorCoord) -> Self {
        Self {
            x: coord.0.div_euclid(REGION_DIM),
            y: coord.1.div_euclid(REGION_DIM),
            z: coord.2.div_euclid(REGION_DIM),
        }
    }

    /// On-disk file name `r.<rx>.<ry>.<rz>.strata`.
    pub fn file_name(&self) -> String {
        format!("r.{}.{}.{}.strata", self.x, self.y, self.z)
    }

    /// Flat index of `coord` within this region (0..32768).
    pub fn local_index(coord: SectorCoord) -> usize {
        let lx = coord.0.rem_euclid(REGION_DIM) as usize;
        let ly = coord.1.rem_euclid(REGION_DIM) as usize;
        let lz = coord.2.rem_euclid(REGION_DIM) as usize;
        (ly * REGION_DIM as usize + lz) * REGION_DIM as usize + lx
    }
}

/// One present sector's location in the payload region.
#[derive(Clone, Copy, Debug)]
struct Slot {
    offset: u32,
    size: u32,
}

/// A single region file: 32³ sectors, atomic 5-step writes (plan 15 §1.2.1 / D10).
///
/// The in-RAM `slots` map is rebuilt from disk on open and kept in sync on write.
/// It is authoritative for *this file's* contents; the durable cross-region index
/// lives in the metadata store (M11d).
pub struct RegionFile {
    path: PathBuf,
    slots: HashMap<SectorCoord, Slot>,
}

impl RegionFile {
    /// Open a region file, creating it (empty) if missing.
    pub fn open(path: &Path) -> StorageResult<Self> {
        let path = path.to_path_buf();
        if !path.exists() {
            let empty = RegionFile {
                path,
                slots: HashMap::new(),
            };
            empty.write_atomic_inner(&[])?;
            return Ok(empty);
        }
        let bytes = std::fs::read(&path)?;
        let slots = Self::parse_slots(&bytes)?;
        Ok(RegionFile { path, slots })
    }

    /// Write `payload` for `coord` behind `header`. Returns the byte offset of the
    /// new payload in the file. Uses the atomic 5-step order (plan 15 §1.2.1 / D10).
    pub fn write_sector(
        &mut self,
        coord: SectorCoord,
        header: &SectorHeader,
        payload: &[u8],
    ) -> StorageResult<u64> {
        let existing = std::fs::read(&self.path)?;
        // Drop any prior slot for this coord so re-saves replace rather than
        // accumulate (otherwise the file grows unboundedly per save).
        let mut slots = self.slots.clone();
        slots.remove(&coord);
        let (new_bytes, new_offset) =
            Self::build_append(&existing, &mut slots, coord, header, payload)?;
        self.write_atomic_inner(&new_bytes)?;
        self.slots = slots;
        Ok(new_offset)
    }

    /// Read the raw (still-compressed) payload + header for `coord`.
    /// The caller is responsible for decompression. Verifies the header's
    /// BLAKE3 hash against the raw bytes; returns `CorruptPayload` on mismatch.
    pub fn read_sector(&self, coord: SectorCoord) -> StorageResult<(SectorHeader, Vec<u8>)> {
        let slot = self
            .slots
            .get(&coord)
            .copied()
            .ok_or(StorageError::NotFound(coord))?;
        let bytes = std::fs::read(&self.path)?;
        if bytes.len() < HEADER_LEN {
            return Err(StorageError::CorruptPayload { coord });
        }
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let payload_start = HEADER_LEN + count * SLOT_LEN;
        let start = payload_start + slot.offset as usize;
        let end = start + slot.size as usize;
        if end > bytes.len() {
            return Err(StorageError::CorruptPayload { coord });
        }
        let raw = bytes[start..end].to_vec();
        if raw.len() < SECTOR_HEADER_LEN {
            return Err(StorageError::Region(format!(
                "sector record size {} < header ({SECTOR_HEADER_LEN})",
                raw.len()
            )));
        }
        let header: SectorHeader = bytemuck::pod_read_unaligned(&raw[..SECTOR_HEADER_LEN]);
        header.verify(&raw[SECTOR_HEADER_LEN..])?;
        Ok((header, raw[SECTOR_HEADER_LEN..].to_vec()))
    }

    /// Delete a sector's slot from the index. Rewrites the file without the
    /// payload (compaction) via the atomic path.
    pub fn delete_sector(&mut self, coord: SectorCoord) -> StorageResult<()> {
        if self.slots.remove(&coord).is_none() {
            return Ok(());
        }
        let existing = std::fs::read(&self.path)?;
        let new_bytes = Self::build_without(&existing, &self.slots)?;
        self.write_atomic_inner(&new_bytes)?;
        Ok(())
    }

    /// fsync the region file to durable storage.
    pub fn flush(&self) -> StorageResult<()> {
        let file = std::fs::OpenOptions::new().write(true).open(&self.path)?;
        file.sync_all()?;
        Ok(())
    }

    // --- internal helpers ---

    /// Read the present-sector count and payload region from an existing file image.
    fn read_existing(existing: &[u8]) -> StorageResult<(usize, Vec<u8>)> {
        if existing.len() < HEADER_LEN {
            return Ok((0, Vec::new()));
        }
        let count = u32::from_le_bytes(existing[8..12].try_into().unwrap()) as usize;
        let payload_start = HEADER_LEN + count * SLOT_LEN;
        let payload = if payload_start <= existing.len() {
            existing[payload_start..].to_vec()
        } else {
            Vec::new()
        };
        Ok((count, payload))
    }

    /// Parse the dense slot table out of an existing region file.
    fn parse_slots(bytes: &[u8]) -> StorageResult<HashMap<SectorCoord, Slot>> {
        if bytes.len() < HEADER_LEN {
            return Err(StorageError::Region("file too small to hold header".into()));
        }
        if bytes[0..4] != REGION_MAGIC {
            return Err(StorageError::Region("bad region magic".into()));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != REGION_VERSION {
            return Err(StorageError::Region(format!(
                "unsupported region version {version}"
            )));
        }
        let sector_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let payload_start = HEADER_LEN + sector_count * SLOT_LEN;

        let mut slots = HashMap::with_capacity(sector_count.min(REGION_SECTOR_COUNT));
        let mut cursor = HEADER_LEN;
        for _ in 0..sector_count {
            if cursor + SLOT_LEN > bytes.len() {
                return Err(StorageError::Region("truncated slot table".into()));
            }
            let offset = u32::from_le_bytes(bytes[cursor..cursor + 4].try_into().unwrap());
            let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap());
            cursor += SLOT_LEN;
            if (size as usize) < SECTOR_HEADER_LEN {
                return Err(StorageError::Region(format!(
                    "slot size {size} < sector header ({SECTOR_HEADER_LEN})"
                )));
            }
            let absolute_offset = payload_start + offset as usize;
            if (absolute_offset + size as usize) > bytes.len() {
                return Err(StorageError::Region("slot points past EOF".into()));
            }
            let hdr: SectorHeader = bytemuck::pod_read_unaligned(
                &bytes[absolute_offset..absolute_offset + SECTOR_HEADER_LEN],
            );
            slots.insert(
                SectorCoord(hdr.coord[0], hdr.coord[1], hdr.coord[2]),
                Slot { offset, size },
            );
        }
        Ok(slots)
    }

    /// Serialize present slots + payload, appending the presence-bitmap trailer.
    fn serialize_slots(slots: &HashMap<SectorCoord, Slot>, payload: &[u8]) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(HEADER_LEN + slots.len() * SLOT_LEN + payload.len() + TRAILER_TOTAL);
        out.extend_from_slice(&REGION_MAGIC);
        out.extend_from_slice(&REGION_VERSION.to_le_bytes());
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&(slots.len() as u32).to_le_bytes());
        out.extend_from_slice(&((HEADER_LEN + slots.len() * SLOT_LEN) as u64).to_le_bytes());
        for slot in slots.values() {
            out.extend_from_slice(&slot.offset.to_le_bytes());
            out.extend_from_slice(&slot.size.to_le_bytes());
        }
        out.extend_from_slice(payload);

        // Trailer: presence bitmap (one bit per sector) + footer.
        let mut trailer = vec![0u8; TRAILER_BITMAP_BYTES];
        for coord in slots.keys() {
            let idx = RegionCoord::local_index(*coord);
            trailer[idx / 8] |= 1 << (idx % 8);
        }
        out.extend_from_slice(&trailer);
        out.extend_from_slice(&REGION_MAGIC);
        out.extend_from_slice(&REGION_VERSION.to_le_bytes());
        out.extend_from_slice(&[0u8; 2]);
        out.extend_from_slice(&blake3::hash(payload).as_bytes()[..8]);
        out
    }

    /// Build a new file image that appends `coord`'s payload, returning the image
    /// and the new payload's offset. `slots` MUST already exclude `coord` so a
    /// re-save replaces rather than accumulates (the caller removes it first).
    fn build_append(
        existing: &[u8],
        slots: &mut HashMap<SectorCoord, Slot>,
        coord: SectorCoord,
        header: &SectorHeader,
        payload: &[u8],
    ) -> StorageResult<(Vec<u8>, u64)> {
        // Re-pack only the surviving slots' bytes out of the existing payload
        // region, so a re-saved coord does not leave its old bytes behind.
        let mut new_payload = Vec::new();
        let mut new_slots: HashMap<SectorCoord, Slot> = HashMap::with_capacity(slots.len());
        if let Ok((_count, old_payload)) = Self::read_existing(existing) {
            for (c, slot) in slots.iter() {
                let start = slot.offset as usize;
                let end = start + slot.size as usize;
                if end > old_payload.len() {
                    return Err(StorageError::Region("slot points past payload".into()));
                }
                let offset = new_payload.len() as u32;
                new_payload.extend_from_slice(&old_payload[start..end]);
                new_slots.insert(
                    *c,
                    Slot {
                        offset,
                        size: slot.size,
                    },
                );
            }
        }

        let header_bytes = bytemuck::bytes_of(header);
        let mut record = Vec::with_capacity(header_bytes.len() + payload.len());
        record.extend_from_slice(header_bytes);
        record.extend_from_slice(payload);

        let offset = new_payload.len() as u32;
        new_payload.extend_from_slice(&record);
        new_slots.insert(
            coord,
            Slot {
                offset,
                size: record.len() as u32,
            },
        );
        *slots = new_slots;

        let image = Self::serialize_slots(slots, &new_payload);
        // `serialize_slots` writes payload at HEADER_LEN + slots.len()*SLOT_LEN,
        // so the absolute offset of the new record is that base plus its offset.
        let absolute_offset = (HEADER_LEN + slots.len() * SLOT_LEN) as u64 + offset as u64;
        Ok((image, absolute_offset))
    }

    /// Build a file image with the given (reduced) slot set, re-packing the
    /// surviving payloads contiguously to reclaim the deleted sector's space.
    fn build_without(
        existing: &[u8],
        slots: &HashMap<SectorCoord, Slot>,
    ) -> StorageResult<Vec<u8>> {
        let (_count, old_payload) = Self::read_existing(existing)?;
        let mut new_payload = Vec::with_capacity(old_payload.len());
        let mut new_slots: HashMap<SectorCoord, Slot> = HashMap::with_capacity(slots.len());
        for (coord, slot) in slots {
            let start = slot.offset as usize;
            let end = start + slot.size as usize;
            if end > old_payload.len() {
                return Err(StorageError::Region("slot points past payload".into()));
            }
            let offset = new_payload.len() as u32;
            new_payload.extend_from_slice(&old_payload[start..end]);
            new_slots.insert(
                *coord,
                Slot {
                    offset,
                    size: slot.size,
                },
            );
        }
        Ok(Self::serialize_slots(&new_slots, &new_payload))
    }

    /// Atomic 5-step write (plan 15 §1.2.1 / D10):
    /// 1) write tmp file, 2) fsync tmp, 3) backup current to `.bak`,
    /// 4) rename tmp → final, 5) corruption detection happens on next read (BLAKE3).
    fn write_atomic_inner(&self, bytes: &[u8]) -> StorageResult<()> {
        let dir = self
            .path
            .parent()
            .ok_or_else(|| StorageError::Region("region path has no parent dir".into()))?;
        let final_name = self
            .path
            .file_name()
            .ok_or_else(|| StorageError::Region("region path has no file name".into()))?;
        write_atomic(dir, final_name, bytes)
    }
}

/// Atomic 5-step write shared by all callers (plan 15 §1.2.1 / D10).
///
/// (1) write tmp, (2) fsync tmp, (3) backup current to `.bak`, (4) rename tmp→final
/// (atomic on NTFS/ext4), (5) readers detect corruption via BLAKE3 on next read.
pub fn write_atomic(dir: &Path, final_name: &std::ffi::OsStr, bytes: &[u8]) -> StorageResult<()> {
    let final_path = dir.join(final_name);
    let tmp_name = format!("._tmp.{}", final_name.to_string_lossy());
    let tmp_path = dir.join(&tmp_name);
    let bak_name = format!("{}.bak", final_name.to_string_lossy());
    let bak_path = dir.join(&bak_name);

    // (1) write tmp file.
    {
        let mut f = std::fs::File::create(&tmp_path)?;
        std::io::Write::write_all(&mut f, bytes)?;
        // (2) fsync tmp so step 4 cannot rename a non-durable file.
        f.sync_all()?;
    }

    // (3) back up the current good file (if any).
    if final_path.exists() {
        std::fs::copy(&final_path, &bak_path)?;
    }

    // (4) atomic rename tmp → final.
    std::fs::rename(&tmp_path, &final_path)?;

    // (5) corruption detection deferred to the next read (BLAKE3 verify).
    Ok(())
}
