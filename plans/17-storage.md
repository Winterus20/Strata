# 09 — Depolama Sistemi

## 1. Depolama — Hybrid Tiered Storage

### 1.1 Genel Bakış

Strata, **3-kademeli hybrid depolama mimarisi** kullanır. Streaming tier'ları ile depolama tier'ları birebir eşleşir.

```
┌──────────────────────────────────────────────────────────────────────┐
│                    HYBRID TIERED STORAGE                             │
├──────────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 1: In-Memory (ACTIVE)                                │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │  XBrickMap (doğrudan erişim, O(1))                   │    │    │
│  │  │  ├── Dirty tracking (atomic<bool>)                   │    │    │
│  │  │  └── Object pool (GC churn yok)                      │    │    │
│  │  └──────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                  │                                     │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 2: LRU Compressed Cache (WARM)                       │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │  ~500 sector kapasiteli                              │    │    │
│  │  │  zstd level 1 (hız öncelikli)                        │    │    │
│  │  │  Write-back (lazy flush)                             │    │    │
│  │  │  └── Async background flush (tokio)                  │    │    │
│  │  └──────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                  │                                     │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 3: Persistent Storage (DISTANT + ARCHIVE)            │    │
│  │  ┌──────────────────────────┐ ┌──────────────────────────┐   │    │
│  │  │  Region Files (.strata)  │ │  Metadata DB (SQLite)    │   │    │
│  │  │  32×32×1 sector grupları │ │  ┌────────────────────┐  │   │    │
│  │  │  zstd level 3 / 19       │ │  │ sector_metadata   │  │   │    │
│  │  │  Content-addressable     │ │  │ dirty_log (WAL)   │  │   │    │
│  │  │  deduplication           │ │  │ gc_candidates     │  │   │    │
│  │  │  └── mmap (sadece read)  │ │  │ world_config      │  │   │    │
│  │  └──────────────────────────┘ │  └────────────────────┘  │   │    │
│  └──────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

**Neden SQLite (Fjall yerine)?**

| Metrik | Fjall KV | SQLite (WAL mode) |
|---|---|---|
| **Batch insert (10K kayıt)** | ~50ms | **~23ms** |
| **Lookup (10K kayıt)** | ~10µs | **~11µs** |
| **Transaction safety** | İyi | **ACID (WAL)** |
| **Crash recovery** | Compaction gerekir | **Otomatik (WAL replay)** |
| **Query esnekliği** | Sadece KV | **SQL (range, join, aggregate)** |
| **Rust desteği** | fjall crate | **rusqlite / libsqlite3-sys** |
| **Olgunluk** | Yeni (3.0) | **30+ yıl, her yerde** |

**Karar:** SQLite metadata + indexing için, Region Files blob storage için.

---

### 1.2 Region File Formatı

```
r.0.0.strata (32×32×1 = 1024 sector)
┌────────────────────────────────────────────────────────┐
│ Header (8KB, 64-bit aligned)                           │
│ ├── Magic: "STRT" (4B)                                │
│ ├── Version: u16                                      │
│ ├── Flags: u16 (compression, dedup, encryption)       │
│ ├── Region coord: I16Vec2 (4B)                        │
│ ├── Sector offsets: [u32; 1024] (4KB)                 │
│ ├── Sector sizes: [u32; 1024] (4KB)                   │
│ └── Sector hashes: [u64; 1024] (8KB) ← integrity      │
├────────────────────────────────────────────────────────┤
│ Dedup Table (değişken)                                 │
│ ├── Content-addressable hash → offset mapping         │
│ └── Aynı geometriye sahip sector'ler tek payload      │
├────────────────────────────────────────────────────────┤
│ Sector Payloads (değişken boyut)                       │
│ ├── Sector 0: [header + compressed payload]           │
│ ├── Sector 1: [header + compressed payload]           │
│ └── ... (aynı hash = shared payload pointer)          │
│     └── Payload format:                                │
│         ├── SectorHeader (32B)                        │
│         │   ├── coord: I16Vec3                        │
│         │   ├── timestamp: u64                        │
│         │   ├── flags: u16                            │
│         │   ├── content_hash: u64 (xxHash64)          │
│         │   └── checksum: u64                         │
│         ├── XBrickMap slab data (compressed)          │
│         └── SVDAG subtree (opsiyonel, compressed)     │
└────────────────────────────────────────────────────────┘
```

---

### 1.3 Content-Addressable Deduplication

```rust
pub struct DedupTable {
    index: HashMap<u64, u64>,
    ref_counts: HashMap<u64, u32>,
}

