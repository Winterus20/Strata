//! Bevy plugin wiring the durable `strata_storage` save layer into the
//! streaming lifecycle (plan 15 §38 / §5, plan 08 integration).
//!
//! The plugin registers a `FlushScheduler` and a `DirtyQueue` as resources,
//! exposes `SectorSave`/`SectorLoad` messages, and drives a configurable
//! auto-save timer off dirty state. `SaveManager` is supplied by the caller
//! (it needs a `MetadataStore`); the plugin reads it when present. Streaming
//! integration is referenced, not edited here (that lives in `strata_world`,
//! out of scope for this task).
//!
//! # F6 client shutdown flush
//!
//! Order (durable region write → metadata → clear dirty) is enforced inside
//! `handle_sector_saves` because `TokioBackend::write_sector*` now awaits
//! completion. For process exit, F6 should:
//! 1. Stop emitting new `SectorSave` / auto-save ticks.
//! 2. Drain remaining dirty via `DirtyQueue` + `SectorSave` (or direct writes).
//! 3. Call [`strata_storage::backend::AsyncStorageBackend::sync`] then
//!    [`strata_storage::backend::AsyncStorageBackend::flush`] on `SaveBackend`.
//! 4. Rely on `process_saved_sectors` / `DirtyTracker::clear` only after those
//!    durable steps (never clear dirty on consume alone).

use std::sync::Arc;
use std::time::Duration;

use bevy::prelude::*;
use strata_core::component::{ChunkDirty, SectorCoord, SectorSnapshot};
use strata_core::prelude::StrataSet;
use strata_storage::backend::AsyncStorageBackend;
use strata_storage::dirty::DirtyTracker;
use strata_storage::metadata::SectorMetadata;

#[derive(Resource, Clone)]
pub struct SaveBackend(pub strata_storage::backend::TokioBackend);

use crate::save_manager::SaveManager;

/// Default auto-save cadence (plan 38/08: 5 minutes).
pub const DEFAULT_AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(300);

/// Default per-frame flush budget (sectors) for the `FlushScheduler`.
pub const DEFAULT_PER_FRAME_BUDGET: usize = 8;

/// Message: request a durable flush of `0` to disk.
#[derive(Event, Message, Debug, Clone, Copy)]
pub struct SectorSave(pub SectorCoord);

/// Message: request a load of `0` from disk (instead of regenerating).
#[derive(Message, Debug, Clone, Copy)]
pub struct SectorLoad(pub SectorCoord);

/// Resource holding the dirty sector queue that the save plugin flushes
/// (plan 15 §1.1.3). In the full engine this mirrors the streaming-side
/// `DirtyTracker`; here it is owned by the plugin and bridged to streaming
/// (when a sector is unloaded & dirty → flush; when loaded & on disk → load).
#[derive(Resource)]
pub struct DirtyQueue {
    /// Sticky dirty-flag bitset + coord queue (plan 15 §1.1.3).
    pub tracker: Arc<DirtyTracker>,
}

impl Default for DirtyQueue {
    fn default() -> Self {
        Self {
            // Capacity for ~1M sectors of dirty flags before sharding grows.
            tracker: Arc::new(DirtyTracker::new(1 << 20)),
        }
    }
}

/// Flush scheduler resource (plan 15 §1.6 write-back).
///
/// Drains the dirty queue up to `per_frame_budget` sectors per frame. When the
/// queue grows past `backpressure_threshold`, the per-frame budget is scaled up
/// (within `max_budget`) so a backlog does not stall indefinitely.
#[derive(Resource)]
pub struct FlushScheduler {
    /// Sectors flushed per frame under normal load.
    pub per_frame_budget: usize,
    /// Hard cap on the per-frame budget during backpressure.
    pub max_budget: usize,
    /// Queue depth above which backpressure ramps the budget up.
    pub backpressure_threshold: usize,
    /// Accumulated time since the last forced auto-save tick.
    pub elapsed: Duration,
}

