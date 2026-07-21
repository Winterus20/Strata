//! Async storage backend (plan 15 §1.4 / D8).
//!
//! A SINGLE tokio runtime drives all I/O. Requests flow through one bounded
//! priority channel; a spawned worker drains the channel, orders by priority
//! (ACTIVE=0 highest … ARCHIVE=3 lowest), groups by region, and performs the
//! file I/O inside `spawn_blocking` (buffered I/O is the default, per D4).
//!
//! # Durability & F6 shutdown
//!
//! - [`AsyncStorageBackend::write_sector`] / [`write_sector_with_priority`](AsyncStorageBackend::write_sector_with_priority)
//!   wait for the worker oneshot before returning `Ok` (durable region write completed).
//! - [`AsyncStorageBackend::sync`] barriers on all previously enqueued work — call
//!   this on client shutdown (F6) after stopping new enqueue.
//! - [`AsyncStorageBackend::flush`] fsyncs opened region files after a [`sync`].
//!
//! Recommended shutdown order for F6:
//! 1. Stop enqueueing new sector saves.
//! 2. `backend.sync().await` — drain in-flight I/O.
//! 3. `backend.flush().await` — fsync region files (best-effort on Windows dirs).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use strata_core::component::SectorCoord;

use crate::envelope::{SectorHeader, Tier};
use crate::error::{StorageError, StorageResult};
use crate::region::{REGION_DIM, RegionCoord, RegionFile};

/// Streaming-tier priority. Lower number = higher priority (plan 15 §1.4 / D8).
pub mod priority {
    /// Highest priority — ACTIVE sectors, resident / GPU-streamed.
    pub const ACTIVE: u8 = 0;
    /// WARM tier (L2 cache staging).
    pub const WARM: u8 = 1;
    /// DISTANT tier.
    pub const DISTANT: u8 = 2;
    /// Lowest priority — ARCHIVE, write-once / read-rarely.
    pub const ARCHIVE: u8 = 3;
}

/// A request to the worker, tagged with a priority and response channel.
pub(crate) struct PriorityRequest {
    priority: u8,
    coord: SectorCoord,
    request: Request,
    respond: Option<oneshot::Sender<StorageResult<Vec<u8>>>>,
}

/// The work to perform for a single sector (or a sync barrier).
enum Request {
    Read,
    Write(Vec<u8>),
    Delete,
    /// Barrier: completes after reaching the worker (prior batch items run first
    /// within a drain; callers should stop enqueueing before `sync`).
    Sync,
}

/// Async durable-storage backend (plan 15 §1.4 / D8).
#[async_trait]
pub trait AsyncStorageBackend: Send + Sync {
    async fn read_sector(&self, coord: SectorCoord) -> StorageResult<Vec<u8>>;
    /// Enqueue a write and **await durable completion** before `Ok`.
    async fn write_sector(&self, coord: SectorCoord, payload: Vec<u8>) -> StorageResult<()>;
    async fn delete_sector(&self, coord: SectorCoord) -> StorageResult<()>;
    /// Fsync region files under the backend root (best-effort). Prefer [`sync`]
    /// first to drain in-flight writes.
    async fn flush(&self) -> StorageResult<()>;
    /// Write a sector with an explicit priority. Awaits durable completion.
    async fn write_sector_with_priority(
        &self,
        coord: SectorCoord,
        payload: Vec<u8>,
        prio: u8,
    ) -> StorageResult<()>;
    /// Drain previously enqueued requests (F6 client shutdown barrier).
    async fn sync(&self) -> StorageResult<()>;
}

/// Bounded priority channel with a fixed `capacity`.
pub(crate) fn bounded_channel(
    capacity: usize,
) -> (
    mpsc::Sender<PriorityRequest>,
    mpsc::Receiver<PriorityRequest>,
) {
    mpsc::channel(capacity)
}

/// tokio-backed backend: one priority channel + one worker task.
#[derive(Clone)]
pub struct TokioBackend {
    tx: mpsc::Sender<PriorityRequest>,
    worker: Arc<Worker>,
}

impl TokioBackend {
    /// Spawn the backend at `root`, creating the worker task on the current runtime.
    pub fn new(root: PathBuf) -> StorageResult<Self> {
        Self::with_order_tracer(root, None)
    }

