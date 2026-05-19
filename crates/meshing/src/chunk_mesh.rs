use crate::mesher::{MeshData, Mesher};
use strata_core::Chunk;

/// Convenience wrapper that pairs a boxed [`Mesher`] with chunk data.
pub struct ChunkMeshBuilder {
    mesher: Box<dyn Mesher>,
}

impl ChunkMeshBuilder {
    /// Creates a new builder with the given mesher implementation.
    pub fn new(mesher: impl Mesher + 'static) -> Self {
        Self {
            mesher: Box::new(mesher),
        }
    }

    /// Builds a mesh for the given chunk.
    pub fn build(&self, chunk: &Chunk) -> MeshData {
        self.mesher.generate_mesh(chunk)
    }

    /// Returns the name of the underlying mesher.
    pub fn mesher_name(&self) -> &str {
        self.mesher.name()
    }
}
