//! Game-state save/load (plan 15 §38): world metadata, player data, the versioned
//! save envelope, migration chain, and the Bevy `SavePlugin` that wires the durable
//! `strata_storage` layer into the streaming lifecycle.

pub mod envelope;
pub mod migration;
pub mod player_save_data;
pub mod plugin;
pub mod save_manager;
pub mod world_metadata;

pub use envelope::SaveEnvelope;
pub use migration::MigrationChain;
pub use player_save_data::PlayerSaveData;
pub use save_manager::SaveManager;
pub use world_metadata::WorldMetadata;
