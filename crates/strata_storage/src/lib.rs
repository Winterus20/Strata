//! Strata hybrid tiered storage layer (plan 15).
//!
//! This crate owns the *durable* side of the world: region files, content-addressable
//! deduplication, compression, the metadata store, and the dirty/write-back pipeline.
//! It is deliberately free of any rendering or ECS concepts — those live in `strata_save`
//! and the world crates, which build on top of the primitives here.

pub mod backend;
pub mod cache;
pub mod compress;
pub mod dedup;
pub mod dirty;
pub mod envelope;
pub mod error;
pub mod metadata;
pub mod region;

pub use error::{StorageError, StorageResult};
