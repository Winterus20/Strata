//! Strata Fluid Simulation — Water flow system.
//!
//! Provides Minecraft-style water flow simulation with:
//! - Per-chunk water level storage (0–15)
//! - BFS-based flow propagation (down-first, horizontal spread)
//! - Chunk border synchronization
//! - Fixed-timestep ECS integration

pub mod flow;
pub mod plugin;
pub mod water_level;

pub use flow::WaterFlow;
pub use plugin::{FluidPlugin, FluidWorld};
pub use water_level::{ChunkWaterLevels, WaterLevel};

#[cfg(test)]
mod tests;
