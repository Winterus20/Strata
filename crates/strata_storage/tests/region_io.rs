//! M11c region file tests (plan 15 §1.2 / §1.2.1 / D9 / D10).

use strata_core::component::SectorCoord;

use strata_storage::envelope::{SectorHeader, Tier};
use strata_storage::region::{REGION_SECTOR_COUNT, RegionCoord, RegionFile};

use std::fs;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strata_test_{}_{}", tag, uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &std::path::Path) {
    let _ = fs::remove_dir_all(dir);
}

fn sample_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_add(0x9e3779b97f4a7c15);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xff) as u8);
    }
    out
}

#[test]
fn region_write_read_round_trip() {
    let dir = temp_dir("region_rt");
    let region = RegionCoord { x: 1, y: -1, z: 2 };
    let path = dir.join(region.file_name());

    let coord = SectorCoord(35, -10, 70); // inside region (1,-1,2)
    let payload = sample_payload(123, 2048);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    {
        let mut rf = RegionFile::open(&path).unwrap();
        let offset = rf.write_sector(coord, &header, &payload).unwrap();
        assert!(offset > 0, "payload must not be at the header offset");
        rf.flush().unwrap();
    }

    let rf = RegionFile::open(&path).unwrap();
    let (read_header, read_payload) = rf.read_sector(coord).unwrap();
    assert_eq!(
        read_payload, payload,
        "compressed payload must round-trip byte-equal"
    );
    assert_eq!(read_header.coord, [coord.0, coord.1, coord.2]);
    assert_eq!(read_header.payload_hash, header.payload_hash);

    cleanup(&dir);
}

#[test]
fn region_atomic_write_no_corrupt() {
    let dir = temp_dir("region_atomic");
    let region = RegionCoord { x: 0, y: 0, z: 0 };
    let path = dir.join(region.file_name());

    let coord = SectorCoord(3, 4, 5);
    let payload = sample_payload(7, 4096);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    // Write a good file first.
    {
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
        rf.flush().unwrap();
    }

    // Simulate a mid-write crash: drop a truncated tmp file and a stale `.bak`,
    // then re-open. The good final file must still be intact (atomic rename
    // guarantees the final was never half-written).
    let tmp = dir.join(format!("._tmp.{}", region.file_name()));
    fs::write(&tmp, &payload[..payload.len() / 2]).unwrap();
    fs::copy(&path, dir.join(format!("{}.bak", region.file_name()))).unwrap();

    let rf = RegionFile::open(&path).unwrap();
    let (_h, read_payload) = rf.read_sector(coord).unwrap();
    assert_eq!(
        read_payload, payload,
        "good file survives a truncated tmp left behind"
    );

    cleanup(&dir);
}

#[test]
fn region_corrupt_payload_detected_via_blake3() {
    let dir = temp_dir("region_corrupt");
    let region = RegionCoord { x: 2, y: 2, z: 2 };
    let path = dir.join(region.file_name());

    let coord = SectorCoord(70, 70, 70);
    let payload = sample_payload(55, 3000);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    {
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
        rf.flush().unwrap();
    }

    // Corrupt one byte of the payload on disk (skip the header, flip a payload byte).
    let mut bytes = fs::read(&path).unwrap();
    let corrupt_at = 100;
    bytes[corrupt_at] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let rf = RegionFile::open(&path).unwrap();
    let err = rf
        .read_sector(coord)
        .expect_err("corrupt payload must be rejected");
    assert!(
        matches!(err, strata_storage::StorageError::CorruptPayload { coord: c } if c == coord),
        "expected CorruptPayload, got {err:?}"
    );

    cleanup(&dir);
}

