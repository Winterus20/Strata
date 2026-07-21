//! M11e region grouping + dedup tests (plan 15 §1.2 / §1.3).

use std::collections::HashSet;

use strata_core::component::SectorCoord;

use strata_storage::dedup::DedupTable;
use strata_storage::envelope::Tier;
use strata_storage::region::RegionCoord;

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
fn region_32_cubed_groups_correctly() {
    // 100 random-but-deterministic coords must each map to the region computed
    // by flooring each axis to a 32-grid (plan 15 §1.2).
    let mut rng = 0xABCD_1234u64;
    let next = |r: &mut u64| {
        *r ^= r.wrapping_shl(13);
        *r ^= r.wrapping_shr(7);
        *r ^= r.wrapping_shl(17);
        *r
    };

    for _ in 0..100 {
        let x = (next(&mut rng) as i32 % 1000) - 500;
        let y = (next(&mut rng) as i32 % 1000) - 500;
        let z = (next(&mut rng) as i32 % 1000) - 500;
        let coord = SectorCoord(x, y, z);
        let rc = RegionCoord::from_sector(coord);
        assert_eq!(rc.x, x.div_euclid(32));
        assert_eq!(rc.y, y.div_euclid(32));
        assert_eq!(rc.z, z.div_euclid(32));
        // And the local index stays in range.
        assert!(RegionCoord::local_index(coord) < 32 * 32 * 32);
    }
}

#[test]
fn dedup_10_identical_sectors_one_region_entry() {
    // 10 identical compressed sectors share one BLAKE3 dedup hash (plan 15 §1.3),
    // so the region appends the payload once and all 10 coords reference it.
    let payload = sample_payload(7, 2048);
    let compressed = strata_storage::compress::compress(&payload, Tier::Distant).unwrap();

    let hash = DedupTable::hash_of(&compressed);
    let mut table = DedupTable::new();

    let mut offsets = HashSet::new();
    for i in 0..10u64 {
        let _coord = SectorCoord(i as i32, 0, 0);
        // Independent compress → identical BLAKE3 (compression is deterministic).
        let h2 = DedupTable::hash_of(
            &strata_storage::compress::compress(&payload, Tier::Distant).unwrap(),
        );
        assert_eq!(h2, hash, "identical input must dedup to one hash");
        // First writer claims the offset; rest share it.
        if let Some((off, _size)) = table.get(&hash) {
            offsets.insert(off);
        } else {
            let off = i * 4096;
            table.insert(hash, off, compressed.len() as u32);
            offsets.insert(off);
        }
    }
    // All 10 reference a single dedup entry (one offset).
    assert_eq!(
        offsets.len(),
        1,
        "10 identical sectors must dedup to one entry"
    );
}