impl Default for FlushScheduler {
    fn default() -> Self {
        Self {
            per_frame_budget: DEFAULT_PER_FRAME_BUDGET,
            max_budget: 64,
            backpressure_threshold: 256,
            elapsed: Duration::ZERO,
        }
    }
}

impl FlushScheduler {
    /// Effective per-frame budget given the current dirty-queue depth.
    pub fn effective_budget(&self, queue_depth: usize) -> usize {
        if queue_depth > self.backpressure_threshold {
            self.max_budget
        } else {
            self.per_frame_budget
        }
    }
}

/// Channel sender to notify Bevy of completed sector flushes.
#[derive(Resource, Clone)]
pub struct SavedSender {
    pub tx: tokio::sync::mpsc::UnboundedSender<SectorCoord>,
}

/// Channel receiver that processes completed sector flushes on the main thread.
#[derive(Resource)]
pub struct SavedReceiver {
    pub rx: std::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<SectorCoord>>,
}

/// Bevy plugin that owns the save/load lifecycle (plan 15 §38).
#[derive(Default)]
pub struct SavePlugin {
    /// Auto-save cadence; defaults to [`DEFAULT_AUTO_SAVE_INTERVAL`].
    pub auto_save_interval: Duration,
}

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.add_message::<SectorSave>();
        app.insert_resource(FlushScheduler::default())
            .insert_resource(DirtyQueue::default())
            .insert_resource(SavedSender { tx })
            .insert_resource(SavedReceiver {
                rx: std::sync::Mutex::new(rx),
            })
            // Writers of `SectorSave` (streaming unload in Streaming, dirty
            // track in Physics) must finish before the flush reader. Auto-save
            // + handle + process are chained after Physics so Input break/place
            // snapshot mutations are visible to the same-frame save path.
            .add_systems(
                Update,
                (
                    track_dirty_sectors.in_set(StrataSet::Physics),
                    (auto_save_tick, handle_sector_saves, process_saved_sectors)
                        .chain()
                        .after(StrataSet::Physics),
                ),
            );
    }
}

/// Tracks newly dirtied sectors, marks them in the DirtyQueue, and instantly queues them for saving.
fn track_dirty_sectors(
    dirty: Res<DirtyQueue>,
    q_dirty: Query<&SectorCoord, Added<ChunkDirty>>,
    mut save_writer: MessageWriter<SectorSave>,
) {
    for coord in &q_dirty {
        dirty.tracker.mark_dirty(*coord);
        save_writer.write(SectorSave(*coord));
        bevy::log::info!(
            "strata_save: Instantly queued save for dirtied sector {:?}",
            coord
        );
    }
}

/// Auto-save tick: every `auto_save_interval` of accumulated time, flush any
/// queued dirty sectors by emitting `SectorSave` messages. The timer uses
/// `Res<Time>` delta accumulation so it pauses correctly when the app is idle.
fn auto_save_tick(
    time: Res<Time>,
    mut scheduler: ResMut<FlushScheduler>,
    manager: Option<Res<SaveManager>>,
    dirty: Res<DirtyQueue>,
    mut save_writer: MessageWriter<SectorSave>,
) {
    scheduler.elapsed = scheduler.elapsed.saturating_add(time.delta());

    let interval = manager
        .as_ref()
        .map(|m| m.auto_save_interval)
        .unwrap_or(DEFAULT_AUTO_SAVE_INTERVAL);

    if scheduler.elapsed < interval {
        return;
    }
    scheduler.elapsed = Duration::ZERO;

    let depth = dirty.tracker.pending();
    if depth == 0 {
        return;
    }
    let budget = scheduler.effective_budget(depth);
    for coord in dirty.tracker.consume_dirty_batch(budget) {
        save_writer.write(SectorSave(coord));
    }
}