impl DedupTable {
    pub fn store_sector(
        &mut self,
        region: &mut RegionFile,
        coord: SectorCoord,
        payload: &[u8],
    ) -> Result<u64> {
        let hash = xxhash64(payload);

        if let Some(&offset) = self.index.get(&hash) {
            *self.ref_counts.get_mut(&hash).unwrap() += 1;
            return Ok(offset);
        }

        let offset = region.append_payload(payload)?;
        self.index.insert(hash, offset);
        self.ref_counts.insert(hash, 1);
        Ok(offset)
    }
}
```

**Beklenen tasarruf:** Tekrarlayan geometri için **%30-60** disk tasarrufu.

---

### 1.4 Async I/O Stratejisi (Windows-optimize)

**mmap kullanmıyoruz** — page fault async thread'i bloklar. Windows'ta **unbuffered I/O + multi-thread** en iyi sonucu verir.

```rust
pub struct AsyncStorageBackend {
    write_pool: tokio::runtime::Handle,
    read_pool: tokio::runtime::Handle,
    flush_scheduler: FlushScheduler,
    prefetch: PrefetchManager,
}

impl AsyncStorageBackend {
    pub async fn load_sector(&self, coord: SectorCoord) -> Result<Sector> {
        if let Some(cached) = self.cache.get(&coord) {
            return Ok(cached);
        }

        self.prefetch.enqueue(coord);

        let data = tokio::task::spawn_blocking(move || {
            region.read_sector_aligned(coord)
        }).await??;

        let sector = self.decompress_and_deserialize(&data)?;
        self.cache.insert(coord, sector.clone());

        Ok(sector)
    }

    pub fn mark_dirty(&self, coord: SectorCoord, sector: Arc<Sector>) {
        self.cache.insert(coord, sector);
        self.flush_scheduler.schedule(coord);
    }
}
```

---

### 1.5 SQLite Metadata Schema

```sql
CREATE TABLE sector_metadata (
    region_x    INTEGER NOT NULL,
    region_z    INTEGER NOT NULL,
    local_x     INTEGER NOT NULL,
    local_z     INTEGER NOT NULL,
    local_y     INTEGER NOT NULL,

    file_offset INTEGER NOT NULL,
    payload_size INTEGER NOT NULL,
    content_hash INTEGER NOT NULL,
    timestamp   INTEGER NOT NULL,
    tier        INTEGER NOT NULL,
    dirty       INTEGER NOT NULL DEFAULT 0,

    PRIMARY KEY (region_x, region_z, local_x, local_z, local_y)
);

CREATE INDEX idx_tier ON sector_metadata(tier);
CREATE INDEX idx_dirty ON sector_metadata(dirty) WHERE dirty = 1;
CREATE INDEX idx_timestamp ON sector_metadata(timestamp);

CREATE TABLE gc_candidates (
    content_hash INTEGER PRIMARY KEY,
    ref_count    INTEGER NOT NULL,
    marked_at    INTEGER NOT NULL
);

CREATE TABLE world_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

---

### 1.6 Write-Back Pipeline

