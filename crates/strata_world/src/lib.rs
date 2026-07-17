//! Strata world: deterministic world generation (M5).
//!
//! Builds 32³ sectors from a density-function terrain, Whittaker-style biomes,
//! 3D-noise caves, and hash-grid tree structures. Generation is a pure function
//! of `SectorCoord` + a constant seed, so it is fully reproducible and
//! chunk-independent.

pub mod biome;
pub mod generator;
pub mod lighting;
pub mod noise;
pub mod plugin;
pub mod rng;
pub mod streaming;

#[cfg(test)]
mod tests;

pub mod prelude {
    pub use crate::biome::{Biome, biome_at};
    pub use crate::generator::{
        WorldBlocks, density, generate_compressed, generate_sector, generate_sector_in, height_at,
        surface_y,
    };
    pub use crate::lighting::{
        LightData, LightEngine, LightingPlugin, LightingTimers, MAX_LIGHT, SECTOR_VOXELS,
        SectorLight,
    };
    pub use crate::plugin::{Generated, SectorSnapshot, WorldGenPlugin};
    pub use crate::rng::{Pcg32, WORLD_SEED, hash64};
    pub use crate::streaming::{
        DEFAULT_HYSTERESIS, DEFAULT_RADIUS, StreamingManager, StreamingPlugin, StreamingTimers,
        chebyshev, load_priority, world_pos_to_sector,
    };
    pub use strata_core::prelude::*;
}
