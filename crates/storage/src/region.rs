use crate::format::{StorageError, deserialize_chunk};
use crate::fjall_store::FjallChunkStore;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;
use strata_core::ChunkPos;

static DB_REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<FjallChunkStore>>>> = OnceLock::new();

/// Manages per-region directories for chunk persistence and provides LSM-Tree database access.
pub struct RegionManager {
    base_path: PathBuf,
    fjall_db: Arc<FjallChunkStore>,
}

impl RegionManager {
    /// Creates a new region manager rooted at the given directory.
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        let base = base_path.into();
        std::fs::create_dir_all(&base).ok();
        
        let abs_path = std::fs::canonicalize(&base).unwrap_or_else(|_| base.clone());
        let db_path = abs_path.join("fjall_db");

        let registry = DB_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
        let mut map = registry.lock().unwrap();
        
        let fjall_db = map.entry(db_path.clone()).or_insert_with(|| {
            Arc::new(FjallChunkStore::new(&db_path).expect("Failed to open Fjall database"))
        }).clone();

        Self {
            base_path: abs_path,
            fjall_db,
        }
    }

    fn old_chunk_path(&self, pos: ChunkPos) -> PathBuf {
        let region_x = pos.0.x.div_euclid(32);
        let region_z = pos.0.y.div_euclid(32);
        let region_dir = self.base_path.join(format!("r{}_r{}", region_x, region_z));
        region_dir.join(format!("c{}_c{}.dat", pos.0.x, pos.0.y))
    }

    /// Serializes and writes a chunk to the Fjall LSM-Tree database.
    pub fn save_chunk(&self, chunk: &strata_core::Chunk) -> Result<(), StorageError> {
        self.fjall_db.save_chunk(chunk)?;
        Ok(())
    }

    /// Loads a chunk, checking Fjall database first. Fallback to old Region files for transparent migration.
    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<strata_core::Chunk>, StorageError> {
        // 1. Try loading from Fjall LSM-Tree
        if let Some(chunk) = self.fjall_db.load_chunk(pos)? {
            return Ok(Some(chunk));
        }

        // 2. If not found in Fjall, try loading from old region `.dat` files
        let old_path = self.old_chunk_path(pos);
        if old_path.exists() {
            let data = std::fs::read(&old_path)?;
            let chunk = deserialize_chunk(&data)?;
            
            // Migrate to Fjall database
            self.fjall_db.save_chunk(&chunk)?;
            
            // Delete the old file to complete migration
            let _ = std::fs::remove_file(&old_path);
            
            tracing::info!("Migrated chunk {:?} from old region file to Fjall LSM-Tree", pos);
            return Ok(Some(chunk));
        }

        Ok(None)
    }

    /// Returns `true` if the chunk exists in Fjall database or as an old file.
    pub fn chunk_exists(&self, pos: ChunkPos) -> bool {
        self.fjall_db.chunk_exists(pos) || self.old_chunk_path(pos).exists()
    }
}
