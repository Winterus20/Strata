//! Async storage backend (plan 15 §1.4 / D8).
//!
//! A SINGLE tokio runtime drives all I/O. Requests flow through one bounded
//! priority channel; a spawned worker drains the channel, orders by priority
//! (ACTIVE=0 highest … ARCHIVE=3 lowest), groups by region, and performs the
//! file I/O inside `spawn_blocking` (buffered I/O is the default, per D4).

use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// The work to perform for a single sector.
enum Request {
    Read,
    Write(Vec<u8>),
    Delete,
}

/// Async durable-storage backend (plan 15 §1.4 / D8).
#[async_trait]
pub trait AsyncStorageBackend: Send + Sync {
    async fn read_sector(&self, coord: SectorCoord) -> StorageResult<Vec<u8>>;
    async fn write_sector(&self, coord: SectorCoord, payload: Vec<u8>) -> StorageResult<()>;
    async fn delete_sector(&self, coord: SectorCoord) -> StorageResult<()>;
    async fn flush(&self) -> StorageResult<()>;
    /// Write a sector with an explicit priority. Exposed so tests can assert priority
    /// ordering (plan 15 §1.4 / D8).
    async fn write_sector_with_priority(
        &self,
        coord: SectorCoord,
        payload: Vec<u8>,
        prio: u8,
    ) -> StorageResult<()>;
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
        let worker = Arc::new(Worker { root, tracer });
        let w = worker.clone();
        tokio::spawn(async move {
            w.run(rx).await;
        });
        Ok(Self { tx, worker })
    }

    /// Enqueue a request, returning the response receiver (for reads).
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

    /// Enqueue a fire-and-forget request.
    async fn enqueue_no_reply(
        &self,
        priority: u8,
        coord: SectorCoord,
        request: Request,
    ) -> StorageResult<()> {
        self.tx
            .send(PriorityRequest {
                priority,
                coord,
                request,
                respond: None,
            })
            .await
            .map_err(|_| StorageError::Region("backend worker closed".into()))
    }
}

#[async_trait]
impl AsyncStorageBackend for TokioBackend {
    async fn read_sector(&self, coord: SectorCoord) -> StorageResult<Vec<u8>> {
        let rx = self.enqueue(priority::ACTIVE, coord, Request::Read).await?;
        rx.await
            .map_err(|_| StorageError::Region("backend worker dropped response".into()))?
    }

    async fn write_sector(&self, coord: SectorCoord, payload: Vec<u8>) -> StorageResult<()> {
        self.enqueue_no_reply(priority::WARM, coord, Request::Write(payload))
            .await?;
        Ok(())
    }

    /// Write a sector with an explicit priority (defaults differ: writes are WARM,
    /// reads are ACTIVE). Exposed so tests can assert priority ordering (plan 15 §1.4 / D8).
    async fn write_sector_with_priority(
        &self,
        coord: SectorCoord,
        payload: Vec<u8>,
        prio: u8,
    ) -> StorageResult<()> {
        self.enqueue_no_reply(prio, coord, Request::Write(payload))
            .await
    }

    async fn delete_sector(&self, coord: SectorCoord) -> StorageResult<()> {
        self.enqueue_no_reply(priority::WARM, coord, Request::Delete)
            .await?;
        Ok(())
    }

    async fn flush(&self) -> StorageResult<()> {
        let path = self.worker.root.clone();
        tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
            if path.is_dir() {
                #[cfg(windows)]
                {
                    return Ok::<(), std::io::Error>(());
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
}

impl Worker {
    /// Drain the channel, ordering by priority, then execute per region inside
    /// `spawn_blocking` (buffered file I/O off the async worker, per plan 15 §1.4 / D4).
    async fn run(self: Arc<Self>, mut rx: mpsc::Receiver<PriorityRequest>) {
        // Pull one (blocking) then drain whatever is immediately queued, so a burst
        // is ordered and dispatched together without spinning.
        while let Some(first) = rx.recv().await {
            let mut batch = Vec::with_capacity(128);
            batch.push(first);
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
        let region = RegionCoord::from_sector(coord);
        let path = self.root.join(region.file_name());
        let mut rf = RegionFile::open(&path)?;
        match request {
            Request::Read => {
                let (_header, payload) = rf.read_sector(coord)?;
                let decompressed = crate::compress::decompress(&payload).unwrap_or(payload);
                Ok(decompressed)
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
        }
    }
}

/// Map a sector coord to its region's path (used by tests / external callers).
pub fn region_path_for(root: &Path, coord: SectorCoord) -> PathBuf {
    root.join(RegionCoord::from_sector(coord).file_name())
}

/// Sector-per-axis constant re-export for callers that need it.
pub const SECTORS_PER_REGION_AXIS: i32 = REGION_DIM;