    /// Like [`new`](Self::new) but, when `tracer` is `Some`, the worker appends the
    /// processed request's priority to it in completion order — used by tests to
    /// assert the priority-channel ordering (plan 15 §1.4 / D8).
    pub fn with_order_tracer(
        root: PathBuf,
        tracer: Option<Arc<tokio::sync::Mutex<Vec<u8>>>>,
    ) -> StorageResult<Self> {
        let (tx, rx) = bounded_channel(128);
        let worker = Arc::new(Worker {
            root,
            tracer,
            region_locks: Mutex::new(HashMap::new()),
        });
        let w = worker.clone();
        tokio::spawn(async move {
            w.run(rx).await;
        });
        Ok(Self { tx, worker })
    }

    /// Enqueue a write without waiting for durable completion.
    ///
    /// Prefer [`AsyncStorageBackend::write_sector`] / `write_sector_with_priority`
    /// when the caller needs `Ok` ⇒ on-disk. Use this only when coalescing a
    /// burst and completing via [`AsyncStorageBackend::sync`] (tests / F6 drain).
    pub async fn write_sector_enqueue(
        &self,
        coord: SectorCoord,
        payload: Vec<u8>,
        prio: u8,
    ) -> StorageResult<()> {
        self.tx
            .send(PriorityRequest {
                priority: prio,
                coord,
                request: Request::Write(payload),
                respond: None,
            })
            .await
            .map_err(|_| StorageError::Region("backend worker closed".into()))
    }

    /// Enqueue a request, returning the response receiver.
    async fn enqueue(
        &self,
        priority: u8,
        coord: SectorCoord,
        request: Request,
    ) -> StorageResult<oneshot::Receiver<StorageResult<Vec<u8>>>> {
        let (respond_tx, respond_rx) = oneshot::channel();
        self.tx
            .send(PriorityRequest {
                priority,
                coord,
                request,
                respond: Some(respond_tx),
            })
            .await
            .map_err(|_| StorageError::Region("backend worker closed".into()))?;
        Ok(respond_rx)
    }

    async fn await_reply(
        rx: oneshot::Receiver<StorageResult<Vec<u8>>>,
    ) -> StorageResult<Vec<u8>> {
        rx.await
            .map_err(|_| StorageError::Region("backend worker dropped response".into()))?
    }
}

#[async_trait]
impl AsyncStorageBackend for TokioBackend {
    async fn read_sector(&self, coord: SectorCoord) -> StorageResult<Vec<u8>> {
        let rx = self.enqueue(priority::ACTIVE, coord, Request::Read).await?;
        Self::await_reply(rx).await
    }

    async fn write_sector(&self, coord: SectorCoord, payload: Vec<u8>) -> StorageResult<()> {
        let rx = self
            .enqueue(priority::WARM, coord, Request::Write(payload))
            .await?;
        Self::await_reply(rx).await?;
        Ok(())
    }

    /// Write a sector with an explicit priority. Awaits durable completion.
    async fn write_sector_with_priority(
        &self,
        coord: SectorCoord,
        payload: Vec<u8>,
        prio: u8,
    ) -> StorageResult<()> {
        let rx = self
            .enqueue(prio, coord, Request::Write(payload))
            .await?;
        Self::await_reply(rx).await?;
        Ok(())
    }

    async fn delete_sector(&self, coord: SectorCoord) -> StorageResult<()> {
        let rx = self
            .enqueue(priority::WARM, coord, Request::Delete)
            .await?;
        Self::await_reply(rx).await?;
        Ok(())
    }

    async fn sync(&self) -> StorageResult<()> {
        // Lowest priority so active work in the same drain batch runs first;
        // callers must stop enqueueing before sync for a true idle barrier.
        let rx = self
            .enqueue(priority::ARCHIVE, SectorCoord(0, 0, 0), Request::Sync)
            .await?;
        Self::await_reply(rx).await?;
        Ok(())
    }

    async fn flush(&self) -> StorageResult<()> {
        let path = self.worker.root.clone();
        tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
            if path.is_dir() {
                // Fsync each region file under root (Windows cannot fsync a directory).
                if let Ok(entries) = std::fs::read_dir(&path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().and_then(|e| e.to_str()) == Some("strata") {
                            if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&p) {
                                let _ = f.sync_all();
                            }
                        }
                    }
                }
                #[cfg(not(windows))]
                {
                    let f = std::fs::OpenOptions::new().read(true).open(&path)?;
                    f.sync_all()?;
                }
            } else if path.exists() {
                let f = std::fs::OpenOptions::new().read(true).open(&path)?;
                f.sync_all()?;
            }
            Ok(())
        })
        .await
        .map_err(|e| StorageError::Region(format!("flush join: {e}")))??;
        Ok(())
    }
}

