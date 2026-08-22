//! XBrickMap: 3-level bitmask voxel store for a 32³ cubic sector (plan 05 / 06).
//!
//! Hierarchy (branchless, shift-only index math — no division on the hot path):
//! ```text
//! Sector  (32³)  -> u64 mask of 64 Bricks       (8³ each)
//! Brick   ( 8³)  -> u64 mask of 64 SubBricks    (2³ each, 8 voxels)
//! SubBrick( 2³)  -> u8  mask of 8 voxels + u8 palette index per voxel
//! ```
//!
//! All live voxel data lives in the single global [`GlobalBrickPool`] (SlotMap +
//! SecondaryMap), never in a per-sector `Vec` (plan 39 heap-fragmentation ban,
//! AGENTS.md §7.G). An empty sector costs zero pool allocation.

use crate::component::SectorCoord;
use crate::registry::{BlockId, PaletteFullError, SectorPalette};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub mod coords;
pub mod pool;

pub use coords::VoxelCoord;
pub use pool::{Brick, BrickHandle, GlobalBrickPool, InnerPool, SubBrick};

/// A fully-filled 32³ sector has this many voxels.
pub const SECTOR_VOXEL_COUNT: usize = 32 * 32 * 32;

/// One 32³ sector's voxel data, addressed by a local [`VoxelCoord`] (0..32).
///
/// `sector_mask` records which of the 64 bricks are live; each live brick is a
/// [`BrickHandle`] into the shared [`GlobalBrickPool`]. This is heap-free (fixed
/// 64-slot handle array, no `Vec<Brick>`) and O(1) for get/set.
///
/// **Not `Clone`:** a shallow copy would alias `BrickHandle`s; freeing one map
/// corrupts the other. Use [`XBrickMap::deep_clone`] (pack→unpack) when a
/// second independent sector is required.
///
/// Stored per sector entity as an ECS `Component` (M3 meshing queries it).
#[derive(Component)]
pub struct XBrickMap {
    pub coord: SectorCoord,
    pub sector_mask: u64,
    bricks: [Option<BrickHandle>; 64],
}

impl XBrickMap {
    pub fn new(coord: SectorCoord) -> Self {
        XBrickMap {
            coord,
            sector_mask: 0,
            bricks: [None; 64],
        }
    }

    /// O(1) handle lookup for an occupied brick index (0..64). Returns `None` when
    /// the brick slot is empty or masked out — used by physics/meshing hot paths
    /// that walk `sector_mask` instead of scanning all 32³ voxels.
    #[inline]
    pub fn brick_handle_at(&self, brick_index: usize) -> Option<BrickHandle> {
        if brick_index >= 64 || (self.sector_mask >> brick_index) & 1 == 0 {
            return None;
        }
        self.bricks[brick_index]
    }

    /// Read the block at `coord`. Returns `BlockId::AIR` (0) for any empty slot
    /// or out-of-range coordinate (pub-field OOB construction).
    /// Branchless bitmask descent; the only branching is the occupancy early-out.
    #[inline]
    pub fn get_block(
        &self,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        coord: VoxelCoord,
    ) -> BlockId {
        if !coord.is_in_sector() {
            return BlockId::AIR;
        }
        let bi = coord.brick_index();
        if bi >= 64 || (self.sector_mask >> bi) & 1 == 0 {
            return BlockId::AIR;
        }
        let brick = match self.bricks[bi].and_then(|h| pool.brick(h)) {
            Some(b) => b,
            None => return BlockId::AIR,
        };
        let si = coord.sub_index();
        if (brick.sub_mask >> si) & 1 == 0 {
            return BlockId::AIR;
        }
        let sub = &brick.subs[si];
        let vb = coord.voxel_bit();
        if (sub.voxel_mask >> vb) & 1 == 0 {
            return BlockId::AIR;
        }
        palette.resolve(sub.indices[vb])
    }

    /// Read the block at `coord` using a pre-locked [`InnerPool`] reference (shared read guard).
    /// Prevents repeated locking overhead inside voxel-iteration hot loops.
    #[inline]
    pub fn get_block_locked(
        &self,
        pool: &InnerPool,
        palette: &SectorPalette,
        coord: VoxelCoord,
    ) -> BlockId {
        if !coord.is_in_sector() {
            return BlockId::AIR;
        }
        let bi = coord.brick_index();
        if bi >= 64 || (self.sector_mask >> bi) & 1 == 0 {
            return BlockId::AIR;
        }
        let handle = match self.bricks[bi] {
            Some(h) => h,
            None => return BlockId::AIR,
        };
        let brick = match pool.bricks.get(handle) {
            Some(b) => b,
            None => return BlockId::AIR,
        };
        let si = coord.sub_index();
        if (brick.sub_mask >> si) & 1 == 0 {
            return BlockId::AIR;
        }
        let sub = &brick.subs[si];
        let vb = coord.voxel_bit();
        if (sub.voxel_mask >> vb) & 1 == 0 {
            return BlockId::AIR;
        }
        palette.resolve(sub.indices[vb])
    }

