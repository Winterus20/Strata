# 11 — Uygulama Planı ve Sözlük

## 1. Crate Organizasyonu

```
crates/
  core/
    ├── sector.rs          ← SectorCoord, Sector
    ├── xbrickmap/
    │   ├── mod.rs         ← XBrickMap, Brick, SubBrick
    │   ├── bitmask.rs     ← Bitmask operasyonları, popcnt
    │   ├── access.rs      ← get_block, set_block
    │   ├── ray_trace.rs   ← CPU ray tracing (debug)
    │   ├── soa.rs         ← SOA layout (Slab, BrickPool)
    │   └── simd.rs        ← SIMD popcnt (wide crate)
    └── registry.rs        ← Block registry, material ID

  meshing/
    ├── mod.rs             ← Mesher trait
    ├── greedy.rs          ← Greedy meshing (Tier 1)
    ├── svdag_builder.rs   ← Brick → SVDAG bake
    ├── transform_aware.rs ← Transform-aware deduplication
    └── shallow_svgdag.rs  ← Shallow SVDAG builder

  render/
    ├── mod.rs             ← Render pipeline
    ├── visibility_buffer.rs ← 64-bit visibility buffer
    ├── xbrickmap_pass.rs  ← Tier 1/2 ray trace pass
    ├── svdag_pass.rs      ← Tier 3 ray march pass
    ├── shallow_pass.rs    ← Shallow SVDAG streaming pass
    ├── color_resolve.rs   ← Shading pass
    ├── hiz_builder.rs     ← Hi-Z buffer
    ├── vertex_pool.rs     ← Global vertex pool
    └── foveated.rs        ← Foveated rendering

  physics/
    ├── mod.rs             ← Physics plugin entry point
    ├── collider.rs        ← Sector → Voxels collider conversion
    ├── broad_phase.rs     ← BVH + spatial hash complement
    ├── incremental.rs     ← Incremental collider update (3-tier strategy)
    ├── boundary.rs        ← Sector boundary sync (combine_voxel_states)
    ├── character/
    │   ├── mod.rs         ← Character controller
    │   ├── ground_check.rs← XBrickMap-optimized ground detection
    │   └── movement.rs    ← Movement + slope handling
    ├── custom/
    │   ├── mod.rs         ← Custom physics layer
    │   ├── falling_sand.rs← Falling particle simulation
    │   └── spatial_hash.rs← Sparse spatial hash grid
    ├── destruction/
    │   ├── mod.rs         ← Destruction system
    │   ├── damage.rs      ← Damage accumulation
    │   ├── voronoi.rs     ← Voronoi fracture
    │   └── fragment.rs    ← Fragment → rigid-body spawn
    ├── tier.rs            ← Physics tier management
    └── gpu/
        ├── mod.rs         ← GPU physics abstraction
        └── backend.rs     ← PhysicsBackend trait (gelecek)

  lighting/
    ├── mod.rs                  ← Lighting plugin entry point
    ├── light_data.rs           ← 16-bit packed light data (sky + RGB)
    ├── engine.rs               ← LightEngine (orchestrator)
    ├── direct/
    │   ├── mod.rs              ← Direct lighting (sun, point lights)
    │   ├── sun.rs              ← Directional sun light (day/night cycle)
    │   └── point.rs            ← Point/spot lights (analytic)
    ├── block/
    │   ├── mod.rs              ← Block light (emissive blocks)
    │   ├── bfs_cpu.rs          ← CPU BFS flood-fill (Starlight-style)
    │   ├── bfs_simd.rs         ← SIMD-accelerated BFS (wide crate)
    │   ├── removal.rs          ← Two-phase removal (voxel-light style)
    │   └── colored.rs          ← RGB channel propagation (packed)
    ├── sky/
    │   ├── mod.rs              ← Sky light system
    │   ├── column_first.rs     ← Column-first propagation (Starlight)
    │   ├── heightmap.rs        ← Slab bitmask'ten heightmap (O(1))
    │   └── day_night.rs        ← Day/night cycle (ambient shift)
    ├── indirect/
    │   ├── mod.rs              ← Indirect GI system
    │   ├── clustered.rs        ← Clustered Voxel GI (CGF 2022)
    │   ├── cone_trace.rs       ← Voxel cone tracing (SVDAG)
    │   ├── irradiance_cache.rs ← Per-face irradiance cache
    │   └── visibility.rs       ← 3D Bresenham visibility test
    ├── culling/
    │   ├── mod.rs              ← Light culling system
    │   ├── hierarchical.rs     ← Hierarchical bitmask implicit grids
    │   ├── morton.rs           ← Morton Z-order sorting
    │   └── priority.rs         ← Light update priority queue
    ├── mesh_bake.rs            ← Light data → vertex color (greedy mesh)
    ├── tier.rs                 ← Tier-bazlı lighting stratejisi
    └── gpu/
        ├── mod.rs              ← GPU lighting pipelines
        ├── svdag_light.rs      ← SVDAG cone tracing (Tier 3/4)
        ├── hi_z.rs             ← Hi-Z occlusion for lighting
        ├── temporal.rs         ← Temporal accumulation (TAA-style)
        └── neural_irradiance.rs← Neural Irradiance Volume (Faz 6)

  network/
    ├── mod.rs             ← Network plugin
    ├── delta.rs           ← Brick delta sync
    ├── snapshot.rs        ← SVDAG snapshot sync
    ├── interest.rs        ← Interest management / AOI
    ├── quantization.rs    ← Position/rotation quantization
    └── delta_encoding.rs  ← Delta encoding for network

  storage/
    ├── mod.rs             ← Storage plugin entry point
    ├── cache/
    │   ├── mod.rs         ← LRU compressed cache
    │   └── lru.rs         ← LRU implementation
    ├── region/
    │   ├── mod.rs         ← Region file I/O
    │   ├── format.rs      ← Binary format spec
    │   ├── read.rs        ← Unbuffered read
    │   └── write.rs       ← Append + dedup write
    ├── metadata/
    │   ├── mod.rs         ← SQLite metadata
    │   ├── schema.rs      ← SQL schema
    │   └── queries.rs     ← Prepared statements
    ├── dedup/
    │   ├── mod.rs         ← Content-addressable dedup
    │   └── hash.rs        ← xxHash64 wrapper
    ├── chunking/
    │   ├── mod.rs         ← Content-defined chunking
    │   ├── gear_hash.rs   ← Gear rolling hash
    │   └── merkle.rs      ← BLAKE3 Merkle tree
    ├── flush/
    │   ├── mod.rs         ← Write-back scheduler
    │   └── batch.rs       ← Batch flush logic
    ├── gc/
    │   ├── mod.rs         ← Garbage collector
    │   └── compaction.rs  ← Region compaction
    └── prefetch/
        ├── mod.rs         ← Predictive read-ahead
        └── predictor.rs   ← Movement-based prediction

  streaming/
    ├── mod.rs             ← Streaming manager
    ├── tier.rs            ← Tier belirleme
    ├── predictor.rs       ← Predictive streaming
    └── priority.rs        ← Yükleme öncelik sırası
```

