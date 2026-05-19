use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use strata_core::{Chunk, ChunkPos};
use strata_meshing::{ChunkMeshBuilder, ClassicGreedyMesher, MeshData};
use strata_storage::RegionManager;
use strata_world_gen::TerrainGenerator;

/// Request to generate or load a chunk on the background thread.
struct GenRequest {
    pos: ChunkPos,
}

/// Result from the background thread: generated chunk + mesh.
pub struct GenResult {
    pub pos: ChunkPos,
    pub chunk: Chunk,
    pub mesh: MeshData,
}

/// Background worker that handles the entire chunk pipeline off the main thread:
/// disk load → terrain generation → disk save → mesh generation.
///
/// This prevents FPS drops caused by synchronous terrain gen and disk I/O
/// blocking the render loop.
pub struct ChunkGenWorker {
    sender: Sender<GenRequest>,
    receiver: Receiver<GenResult>,
    _handles: Vec<thread::JoinHandle<()>>,
}

impl ChunkGenWorker {
    /// Creates a new worker with `thread_count` background threads.
    pub fn new(seed: u32, world_data_path: &str, thread_count: usize) -> Self {
        let (send_req, recv_req) = mpsc::channel::<GenRequest>();
        let recv_req = std::sync::Arc::new(std::sync::Mutex::new(recv_req));
        let (send_res, recv_res) = mpsc::channel::<GenResult>();

        let mut handles = Vec::with_capacity(thread_count);

        for i in 0..thread_count {
            let recv = std::sync::Arc::clone(&recv_req);
            let sender = send_res.clone();
            let path = world_data_path.to_string();

            let handle = thread::Builder::new()
                .name(format!("chunk-gen-{}", i))
                .spawn(move || {
                    let terrain_gen = TerrainGenerator::new(seed);
                    let region = RegionManager::new(&path);
                    let mesh_builder = ChunkMeshBuilder::new(ClassicGreedyMesher);

                    loop {
                        // Steal work from the shared queue
                        let req = {
                            let lock = recv.lock().unwrap();
                            lock.recv()
                        };

                        let Ok(req) = req else {
                            break; // Channel closed, exit thread
                        };

                        // 1. Try loading from disk
                        let chunk = if let Ok(Some(chunk)) = region.load_chunk(req.pos) {
                            chunk
                        } else {
                            // 2. Generate terrain
                            let mut chunk = Chunk::new(req.pos);
                            terrain_gen.generate(&mut chunk);
                            // 3. Save to disk (non-critical, ignore errors)
                            let _ = region.save_chunk(&chunk);
                            chunk
                        };

                        // 4. Build mesh
                        let mesh = mesh_builder.build(&chunk);

                        // 5. Send result back to main thread
                        if sender.send(GenResult { pos: req.pos, chunk, mesh }).is_err() {
                            break;
                        }
                    }
                })
                .expect("Failed to spawn chunk-gen thread");

            handles.push(handle);
        }

        Self {
            sender: send_req,
            receiver: recv_res,
            _handles: handles,
        }
    }

    /// Submit a chunk generation request (non-blocking).
    pub fn submit(&self, pos: ChunkPos) {
        let _ = self.sender.send(GenRequest { pos });
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
