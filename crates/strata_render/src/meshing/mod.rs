//! CPU greedy meshing for Strata (plan 06-meshing-cpu / 09-meshing).
//!
//! Pure-CPU, heap-free-on-the-hot-path meshing that turns a 32³ [`XBrickMap`]
//! (plus its 6 neighbor sectors) into a [`MeshData`] of 8-byte [`PackedQuad`]s.
//! No GPU code lives here — M4 consumes the result.

pub mod ecs;
pub mod greedy;
pub mod occupancy;
pub mod packed_quad;

pub use ecs::{MeshStorage, MeshingPlugin};
pub use greedy::GreedyMesher;
pub use occupancy::{OccupancyScratch, SECTOR_DIM, fill_scratch};
pub use packed_quad::{FaceDir, PackedQuad, PackedVertex4};

use bevy::prelude::*;
use strata_core::prelude::*;

use crate::pipeline::cull::Aabb;

/// Lightmap input to the mesher. Hides the concrete `SectorLight` (ECS
/// component) from the mesher so the meshing crate stays free of
/// `strata_world` types; the client builds a view per sector and passes it
/// into `mesh_sector_planes`.
///
/// The view is a thin wrapper over a 32³ `LightData` array (`sky` + `block`,
/// packed by [`LightData::pack`]). The mesher samples the 4 corners of every
/// quad and writes the average into `PackedQuad.light` (M10a.4).
#[derive(Clone, Copy)]
pub struct SectorLightView<'a> {
    data: &'a [u16; 32 * 32 * 32],
}

impl<'a> SectorLightView<'a> {
    /// Wrap a `SectorLight` (or any compatible array). Borrow-only; the
    /// underlying data is owned by the ECS component the client holds.
    pub fn new(data: &'a [u16; 32 * 32 * 32]) -> Self {
        Self { data }
    }

    /// Build from a 32³ `LightData` array. `data` MUST be length 32768;
    /// callers own the array (typically a frozen `SectorLight`).
    pub fn from_light_data(data: &'a [u16; 32 * 32 * 32]) -> Self {
        Self { data }
    }

    #[inline]
    pub fn get(&self, c: VoxelCoord) -> LightSample {
        let idx = (c.y as usize * 32 + c.z as usize) * 32 + c.x as usize;
        let raw = self.data[idx];
        LightSample {
            sky: (raw & 0xF) as u8,
            block: ((raw >> 4) & 0xF) as u8,
        }
    }
}

/// Decoded `LightData` nibble pair. `0..=15` for both channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LightSample {
    pub sky: u8,
    pub block: u8,
}

/// Mesh output: separate opaque / transparent quad batches (plan 05 §18, 09 §3).
///
/// The `Vec<PackedQuad>` buffers are *result* buffers (per-sector live voxel
/// data never lives in a `Vec` — that ban is about storage, not outputs).
///
/// `opaque_gpu`/`transparent_gpu` are the pre-flattened packed-vertex byte
/// buffers (computed on the worker thread so the render frame only clones them),
/// and `aabb` is the precomputed sector-local bounding box for frustum culling.
/// `generation` is bumped by `MeshStorage` so the renderer can tell when a
/// cached sector's GPU buffer must be rebuilt (plan 09 §3, incremental upload).
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub opaque: Vec<PackedQuad>,
    pub transparent: Vec<PackedQuad>,
    pub opaque_gpu: Vec<u8>,
    pub transparent_gpu: Vec<u8>,
    pub aabb: Aabb,
    pub generation: u64,
}

impl MeshData {
    pub fn total_quads(&self) -> usize {
        self.opaque.len() + self.transparent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.total_quads() == 0
    }
}

/// One neighbor sector view for cross-border face culling and AO (plan 09 §4.0).
///
/// `sector`/`palette` are `None` when the neighbor is not loaded; the mesher
/// then treats it as conservative solid (plan 09 §4.0), culling boundary faces.
#[derive(Clone, Copy)]
pub struct NeighborView<'a> {
    pub sector: Option<&'a XBrickMap>,
    pub palette: Option<&'a SectorPalette>,
    pub pool: &'a GlobalBrickPool,
}

/// Trait implemented by every meshing algorithm (plan 09 §2).
pub trait Mesher {
    /// Mesh `sector` and its 6 neighbors into a [`MeshData`].
    ///
    /// NOTE: the constitution's `XBrickMap` reads require `&GlobalBrickPool` and
    /// `&SectorPalette` (core storage is pool-backed, not self-contained), so this
    /// signature carries them plus `registry` for transparency/visibility. This is
    /// a deliberate, noted deviation from the simplified M3 spec signature that
    /// omitted them — block sampling is impossible without them.
    fn mesh_sector(
        &self,
        sector: &XBrickMap,
        palette: &SectorPalette,
        pool: &GlobalBrickPool,
        registry: &BlockRegistry,
        neighbors: &[NeighborView<'_>; 6],
    ) -> MeshData;
}

#[cfg(test)]
mod tests;