---

## 2. Uygulama Sırası

### Faz 1 (Hafta 1-4): Temel Altyapı + Direct Light

1. **Hafta 1:** `core` crate — SectorCoord, XBrickMap temel yapılar
2. **Hafta 2:** `core` — get_block/set_block, bitmask operasyonları
3. **Hafta 3:** `meshing` — Greedy meshing, basit render
4. **Hafta 4:** `physics` — Rapier Voxels collider + character controller + boundary sync
5. **Hafta 4:** `lighting` — L0 direct light (sun, point lights), 16-bit packed light data

### Faz 2 (Hafta 5-8): Render + Streaming + Block/Sky Light

5. **Hafta 5:** `render` — wgpu pipeline, visibility buffer
6. **Hafta 6:** `render` — XBrickMap ray trace pass (WGSL)
7. **Hafta 7:** `streaming` — Tier sistemi, sector yükleme/boşaltma
8. **Hafta 8:** `storage` — Region file format, SQLite metadata, rkyv + zstd
9. **Hafta 7-8:** `lighting` — L1 block light (BFS CPU), L2 sky light (column-first + heightmap)

### Faz 3 (Hafta 9-12): SVDAG + Indirect GI

9. **Hafta 9:** `meshing` — SVDAG builder (CPU)
10. **Hafta 10:** `meshing` — GPU SVDAG bake (compute shader)
11. **Hafta 11:** `render` — SVDAG ray march pass (WGSL)
12. **Hafta 12:** `render` — Hi-Z occlusion, unified pipeline
13. **Hafta 10-11:** `lighting` — L3 clustered GI + L4 SVDAG cone tracing, temporal accumulation
14. **Hafta 12:** `lighting` — SIMD acceleration (wide crate), two-phase removal, mesh bake

### Faz 4 (Hafta 13-18): Network + Lighting Optimizasyon

13. **Hafta 13:** `network` — Brick delta sync
14. **Hafta 14:** `network` — SVDAG snapshot sync
15. **Hafta 15:** `streaming` — Predictive preload
16. **Hafta 16:** `lighting` — Hierarchical light culling, Morton Z-order, day/night cycle
17. **Hafta 17:** Optimizasyon — profil, benchmark, GPU memory, cache
18. **Hafta 18:** Optimizasyon — network, TPS, colored light mixing

### Faz 5 (Hafta 19-24): Wasm Modding + Plugin API

19-24. Wasm modding, plugin API refactor, native core-mods

### Faz 6 (Hafta 25-30): Storage + Neural GI + Final

25-27. Dedup optimization, GC/compaction
28-29. Neural Irradiance Volume (research integration)
30. Profiling, benchmarks, release

---

## 3. Sözlük