```rust
pub struct FlushScheduler {
    dirty_queue: VecDeque<(SectorCoord, Arc<Sector>)>,
    in_flight: HashMap<SectorCoord, JoinHandle<()>>,
    max_batch_size: usize,
    max_wait_time: Duration,
    flush_interval: Duration,
}

impl FlushScheduler {
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.flush_interval);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.flush_if_needed().await;
                }
                _ = self.max_wait_expired() => {
                    self.flush_all().await;
                }
            }
        }
    }

    async fn flush_batch(&mut self, batch: Vec<(SectorCoord, Arc<Sector>)>) {
        let by_region = self.group_by_region(&batch);

        let tasks: Vec<_> = by_region.into_iter().map(|(region, sectors)| {
            tokio::task::spawn_blocking(move || {
                // Compress, dedup check, write, SQLite metadata update
            })
        }).collect();

        for task in tasks {
            task.await.unwrap();
        }

        self.clear_dirty_flags(&batch);
    }
}
```

---

### 1.7 Tier-Bazlı Compression Stratejisi

| Tier | Compression | Hedef | Beklenen Oran |
|---|---|---|---|
| **WARM (cache)** | zstd level 1 | Hız > boyut | 3:1 |
| **DISTANT** | zstd level 3 | Denge | 8:1 |
| **ARCHIVE** | zstd level 19 | Boyut > hız | 15:1 |
| **Dedup payload** | zstd level 3 + dedup | Tekrar eden geometri | 20:1+ |

---

### 1.8 Garbage Collection & Compaction

```rust
pub struct GarbageCollector {
    db: rusqlite::Connection,
    dedup_table: DedupTable,
}

impl GarbageCollector {
    pub async fn run_gc(&mut self) {
        let candidates = self.db.prepare(
            "SELECT content_hash FROM gc_candidates WHERE ref_count = 0"
        ).unwrap();

        for hash in candidates {
            self.dedup_table.remove_payload(hash);
        }

        self.compact_regions().await;
        self.db.execute("PRAGMA wal_checkpoint(TRUNCATE)", []).unwrap();
    }

    async fn compact_regions(&mut self) {
        // Her region file için:
        // 1. Canlı payload'ları yeni dosyaya kopyala
        // 2. Eski dosyayı sil, yenisini rename et
        // 3. SQLite offset'leri güncelle (transaction)
    }
}
```

---

### 1.9 Content-Defined Chunking (GearHash)

Sabit sector sınırları = deduplication verimsiz. **Content-defined chunking** (HuggingFace Xet yaklaşımı) ile sınır içerik hash'ine göre belirlenir.

#### Gear Hash ile Sınır Belirleme

```rust
pub struct ContentDefinedChunker {
    gear_state: u64,
    min_chunk_size: u32,
    max_chunk_size: u32,
    target_chunk_size: u32,
    boundary_mask: u64,
}

impl ContentDefinedChunker {
    pub fn should_split(&mut self, byte: u8) -> bool {
        self.gear_state = (self.gear_state << 1) ^ GEAR_TABLE[byte as usize];
        (self.gear_state & self.boundary_mask) == 0
    }

    pub fn chunk_sector(&mut self, sector_data: &[u8]) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut chunk_start = 0;
        let mut chunk_size = 0;

        for (i, &byte) in sector_data.iter().enumerate() {
            chunk_size += 1;

            if chunk_size >= self.min_chunk_size {
                if self.should_split(byte) || chunk_size >= self.max_chunk_size {
                    let chunk_data = &sector_data[chunk_start..chunk_start + chunk_size];
                    let hash = blake3::hash(chunk_data);

                    chunks.push(Chunk {
                        hash: hash.into(),
                        offset: chunk_start as u32,
                        size: chunk_size as u32,
                    });

                    chunk_start = chunk_start + chunk_size;
                    chunk_size = 0;
                }
            }
        }

        if chunk_size > 0 {
            let chunk_data = &sector_data[chunk_start..];
            let hash = blake3::hash(chunk_data);
            chunks.push(Chunk {
                hash: hash.into(),
                offset: chunk_start as u32,
                size: chunk_size as u32,
            });
        }

        chunks
    }
}
```

#### MerkleHash ile Integrity Verification

