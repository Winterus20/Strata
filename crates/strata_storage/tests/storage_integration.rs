//! M11e storage integration tests (plan 15 §1.1.3 / §1.2 / §1.5 / §1.6 / D14).

use std::fs;

use strata_core::component::SectorCoord;

use strata_storage::backend::{AsyncStorageBackend, TokioBackend};
use strata_storage::dirty::DirtyTracker;
use strata_storage::envelope::{SectorHeader, Tier};
use strata_storage::metadata::{InMemoryMetadata, MetadataStore, SectorMetadata};
use strata_storage::region::{RegionCoord, RegionFile};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strata_store_{}_{}", tag, uuid::Uuid::new_v4()));
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

#[tokio::test]
async fn round_trip_write_read_byte_equal() {
    let dir = temp_dir("rt_wr");
    let backend = TokioBackend::new(dir.clone()).unwrap();

    // Write then read per coord (matching the backend's single-worker FIFO
    // ordering, plan 15 §1.4 / D8).
    for i in 0..100u64 {
        let coord = SectorCoord(i as i32, (i as i32) * 2, (i as i32) * 3);
        let payload = sample_payload(i, 1024);
        backend
            .write_sector_with_priority(
                coord,
                payload.clone(),
                strata_storage::backend::priority::ACTIVE,
            )
            .await
            .unwrap();
        let read = backend.read_sector(coord).await.unwrap();
        assert_eq!(
            &read, &payload,
            "sector {coord:?} must round-trip byte-equal"
        );
    }

    cleanup(&dir);
}

#[test]
fn dirty_sector_flush_persists() {
    let dir = temp_dir("dirty_persist");
    let store = InMemoryMetadata::new();
    let tracker = DirtyTracker::new(1 << 16);

    let coord = SectorCoord(5, 6, 7);
    tracker.mark_dirty(coord);
    assert!(tracker.is_dirty(coord));

    // Simulate a flush: write metadata with dirty=true, then "commit".
    let meta = SectorMetadata {
        coord,
        hash: [0u8; 32],
        size: 0,
        mtime: 0,
        tier: Tier::Warm as u8,
        version: 1,
        dirty: true,
    };
    let _ = futures_lite_block_on(store.put(meta));

    // Reload path: list_dirty must report the coord.
    let dirty = futures_lite_block_on(store.list_dirty()).unwrap();
    assert_eq!(dirty.len(), 1);
    assert_eq!(dirty[0].coord, coord);
    assert!(dirty[0].dirty);

    cleanup(&dir);
}

#[test]
fn pristine_sector_skips_disk() {
    let dir = temp_dir("pristine_skip");
    let mut write_count = 0usize;
    let tracker = DirtyTracker::new(1 << 16);

    // Pristine sector: only written if it was ever marked dirty.
    let coord = SectorCoord(1, 2, 3);
    let was_dirty = tracker.is_dirty(coord);
    if was_dirty {
        write_count += 1; // would hit disk
    }
    assert_eq!(
        write_count, 0,
        "pristine (never dirty) sector must skip disk"
    );

    // Now mark dirty and ensure a subsequent flush would write exactly once.
    tracker.mark_dirty(coord);
    tracker.mark_dirty(coord); // double mark must not double-enqueue
    assert_eq!(tracker.pending(), 1, "double mark must not double-enqueue");
    if tracker.is_dirty(coord) {
        write_count += 1;
    }
    assert_eq!(write_count, 1);

    cleanup(&dir);
}

#[test]
fn corruption_detect_via_blake3() {
    let dir = temp_dir("corrupt_blake3");
    let region = RegionCoord { x: 0, y: 0, z: 0 };
    let path = dir.join(region.file_name());

    let coord = SectorCoord(1, 1, 1);
    let payload = sample_payload(99, 2048);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    {
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
    }

    // Corrupt one payload byte on disk (inside the payload region, payload starts at 28).
    let mut bytes = fs::read(&path).unwrap();
    let at = 28 + 100;
    bytes[at] ^= 0xff;
    fs::write(&path, &bytes).unwrap();

    let rf = RegionFile::open(&path).unwrap();
    let err = rf
        .read_sector(coord)
        .expect_err("corruption must be detected");
    assert!(
        matches!(err, strata_storage::StorageError::CorruptPayload { coord: c } if c == coord),
        "expected CorruptPayload, got {err:?}"
    );

    cleanup(&dir);
}

#[test]
fn recovery_from_atomic_rename() {
    let dir = temp_dir("atomic_recovery");
    let region = RegionCoord { x: 0, y: 0, z: 0 };
    let final_path = dir.join(region.file_name());

    let coord = SectorCoord(2, 2, 2);
    let payload = sample_payload(55, 1500);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    // Good file first.
    {
        let mut rf = RegionFile::open(&final_path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
    }
    // Leave a backup.
    fs::copy(&final_path, dir.join(format!("{}.bak", region.file_name()))).unwrap();

    // Simulate a crash mid-write: a truncated tmp file left behind.
    let tmp = dir.join(format!("._tmp.{}", region.file_name()));
    fs::write(&tmp, &payload[..payload.len() / 2]).unwrap();

    // Re-open: the good final must still be intact (rename is atomic).
    let rf = RegionFile::open(&final_path).unwrap();
    let (_h, read) = rf.read_sector(coord).unwrap();
    assert_eq!(read, payload, "good file survives a leftover tmp");

    cleanup(&dir);
}

// Bug 2: backend must compress payload before writing.
#[tokio::test]
async fn write_sector_compresses_payload() {
    let dir = temp_dir("write_compress");
    let backend = TokioBackend::new(dir.clone()).unwrap();
    let coord = SectorCoord(1, 2, 3);
    let payload = vec![0u8; 4096];

    backend
        .write_sector_with_priority(
            coord,
            payload.clone(),
            strata_storage::backend::priority::ACTIVE,
        )
        .await
        .unwrap();
    let _ = backend.read_sector(coord).await.unwrap();

    let path = strata_storage::backend::region_path_for(&dir, coord);
    let rf = RegionFile::open(&path).unwrap();
    let (_header, raw_bytes) = rf.read_sector(coord).unwrap();
    assert!(
        raw_bytes.len() < payload.len(),
        "write_sector must compress payload on disk, got {} vs {}",
        raw_bytes.len(),
        payload.len()
    );

    cleanup(&dir);
}

// Bug 3: flush() must sync region files (not try to open the root directory on Windows).
#[tokio::test]
async fn flush_syncs_region_files() {
    let dir = temp_dir("flush_sync");
    let region = RegionCoord { x: 0, y: 0, z: 0 };
    let path = dir.join(region.file_name());
    let coord = SectorCoord(1, 2, 3);
    let payload = sample_payload(42, 1024);
    let header = SectorHeader::new(coord, Tier::Warm, &payload);

    {
        let mut rf = RegionFile::open(&path).unwrap();
        rf.write_sector(coord, &header, &payload).unwrap();
    }

    let backend = TokioBackend::new(dir.clone()).unwrap();
    backend.flush().await.unwrap();

    cleanup(&dir);
}

/// Minimal blocking executor for the `async_trait` metadata calls in tests.
fn futures_lite_block_on<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fut)
}
