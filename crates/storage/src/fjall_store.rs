use std::path::Path;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use strata_core::{Chunk, ChunkPos};
use crate::format::{StorageError, deserialize_chunk, serialize_chunk};

/// Fjall LSM-tree based persistent storage for chunks using Fjall 3.x API.
pub struct FjallChunkStore {
    db: Database,
    keyspace: Keyspace,
}

impl FjallChunkStore {
    /// Opens or creates a new Fjall database at the given path.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, StorageError> {
        let db = Database::builder(path)
            .open()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        
        let keyspace = db.keyspace("chunks", || KeyspaceCreateOptions::default())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        
        Ok(Self { db, keyspace })
    }

    /// Generates a unique 8-byte key from a chunk position.
    #[inline]
    fn make_key(pos: ChunkPos) -> [u8; 8] {
        let mut key = [0u8; 8];
        key[0..4].copy_from_slice(&pos.0.x.to_be_bytes());
        key[4..8].copy_from_slice(&pos.0.y.to_be_bytes());
        key
    }

    /// Serializes and saves a chunk to the database.
    pub fn save_chunk(&self, chunk: &Chunk) -> Result<(), StorageError> {
        let key = Self::make_key(chunk.position);
        let data = serialize_chunk(chunk)?;
        self.keyspace.insert(key, data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(())
    }

    /// Loads a chunk from the database, returning `None` if not found.
    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<Chunk>, StorageError> {
        let key = Self::make_key(pos);
        let raw_data = self.keyspace.get(key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
            
        if let Some(data) = raw_data {
            let chunk = deserialize_chunk(&data)?;
            Ok(Some(chunk))
        } else {
            Ok(None)
        }
    }

    /// Checks if a chunk exists in the database.
    pub fn chunk_exists(&self, pos: ChunkPos) -> bool {
        let key = Self::make_key(pos);
        self.keyspace.contains_key(key).unwrap_or(false)
    }

    /// Force database write to disk.
    pub fn persist(&self) -> Result<(), StorageError> {
        self.db.persist(PersistMode::SyncAll)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(())
    }
}
