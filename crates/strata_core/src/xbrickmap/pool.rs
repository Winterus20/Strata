//! Global brick pool: the single owner of all live voxel data (plan 06 §1.3,
//! §2.6 / plan 39). Using one `SlotMap` across every sector gives O(1)
//! alloc/free with generational versioning (no dangling handles, zero heap
//! fragmentation). A `SecondaryMap` caches a per-brick uniform-material index.
//!
//! No per-sector `Vec<Brick>` exists anywhere — the heap-fragmentation ban
//! (AGENTS.md §7.G) is enforced structurally here.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use slotmap::{SecondaryMap, SlotMap, new_key_type};

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

/// Global pool of all live bricks. Shared across sectors; the XBrickMap only
/// stores [`BrickHandle`]s plus a `sector_mask`.
#[derive(Debug, Default, Resource)]
pub struct GlobalBrickPool {
    bricks: SlotMap<BrickHandle, Brick>,
    /// Cached uniform material index per brick (0xFFFF = mixed or empty).
    uniform: SecondaryMap<BrickHandle, u16>,
}

impl GlobalBrickPool {
    pub fn new() -> Self {
        GlobalBrickPool {
            bricks: SlotMap::with_key(),
            uniform: SecondaryMap::new(),
        }
    }

    /// O(1) allocation of a fresh, empty brick.
    pub fn alloc_brick(&mut self) -> BrickHandle {
        let k = self.bricks.insert(Brick::default());
        self.uniform.insert(k, 0xFFFF);
        k
    }

    /// O(1) free. The brick's data is dropped; the SecondaryMap slot is
    /// automatically hidden by the version bump, so no manual cleanup.
    pub fn free_brick(&mut self, k: BrickHandle) {
        self.bricks.remove(k);
    }

    #[inline]
    pub fn brick(&self, k: BrickHandle) -> Option<&Brick> {
        self.bricks.get(k)
    }

    #[inline]
    pub fn brick_mut(&mut self, k: BrickHandle) -> Option<&mut Brick> {
        self.bricks.get_mut(k)
    }

    /// True when no bricks are allocated (e.g. every sector is empty).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bricks.is_empty()
    }

    /// Number of currently-allocated bricks (for boundary assertions).
    #[inline]
    pub fn brick_count(&self) -> usize {
        self.bricks.len()
    }

    /// Returns the single palette index if the whole brick is one uniform,
    /// non-empty material, else `None`. Result is cached in `uniform`.
    pub fn uniform_index(&mut self, k: BrickHandle) -> Option<u8> {
        if let Some(&cached) = self.uniform.get(k) {
            return if cached == 0xFFFF {
                None
            } else {
                Some(cached as u8)
            };
        }
        let brick = match self.bricks.get(k) {
            Some(b) => b,
            None => {
                self.uniform.insert(k, 0xFFFF);
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
                        self.uniform.insert(k, 0xFFFF);
                        return None;
                    }
                    _ => {}
                }
            }
        }
        match first {
            Some(f) => {
                self.uniform.insert(k, f as u16);
                Some(f)
            }
            None => {
                self.uniform.insert(k, 0xFFFF);
                None
            }
        }
    }
}