    /// True if a non-AIR voxel exists at `coord`.
    #[inline]
    pub fn is_occupied(&self, pool: &GlobalBrickPool, coord: VoxelCoord) -> bool {
        if !coord.is_in_sector() {
            return false;
        }
        let bi = coord.brick_index();
        if bi >= 64 || (self.sector_mask >> bi) & 1 == 0 {
            return false;
        }
        let brick = match self.bricks[bi].and_then(|h| pool.brick(h)) {
            Some(b) => b,
            None => return false,
        };
        let si = coord.sub_index();
        if (brick.sub_mask >> si) & 1 == 0 {
            return false;
        }
        let sub = &brick.subs[si];
        let vb = coord.voxel_bit();
        (sub.voxel_mask >> vb) & 1 != 0
    }

    /// Write `block` at `coord`. Allocates the brick (and a pool entry) on first
    /// fill; frees the brick when it becomes empty again. O(1) amortized.
    ///
    /// Out-of-range coords are a no-op (`Ok`). Inconsistent mask/handle state is
    /// recovered (cleared or re-allocated). Palette overflow returns
    /// [`PaletteFullError`] without mutating the voxel.
    #[inline]
    pub fn set_block(
        &mut self,
        pool: &mut GlobalBrickPool,
        palette: &mut SectorPalette,
        coord: VoxelCoord,
        block: BlockId,
    ) -> Result<(), PaletteFullError> {
        if !coord.is_in_sector() {
            return Ok(());
        }
        let bi = coord.brick_index();
        if bi >= 64 {
            return Ok(());
        }

        if block == BlockId::AIR {
            if (self.sector_mask >> bi) & 1 == 0 {
                return Ok(()); // already empty
            }
            let handle = match self.bricks[bi] {
                Some(h) => h,
                None => {
                    // Corrupt: mask bit set without a handle — clear and exit.
                    self.sector_mask &= !(1u64 << bi);
                    return Ok(());
                }
            };
            // Mutate the brick under the write guard, then drop the guard before
            // freeing (RwLock forbids re-entrant write locking). Reborrow the
            // guard into a plain `&mut Brick` so disjoint-field borrows work.
            let became_empty = {
                let Some(mut g) = pool.brick_mut(handle) else {
                    self.bricks[bi] = None;
                    self.sector_mask &= !(1u64 << bi);
                    return Ok(());
                };
                let brick = &mut *g;
                let si = coord.sub_index();
                let sub = &mut brick.subs[si];
                let vb = coord.voxel_bit();
                sub.voxel_mask &= !(1u8 << vb);
                sub.indices[vb] = 0;
                let mut empty = false;
                if sub.voxel_mask == 0 {
                    brick.sub_mask &= !(1u64 << si);
                    if brick.sub_mask == 0 {
                        empty = true;
                    }
                }
                empty
            };
            if became_empty {
                pool.free_brick(handle);
                self.bricks[bi] = None;
                self.sector_mask &= !(1u64 << bi);
            } else {
                pool.invalidate_uniform(handle);
            }
            return Ok(());
        }

        let local = palette.get_or_insert(block)?;

        if (self.sector_mask >> bi) & 1 == 0 || self.bricks[bi].is_none() {
            let handle = pool.alloc_brick();
            self.bricks[bi] = Some(handle);
            self.sector_mask |= 1u64 << bi;
        }
        let mut handle = self.bricks[bi].expect("mask bit implies handle after alloc");
        if pool.brick_mut(handle).is_none() {
            // Dangling handle — replace with a fresh brick.
            handle = pool.alloc_brick();
            self.bricks[bi] = Some(handle);
            self.sector_mask |= 1u64 << bi;
        }
        {
            let mut g = pool
                .brick_mut(handle)
                .expect("fresh or live handle must resolve");
            let brick = &mut *g;
            let si = coord.sub_index();
            let sub = &mut brick.subs[si];
            let vb = coord.voxel_bit();
            brick.sub_mask |= 1u64 << si;
            sub.voxel_mask |= 1u8 << vb;
            sub.indices[vb] = local;
        }
        pool.invalidate_uniform(handle);
        Ok(())
    }

