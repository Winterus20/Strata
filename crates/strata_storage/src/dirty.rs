//! Dirty tracking / write-back pipeline (plan 15 §1.1.3 / §1.6).
//!
//! The sticky dirty flag is a *signal*, not a correctness source (§1.1.3): the
//! durable `dirty` column of the metadata store is the recovery authority. The
//! queue here holds only `SectorCoord` (never `Arc<Sector>`), so a COW swap can
//! never strand a stale edit in the queue. Flags are packed into a sharded
//! `AtomicU64` bitset (64 flags per word) to avoid false sharing on the hot path.

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use strata_core::component::SectorCoord;

/// Mix a sector coord into a 64-bit hash for shard/bit mapping (plan 15 §1.1.3).
fn hash_coord(coord: SectorCoord) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for axis in [coord.0 as u64, coord.1 as u64, coord.2 as u64] {
        h ^= axis.wrapping_add(0x9e37_79b9_7f4a_7c15);
        h = h.rotate_left(13).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    }
    h ^= h >> 29;
    h.wrapping_mul(0xbf58_476d_1ce4_e5b9)
}

#[derive(Default)]
struct QueueState {
    queue: VecDeque<SectorCoord>,
    set: HashSet<SectorCoord>,
}

/// Sticky dirty-flag bitset + per-coord dirty queue (plan 15 §1.1.3 / §1.6).
pub struct DirtyTracker {
    /// Sharded bitset, 64 sticky flags per `AtomicU64`.
    shards: Vec<AtomicU64>,
    /// Coords queued for flush. Holds only coordinates, never `Arc<Sector>`.
    queue: Mutex<QueueState>,
    /// log2 of the bit window carved out of each hash for the in-shard bit.
    shard_bits: usize,
}

impl DirtyTracker {
    /// Build a tracker sized for `capacity_sectors` dirty flags.
    ///
    /// `capacity_sectors` is rounded up to a multiple of 64 sharded words.
    pub fn new(capacity_sectors: usize) -> Self {
        let words = capacity_sectors.div_ceil(64).max(1);
        let shards = (0..words).map(|_| AtomicU64::new(0)).collect();
        Self {
            shards,
            queue: Mutex::new(QueueState::default()),
            shard_bits: 6,
        }
    }

    /// Map a coord to its `(shard_index, in_shard_bit)`.
    fn locate(&self, coord: SectorCoord) -> (usize, u64) {
        let h = hash_coord(coord);
        let shard = (h % self.shards.len() as u64) as usize;
        let bit = (h >> self.shard_bits) & 63;
        (shard, 1u64 << bit)
    }

    /// Mark `coord` dirty: set the sticky bit and enqueue it exactly once.
    ///
    /// The bit guards against double-enqueue — if the bit is already set the
    /// coord is already queued (or inflight), so we skip the push.
    pub fn mark_dirty(&self, coord: SectorCoord) {
        let (shard, bit) = self.locate(coord);
        self.shards[shard].fetch_or(bit, Ordering::Release);
        let mut q = self.queue.lock().unwrap();
        if q.set.insert(coord) {
            q.queue.push_back(coord);
        }
    }

    /// Pop up to `n` dirty coords for flushing, clearing each one's sticky bit
    /// as it leaves the queue. A later `mark_dirty` re-sets the bit (§1.1.3).
    pub fn consume_dirty_batch(&self, n: usize) -> Vec<SectorCoord> {
        let mut q = self.queue.lock().unwrap();
        let mut out = Vec::with_capacity(n.min(q.queue.len()));
        for _ in 0..n {
            let Some(coord) = q.queue.pop_front() else {
                break;
            };
            q.set.remove(&coord);
            let (shard, bit) = self.locate(coord);
            if !q.set.iter().any(|c| self.locate(*c) == (shard, bit)) {
                self.shards[shard].fetch_and(!bit, Ordering::Release);
            }
            out.push(coord);
        }
        out
    }

    /// Clear the sticky bit only after a *durable* commit (plan 15 §1.1.3).
    ///
    /// This is distinct from `consume_dirty_batch`: consumption takes a coord
    /// off the queue, but the bit is only cleared post-commit here.
    pub fn clear(&self, coord: SectorCoord) {
        let mut q = self.queue.lock().unwrap();
        q.set.remove(&coord);
        let (shard, bit) = self.locate(coord);
        if !q.set.iter().any(|c| self.locate(*c) == (shard, bit)) {
            self.shards[shard].fetch_and(!bit, Ordering::Release);
        }
    }

    /// True if `coord` is currently flagged dirty.
    pub fn is_dirty(&self, coord: SectorCoord) -> bool {
        let (shard, bit) = self.locate(coord);
        if self.shards[shard].load(Ordering::Acquire) & bit == 0 {
            return false;
        }
        self.queue.lock().unwrap().set.contains(&coord)
    }

    /// Number of coords currently waiting in the flush queue.
    pub fn pending(&self) -> usize {
        self.queue.lock().unwrap().queue.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_tracker_hash_collision() {
        let tracker = DirtyTracker::new(1);
        let mut found = None;
        let c1 = SectorCoord(0, 0, 0);
        let loc1 = tracker.locate(c1);
        for x in 0..1000 {
            for y in 0..1000 {
                let c2 = SectorCoord(x, y, 1);
                if tracker.locate(c2) == loc1 && c1 != c2 {
                    found = Some(c2);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let c2 = found.expect("must find a colliding coord");
        assert_eq!(tracker.locate(c1), tracker.locate(c2));
        assert_ne!(c1, c2);

        tracker.mark_dirty(c1);
        tracker.mark_dirty(c2);

        assert_eq!(tracker.pending(), 2, "colliding coords must both be queued");
        let batch = tracker.consume_dirty_batch(2);
        assert_eq!(batch.len(), 2);
        assert!(batch.contains(&c1));
        assert!(batch.contains(&c2));
    }
}