#[test]
fn region_rewrite_same_coord_does_not_accumulate() {
    let dir = temp_dir("region_rewrite");
    let region = RegionCoord { x: 0, y: 0, z: 0 };
    let path = dir.join(region.file_name());

    let coord = SectorCoord(3, 4, 5);

    // Re-save the same coord many times with different payloads. A correct region
    // file must replace in place and keep exactly one slot, so the file must NOT
    // grow unboundedly and reads must always return the latest payload.
    let mut last_payload = Vec::new();
    let mut first_size: u64 = 0;
    for i in 0..50u64 {
        let payload = sample_payload(i.wrapping_mul(2654435761), 4096);
        let header = SectorHeader::new(coord, Tier::Warm, &payload);
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
        rf.flush().unwrap();
        last_payload = payload;

        let size = fs::metadata(&path).unwrap().len();
        if i == 0 {
            first_size = size;
        }
        // Allow only modest growth (trailer overhead), never linear per-write growth.
        assert!(
            size <= first_size * 2,
            "region file grew unbounded on rewrite: size={size} first={first_size} iter={i}"
        );
    }

    let rf = RegionFile::open(&path).unwrap();
    let (_h, read) = rf.read_sector(coord).unwrap();
    assert_eq!(read, last_payload, "latest write must win, not an old copy");

    cleanup(&dir);
}

// Bug 1: Windows atomic rename must overwrite existing file.
#[test]
fn write_atomic_overwrite_existing_file() {
    let dir = temp_dir("atomic_overwrite");
    let path = dir.join("test.strata");
    std::fs::write(&path, b"original").unwrap();

    strata_storage::region::write_atomic(&dir, std::ffi::OsStr::new("test.strata"), b"new data")
        .unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"new data");
    cleanup(&dir);
}

// Bug 4: malicious sector_count must not cause DoS.
#[test]
fn region_malicious_sector_count_no_dos() {
    let dir = temp_dir("region_malicious");
    let path = dir.join("r.0.0.0.strata");

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"STRG");
    bytes.extend_from_slice(&[1u8, 0]);
    bytes.extend_from_slice(&[0u8; 2]);
    bytes.extend_from_slice(&(REGION_SECTOR_COUNT as u32 + 1).to_le_bytes()); // sector_count = 32769
    bytes.extend_from_slice(&[0u8; 8]);

    std::fs::write(&path, &bytes).unwrap();

    let start = std::time::Instant::now();
    let result = RegionFile::open(&path);
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "sector_count exceeding REGION_SECTOR_COUNT must be rejected"
    );
    assert!(
        elapsed.as_millis() < 1000,
        "parse_slots must not loop excessively, took {}ms",
        elapsed.as_millis()
    );

    cleanup(&dir);
}

/// Undersized slot (size < SectorHeader) must Error in parse_slots — never slice-panic.
#[test]
fn region_undersized_slot_rejected_no_panic() {
    let dir = temp_dir("region_undersized_slot");
    let path = dir.join("r.0.0.0.strata");

    // Craft: valid region magic/version, one slot with size=8 (< SectorHeader len).
    let sector_header_len = std::mem::size_of::<SectorHeader>();
    assert!(sector_header_len > 8);

    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"STRG");
    bytes.extend_from_slice(&1u16.to_le_bytes()); // version
    bytes.extend_from_slice(&[0u8; 2]); // pad
    bytes.extend_from_slice(&1u32.to_le_bytes()); // sector_count = 1
    bytes.extend_from_slice(&20u64.to_le_bytes()); // payload_base (HEADER_LEN)
    // Slot: offset=0, size=8 (too small for SectorHeader)
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&8u32.to_le_bytes());
    // Tiny fake payload so EOF checks pass before header parse.
    bytes.extend_from_slice(&[0u8; 8]);

    fs::write(&path, &bytes).unwrap();

    let result = std::panic::catch_unwind(|| RegionFile::open(&path));
    assert!(result.is_ok(), "parse must not panic");
    let open = result.unwrap();
    assert!(
        open.is_err(),
        "slot size < SectorHeader must be rejected"
    );

    cleanup(&dir);
}

// Bug 5/6: read_sector on a file that became smaller than header must not panic.
#[test]
fn region_read_sector_truncated_file_no_panic() {
    let dir = temp_dir("region_truncated");
    let path = dir.join("r.0.0.0.strata");
    let coord = SectorCoord(1, 2, 3);
    let payload = sample_payload(42, 1024);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    {
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
    }

    let rf = RegionFile::open(&path).unwrap();

    // Truncate file to 10 bytes (< HEADER_LEN=20, < 12 needed for count)
    std::fs::write(&path, b"STRG\x00\x01\x00\x00\x00").unwrap();

    let result = rf.read_sector(coord);
    assert!(
        result.is_err(),
        "truncated file must return error, not panic"
    );

    cleanup(&dir);
}
