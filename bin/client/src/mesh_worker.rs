use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use strata_core::{Chunk, ChunkPos};
use strata_meshing::{ChunkMeshBuilder, MeshData};

struct MeshRequest {
    pos: ChunkPos,
    chunk: Chunk,
}

struct MeshResult {
    pos: ChunkPos,
    mesh: MeshData,
}

/// Background thread for mesh generation to avoid main thread stalls.
pub struct MeshWorker {
    sender: Sender<MeshRequest>,
    receiver: Receiver<MeshResult>,
    _handle: thread::JoinHandle<()>,
}

impl MeshWorker {
    pub fn new(builder: ChunkMeshBuilder) -> Self {
        let (send_req, recv_req) = mpsc::channel::<MeshRequest>();
        let (send_res, recv_res) = mpsc::channel::<MeshResult>();

        let handle = thread::spawn(move || {
            while let Ok(req) = recv_req.recv() {
                let mesh = builder.build(&req.chunk);
                if send_res.send(MeshResult { pos: req.pos, mesh }).is_err() {
                    break;
                }
            }
        });

        Self {
            sender: send_req,
            receiver: recv_res,
            _handle: handle,
        }
    }

    /// Submit a mesh generation request (non-blocking).
    pub fn submit(&self, pos: ChunkPos, chunk: Chunk) {
        let _ = self.sender.send(MeshRequest { pos, chunk });
    }

    /// Poll for completed meshes. Returns all finished results.
    pub fn poll(&self) -> Vec<(ChunkPos, MeshData)> {
        let mut results = Vec::new();
        while let Ok(res) = self.receiver.try_recv() {
            results.push((res.pos, res.mesh));
        }
        results
    }
}
