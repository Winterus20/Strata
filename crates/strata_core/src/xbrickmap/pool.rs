//! Global brick pool: the single owner of all live voxel data (plan 06 §1.3,
//! §2.6 / plan 39). Using one `SlotMap` across every sector gives O(1)
//! alloc/free with generational versioning (no dangling handles, zero heap
//! fragmentation). A `SecondaryMap` caches a per-brick uniform-material index.
//!
//! The pool is wrapped in `Arc<RwLock<..>>` so it is `Clone` + `Sync` and can be
//! handed to background meshing threads (the `AsyncComputeTaskPool`) without
//! touching the live `SlotMap` from the main thread. Reads take the shared lock;
//! the few mutations (streaming free/alloc, world-gen) take the exclusive lock.
//! No per-sector `Vec<Brick>` exists anywhere — the heap-fragmentation ban
//! (AGENTS.md §7.G) is enforced structurally here.

use bevy::prelude::*;
use parking_lot::{
    MappedRwLockReadGuard, MappedRwLockWriteGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use serde::{Deserialize, Serialize};
use slotmap::{SecondaryMap, SlotMap, new_key_type};
use std::sync::Arc;

new_key_type! {
    /// Generational handle to a pooled [`Brick`].
    pub struct BrickHandle;
}

/// One 2³ sub-brick: 8 voxels, each storing a sector-local palette index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubBrick {
    /// Per-voxel occupancy (bit set = non-AIR).
    pub voxel_mask: u8,
    /// Sector-local palette index for each of the 8 voxels (0 = AIR).
    pub indices: [u8; 8],
}

/// One 8³ brick: 64 sub-bricks addressed by a `u64` occupancy mask.
///
/// NOTE: plan 05 sketches `Brick = { sub_mask, voxel: [u8; 8] }`, but that
/// single array cannot hold a full 8³ brick's 64 sub-bricks; the constitution
/// (plan 06 §1) is authoritative, so a brick stores all 64 sub-bricks in a
/// fixed, heap-free array. See the milestone report for the deviation note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brick {
    /// Occupancy of the 64 sub-bricks (bit set = at least one voxel present).
    pub sub_mask: u64,
    /// Sub-brick data, brick-local order (index 0..64).
    pub subs: [SubBrick; 64],
}

impl Default for Brick {
    fn default() -> Self {
        Brick {
            sub_mask: 0,
            subs: [SubBrick {
                voxel_mask: 0,
                indices: [0u8; 8],
            }; 64],
        }
    }
}

/// Inner pool storage, shared (via `Arc`) across every `GlobalBrickPool` handle.
pub struct InnerPool {
    pub bricks: SlotMap<BrickHandle, Brick>,
    /// Cached uniform material index per brick (0xFFFF = mixed or empty).
    pub uniform: SecondaryMap<BrickHandle, u16>,
}

impl Default for InnerPool {
    fn default() -> Self {
        InnerPool {
            bricks: SlotMap::with_key(),
            uniform: SecondaryMap::new(),
        }
    }
}

/// Global pool of all live bricks. Shared across sectors; the XBrickMap only
/// stores [`BrickHandle`]s plus a `sector_mask`.
///
/// `Clone` is cheap (clones the `Arc`); the same underlying `SlotMap` is shared,
/// so a `GlobalBrickPool` cloned into a background task reads the same live
/// bricks. Mutations go through the internal `RwLock`.
#[derive(Resource, Default, Clone)]
pub struct GlobalBrickPool {
    inner: Arc<RwLock<InnerPool>>,
}

impl GlobalBrickPool {
    pub fn new() -> Self {
        GlobalBrickPool {
            inner: Arc::new(RwLock::new(InnerPool::default())),
        }
    }

    /// O(1) allocation of a fresh, empty brick.
    ///
    /// The `uniform` cache is left *absent* (not pre-seeded): an absent entry
    /// means "not yet computed", so the first `uniform_index` call actually
    /// derives and caches it. Pre-seeding `0xFFFF` here made `uniform_index`
    /// short-circuit to `None` forever (the compute path was dead code).
    pub fn alloc_brick(&self) -> BrickHandle {
        let mut g = self.inner.write();
        g.bricks.insert(Brick::default())
    }

