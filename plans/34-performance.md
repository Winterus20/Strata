# 10 — Performans Hedefleri, Riskler ve Alternatifler

## 1. Performans Hedefleri

### 1.1 Render

| Metrik | Hedef | Not |
|---|---|---|
| Görünür sector | 100+ @ 60 FPS | XBrickMap + SVDAG ile |
| XBrickMap ray trace | <500µs/sector | 4-level skip + SIMD ile |
| SVDAG ray march | <50µs/sector | Hi-Z occlusion + shallow SVDAG ile |
| VRAM kullanımı | <2GB | SVDAG deduplication + streaming + transform-aware |
| GPU node pool | ~10MB | 256K node × 40B |
| Mesh rebuild | <3ms/sector | Vertex pool (VBO recreate yok) |
| Foveated rendering | %60-80 ray/pixel azalması | Adaptive step size |
| Frame time (foveated) | 6-10ms | Uniform 16.7ms'e kıyasla |

### 1.2 Fizik

| Metrik | Hedef | Not |
|---|---|---|
| Collider güncelleme (tek voxel) | <0.1ms | `set_voxel` + `propagate_voxel_change` |
| Collider güncelleme (bölgesel) | <1ms | `split_with_box` + rebuild |
| Collider güncelleme (tam rebuild) | <5ms | 32×32×32 sector için |
| Boundary sync (2 sector) | <0.5ms | `combine_voxel_states` |
| Character ground check | <0.05ms | XBrickMap 4-level skip |
| Broad-phase (ACTIVE) | <2ms | BVH traversal, 100+ sector |
| Falling sand (1K particle) | <3ms | Custom spatial hash |
| Fracture (patlama) | <10ms | Voronoi + flood-fill + rigid-body spawn |

### 1.3 Network

| Metrik | Hedef | Not |
|---|---|---|
| Delta sync (ham) | <2KB/s/oyuncu | Brick delta |
| Delta sync (quantized) | **<200B/s/oyuncu** | Position quantization + delta encoding |
| Snapshot | 1-5KB/sector | SVDAG compressed |
| TPS | 20+ | Server-authoritative |
| AOI bant genişliği | **10-20KB/s/oyuncu** | Sadece yakın sector'lar |
| Maks oyuncu | **600+** | AOI + quantization ile |

### 1.4 Streaming

| Metrik | Hedef | Not |
|---|---|---|
| Bake süresi | <15ms | GPU compute (pipeline stall mitigate) |
| Unbake süresi | <5ms | SVDAG → Brickmap |
| Pop-in | Yok | Tier 2 yumuşak geçiş |
| Predictive preload | %80 azalma | Hareket vektörü tahmini |
| Shallow SVDAG VRAM | **%5** | Sadece görünür tile'lar |
| Shallow SVDAG hız | **2-4×** | Derin SVDAG'e kıyasla |

### 1.5 Storage

| Metrik | Hedef | Not |
|---|---|---|
| Hot load (cache hit) | <0.1ms | RAM'den doğrudan |
| Warm load (cache miss) | <2ms | Decompress + deserialize |
| Cold load (disk) | <5ms | Unbuffered I/O + decompress |
| Batch save (64 sector) | <50ms | Paralel compress + SQLite WAL |
| Write throughput | >500MB/s | Multi-thread unbuffered |
| Dedup tasarrufu (sabit) | %30-60 | Tekrarlayan geometri |
| Dedup tasarrufu (content-defined) | **%50-80** | GearHash chunking |
| Crash recovery | <100ms | SQLite WAL replay |
| GC cycle | <200ms | Periyodik, background |
| Integrity verification | **BLAKE3 Merkle** | Chunk-level doğrulama |

---

## 2. Riskler ve Mitigasyon