/// Handle incoming `SectorSave` events: serialize the snapshot on the background TaskPool
/// and commit the payload to disk + LSM metadata store, then notify the main thread.
fn handle_sector_saves(
    mut events: MessageReader<SectorSave>,
    manager: Option<Res<SaveManager>>,
    backend: Option<Res<SaveBackend>>,
    sectors: Query<(&SectorCoord, &SectorSnapshot)>,
    sender: Res<SavedSender>,
) {
    let Some(mgr) = manager else {
        return;
    };
    let Some(bk) = backend else {
        return;
    };

    let sector_map: std::collections::HashMap<SectorCoord, &SectorSnapshot> = sectors
        .iter()
        .map(|(coord, snapshot)| (*coord, snapshot))
        .collect();

    for SectorSave(coord) in events.read() {
        if let Some(snapshot) = sector_map.get(coord) {
            let coord_val = *coord;
            let snapshot_data = snapshot.0.clone();
            let bk_val = bk.0.clone();
            let metadata_store = mgr.metadata.clone();
            let tx_val = sender.tx.clone();

            let payload = match postcard::to_allocvec(&*snapshot_data) {
                Ok(bytes) => bytes,
                Err(e) => {
                    bevy::log::error!("Failed to serialize sector {coord_val:?}: {e}");
                    continue;
                }
            };

            let payload_hash = blake3::hash(&payload).into();
            let payload_size = payload.len() as u64;

            tokio::spawn(async move {
                // (1) Write payload using TokioBackend with ACTIVE priority to ensure arrival order
                if let Err(e) = bk_val
                    .write_sector_with_priority(
                        coord_val,
                        payload,
                        strata_storage::backend::priority::ACTIVE,
                    )
                    .await
                {
                    bevy::log::error!("Failed to write sector {coord_val:?} to disk: {e}");
                    return;
                }

                // (2) Save metadata to durable store
                let mtime = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let meta = SectorMetadata {
                    coord: coord_val,
                    hash: payload_hash,
                    size: payload_size,
                    mtime,
                    tier: 0, // WARM
                    version: 1,
                    dirty: false,
                };

                if let Err(e) = metadata_store.put(meta).await {
                    bevy::log::error!("Failed to write sector {coord_val:?} metadata: {e}");
                    return;
                }

                // (3) Notify main thread to clear dirty flag
                tx_val.send(coord_val).ok();
            });
        }
    }
}

/// Drain completed sector saves from the channel and clear their dirty flags.
fn process_saved_sectors(receiver: Res<SavedReceiver>, dirty: Res<DirtyQueue>) {
    if let Ok(mut rx) = receiver.rx.lock() {
        while let Ok(coord) = rx.try_recv() {
            dirty.tracker.clear(coord);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::component::SectorSnapshot;
    use strata_core::xbrickmap::CompressedChunkData;
    use strata_storage::metadata::InMemoryMetadata;

    #[tokio::test]
    async fn test_save_tracking_and_hashmap_lookup() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(SavePlugin::default());

        let dir = std::env::temp_dir().join(format!("strata_save_test_{}", uuid::Uuid::new_v4()));
        let backend = strata_storage::backend::TokioBackend::new(dir.clone()).unwrap();
        let meta_store = Arc::new(InMemoryMetadata::new());
        let save_mgr = SaveManager::new(meta_store, DEFAULT_AUTO_SAVE_INTERVAL);

        app.insert_resource(SaveBackend(backend));
        app.insert_resource(save_mgr);

        let coord = SectorCoord(10, 20, 30);
        let snapshot_data = Arc::new(CompressedChunkData {
            coord: [10, 20, 30],
            sector_mask: 0,
            palette: vec![],
            bricks: vec![],
        });

        app.world_mut()
            .spawn((coord, SectorSnapshot(snapshot_data), ChunkDirty));

        app.update();

        let dirty_queue = app.world().resource::<DirtyQueue>();
        assert!(
            dirty_queue.tracker.is_dirty(coord),
            "Save tracking system must catch ChunkDirty before removal"
        );
    }
}