/// Owns the on-disk region files; one per region, opened lazily and cached.
struct Worker {
    root: PathBuf,
    tracer: Option<Arc<tokio::sync::Mutex<Vec<u8>>>>,
    /// Serialize concurrent I/O to the same region file.
    region_locks: Mutex<HashMap<RegionCoord, Arc<Mutex<()>>>>,
}

impl Worker {
    fn lock_region(&self, region: RegionCoord) -> Arc<Mutex<()>> {
        let mut map = self.region_locks.lock().unwrap();
        map.entry(region)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Drain the channel, ordering by priority, then execute per region inside
    /// `spawn_blocking` (buffered file I/O off the async worker, per plan 15 §1.4 / D4).
    async fn run(self: Arc<Self>, mut rx: mpsc::Receiver<PriorityRequest>) {
        // Pull one (blocking) then drain whatever is immediately queued, so a burst
        // is ordered and dispatched together without spinning.
        while let Some(first) = rx.recv().await {
            let mut batch = Vec::with_capacity(128);
            batch.push(first);
            // Coalesce concurrent enqueues into one priority-sorted batch.
            tokio::task::yield_now().await;
            while let Ok(req) = rx.try_recv() {
                batch.push(req);
            }
            // Higher priority (lower number) first; stable by arrival within a tier.
            batch.sort_by_key(|r| r.priority);
            for req in batch {
                let worker = self.clone();
                // Move the request into the blocking task; respond via the oneshot.
                let PriorityRequest {
                    coord,
                    request,
                    respond,
                    priority,
                } = req;
                let tracer = self.tracer.clone();
                let blocker = move || {
                    let r = worker.execute(coord, request);
                    if let Some(t) = &tracer {
                        // Record completion order (best-effort; ignore lock errors).
                        if let Ok(mut guard) = t.try_lock() {
                            guard.push(priority);
                        }
                    }
                    r
                };
                let join = tokio::task::spawn_blocking(blocker);
                match join.await {
                    Ok(result) => {
                        if let Some(tx) = respond {
                            let _ = tx.send(result);
                        }
                    }
                    Err(_) => {
                        if let Some(tx) = respond {
                            let _ =
                                tx.send(Err(StorageError::Region("blocking task panicked".into())));
                        }
                    }
                }
            }
        }
    }

    /// Perform one sector's file I/O. Runs inside `spawn_blocking`.
    fn execute(&self, coord: SectorCoord, request: Request) -> StorageResult<Vec<u8>> {
        if matches!(request, Request::Sync) {
            return Ok(Vec::new());
        }
        let region = RegionCoord::from_sector(coord);
        let region_lock = self.lock_region(region);
        let _guard = region_lock.lock().unwrap();
        let path = self.root.join(region.file_name());
        let mut rf = RegionFile::open(&path)?;
        match request {
            Request::Read => {
                // Header verify already checked BLAKE3/xxHash of stored bytes.
                // Legacy records may be uncompressed; only zstd-magic frames decode.
                let (_header, payload) = rf.read_sector(coord)?;
                crate::compress::decode_stored_payload(&payload)
            }
            Request::Write(payload) => {
                let compressed = crate::compress::compress(&payload, Tier::Warm)?;
                let header = SectorHeader::new(coord, Tier::Warm, &compressed);
                rf.write_sector(coord, &header, &compressed)?;
                Ok(Vec::new())
            }
            Request::Delete => {
                rf.delete_sector(coord)?;
                Ok(Vec::new())
            }
            Request::Sync => Ok(Vec::new()),
        }
    }
}

/// Map a sector coord to its region's path (used by tests / external callers).
pub fn region_path_for(root: &Path, coord: SectorCoord) -> PathBuf {
    root.join(RegionCoord::from_sector(coord).file_name())
}

/// Sector-per-axis constant re-export for callers that need it.
pub const SECTORS_PER_REGION_AXIS: i32 = REGION_DIM;