    /// Independent copy via pack→unpack (fresh pool bricks). Shallow `Clone` is
    /// intentionally unsupported — see type docs.
    pub fn deep_clone(
        &self,
        pool: &mut GlobalBrickPool,
        palette: &SectorPalette,
    ) -> Result<(Self, SectorPalette), ChunkDataError> {
        self.pack(pool, palette)?.unpack(pool)
    }

    /// Release every pooled brick owned by this sector back to the pre-locked [`InnerPool`].
    /// Avoids repeated lock acquisition in loops.
    pub fn free_locked(&self, pool: &mut InnerPool) {
        for handle in self.bricks.iter().flatten() {
            pool.bricks.remove(*handle);
            pool.uniform.remove(*handle);
        }
    }

    /// Release every pooled brick owned by this sector back to `GlobalBrickPool`.
    ///
    /// Called by the streaming unload system *before* despawning the sector
    /// entity so the shared pool's memory is reclaimed (LRU steady-state). The
    /// `XBrickMap` itself is dropped on despawn; without this explicit free its
    /// bricks would leak (they live in the global pool, not in the component).
    pub fn free(&self, pool: &mut GlobalBrickPool) {
        let mut inner = pool.write_inner();
        self.free_locked(&mut inner);
    }

    /// Snapshot this sector into a serializable [`CompressedChunkData`].
    ///
    /// Inconsistent mask/handle slots are omitted (recovered) rather than
    /// panicking — the output mask matches bricks that were actually packed.
    pub fn pack(
        &self,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
    ) -> Result<CompressedChunkData, ChunkDataError> {
        let mut out_mask = 0u64;
        let mut bricks = Vec::with_capacity(self.sector_mask.count_ones() as usize);
        for (bi, slot) in self.bricks.iter().enumerate() {
            if (self.sector_mask >> bi) & 1 == 0 {
                continue;
            }
            let Some(handle) = *slot else {
                continue; // recover: mask bit without handle
            };
            let Some(brick) = pool.brick(handle) else {
                continue; // recover: dangling handle
            };
            out_mask |= 1u64 << bi;
            bricks.push(CompressedBrick {
                brick_idx: bi as u8,
                sub_mask: brick.sub_mask,
                subs: brick.subs.to_vec(),
            });
        }
        Ok(CompressedChunkData {
            coord: [self.coord.0, self.coord.1, self.coord.2],
            sector_mask: out_mask,
            palette: palette.entries().to_vec(),
            bricks,
        })
    }
}

// ── Serialized sector snapshot (plan 06 §1.4) ───────────────────────────────

/// Fail-closed errors from pack/unpack of [`CompressedChunkData`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkDataError {
    /// `sector_mask` bit set but no matching brick payload (or vice versa).
    MaskBrickMismatch { brick_index: u8 },
    /// `CompressedBrick::brick_idx` >= 64.
    BrickIdxOutOfRange { brick_idx: u8 },
    /// Brick payload must carry exactly 64 sub-bricks.
    BadSubCount { brick_idx: u8, got: usize },
}

/// Thread-safe, shareable snapshot of one 32³ sector (used later for meshing,
/// SVDAG bake, and network deltas). Round-trips through `postcard`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedChunkData {
    /// Sector coordinate as `[x, y, z]` (serialized without `SectorCoord`'s
    /// `Component` derive to keep the snapshot self-contained).
    pub coord: [i32; 3],
    pub sector_mask: u64,
    /// Sector-local palette: local index -> BlockId (index 0 = AIR).
    pub palette: Vec<BlockId>,
    /// Only occupied bricks (left-packed by `brick_idx`).
    pub bricks: Vec<CompressedBrick>,
}

/// Occupied brick within a [`CompressedChunkData`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedBrick {
    pub brick_idx: u8,
    pub sub_mask: u64,
    /// Exactly 64 sub-bricks (2³ each), in brick-local order.
    pub subs: Vec<SubBrick>,
}

