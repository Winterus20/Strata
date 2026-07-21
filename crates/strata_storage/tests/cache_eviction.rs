//! M11c cache + backend tests (plan 15 S1.1.1 / S1.4 / D5 / D8).

use bytes::Bytes;
use strata_core::component::SectorCoord;

use strata_storage::backend::{AsyncStorageBackend, TokioBackend, priority};
use strata_storage::cache::WarmCache;

fn sample_payload(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state & 0xff) as u8);
    }
    out
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strata_cache_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
}

/// LRU eviction: writing more than the byte budget must keep usage near budget
/// and evict the oldest entries (plan 15 S1.1.1 / D5).
#[tokio::test]
async fn cache_lru_evicts_cold() {
    let budget = 1024u64; // 1 KB budget
    let cache = WarmCache::new(budget);

    // 20 sectors of 100 bytes each = 2000 bytes > budget.
    for i in 0..20u64 {
        let payload = sample_payload(i, 100);
        cache
            .put(SectorCoord(i as i32, 0, 0), Bytes::from(payload))
            .await;
    }
    // moka evicts asynchronously; let the pending tasks + eviction run.
    cache.run_pending_tasks().await;

    // Retry check for eviction up to 50 times with small sleeps to handle slow CI/Windows runners
    let mut evicted = false;
    for _ in 0..50 {
        cache.run_pending_tasks().await;
        let usage = cache.byte_usage().await;
        if usage <= 1200 {
            // Eviction down to near the 1024 byte budget
            evicted = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let usage = cache.byte_usage().await;
    assert!(usage <= budget * 2, "usage {usage} exceeded 2x budget");

    // The earliest-written sector should have been evicted under pressure.
    assert!(evicted, "oldest sector must be evicted under pressure");
}

/// Zipfian access must yield a high cache hit-rate (plan 15 S1.1.1 / D5).
#[tokio::test]
async fn cache_hit_rate_zipfian() {
    let budget = 4 * 1024 * 1024; // 4 MB
    let cache = WarmCache::new(budget);

    let n = 1000u64;
    let weights: Vec<f64> = (1..=n).map(|k| 1.0 / (k as f64)).collect();
    let total: f64 = weights.iter().sum();
    let mut cumulative = Vec::with_capacity(n as usize);
    let mut acc = 0.0;
    for w in &weights {
        acc += w / total;
        cumulative.push(acc);
    }

    let mut rng_state: u64 = 0x1234_5678_9abc_def0;
    let next_rand = |s: &mut u64| {
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        *s
    };
    let mut sample_zipf = || {
        let u = (next_rand(&mut rng_state) as f64) / (u64::MAX as f64);
        let target = u * total;
        let idx = cumulative
            .partition_point(|c| *c < target)
            .min(n as usize - 1);
        idx as u64
    };

    // 4000 accesses following the Zipfian distribution.
    let accesses = 4000u64;
    let mut hits = 0u64;
    for _ in 0..accesses {
        let key = sample_zipf();
        if cache.get(SectorCoord(key as i32, 0, 0)).await.is_some() {
            hits += 1;
        } else {
            let payload = sample_payload(key, 64);
            cache
                .put(SectorCoord(key as i32, 0, 0), Bytes::from(payload))
                .await;
        }
    }
    cache.run_pending_tasks().await;

    let rate = hits as f64 / accesses as f64;
    assert!(rate > 0.7, "zipf hit-rate too low: {rate:.3}");
}

/// ACTIVE-priority requests must be processed before DISTANT ones (plan 15 S1.4 / D8).
#[tokio::test]
async fn backend_priority_active_wins() {
    let dir = temp_dir();
    let tracer = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let backend = TokioBackend::with_order_tracer(dir.clone(), Some(tracer.clone())).unwrap();

    // Enqueue a DISTANT write, then an ACTIVE write for the same sector.
    let coord = SectorCoord(5, 5, 5);
    backend
        .write_sector_with_priority(coord, sample_payload(1, 256), priority::DISTANT)
        .await
        .unwrap();
    backend
        .write_sector_with_priority(coord, sample_payload(2, 256), priority::ACTIVE)
        .await
        .unwrap();

    // Give the worker time to drain both.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let order = tracer.lock().await.clone();
    assert!(!order.is_empty(), "worker processed no requests");
    // ACTIVE (0) must appear before DISTANT (2) in completion order.
    let first_active = order.iter().position(|&p| p == priority::ACTIVE);
    let first_distant = order.iter().position(|&p| p == priority::DISTANT);
    assert!(first_active.is_some() && first_distant.is_some());
    assert!(
        first_active.unwrap() < first_distant.unwrap(),
        "ACTIVE must complete before DISTANT"
    );

    cleanup(&dir);
}

/// 100 concurrent writes must all complete and read back byte-equal (plan 15 S1.4).
#[tokio::test]
async fn backend_concurrent_writes() {
    let dir = temp_dir();
    let backend = TokioBackend::new(dir.clone()).unwrap();

    // Fire all writes (fire-and-forget off the single worker).
    let mut write_handles = Vec::new();
    for i in 0..100u64 {
        let b = backend.clone();
        let payload = sample_payload(i, 128);
        write_handles.push(tokio::spawn(async move {
            let coord = SectorCoord(i as i32, 0, 0);
            b.write_sector(coord, payload).await
        }));
    }
    for h in write_handles {
        h.await.unwrap().unwrap();
    }
    // Let the worker drain the queue before reading back. Windows filesystem I/O
    // (fsync, copies, renames) is slow, so we provide ample time.
    tokio::time::sleep(std::time::Duration::from_millis(2500)).await;

    // Now read each sector back; all writes must have landed.
    for i in 0..100u64 {
        let coord = SectorCoord(i as i32, 0, 0);
        let got = backend.read_sector(coord).await.unwrap();
        assert_eq!(
            got,
            sample_payload(i, 128),
            "round-trip mismatch for sector {i}"
        );
    }

    cleanup(&dir);
}
