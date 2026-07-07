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
use crate::registry::{BlockId, SectorPalette};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub mod coords;
pub mod pool;

pub use coords::VoxelCoord;
pub use pool::{Brick, BrickHandle, GlobalBrickPool, SubBrick};

/// A fully-filled 32³ sector has this many voxels.
pub const SECTOR_VOXEL_COUNT: usize = 32 * 32 * 32;

/// One 32³ sector's voxel data, addressed by a local [`VoxelCoord`] (0..32).
///
/// `sector_mask` records which of the 64 bricks are live; each live brick is a
/// [`BrickHandle`] into the shared [`GlobalBrickPool`]. This is heap-free (fixed
/// 64-slot handle array, no `Vec<Brick>`) and O(1) for get/set.
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

    /// Read the block at `coord`. Returns `BlockId::AIR` (0) for any empty slot.
    /// Branchless bitmask descent; the only branching is the occupancy early-out.
    #[inline]
    pub fn get_block(
        &self,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        coord: VoxelCoord,
    ) -> BlockId {
        let bi = coord.brick_index();
        if (self.sector_mask >> bi) & 1 == 0 {
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

    /// True if a non-AIR voxel exists at `coord`.
    #[inline]
    pub fn is_occupied(&self, pool: &GlobalBrickPool, coord: VoxelCoord) -> bool {
        let bi = coord.brick_index();
        if (self.sector_mask >> bi) & 1 == 0 {
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
    #[inline]
    pub fn set_block(
        &mut self,
        pool: &mut GlobalBrickPool,
        palette: &mut SectorPalette,
        coord: VoxelCoord,
        block: BlockId,
    ) {
        let bi = coord.brick_index();

        if block == BlockId::AIR {
            if (self.sector_mask >> bi) & 1 == 0 {
                return; // already empty
            }
            let handle = self.bricks[bi].unwrap();
            let brick = pool.brick_mut(handle).unwrap();
            let si = coord.sub_index();
            let sub = &mut brick.subs[si];
            let vb = coord.voxel_bit();
            sub.voxel_mask &= !(1u8 << vb);
            sub.indices[vb] = 0;
            if sub.voxel_mask == 0 {
                brick.sub_mask &= !(1u64 << si);
                if brick.sub_mask == 0 {
                    pool.free_brick(handle);
                    self.bricks[bi] = None;
                    self.sector_mask &= !(1u64 << bi);
                }
            }
            return;
        }

        if (self.sector_mask >> bi) & 1 == 0 {
            let handle = pool.alloc_brick();
            self.bricks[bi] = Some(handle);
            self.sector_mask |= 1u64 << bi;
        }
        let handle = self.bricks[bi].unwrap();
        let brick = pool.brick_mut(handle).unwrap();
        let local = palette.get_or_insert(block);
        let si = coord.sub_index();
        let sub = &mut brick.subs[si];
        let vb = coord.voxel_bit();
        brick.sub_mask |= 1u64 << si;
        sub.voxel_mask |= 1u8 << vb;
        sub.indices[vb] = local;
    }

    /// Release every pooled brick owned by this sector back to `GlobalBrickPool`.
    ///
    /// Called by the streaming unload system *before* despawning the sector
    /// entity so the shared pool's memory is reclaimed (LRU steady-state). The
    /// `XBrickMap` itself is dropped on despawn; without this explicit free its
    /// bricks would leak (they live in the global pool, not in the component).
    pub fn free(&self, pool: &mut GlobalBrickPool) {
        for handle in self.bricks.iter().flatten() {
            pool.free_brick(*handle);
        }
    }

    /// Snapshot this sector into a serializable [`CompressedChunkData`].
    pub fn pack(&self, pool: &GlobalBrickPool, palette: &SectorPalette) -> CompressedChunkData {
        let mut bricks = Vec::with_capacity(self.sector_mask.count_ones() as usize);
        for (bi, slot) in self.bricks.iter().enumerate() {
            if (self.sector_mask >> bi) & 1 == 0 {
                continue;
            }
            let handle = slot.unwrap();
            let brick = pool.brick(handle).unwrap();
            bricks.push(CompressedBrick {
                brick_idx: bi as u8,
                sub_mask: brick.sub_mask,
                subs: brick.subs.to_vec(),
            });
        }
        CompressedChunkData {
            coord: [self.coord.0, self.coord.1, self.coord.2],
            sector_mask: self.sector_mask,
            palette: palette.entries().to_vec(),
            bricks,
        }
    }
}

// ── Serialized sector snapshot (plan 06 §1.4) ───────────────────────────────

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
    pub fn unpack(&self, pool: &mut GlobalBrickPool) -> (XBrickMap, SectorPalette) {
        let coord = SectorCoord(self.coord[0], self.coord[1], self.coord[2]);
        let mut map = XBrickMap::new(coord);
        map.sector_mask = self.sector_mask;
        let palette = SectorPalette::from_entries(self.palette.clone());
        for cb in &self.bricks {
            let handle = pool.alloc_brick();
            let brick = pool.brick_mut(handle).unwrap();
            brick.sub_mask = cb.sub_mask;
            for (i, s) in cb.subs.iter().enumerate() {
                brick.subs[i] = *s;
            }
            map.bricks[cb.brick_idx as usize] = Some(handle);
        }
        (map, palette)
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
                    map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, y, z), id);
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
        let snapshot = map.pack(&pool, &palette);
        let (map2, palette2) = snapshot.unpack(&mut pool);

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
        map.set_block(&mut pool, &mut palette, c, BlockId(1));
        assert!(map.is_occupied(&pool, c));
        assert_eq!(map.get_block(&pool, &palette, c), BlockId(1));
        map.set_block(&mut pool, &mut palette, c, BlockId::AIR);
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
}