```rust
pub struct MerkleTree {
    leaves: Vec<[u8; 32]>,
    nodes: Vec<[u8; 32]>,
    root: [u8; 32],
}

impl MerkleTree {
    pub fn from_chunks(chunks: &[Chunk]) -> Self {
        let mut leaves: Vec<[u8; 32]> = chunks.iter().map(|c| c.hash).collect();
        let mut level = leaves.clone();
        let mut nodes = Vec::new();

        while level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in level.chunks(2) {
                let parent = if chunk.len() == 2 {
                    blake3::hash(&[chunk[0], chunk[1]].concat()).into()
                } else {
                    chunk[0]
                };
                next_level.push(parent);
                nodes.push(parent);
            }
            level = next_level;
        }

        Self { leaves, nodes, root: level[0] }
    }

    pub fn verify_chunk(&self, chunk_index: usize, chunk_data: &[u8]) -> bool {
        let expected_hash = self.leaves[chunk_index];
        let actual_hash = blake3::hash(chunk_data).into();
        expected_hash == actual_hash
    }
}
```

#### Deduplication ile Entegrasyon

```rust
pub struct ChunkedDedupStorage {
    chunk_store: HashMap<[u8; 32], ChunkData>,
    sector_chunks: HashMap<SectorCoord, Vec<[u8; 32]>>,
    chunk_ref_counts: HashMap<[u8; 32], u32>,
}

impl ChunkedDedupStorage {
    pub fn store_sector(&mut self, coord: SectorCoord, data: &[u8]) {
        let mut chunker = ContentDefinedChunker::new();
        let chunks = chunker.chunk_sector(data);

        let mut chunk_hashes = Vec::new();
        for chunk in &chunks {
            let hash = chunk.hash;

            if !self.chunk_store.contains_key(&hash) {
                self.chunk_store.insert(hash, ChunkData {
                    data: data[chunk.offset as usize..(chunk.offset + chunk.size) as usize].to_vec(),
                    size: chunk.size,
                });
                self.chunk_ref_counts.insert(hash, 1);
            } else {
                *self.chunk_ref_counts.get_mut(&hash).unwrap() += 1;
            }

            chunk_hashes.push(hash);
        }

        self.sector_chunks.insert(coord, chunk_hashes);
    }

    pub fn load_sector(&self, coord: &SectorCoord) -> Option<Vec<u8>> {
        let chunk_hashes = self.sector_chunks.get(coord)?;

        let mut data = Vec::new();
        for hash in chunk_hashes {
            let chunk = self.chunk_store.get(hash)?;
            data.extend_from_slice(&chunk.data);
        }

        Some(data)
    }
}
```

#### Performans

| Metrik | Sabit Sector | Content-Defined | Fark |
|---|---|---|---|
| **Dedup oranı** | %30-60 | **%50-80** | **+20-30%** |
| **Storage efficiency** | Sector boundary waste | **Zero waste** | **+15-25%** |
| **Integrity check** | xxHash64 (sector) | **BLAKE3 Merkle** | **Güvenli** |
| **Chunk overhead** | Yok | ~4 byte/chunk | Minimal |

---

### 1.10 Performans Hedefleri (Depolama)

| Metrik | Hedef | Not |
|---|---|---|
| **Hot load (cache hit)** | <0.1ms | RAM'den doğrudan |
| **Warm load (cache miss)** | <2ms | Decompress + deserialize |
| **Cold load (disk)** | <5ms | Unbuffered I/O + decompress |
| **Batch save (64 sector)** | <50ms | Paralel compress + SQLite WAL |
| **Write throughput** | >500MB/s | Multi-thread unbuffered |
| **Dedup tasarrufu (sabit)** | %30-60 | Tekrarlayan geometri |
| **Dedup tasarrufu (content-defined)** | **%50-80** | GearHash chunking |
| **Crash recovery** | <100ms | SQLite WAL replay |
| **GC cycle** | <200ms | Periyodik, background |
| **Integrity verification** | **BLAKE3 Merkle** | Chunk-level doğrulama |