impl CompressedChunkData {
    /// Rebuild an `XBrickMap` + `SectorPalette` from this snapshot, allocating
    /// fresh bricks in `pool`.
    ///
    /// Fail-closed: every set bit in `sector_mask` must have exactly one brick
    /// with that `brick_idx`, and every brick must be covered by the mask.
    pub fn unpack(
        &self,
        pool: &mut GlobalBrickPool,
    ) -> Result<(XBrickMap, SectorPalette), ChunkDataError> {
        let coord = SectorCoord(self.coord[0], self.coord[1], self.coord[2]);
        let mut map = XBrickMap::new(coord);
        let palette = SectorPalette::from_entries(self.palette.clone());

        let mut seen = 0u64;
        for cb in &self.bricks {
            if cb.brick_idx >= 64 {
                return Err(ChunkDataError::BrickIdxOutOfRange {
                    brick_idx: cb.brick_idx,
                });
            }
            let bit = 1u64 << cb.brick_idx;
            if self.sector_mask & bit == 0 {
                return Err(ChunkDataError::MaskBrickMismatch {
                    brick_index: cb.brick_idx,
                });
            }
            if seen & bit != 0 {
                return Err(ChunkDataError::MaskBrickMismatch {
                    brick_index: cb.brick_idx,
                });
            }
            if cb.subs.len() != 64 {
                return Err(ChunkDataError::BadSubCount {
                    brick_idx: cb.brick_idx,
                    got: cb.subs.len(),
                });
            }
            seen |= bit;
        }
        if seen != self.sector_mask {
            // Mask bits without a brick payload.
            let missing = self.sector_mask & !seen;
            let brick_index = missing.trailing_zeros() as u8;
            return Err(ChunkDataError::MaskBrickMismatch { brick_index });
        }

        map.sector_mask = self.sector_mask;
        let mut inner = pool.write_inner();
        for cb in &self.bricks {
            let handle = inner.bricks.insert(Brick::default());
            let brick = &mut inner.bricks[handle];
            brick.sub_mask = cb.sub_mask;
            for (i, s) in cb.subs.iter().enumerate() {
                brick.subs[i] = *s;
            }
            map.bricks[cb.brick_idx as usize] = Some(handle);
        }
        Ok((map, palette))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled_map() -> (XBrickMap, GlobalBrickPool, SectorPalette) {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        for x in 0..32u32 {
            for y in 0..32u32 {
                for z in 0..32u32 {
                    let id = BlockId(
                        ((x.wrapping_mul(7)
                            .wrapping_add(y.wrapping_mul(13))
                            .wrapping_add(z.wrapping_mul(29)))
                            % 16) as u16,
                    );
                    map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, y, z), id)
                        .unwrap();
                }
            }
        }
        (map, pool, palette)
    }

    #[test]
    fn empty_sector_has_no_pool_bricks() {
        let pool = GlobalBrickPool::new();
        let map = XBrickMap::new(SectorCoord(0, 0, 0));
        assert_eq!(map.sector_mask, 0);
        assert!(pool.is_empty(), "empty sector must allocate zero bricks");
    }

    #[test]
    fn fully_filled_sector_allocates_all_bricks() {
        let (map, pool, _) = filled_map();
        assert_eq!(map.sector_mask.count_ones(), 64);
        assert_eq!(pool.brick_count(), 64);
    }

    #[test]
    fn edge_coord_maps_to_top_brick() {
        let (map, pool, palette) = filled_map();
        let edge = VoxelCoord::new(31, 31, 31);
        assert_eq!(edge.brick_index(), 63, "edge must land in the last brick");
        let id = map.get_block(&pool, &palette, edge);
        assert_eq!(id, BlockId(15));
    }

    #[test]
    fn round_trip_pack_unpack_equal() {
        let (map, mut pool, palette) = filled_map();
        let snapshot = map.pack(&pool, &palette).unwrap();
        let (map2, palette2) = snapshot.unpack(&mut pool).unwrap();

        for x in 0..32u32 {
            for y in 0..32u32 {
                for z in 0..32u32 {
                    let c = VoxelCoord::new(x, y, z);
                    assert_eq!(
                        map.get_block(&pool, &palette, c),
                        map2.get_block(&pool, &palette2, c),
                        "mismatch at ({x},{y},{z})"
                    );
                }
            }
        }
        assert_eq!(map.sector_mask, map2.sector_mask);
    }

    #[test]
    fn air_round_trips_through_set_clear() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(1, 2, 3));
        let c = VoxelCoord::new(5, 6, 7);
        map.set_block(&mut pool, &mut palette, c, BlockId(1))
            .unwrap();
        assert!(map.is_occupied(&pool, c));
        assert_eq!(map.get_block(&pool, &palette, c), BlockId(1));
        map.set_block(&mut pool, &mut palette, c, BlockId::AIR)
            .unwrap();
        assert!(!map.is_occupied(&pool, c));
        assert_eq!(map.get_block(&pool, &palette, c), BlockId::AIR);
    }

    #[test]
    fn throughput_1m_get_is_heap_free() {
        let (map, pool, palette) = filled_map();
        let mut sink = 0u16;
        for i in 0..1_000_000u32 {
            let x = i % 32;
            let y = (i / 32) % 32;
            let z = (i / 1024) % 32;
            sink = sink.wrapping_add(map.get_block(&pool, &palette, VoxelCoord::new(x, y, z)).0);
        }
        std::hint::black_box(sink);
    }

    #[test]
    fn unpack_rejects_out_of_bounds_brick_idx() {
        let mut pool = GlobalBrickPool::new();
        let snapshot = CompressedChunkData {
            coord: [0, 0, 0],
            sector_mask: 0,
            palette: vec![BlockId::AIR],
            bricks: vec![CompressedBrick {
                brick_idx: 64,
                sub_mask: 0,
                subs: vec![SubBrick::default(); 64],
            }],
        };
        assert!(snapshot.unpack(&mut pool).is_err());
    }

    #[test]
    fn test_set_block_invalidates_uniform_cache() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        let c1 = VoxelCoord::new(0, 0, 0);
        let c2 = VoxelCoord::new(0, 0, 1);

        map.set_block(&mut pool, &mut palette, c1, BlockId(1))
            .unwrap();
        let handle = map.brick_handle_at(c1.brick_index()).unwrap();

        let idx1 = pool.uniform_index(handle);
        assert!(idx1.is_some());

        map.set_block(&mut pool, &mut palette, c2, BlockId(2))
            .unwrap();

        let idx2 = pool.uniform_index(handle);
        assert_eq!(
            idx2, None,
            "uniform cache must be invalidated after set_block"
        );
    }

    /// Shallow `Clone` of handle arrays aliases pool ownership: freeing one map
    /// must not corrupt an independent deep copy.
    #[test]
    fn deep_clone_survives_original_free() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        let c = VoxelCoord::new(5, 6, 7);
        map.set_block(&mut pool, &mut palette, c, BlockId(3))
            .unwrap();

        let (map2, palette2) = map.deep_clone(&mut pool, &palette).unwrap();
        map.free(&mut pool);

        assert_eq!(
            map2.get_block(&pool, &palette2, c),
            BlockId(3),
            "deep_clone must own independent pool bricks"
        );
        map2.free(&mut pool);
    }

    #[test]
    fn unpack_rejects_mask_bit_without_brick() {
        let mut pool = GlobalBrickPool::new();
        let snapshot = CompressedChunkData {
            coord: [0, 0, 0],
            sector_mask: 1, // bit 0 set
            palette: vec![BlockId::AIR],
            bricks: vec![], // no brick payload
        };
        assert!(
            snapshot.unpack(&mut pool).is_err(),
            "mask bit without brick must fail closed"
        );
    }

    #[test]
    fn unpack_rejects_brick_without_mask_bit() {
        let mut pool = GlobalBrickPool::new();
        let snapshot = CompressedChunkData {
            coord: [0, 0, 0],
            sector_mask: 0,
            palette: vec![BlockId::AIR],
            bricks: vec![CompressedBrick {
                brick_idx: 0,
                sub_mask: 0,
                subs: vec![SubBrick::default(); 64],
            }],
        };
        assert!(snapshot.unpack(&mut pool).is_err());
    }

    #[test]
    fn set_block_recovers_mask_without_handle() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        // Corrupt: mask claims brick 0 occupied, but no handle.
        map.sector_mask = 1;
        map.bricks[0] = None;
        let c = VoxelCoord::new(0, 0, 0);
        map.set_block(&mut pool, &mut palette, c, BlockId::AIR)
            .unwrap();
        assert_eq!(map.sector_mask, 0);
        map.set_block(&mut pool, &mut palette, c, BlockId(1))
            .unwrap();
        assert_eq!(map.get_block(&pool, &palette, c), BlockId(1));
    }

    #[test]
    fn try_new_rejects_out_of_range() {
        assert!(VoxelCoord::try_new(0, 0, 0).is_some());
        assert!(VoxelCoord::try_new(31, 31, 31).is_some());
        assert!(VoxelCoord::try_new(32, 0, 0).is_none());
        assert!(VoxelCoord::try_new(0, 32, 0).is_none());
        assert!(VoxelCoord::try_new(0, 0, 32).is_none());
    }

    #[test]
    fn get_set_block_refuse_oob_voxel_fields() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        // Pub fields allow constructing an OOB coord without `new`/`try_new`.
        let oob = VoxelCoord { x: 32, y: 0, z: 0 };
        assert_eq!(map.get_block(&pool, &palette, oob), BlockId::AIR);
        map.set_block(&mut pool, &mut palette, oob, BlockId(1))
            .unwrap();
        assert_eq!(map.sector_mask, 0, "OOB set must not touch bricks");
        assert!(pool.is_empty());
    }
}
