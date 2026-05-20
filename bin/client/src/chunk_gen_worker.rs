use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use strata_core::{Chunk, ChunkPos};
use strata_storage::RegionManager;
use strata_world_gen::TerrainGenerator;

/// Shared work queue — all threads compete for work from the same queue.
/// No per-thread channels, no lost work when a thread dies.
struct SharedState {
    queue: Mutex<VecDeque<ChunkPos>>,
    condvar: Condvar,
}

/// Result from a background thread: generated chunk (not yet meshed).
/// Meshing happens after borders are filled from neighbors.
pub struct GenResult {
    pub pos: ChunkPos,
    pub chunk: Chunk,
}

/// Background worker pool with a **shared work queue** and `Condvar`.
///
/// - All threads pull from one `Mutex<VecDeque>` – no work gets stranded.
/// - `Condvar` wakes one thread when new work arrives.
/// - If a thread panics, surviving threads continue processing the queue.
///   The lost chunk will be re-requested on the next `should_update`.
pub struct ChunkGenWorker {
    shared: Arc<SharedState>,
    receiver: Receiver<GenResult>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl ChunkGenWorker {
    /// Creates a new worker pool with `thread_count` background threads.
    pub fn new(seed: u32, world_data_path: &str, thread_count: usize) -> Self {
        let shared = Arc::new(SharedState {
            queue: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        });

        let (send_res, recv_res) = mpsc::channel::<GenResult>();
        let mut handles = Vec::with_capacity(thread_count);

        for i in 0..thread_count {
            let shared_clone = Arc::clone(&shared);
            let sender = send_res.clone();
            let path = world_data_path.to_string();

            let handle = thread::Builder::new()
                .name(format!("chunk-gen-{}", i))
                .spawn(move || {
                    let mut terrain_gen = TerrainGenerator::new(seed);
                    let region = RegionManager::new(&path);

                    loop {
                        // Pop work from the shared queue (block with condvar)
                        let pos = {
                            let mut queue = shared_clone.queue.lock().unwrap();
                            loop {
                                if let Some(pos) = queue.pop_front() {
                                    break pos;
                                }
                                queue = shared_clone.condvar.wait(queue).unwrap();
                            }
                        };

                        let chunk = if let Ok(Some(chunk)) = region.load_chunk(pos) {
                            chunk
                        } else {
                            let mut chunk = Chunk::new(pos);
                            terrain_gen.generate(&mut chunk);
                            let _ = region.save_chunk(&chunk);
                            chunk
                        };

                        if sender.send(GenResult { pos, chunk }).is_err() {
                            break;
                        }
                    }
                })
                .expect("Failed to spawn chunk-gen thread");

            handles.push(handle);
        }

        Self {
            shared,
            receiver: recv_res,
            _handles: handles,
        }
    }

    /// Submit a chunk generation request (non-blocking).
    pub fn submit(&mut self, pos: ChunkPos) {
        let mut queue = self.shared.queue.lock().unwrap();
        queue.push_back(pos);
        self.shared.condvar.notify_one();
    }

    /// Poll for completed chunks. Returns all finished results.
    pub fn poll(&self) -> Vec<GenResult> {
        let mut results = Vec::new();
        while let Ok(res) = self.receiver.try_recv() {
            results.push(res);
        }
        results
    }
}