| Terim | Açıklama |
|---|---|
| **Sector** | 32×128×32 voksellik temel dünya birimi (131.072 voxel) |
| **Slab** | 32×32×32 voksellik dikey alt birim (4 slab = 1 sector) |
| **XBrickMap** | 4-level hiyerarşik brickmap (sector → slab → brick → sub-brick) |
| **Brick** | 8³ voksellik alt birim |
| **Sub-brick** | 2³ = 8 voksellik en küçük birim |
| **SVDAG** | Sparse Voxel Directed Acyclic Graph |
| **Shared Node Pool** | Tüm SVDAG'ların paylaştığı global node havuzu (32-bit atomic allocator) |
| **Tier** | Streaming kademesi (Active/Warm/Distant/Archive) |
| **Visibility Buffer** | 64-bit, tüm render pass'lerinin ortak yazdığı buffer |
| **Hi-Z** | Hierarchical Z-buffer, occlusion culling için |
| **Bake** | Brickmap → SVDAG dönüşümü |
| **Unbake** | SVDAG → Brickmap dönüşümü |
| **Left-packed** | Boş entry'lerin atlandığı, sıkıştırılmış dizi düzeni |
| **Popcnt** | Population count — set bit sayma işlemi |
| **Region File** | 32×32×1 sector grubu içeren binary dosya (.strata) |
| **Content-Addressable** | İçerik hash'i ile adresleme, deduplication için |
| **Write-Back** | Lazy flush stratejisi, dirty cache'ten arka plan yazma |
| **WAL** | Write-Ahead Logging, SQLite crash recovery mekanizması |
| **Unbuffered I/O** | OS cache bypass, doğrudan SSD'den okuma/yazma |
| **xxHash64** | Hızlı non-kriptografik hash fonksiyonu (dedup için) |
| **BVH** | Bounding Volume Hierarchy — Rapier 0.27+ broad-phase yapısı |
| **Ghost Collision** | Internal edge'lerde oluşan takılma sorunu (Rapier Voxels otomatik önler) |
| **Persistent Islands** | Frame'ler arası persist olan simulation connected components |
| **Voronoi Fracture** | Patlama hasarına göre voxel bölme (Teardown yaklaşımı) |
| **Spatial Hash** | 3D koordinatları 1D hash table'a map eden collision detection yapısı |
| **PhysicsBackend** | CPU/GPU physics soyutlama trait'i (gelecek) |
| **SOA** | Structure of Arrays — AOS'a alternatif, SIMD-friendly bellek layout |
| **Transform-Aware SVDAG** | Simetri ve dönüşümleri kullanan gelişmiş deduplication (%20-45 tasarruf) |
| **Shallow SVDAG** | Aokana yaklaşımı — sığ SVDAG'lar + view-dependent streaming (%5 VRAM) |
| **Vertex Pool** | Tek büyük vertex buffer — mesh rebuild'de VBO recreate yok |
| **Foveated Rendering** | İnsan gözü peripheral vision sınırlarını kullanan adaptive rendering |
| **Quantization** | Veri boyutunu azaltmak için hassasiyet düşürme |
| **Delta Encoding** | Mutlak değer yerine değişim gönderme — network bant genişliği optimizasyonu |
| **AOI** | Area of Interest — her oyuncu sadece yakınındaki sector'ları alır |
| **Content-Defined Chunking** | GearHash ile içerik bazlı chunk boundary belirleme |
| **Merkle Tree** | BLAKE3 hash'leri ile chunk integrity verification |
| **GearHash** | Rolling hash fonksiyonu — content-defined chunking için |
| **BFS Flood-Fill** | Breadth-First Search ile ışık yayılımı |
| **Two-Phase Removal** | Işık kaynağı kaldırma: Phase 1 bağımlıları sıfırla, Phase 2 yeniden propagate et |
| **Column-First Sky** | Sky light'ı dikey sütunlardan başlatıp yatay BFS ile yayma (Starlight) |
| **Word-Level Parallelism** | Bitwise operasyonlarla 4-bit kanalları tek u32'de paralel işleme |
| **Clustered Voxel GI** | Normal-benzeri voxel'leri cluster'layarak visibility test sayısını azaltma |
| **Voxel Cone Tracing** | SVDAG üzerinden hiyerarşik LOD ile cone sampling — indirect GI |
| **Hierarchical Light Culling** | Morton Z-order + hierarchical bitmask ile boş alanları O(1) atlama |
| **Temporal Accumulation** | Önceki frame'lerle blending — noise-free GI, voxel-specific TAA |
| **LightData (16-bit)** | Packed light formatı: Sky 4-bit + Block RGB 4×4-bit |
| **Smooth Lighting** | Vertex başına 4 komşu light ortalaması — harsh geçiş önleme |
| **Neural Irradiance Volume** | MLP ile sıkıştırılmış irradiance field — 1-5MB, ~1ms inference |
| **Irradiance Cache** | Per-voxel-face cached indirect lighting — Gaussian filtering ile yumuşatma |
| **Heightmap** | Slab bitmask'inden türetilen en yüksek dolu voxel haritası — sky source setup |