| Risk | Olasılık | Etki | Mitigasyon |
|---|---|---|---|
| GPU SVDAG bake süresi >15ms | Orta | Yüksek | Kademeli bake (her frame küçük bölüm) |
| Visibility buffer 64-bit yetersiz | Düşük | Orta | 128-bit'e genişlet (2× u64) |
| WGSL 64-bit atomik desteği eksik (eski GPU'lar) | Yüksek | Düşük | `uvec2` + `atomicMin` fallback |
| Node pool allocator eski GPU'larda çalışmıyor | Orta | Orta | 32-bit `atomic<uint>` allocator kullan |
| Rapier Voxels deneysel sınırlamalar | Orta | Orta | Custom physics layer fallback olarak hazır |
| SVDAG node pool fragmentasyonu | Düşük | Yüksek | Periyodik compact + defrag |
| Tier 2'de çift bellek kullanımı | Yüksek | Orta | Sadece gerekli sector'larda Tier 2 |
| Network snapshot boyutu büyük | Orta | Orta | Delta compression + LOD bazlı gönderim |
| mmap async thread blokluyor (page fault) | Yüksek | Orta | Unbuffered I/O + spawn_blocking kullan |
| SQLite WAL dosyası büyüyor | Düşük | Düşük | Periyodik wal_checkpoint(TRUNCATE) |
| Dedup hash collision | Çok düşük | Yüksek | xxHash64 yeterli (collision prob ~10⁻¹⁹) |
| Region file fragmentasyonu | Orta | Orta | Periyodik compaction (GC cycle) |
| SOA layout migration cost | Orta | Düşük | AOS→SOA geçiş incremental, runtime'da seçilebilir |
| Transform-aware SVDAG hash overhead | Düşük | Düşük | 48 transform lookup O(1), lookup table ile |
| Shallow SVDAG streaming stutter | Orta | Orta | Async preload + budget management |
| Vertex pool fragmentation | Orta | Orta | Free list merge + periyodik defrag |
| Foveated rendering artefact | Düşük | Orta | Smooth transition between zones |
| GearHash chunk boundary instability | Düşük | Düşük | Min/max chunk size ile stabilize |
| BFS queue overflow (çok ışık) | Orta | Orta | Max queue size + priority-based pruning |
| SIMD desteği eksik (eski CPU) | Düşük | Düşük | Scalar fallback (15x yavaş ama çalışır) |
| Colored light removal over-zero | Düşük | Düşük | Per-channel boundary tracking |
| Clustered GI cluster explosion | Orta | Orta | Max cluster count + LOD-based merge |
| SVDAG cone tracing noise | Orta | Orta | Temporal accumulation + TAA-style blending |
| Day/night cycle stutter | Düşük | Orta | Gradual ambient shift (per-frame delta) |
| Neural Irradiance training time | Yüksek | Düşük | Offline training, runtime sadece inference |

---

## 3. Alternatifler ve Neden Reddedildi

| Alternatif | Neden Reddedildi |
|---|---|
| **Saf SVO** | Edit cost çok yüksek, cache performansı kötü, network sync karmaşık |
| **Clipmap** | Multiplayer'da her oyuncu için ayrı clipmap = kaos, mağara için uygun değil |
| **Tree64** | Hâlâ chunk hierarchy kullanıyor, edit zor |
| **Flat Vec<u16>** | Bellek verimsiz, LOD/ ray tracing doğal değil |
| **Global SVDAG** | Derin traversal, çoklu indirect jump, GPU cache miss |
| **WGSL native u64 her yerde** | Eski GPU'larda `atomicAdd` u64 desteği yok → `uvec2` fallback gerekli |
| **64-bit atomic node allocator** | Eski GPU'larda `atomicAdd(uint64_t)` yok → 32-bit `atomic<uint>` allocator yeterli |
| **File-per-Sector** | 10K+ dosya, NTFS verimsiz, I/O pattern kötü |
| **Fjall KV Store** | Genel amaçlı, voxel için optimize değil, SQLite'dan yavaş batch write |
| **mmap async I/O** | Page fault = blocking (async hazard), Windows'ta unpredictable |
| **AOS layout (SOA yerine)** | Pointer chasing, cache miss, SIMD kullanılamaz |
| **Birebir SVDAG dedup** | Transform-aware ile %20-45 ek tasarruf mümkün |
| **Derin SVDAG (tek ağaç)** | Shallow SVDAG + streaming ile 2-4× hız, %95 VRAM azalması |
| **Ayrı VBO per sector** | Vertex pool ile %40 frame time, %25 meshing time azalması |
| **Uniform rendering** | Foveated ile %60-80 ray/pixel azalması |
| **Ham network data** | Quantization + delta encoding ile %85-90 bant genişliği azalması |
| **Tüm sector broadcast** | AOI ile %80-90 bant genişliği azalması, 6× oyuncu kapasitesi |
| **Sabit sector chunking** | Content-defined chunking ile %20-30 ek deduplication |
