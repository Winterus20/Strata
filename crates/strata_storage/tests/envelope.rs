//! M11b tests: envelope round-trip, three-tier distinct sizes, dedup, hash
//! collision-smoke, frame-checksum bitflip, empty-sector round-trip (plan 15 §1.2/§1.3).

use strata_core::component::SectorCoord;

use strata_storage::compress::{compress, decompress};
use strata_storage::dedup::DedupTable;
use strata_storage::envelope::{SectorHeader, Tier, compute_frame_checksum, compute_hash};

fn sample_payload(seed: u64, len: usize) -> Vec<u8> {
    // Deterministic pseudo-random payload (xorshift) — no external rng dep.
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
fn header_round_trip_preserves_bytes() {
    for tier in [Tier::Warm, Tier::Distant, Tier::Archive] {
        for sample in 0..16u64 {
            let payload = sample_payload(sample, 256 + (sample as usize) * 37);
            let compressed = compress(&payload, tier).unwrap();
            let header = SectorHeader::new(
                SectorCoord(sample as i32, (sample * 3) as i32, (sample * 7) as i32),
                tier,
                &compressed,
            );
            header
                .verify(&compressed)
                .expect("fresh header must verify");

            let bytes = bytemuck::bytes_of(&header);
            let restored: &SectorHeader = bytemuck::from_bytes(bytes);
            assert_eq!(restored.magic, SectorHeader::MAGIC);
            assert_eq!(restored.version, SectorHeader::VERSION);
            assert_eq!(restored.tier, tier as u8);
            assert_eq!(restored.payload_size, compressed.len() as u32);
            assert_eq!(
                restored.coord,
                [sample as i32, (sample * 3) as i32, (sample * 7) as i32]
            );
            restored
                .verify(&compressed)
                .expect("round-tripped header must verify");
        }
    }
}

#[test]
fn compress_three_tiers_distinct() {
    let payload = sample_payload(42, 4096);
    let w = compress(&payload, Tier::Warm).unwrap();
    let d = compress(&payload, Tier::Distant).unwrap();
    let a = compress(&payload, Tier::Archive).unwrap();
    // Higher level must not produce a *larger* blob than a lower one for this payload,
    // and archive must be the smallest (best ratio) for compressible data.
    assert!(a.len() <= d.len());
    assert!(d.len() <= w.len());
    // Decompress back to identical bytes at every tier.
    assert_eq!(decompress(&w).unwrap(), payload);
    assert_eq!(decompress(&d).unwrap(), payload);
    assert_eq!(decompress(&a).unwrap(), payload);
}

#[test]
fn dedup_identical_payloads_share_hash() {
    let payload = sample_payload(7, 1024);
    let c1 = compress(&payload, Tier::Distant).unwrap();
    let c2 = compress(&payload, Tier::Distant).unwrap();
    assert_eq!(c1, c2, "same input must compress identically");
    assert_eq!(compute_hash(&c1), compute_hash(&c2));

    let mut table = DedupTable::new();
    let h = compute_hash(&c1);
    table.insert(h, 0x1000, c1.len() as u32);
    assert!(table.contains(&h));
    assert_eq!(table.get(&h), Some((0x1000, c1.len() as u32)));
}

#[test]
fn blake3_collision_resistance_smoke() {
    let a = sample_payload(1, 512);
    let b = sample_payload(2, 512);
    assert_ne!(
        compute_hash(&a),
        compute_hash(&b),
        "distinct payloads must hash distinctly"
    );
    // A single byte flip must change the digest.
    let mut flipped = a.clone();
    flipped[0] ^= 0x01;
    assert_ne!(compute_hash(&a), compute_hash(&flipped));
}

#[test]
fn frame_checksum_detects_bitflip() {
    let payload = sample_payload(99, 2048);
    let c = compress(&payload, Tier::Distant).unwrap();
    let sum = compute_frame_checksum(&c);
    let mut corrupted = c.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xff;
    assert_ne!(
        sum,
        compute_frame_checksum(&corrupted),
        "bitflip must change xxh64"
    );
}

#[test]
fn empty_sector_round_trip() {
    // An empty (air-only) sector serializes to a tiny payload. We model that as an
    // 8-byte mask of all-zeroes (plan 06: empty brick = 8-byte mask only).
    let payload = [0u8; 8];
    let c = compress(&payload, Tier::Warm).unwrap();
    let header = SectorHeader::new(SectorCoord(0, 0, 0), Tier::Warm, &c);
    header.verify(&c).expect("empty sector header verifies");
    assert_eq!(decompress(&c).unwrap(), payload.to_vec());
}

/// Compression bomb: decompressed size past the sector cap must Error (not OOM).
#[test]
fn decompress_rejects_over_max_decompressed_size() {
    use strata_storage::compress::{decompress_with_limit, MAX_DECOMPRESSED_SECTOR_BYTES};
    use strata_storage::envelope::compress as env_compress;

    // Highly compressible zeros — tiny on disk, huge when decoded.
    let huge = vec![0u8; 256 * 1024];
    let compressed = env_compress(&huge, Tier::Warm).unwrap();
    assert!(compressed.len() < huge.len());

    let tiny_cap = 64 * 1024;
    assert!(tiny_cap < huge.len());
    let err = decompress_with_limit(&compressed, tiny_cap).expect_err("must cap output");
    assert!(
        matches!(err, strata_storage::StorageError::Decompress(_)),
        "expected Decompress error, got {err:?}"
    );

    // Default API must accept a normal sector-sized blob under the public cap.
    assert!(huge.len() < MAX_DECOMPRESSED_SECTOR_BYTES);
    assert_eq!(
        strata_storage::compress::decompress(&compressed).unwrap(),
        huge
    );
}

#[test]
fn zstd_magic_detection_and_decode_stored_payload() {
    use strata_storage::compress::{
        decode_stored_payload, is_zstd_frame, ZSTD_MAGIC,
    };

    assert!(!is_zstd_frame(&[]));
    assert!(!is_zstd_frame(&[0x28, 0xB5]));
    assert!(!is_zstd_frame(b"postcard-ish-bytes"));
    assert!(is_zstd_frame(&ZSTD_MAGIC));

    let raw = sample_payload(3, 128);
    assert!(!is_zstd_frame(&raw));
    assert_eq!(decode_stored_payload(&raw).unwrap(), raw);

    let compressed = compress(&raw, Tier::Warm).unwrap();
    assert!(is_zstd_frame(&compressed));
    assert_eq!(decode_stored_payload(&compressed).unwrap(), raw);

    // Magic present but truncated frame → fail closed (not raw truncated bytes).
    let truncated = compressed[..6.min(compressed.len())].to_vec();
    assert!(is_zstd_frame(&truncated));
    let err = decode_stored_payload(&truncated).expect_err("bad frame must error");
    assert!(matches!(err, strata_storage::StorageError::Decompress(_)));
}