    /// O(1) free. Drops the brick data and its cached uniform index so a reused
    /// slot never resolves a stale uniform value.
    pub fn free_brick(&self, k: BrickHandle) {
        let mut g = self.inner.write();
        g.bricks.remove(k);
        g.uniform.remove(k);
    }

    /// Drop the cached uniform index for `k` after its voxels change. Callers
    /// that use [`Self::uniform_index`] must invalidate on edit; it is *not*
    /// called from `XBrickMap::set_block` to keep that (per-voxel, world-gen)
    /// hot path free of an extra lock acquisition.
    #[inline]
    pub fn invalidate_uniform(&self, k: BrickHandle) {
        self.inner.write().uniform.remove(k);
    }

    /// Immutably borrow a brick (shared lock). Returns `None` if the handle was
    /// freed — background tasks must treat that as AIR.
    #[inline]
    pub fn brick(&self, k: BrickHandle) -> Option<MappedRwLockReadGuard<'_, Brick>> {
        let g = self.inner.read();
        if g.bricks.contains_key(k) {
            Some(RwLockReadGuard::map(g, |inner| &inner.bricks[k]))
        } else {
            None
        }
    }

    /// Acquire a read guard on the inner pool storage for batch operations.
    /// Helps avoid acquiring the lock repeatedly in hot loops.
    #[inline]
    pub fn read_inner(&self) -> RwLockReadGuard<'_, InnerPool> {
        self.inner.read()
    }

    /// Acquire a write guard on the inner pool storage for batch allocation.
    /// [`CompressedChunkData::unpack`] holds this once per sector instead of
    /// per-brick lock churn (a major source of streaming main-thread stalls).
    #[inline]
    pub fn write_inner(&self) -> RwLockWriteGuard<'_, InnerPool> {
        self.inner.write()
    }

    /// Mutably borrow a brick (exclusive lock).
    #[inline]
    pub fn brick_mut(&self, k: BrickHandle) -> Option<MappedRwLockWriteGuard<'_, Brick>> {
        let g = self.inner.write();
        if g.bricks.contains_key(k) {
            Some(RwLockWriteGuard::map(g, |inner| &mut inner.bricks[k]))
        } else {
            None
        }
    }

    /// True when no bricks are allocated (e.g. every sector is empty).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.read().bricks.is_empty()
    }

    /// Number of currently-allocated bricks (for boundary assertions).
    #[inline]
    pub fn brick_count(&self) -> usize {
        self.inner.read().bricks.len()
    }

    /// Returns the single palette index if the whole brick is one uniform,
    /// non-empty material, else `None`. Computed on first call and cached in
    /// `uniform`; callers must [`Self::invalidate_uniform`] after editing the
    /// brick or the cached value goes stale.
    pub fn uniform_index(&mut self, k: BrickHandle) -> Option<u8> {
        let mut g = self.inner.write();
        if let Some(&cached) = g.uniform.get(k) {
            return if cached == 0xFFFF {
                None
            } else {
                Some(cached as u8)
            };
        }
        let brick = match g.bricks.get(k) {
            Some(b) => b,
            None => {
                g.uniform.insert(k, 0xFFFF);
                return None;
            }
        };
        let mut first: Option<u8> = None;
        for (si, sub) in brick.subs.iter().enumerate() {
            if (brick.sub_mask >> si) & 1 == 0 {
                continue;
            }
            for (vb, &idx) in sub.indices.iter().enumerate() {
                if (sub.voxel_mask >> vb) & 1 == 0 {
                    continue;
                }
                match first {
                    None => first = Some(idx),
                    Some(f) if f != idx => {
                        g.uniform.insert(k, 0xFFFF);
                        return None;
                    }
                    _ => {}
                }
            }
        }
        match first {
            Some(f) => {
                g.uniform.insert(k, f as u16);
                Some(f)
            }
            None => {
                g.uniform.insert(k, 0xFFFF);
                None
            }
        }
    }
}
