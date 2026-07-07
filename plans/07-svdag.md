# 07 — SVDAG Veri Yapısı (Optimize, SOTA 2024-2026)

> **Olgunluk:** 🔒 Kesinleşti (`01-overview.md` §1.1, 2026-06-04). Uzak-alan temsil + bake/unbake + streaming + SVDAG ray pipeline (Tier 2–3). `16`+ taslaklarla çelişirse **bu dosya** esas alınır; `01`–`06`, `08`–`10` ile çelişirse önce anayasa güncellenir veya `07` revize edilir.
> **Bağımlılıklar:** `06-xbrickmap.md` (32³ sektör, `CompressedChunkData`, `GlobalBrickPool`), `05-block-registry.md` (`SectorPalette`), `03-ecs-architecture.md` (component/sistem setleri), `08-streaming.md` (4-tier mesafeler).
> **Harici doğrulama (2026-06):** [Aokana I3D 2025](https://arxiv.org/abs/2505.02017) (shallow SVDAG, visibility buffer, LOD `density=2`), [GigaVoxels DP HPG 2024](https://hal.science/hal-04654692) (starvation-free / sparse page), [Molenaar PG 2024 GPU edit](https://diglib.eg.org/handle/10.2312/pg20241310), [Transform-Aware SVDAG 2025](https://doi.org/10.1145/3728301).

## 1. SVDAG — Uzak Alan Veri Yapısı

### 1.1 Shared Node Pool + Generational GC

Tüm sektörlerin SVDAG'ları **tek bir global node havuzunu** paylaşır. Aynı geometrinin birden fazla sector'de **tek node** olarak saklanmasını sağlar.

GC için **epoch-based generational yaklaşım** (Molenaar & Eisemann, Pacific Graphics 2024). Cascading free yerine deferred free list + GPU hash table kullanılır.

```rust
pub struct GenerationalNodePool {
    nodes: Vec<SvdagNode>,
    generations: Vec<u32>,           // Her node için version
    free_list: GpuAtomicStack,       // GPU'dan yönetilen deferred free list
    current_epoch: AtomicU32,        // Global epoch
    epoch_size: u32,                 // Kaç sektör silinince epoch artar
    capacity: u32,
    gpu_free_head: wgpu::Buffer,       // GPU atomic stack head
}

/// GPU hash table: shared node'ların referans takibi
pub struct GpuHashTable {
    buckets: Vec<GpuHashBucket>,
    locks: Vec<AtomicU32>,           // atomic spinlock
}

pub struct GpuHashBucket {
    key: u64,                        // geometry hash
    node_index: u32,                 // node pool index
    epoch: u32,                      // son kullanım epoch'u
}
```

**Çalışma prensibi:**
- Her sector root'u, kullandığı node'ların generation'ını kaydeder
- Sector silinince: epoch++ (tüm sector kökleri geçersiz)
- GPU compute shader: generation'ı güncel olmayan node'ları deferred free list'e ekler
- Yeni alloc: free list'ten al veya yeni node oluştur
- Cascading free: sadece root node free list'e eklenir, children sonraki epoch'ta otomatik temizlenir

**GPU Allocator (32-bit atomic + deferred stack, WGSL):**

```wgsl
struct NodePool {
    free_head: atomic<u32>,          // stack head (deferred free, atomic)
    capacity: u32,
    nodes: array<SvdagNode, 262144>,
    generations: array<atomic<u32>, 262144>,  // epoch-based versioning
};

@group(0) @binding(0)
var<storage, read_write> pool: NodePool;

fn node_alloc() -> u32 {
    // Önce deferred free list'ten al
    let idx = atomicAdd(&pool.free_head, 0xFFFFFFFFu) - 1u;  // atomicSub emulation
    if (idx < 0x80000000u) {        // valid index
        return idx;
    }
    // Yoksa yeni node oluştur
    let new_idx = atomicAdd(&pool.free_head, 1u);  // head'i geri al
    // ... (linear alloc)
    return new_idx;
}
```

### 1.2 Node Bellek Hesabı — Değişken Uzunluklu Encoding

**Modisett & Billeter (CGF 2025)** yaklaşımı: child_mask ayrı bir byte yerine **pointer'ın üst bitlerine** gömülür. Bu sayede hem node boyutu küçülür hem de memory access azalır.

**Değişken Uzunluklu Node Formatı** (SSVDAG + Transform-Aware + Occupancy Encoding birleşimi):

| Format | Açıklama | Boyut | Sıklık |
|--------|----------|-------|--------|
| **Compact2** | 2 child index, homojen bölge | **16B** | ~%40 |
| **Compact4** | 4 child index, az çeşitlilik | **24B** | ~%30 |
| **Occupancy8** | Occupancy encoded pointer (8 child) | **32B** | ~%20 |
| **Transform8** | Occupancy + transform bilgisi | **36B** | ~%8 |
| **Full8** | Full child_indices + transform (fallback) | **48B** | ~%2 |
| **Weighted avg** | — | **~23B** | **%100** |

```rust
#[repr(u8)]
pub enum NodeFormat {
    Compact2 = 0,    // 16B: mask + 2 child
    Compact4 = 1,    // 24B: mask + 4 child
    Occupancy8 = 2,  // 32B: occupancy encoded pointer
    Transform8 = 3,  // 36B: occupancy + transform
    Full8 = 4,       // 48B: full fallback
}
```

**Occupancy-Encoded Node (32 bayt):**

```rust
#[repr(C)]
pub struct OccupancyNode {
    /// 8 child pointer: üst 4 bit = occupancy/transform, alt 28 bit = node index
    pub child_pointers: [u32; 8],
    /// LOD-0 yaprak: `06` snapshot’taki `u8` palet indeksi. Uzak LOD yaprak: baskın `block_type` (u16).
    pub material: u16,
    pub padding: [u8; 6],  // 32-byte alignment
}
```

Her pointer'ın bit dağılımı: `bit[31:29]` = occupancy mask (3 bit = 8 alt-dal), `bit[28]` = has_transform, `bit[27:0]` = node index (28 bit = 268M node).

**Materyal kodlama (05/06 ile uyum):**

| LOD | Yaprak `material` alanı | Unbake |
|-----|-------------------------|--------|
| **0** (32³ sektör SVDAG) | `palette_index: u8` → `u16` içinde düşük byte; üst byte 0 | `SectorPalette::get_or_insert(PaletteEntry)` |
| **≥1** (8 sektör aggregate) | Baskın `block_type: u16` (Aokana `density≥2` ortalama renk) | Yakınlaşınca LOD-0 bake zorunlu |

32×32×32 sector için tipik SVDAG:
- Boş/homojen alan: **~3-5KB** (Compact2 ağırlıklı)
- Karmaşık arazi: **~12-20KB** (Occupancy8/Transform8 ağırlıklı)
- Deduplication + transform-aware: **%40-60 ek tasarruf**

**GPU Node Pool Kapasitesi:** 256K node × 23B (ort.) = **~5.9MB** (Plan'daki ~10MB'dan %41 az)

### 1.3 Brick → SVDAG Bake (`CompressedChunkData` + GPU)

Bake **canlı** `Sector` üzerinden değil; `06-xbrickmap.md` §1.4’teki **immutable snapshot** üzerinden yapılır. Böylece meshing, network ve SVDAG bake aynı kilitsiz `Arc<CompressedChunkData>` kopyasını paylaşır (`03` `SectorData`).

```rust
pub struct SvdagBaker {
    bake_queue: crossbeam::channel::Receiver<SvdagBakeRequest>,
    hash_table: GpuHashTable,         // HashDAG (Careil et al. 2020); PG 2024 GPU hash tablosu
    node_pool: GpuNodePool,
    occupancy_encoder: GpuOccupancyEncoder,
}

/// Main thread → bake worker (GPU async, Tier 1→2 geçişinde tetiklenir; `08` §3).
pub struct SvdagBakeRequest {
    pub sector_entity: Entity,
    pub coord: IVec3,
    pub snapshot: Arc<CompressedChunkData>,  // palette + bricks frozen
    pub target_lod: u8,                       // 0 = tam 32³; 1+ = aggregate (§1.6)
}

/// SVDAG yapraklarında LOD-0 için snapshot-local palet indeksi (u8).
#[repr(C)]
pub struct SvdagVoxelDense {
    /// 32×32×32 = 32768 byte — snapshot palet indeksleri (hava = 0).
    pub indices: [u8; 32 * 32 * 32],
}
```

**Snapshot → dense grid (CPU, main thread veya bake worker başlangıcı):**

```rust
impl SvdagVoxelDense {
    /// `CompressedChunkData` + `GlobalBrickPool` dilimlerinden 32³ `u8` grid doldur.
    /// `palette_index` her zaman snapshot `palette` tablosuna referans verir (05 §14).
    pub fn from_snapshot(
        snapshot: &CompressedChunkData,
        pool: &GlobalBrickPool,
    ) -> Self { /* unpack left-packed bricks → linear u8[] */ }

    #[inline]
    pub fn resolve_block_type(&self, snapshot: &CompressedChunkData, idx: u8) -> u16 {
        snapshot.palette[idx as usize].block_type
    }
}
```

**Bake pipeline (doğrulanmış sıra — Molenaar PG 2024 GPU edit + HashDAG merge):**

```
0. Tier geçişi: `TierChange` + `NeedsSvdagBake` (§1.9) → kuyruğa `SvdagBakeRequest`
1. `snapshot_from_pool` zaten alınmış `Arc<CompressedChunkData>` (veya bu adımda clone)
2. CPU: `SvdagVoxelDense::from_snapshot` (~0.5–1ms / dolu sektör)
3. GPU upload: dense u8 grid + palette table (GPU SSBO, max 256 × PaletteEntry)
4. GPU compute: bottom-up SVO (~5ms)
5. GPU compute: HashDAG merge + dedup (~5ms)
6. GPU compute: occupancy pointer encoding (~2ms)
7. GPU compute: transform-aware dedup (Molenaar 2025, opsiyonel Tier 2+)
8. CPU: `SectorSvdag.root_index = Some(...)`; `remove::<NeedsSvdagBake>`; epoch++
```

**Kurallar (06/05):**

- Geometry dedup hash’i **şekil** (dolu/boş) üzerinden; renk/variant için yaprakta `u8` palet indeksi korunur.
- Snapshot `data_version` değişirse devam eden bake **iptal**; yeni snapshot ile yeniden kuyruklanır.
- **ACTIVE (Tier 1)** sektörlerde SVDAG isteğe bağlı (ön-bake); **WARM+** zorunlu (`08` §3).

**Toplam süre:** ~18–20ms / 32³ sektör (GPU async; frame budget’ı bloklamaz).

### 1.4 Transform-Aware Deduplication (SIGGRAPH 2025)

**Transform-Aware SVDAG** (Molenaar & Eisemann, SIGGRAPH 2025) simetri ve dönüşümleri kullanarak ek **%20-45** tasarruf sağlar.

#### Simetri Tipleri

| Simetri | Açıklama | Tasarruf |
|---|---|---|
| **Mirror X/Y/Z** | Eksenlerde ayna | %10-15 |
| **Rotation 90°/180°/270°** | Y ekseni etrafında dönüş | %10-20 |
| **Translation** | Öteleme ile eşleştirme | %5-10 |
| **Kombinasyonlar** | Mirror + Rotation + Translation | **%20-45** |

#### Translation Matching (i3D 2025 Best Paper)

Molenaar & Eisemann (i3D 2025 Best Paper Award), öteleme bazlı geometri eşleştirmeyi de kapsayacak şekilde **genelleştirilmiş bir çerçeve** sunar. Temel fikir: aynı geometri farklı konumlarda tekrarlanıyorsa (örneğin bir evin aynı tip kolonları), sadece bir kere saklanır ve öteleme transform'u ile referans verilir.

**Çalışma prensibi:**
- Her SVDAG node'unun geometrisi hash'lenir
- Öteleme adayları (`TRANSLATION_CANDIDATES`) taranır
- Aynı hash altında farklı öteleme ile eşleşen node'lar tek örneğe indirgenir

```rust
pub struct TransformAwareHashTable {
    normal_map: HashMap<u64, u32>,                                // identity match
    transform_map: HashMap<(u64, SvdagTransform), u32>,           // mirror/rotation match
    translation_map: HashMap<(TransformHash, SvdagTransform), u32>, // translation match
}

impl TransformAwareHashTable {
    /// Öteleme dahil tüm dönüşümleri dene
    pub fn lookup_with_transforms(&self, geometry_hash: u64) -> Option<(u32, SvdagTransform)> {
        if let Some(&idx) = self.normal_map.get(&geometry_hash) {
            return Some((idx, SvdagTransform::Identity));
        }

        // Mirror + Rotation kontrolü (mevcut)
        let transform_order = [
            SvdagTransform::MirrorX, SvdagTransform::MirrorY, SvdagTransform::MirrorZ,
            SvdagTransform::Rotate90, SvdagTransform::Rotate180, SvdagTransform::Rotate270,
        ];
        for transform in transform_order {
            if let Some(&idx) = self.transform_map.get(&(geometry_hash, transform)) {
                return Some((idx, transform));
            }
        }

        // Translation matching (yeni — i3D 2025)
        for offset in TRANSLATION_CANDIDATES {
            if let Some(&idx) = self.translation_map.get(&(
                translate_hash(geometry_hash, offset),
                SvdagTransform::Translation(offset)
            )) {
                return Some((idx, SvdagTransform::Translation(offset)));
            }
        }

        None
    }
}

/// Öteleme adayları: 2³ küp içindeki 26 yön (tüm ±x,±y,±z kombinasyonları)
pub const TRANSLATION_CANDIDATES: [IVec3; 26] = [
    IVec3::new(1,0,0), IVec3::new(-1,0,0), IVec3::new(0,1,0), IVec3::new(0,-1,0),
    IVec3::new(0,0,1), IVec3::new(0,0,-1), IVec3::new(1,1,0), IVec3::new(-1,1,0),
    // ... tüm 26 yön ...
];
```

**Performans notu:** Translation matching, bake süresini **~2-3× artırabilir** (26 yön × hash lookup). Ancak bake zaten GPU'da async çalışır ve sadece sector uzaklaşınca tetiklenir — oyuncu deneyimini etkilemez. Karşılığında **ek %5-10 sıkıştırma** sağlanır.

#### Transform-Aware Node Yapısı (Occupancy Encoding ile Birleşik)

Molenaar & Eisemann (2025) transform-aware yapısı, Modisett & Billeter (2025) occupancy encoding ile birleştirilmiştir. Transform bilgisi child pointer'ının üst bitlerinde taşınır, ayrı bir alan gerekmez.

```rust
/// Her child pointer'ın bit dağılımı:
/// bit[31:29]: occupancy mask (3 bit = 8 alt-dal)
/// bit[28]:    has_transform flag
/// bit[27]:    reserved
/// bit[26:24]: transform type (0-7, sadece has_transform=1 ise)
/// bit[23:0]:  node index (24 bit = 16M node)
///
/// Transform, sadece gerektiğinde encode edilir (bit[28]=1).
/// Identity transform hiç yer kaplamaz.
pub struct OccupancyTransformNode {
    pub child_pointers: [u32; 8],  // occupancy + transform encoded
    pub material: u16,
    pub padding: [u8; 6],          // 32-byte alignment
}
```

**Transform tipleri (pointer encoding'e gömülü):**

```rust
#[repr(u8)]
pub enum SvdagTransform {
    Identity = 0,     // Hiç yer kaplamaz, bit[28]=0
    MirrorX = 1,      // Occupancy pointer'da bit[26:24]=001
    MirrorY = 2,
    MirrorZ = 3,
    Rotate90 = 4,
    Rotate180 = 5,
    Rotate270 = 6,
    MirrorRotate = 7, // Mirror + Rotation kombinasyonu
}
```


### 1.5 Ghost Page Table — SVDAG ↔ XBrickMap Geçişi

**GigaVoxels DP** (Richermoz & Neyret, [HPG 2024](https://hal.science/hal-04654692)): sparse brick pool + sayfa tablosu; veri üretimi/render GPU tarafında senkronizasyon açlığını (starvation) azaltır. Strata’da aynı fikir: SVDAG node sayfaları yüklenmeden **XBrickMap render devam eder** (`08-streaming.md` §3 yumuşak geçiş).

**4-tier mesafe eşlemesi** (`01-overview.md` §2.1, `08-streaming.md` §1–2) — eski “32/128 blok” prototip değerleri **kaldırıldı**:

| Tier | Mesafe (m) | XBrickMap | SVDAG | Ghost page durumu |
|------|------------|-----------|-------|-------------------|
| **ACTIVE** | &lt; 96 | Aktif (edit) | Opsiyonel ön-bake | `Ghost` (placeholder) |
| **WARM** | 96 – 384 | Aktif (render+fizik) | Bake / yükleme | `Loading` → `Ready` |
| **DISTANT** | 384 – 1536 | Boşaltılmış | Tek kaynak | `Ready` |
| **ARCHIVE** | ≥ 1536 | Yok | Disk / sıkıştırılmış | Sayfa yok (stream-in) |

```rust
/// Sektör başına geçiş durumu (SOĞUK component — `03` §4.5).
#[derive(Component)]
pub struct SectorTransition {
    pub svdag_root: Option<u32>,
    /// `06` `GlobalBrickPool` — Tier 3’te `None` (brick serbest).
    pub pool_anchor: Option<NonZeroU32>,
    pub ghost: GhostPageTable,
    pub transition_frame: u32,
}

pub struct GhostPageTable {
    /// atomic u32: 0 = ghost, 1 = loading, 2 = ready (WGSL `select` ile branchless)
    pages: Vec<AtomicU32>,
    /// Ghost sırasında ray hit — genelde `block_type` AIR veya yakın LOD ortalaması (u16).
    pub fallback_block_type: u16,
}
```

**WGSL ghost loading (branchless):**

```wgsl
fn load_svdag_node(node_idx: u32, fallback_block_type: u32) -> SvdagNode {
    let page_state = atomicLoad(&ghost_pages.pages[node_idx / NODES_PER_PAGE]);
    return select(
        SvdagNode(0xFFu, array<u32, 8>(0u,0u,0u,0u,0u,0u,0u,0u), fallback_block_type),
        node_pool.nodes[node_idx],
        page_state == 2u
    );
}
```

**Kazanç:** WARM geçişinde XBrickMap + ghost SVDAG paralel; render thread beklemez (GigaVoxels DP / `08` §3).

### 1.6 Shallow SVDAG Streaming + LOD (Aokana → Strata 32³)

Derin SVDAG traversal = çoklu indirect jump = GPU cache miss. **Aokana** ([I3D 2025](https://arxiv.org/abs/2505.02017)) çoklu **sığ** SVDAG + view-dependent streaming + LOD (`density = 2`) kullanır; tek derin ağaç yerine sahne parçalara bölünür.

#### Strata uyarlaması (06 anayasa)

Aokana orijinalinde LOD-0 chunk **M³ dünya / 256³ çözünürlük** (edit yok, saf render). Strata **edit** için `06` sektörünü korur:

| Kavram | Aokana (paper) | Strata |
|--------|----------------|--------|
| LOD-0 birim | M³ voxel chunk, 256³ grid | **32³ sektör** (`Sector`), shallow SVDAG max depth **4–5** |
| LOD-1+ | 8 chunk → 1 chunk, yine 256³ | **8 sektör (2×2×2)** → 1 aggregate SVDAG; dünya alanı **64³ sektör voxel** |
| Edit | Yok | Tier 1–2: XBrickMap; bake snapshot’tan |
| VRAM | ~%5 yüklü | Aynı hedef (`SvdagStreamingManager`) |

**256³ tek chunk bake** Strata’da **uygulanmaz** (edit maliyeti; `§3.5`).

| Özellik | Derin SVDAG (Geleneksel) | Shallow SVDAG (Aokana) |
|---|---|---|
| **Max depth** | 8-12 level | 4-5 level |
| **Traversal** | Çoklu indirect jump | Az indirect jump |
| **VRAM kullanımı** | Tüm sahne | Sadece **%5** |
| **Chunk yapısı** | Tek bir SVDAG | Çoklu bağımsız SVDAG |
| **32K+ çözünürlük** | Yavaş | **2-4× daha hızlı** |
| **Streaming** | Yok | View-dependent, LOD bazlı |

#### Recursive LOD (sektör koordinatı)

```rust
/// LOD 0 = tek `SectorCoord`; LOD n = 2^n sektör kenarı (8 çocuk birleşimi).
pub struct LodSectorSvdag {
    pub lod_level: u8,
    /// LOD 0: sektör indeksi; LOD 1+: üst sektör kümesinin köşe coord’u
    pub anchor: IVec3,
    pub svdag_root: u32,
    /// Aokana default — [arxiv:2505.02017](https://arxiv.org/abs/2505.02017) §3.4
    pub density_threshold: u8,  // default: 2
}

pub const SECTOR_VOXELS: u32 = 32;
pub const SHALLOW_SVDAG_MAX_DEPTH: u8 = 5;
```

**Aggregation:** 8 alt sektör hücresinden ≥ `density_threshold` dolu ise üst hücre dolu; yaprak `block_type` baskın oy + `GlobalPalette` ortalama renk (variant unbake’te kaybolur — yakınlaşınca LOD-0 bake zorunlu).

```
LOD 0: 1× Sector (32³) → shallow SVDAG, yaprak u8 palet indeksi
LOD 1: 2×2×2 sektör (64³) → tek aggregate SVDAG
LOD 2: 4×4×4 sektör (128³) …
```

#### Shallow SVDAG havuzu

```rust
pub struct ShallowSvdagPool {
    roots: Vec<ShallowSvdagRoot>,
    node_pool: GenerationalNodePool,  // §1.1 global
}

pub struct ShallowSvdagRoot {
    pub sector_coord: IVec3,   // LOD 0; üst LOD’da anchor
    pub root_index: u32,
    pub lod_level: u8,
    pub loaded: bool,
    pub priority: f32,
}
```

#### View-Dependent Streaming (Ghost Page Table ile)

Streaming sistemi, kameranın görüş alanındaki chunk'ları önceliklendirir ve VRAM bütçesi dahilinde yükler. Aokana'nın yaklaşımı:

- **Toplam sahnenin sadece %5'i** VRAM'de tutulur
- **200 MB/s streaming hızı** ile PCIe bandwidth'i verimli kullanılır
- **%95 hit rate** ile çoğu erişim önceden yüklenmiş chunk'lara gider
- **LRU eviction policy** ile görüş alanı dışındaki chunk'lar boşaltılır

```rust
#[derive(Resource)]
pub struct SvdagStreamingManager {
    loaded_sectors: HashMap<IVec3, SectorStreamState>,
    load_queue: PriorityQueue<IVec3, f32>,
    disk_index: SvdagDiskIndex,
    ghost_table: GhostPageTable,
    vram_budget: f32,
    streaming_rate: f32,
    epoch: u32,
}

pub enum SectorStreamState {
    Ghost,
    Loading { progress: f32 },
    Ready(ShallowSvdagRoot),
}

impl SvdagStreamingManager {
    /// `08` `determine_tier` ile aynı eşikler (metre).
    pub fn select_lod(distance_m: f32) -> u8 {
        if distance_m < 96.0 { 0 }        // ACTIVE
        else if distance_m < 384.0 { 1 }  // WARM
        else if distance_m < 1536.0 { 2 } // DISTANT (`08` §2)
        else { 3 }                        // ARCHIVE
    }

    const DEFAULT_VRAM_BUDGET: f32 = 0.05;
    const TARGET_STREAMING_RATE: f32 = 200.0; // MB/s — Aokana hedefi
}
```

**Performans (Aokana sonuçları + optimizasyonlar):**
- **4.8× hız artışı** (daha az indirect jump)
- **9× VRAM azalması** (sadece %5 yüklü)
- **32K+ çözünürlük** HashDAG'den 2-4× daha hızlı
- **Streaming overhead:** <1ms/frame
- **200 MB/s** streaming hızı, **%95** hit rate
- **Node boyutu:** 40B → ortalama 23B (değişken uzunluklu encoding)
- **GPU Node Pool:** ~10MB → **~5.9MB** (%41 az)
- **Traversal hızı:** +%15 (occupancy encoding ile daha az memory access)
- **GC overhead:** ~0.01ms/frame (epoch-based, cascading free yok)
- **SVDAG→XBrickMap geçiş:** 0ms starvation-free (ghost page table)
- **LOD density threshold:** 2 (Aokana default)

### 1.7 GPU-Driven Voxel Render Pipeline: Hi-Z + Visibility Buffer

Aokana ([arxiv:2505.02017](https://arxiv.org/html/2505.02017v1)) SVDAG ray marching’i GPU-driven pipeline ile birleştirir. Strata’da Tier 2–3 SVDAG pass’leri bu düzeni kullanır; Tier 1 XBrickMap pass’i aynı buffer’a yazar (unified visibility, `10` placeholder olsa da layout burada sabit).

#### Pipeline Genel Akışı

```
Pass 1 — Tile Selection (Compute):
  → Ekran 8×8 pixel tile'larına bölünür
  → Her 4×4 tile için bir thread group atanır
  → Her tile'dan screen ray projekte edilir, hangi sektörlerin katkıda bulunduğu belirlenir
  → Bir önceki frame'in Hi-Z texture'ı kullanılarak gizli tile'lar elenir
  → TileInfo (ekran tile + sector_id) çıktısı

Pass 2 — DAG Ray Marching (Compute, indirect dispatch):
  → Her tile–sektör çifti için SVDAG ray marching
  → 64-bit visibility buffer'a sonuç yazılır
  → InterlockedMax() / atomicMax() ile depth test (depth yüksek bitlerde, en yakın piksel kazanır)
  
Pass 3 — Hi-Z Re-execution (Compute):
  → Son frame'in depth bilgisinden Hi-Z texture oluştur
  → Bir önceki pass'te culled edilen tile'ları mevcut frame'in Hi-Z'i ile tekrar test et
  → Hatalı culling'leri düzelt, eksik tile'ları yeniden işle

Pass 4 — Color Resolve (Compute):
  → Visibility buffer'dan normal, sector_id, voxel pozisyonu çıkar
  → DFS order ile intersection node'un rengini binary search ile bul
  → Normal + renk ile shading yap
```

#### Visibility Buffer (64-bit, Aokana layout)

Literatür / Aokana (I3D 2025) Figure 7 özeti: depth **en yüksek 24 bit**, normal 3 bit, chunk (sector) ID 13 bit, voxel koordinatları **en düşük 24 bit** — `atomicMax` ile en yakın piksel seçimi.

```rust
/// bit[0:23]   voxel_pos (24 bit, sektör-içi)
/// bit[24:36]  sector_id (13 bit, max 8192 görünür sektör)
/// bit[37:39]  axis-aligned normal (3 bit)
/// bit[40:63]  depth (24-bit, reversed-z) — yüksek bitlerde
pub struct VisibilityBufferEntry(u64);

impl VisibilityBufferEntry {
    pub fn encode(depth: f32, normal: u8, sector_id: u16, voxel_pos: u32) -> u64 {
        let d = ((1.0 - depth) * 16777215.0) as u64;
        (voxel_pos as u64 & 0xFF_FFFF)
            | ((sector_id as u64 & 0x1FFF) << 24)
            | ((normal as u64 & 0x7) << 37)
            | (d << 40)
    }

    pub fn decode_depth(entry: u64) -> f32 {
        1.0 - ((entry >> 40) as f32 / 16777215.0)
    }

    pub fn decode_normal(entry: u64) -> u8 {
        ((entry >> 37) & 0x7) as u8
    }

    pub fn decode_sector_id(entry: u64) -> u16 {
        ((entry >> 24) & 0x1FFF) as u16
    }

    pub fn decode_voxel_pos(entry: u64) -> [u8; 3] {
        let v = (entry & 0xFF_FFFF) as u32;
        [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
    }
}
```

#### Hi-Z Occlusion Culling

Hi-Z texture, bir önceki frame'in depth buffer'ından oluşturulan **mipmap piramidi**'dir. Her mip seviyesindeki texel, bir alt seviyedeki 2×2 bloğun **maximum depth** değerini içerir.

```rust
pub struct HiZBuffer {
    pub depth_texture: wgpu::Texture,   // Mevcut frame depth
    pub hiz_texture: wgpu::Texture,     // Mipmap piramidi
    pub mip_levels: u32,
}

impl HiZBuffer {
    /// Tile bounding box'ını Hi-Z ile test et
    pub fn test_tile_occlusion(&self, tile_bbox: &AABB, view_proj: &Mat4) -> bool {
        // Tile'ın screen space bounding box'ını hesapla
        let screen_rect = self.project_to_screen(tile_bbox, view_proj);
        // Bounding box boyutuna göre mip level seç (maks 2 pixel kaplayacak şekilde)
        let mip = self.select_mip_level(screen_rect);
        // Hi-Z texture'dan ilgili bölgenin max depth'ini al
        let hiz_depth = self.sample_hiz(mip, screen_rect.center());
        // Tile'ın min depth'i Hi-Z'den büyükse → gizli
        screen_rect.min_depth >= hiz_depth
    }

    fn select_mip_level(&self, rect: &ScreenRect) -> u32 {
        // Tile'ın screen'de kapladığı alanın log2'si
        let pixel_area = (rect.width * rect.height) as f32;
        (pixel_area.log2() / 2.0).floor() as u32
    }
}
```

**WGSL Hi-Z Build (basitleştirilmiş):**

```wgsl
@compute @workgroup_size(8, 8, 1)
fn build_hiz(@builtin(global_invocation_id) gid: vec3<u32>,
             @builtin(workgroup_id) wg: vec3<u32>) {
    // Mip level 0'dan mip level 1+ oluştur
    let src = textureLoad(depth_texture, gid.xy * 2, 0);
    let depth = max(src.r, max(
        textureLoad(depth_texture, gid.xy * 2 + vec2(1, 0), 0).r,
        max(
            textureLoad(depth_texture, gid.xy * 2 + vec2(0, 1), 0).r,
            textureLoad(depth_texture, gid.xy * 2 + vec2(1, 1), 0).r
        )
    ));
    textureStore(hiz_texture, gid.xy, 1, vec4(depth));
}
```

#### Performans Kazanımları

| Bileşen | Süre |
|---------|------|
| Hi-Z build | <0.2ms |
| Tile selection + occlusion test | ~0.5ms |
| DAG ray marching | ~2-4ms (sahne karmaşıklığına bağlı) |
| Color resolve | ~1ms |
| **Toplam pipeline** | **~4-6ms** |
| Overdraw azalması | **%50-70** (Hi-Z culling ile) |

### 1.8 SVDAG → XBrickMap Unbake (Wavefront + `SectorPalette`)

**Tetik:** `08` §3 — Tier 3→2 veya mesafe &lt; 384m; `NeedsSvdagUnbake` ZST (`§1.9`). HashDAG reverse traversal (Careil et al. 2020).

```rust
#[derive(Resource)]
pub struct UnbakeScheduler {
    queue: BinaryHeap<UnbakeJob>,
    max_concurrent: u32,
    budget_ms: f32,
}

pub struct UnbakeJob {
    pub sector_entity: Entity,
    pub coord: IVec3,
    pub svdag_root: u32,
    pub priority: f32,
    pub progress: u32,
    pub total_nodes: u32,
}
```

**CPU tamamlama (main thread, wavefront bittikten sonra):**

```rust
/// GPU’dan gelen (pos, palette_index) çiftlerini canlı sektöre yazar.
pub fn commit_unbake_to_sector(
    sector: &mut Sector,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    writes: &[(IVec3, u8)],  // palette_index — LOD-0 SVDAG yaprakları
) {
    for (pos, local_idx) in writes {
        let entry = /* bake sırasında saklanan snapshot palette[local_idx] veya */
            palette.resolve(*local_idx);
        let idx = palette.get_or_insert(entry).expect("palette full");
        pool.set_voxel(sector, *pos, idx);  // 06 API
    }
}
```

**Wavefront (WGSL):** GPU geçici buffer’a `(voxel_pos, u8 palette_index)` yazar; `block_type` doğrudan pool’a yazılmaz (05 §14).

**Incremental akış (`08` Tier 3→2):**
```
Frame N:   determine_tier → WARM; insert NeedsSvdagUnbake
           Wavefront-0: root
Frame N+k: leaf’ler → staging buffer
Frame N+4: commit_unbake_to_sector; insert ChunkDirty + NeedsRemesh (03)
           remove NeedsSvdagUnbake; SectorSvdag.root kalabilir (Tier 2 dual)
           Tier 2→1: SVDAG root drop + epoch++ (08 §3)
```

| Adım | Süre |
|------|------|
| Wavefront (4–5 level) | ~1.5ms |
| Palette commit (CPU) | ~0.3ms |
| Pool write + mask update | ~1.0ms |
| **Toplam** | **~2.8ms** (frame budget ile yayılır) |

**ECS:** `ChunkDirty` / `NeedsRemesh` unbake sonrası; `Sector.dirty` alanı yok (`06` `Changed<Sector>` / ZST modeli).

### 1.9 ECS Entegrasyonu (`03-ecs-architecture.md`)

SVDAG mantığı `core` crate’inde; sistemler **World / Streaming** plugin’inde (`03` `WorldSystems`, `04` plugin-first).

#### Component’lar

```rust
/// SVDAG kökü + bake nesli (SOĞUK — tier değişiminde okunur).
#[derive(Component, Clone, Copy)]
pub struct SectorSvdag {
    pub root_index: Option<u32>,
    pub bake_epoch: u32,
}

#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct NeedsSvdagBake;

#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct NeedsSvdagUnbake;

// Tier geçişi — 03 §2.1 (zaten tanımlı)
// TierChange { old_tier, new_tier, timestamp }
// SectorData(Arc<CompressedChunkData>) — bake girişi
// SectorTransform { position, tier } — determine_tier girişi
```

#### Resource’lar

```rust
#[derive(Resource)]
pub struct GenerationalNodePool { /* §1.1 */ }

#[derive(Resource)]
pub struct SvdagBakeQueue {
    pub sender: crossbeam::channel::Sender<SvdagBakeRequest>,
    pub results: crossbeam::channel::Receiver<SvdagBakeResult>,
}

pub struct SvdagBakeResult {
    pub sector_entity: Entity,
    pub root_index: u32,
    pub snapshot_version: u64,
}
```

#### Sistem setleri ve sıra

```rust
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SvdagSystems {
    TierTransition,   // mesafe → TierChange, ghost, bake/unbake ZST
    EnqueueBake,
    CollectBake,      // async GPU sonuç → SectorSvdag
    EnqueueUnbake,
    CollectUnbake,    // GPU staging → commit_unbake_to_sector
}

// StrataPlugin (03 §5.2) — örnek zincir:
// WorldSystems::Streaming → SvdagSystems::TierTransition
//   → SvdagSystems::EnqueueBake.after(TierTransition)
//   → SvdagSystems::CollectBake
//   → (paralel) MeshingQueue NeedsRemesh
```

**`tier_transition_system` (filter-first):**

```rust
fn tier_transition_system(
    mut commands: Commands,
    camera: Res<MainCamera>,
    mut q: Query<(Entity, &SectorEntity, &mut SectorTransform), Without<Disabled>>,
) {
    for (entity, sector, mut transform) in &mut q {
        let dist = sector_distance_m(sector.coord, camera.position); // 08 §2.2
        let new_tier = determine_tier_hysteresis(dist, Some(transform.tier), &thresholds, &hysteresis);
        let old_tier = transform.tier;
        if old_tier == new_tier {
            continue;
        }
        commands.entity(entity).insert(TierChange {
            old_tier,
            new_tier,
            timestamp: Instant::now(),
        });

        match (old_tier, new_tier) {
            (Tier::Active, Tier::Warm) => {
                commands.entity(entity).insert(NeedsSvdagBake);
            }
            (Tier::Distant, Tier::Warm) => {
                commands.entity(entity).insert(NeedsSvdagUnbake);
            }
            (Tier::Warm, Tier::Active) => {
                // 08 §3: SVDAG serbest — CollectBake tersine epoch++
                commands.entity(entity).remove::<NeedsSvdagBake>();
            }
            _ => {}
        }

        transform.tier = new_tier;
    }
}
```

> **Not:** `SectorTransform.tier` yalnızca gerçekten değişince atanır (`03` change-detection kuralı).

**`enqueue_svdag_bake_system`:** `With<NeedsSvdagBake>` + `SectorData` → `CompressedChunkData::snapshot_from_pool` gerekirse tazele → `SvdagBakeQueue` gönder → ZST kaldırma **CollectBake**’te.

#### Network (`03` §4.4)

| Kanal | İçerik |
|-------|--------|
| Replication | `TierChange`, `ChunkDirty` metadata only |
| `BrickDelta` RPC | Aktif edit (Tier 1) |
| **`SvdagSnapshot` RPC** | Sıkıştırılmış `SectorSvdag` + node pool delta veya önceden bake edilmiş blob (Tier 2+) |

```rust
#[derive(Serialize, Deserialize)]
pub struct SvdagSnapshot {
    pub coord: IVec3,
    pub root_index: u32,
    pub node_blob: Vec<u8>,       // zstd
    pub palette: heapless::Vec<PaletteEntry, 256>,  // unbake / renk tutarlılığı
    pub data_version: u64,
}
```

Sunucu-authoritative: istemci snapshot’ı doğrulamadan `SectorSvdag` güncellemez.

#### Render SubApp extract (`04` §2)

`DirtySectorDelta` yanında `SvdagRootDelta { entity, root_index, lod }` — Render world yalnızca kök indeks + epoch alır; node pool GPU’da paylaşımlı SSBO.

---

## 2. Referans Alınan Makaleler

| # | Makale | Yıl | DOI / Link | Ana Katkı | Strata bölümü |
|---|---|---|---|---|---|
| M1 | Transform-Aware SVDAG (Molenaar & Eisemann) | 2025 | [10.1145/3728301](https://doi.org/10.1145/3728301) | Dönüşüm dedup | §1.4 |
| M2 | Aokana (Fang, Wang, Wang) | 2025 | [arxiv:2505.02017](https://arxiv.org/abs/2505.02017) | Shallow SVDAG, visibility buffer, LOD | §1.6–1.7 |
| M3 | Occupancy Encoding (Modisett & Billeter) | 2025 | CGF | Pointer encoding | §1.2 |
| M4 | Editing SVDAG on GPU (Molenaar & Eisemann) | 2024 | [PG 10.2312/pg.20241310](https://diglib.eg.org/handle/10.2312/pg20241310) | GPU hash, edit | §1.1, §1.3 |
| M5 | HashDAG (Careil et al.) | 2020 | CGF | Merge / reverse traverse | §1.3, §1.8 |
| M6 | GigaVoxels DP (Richermoz & Neyret) | 2024 | [HAL](https://hal.science/hal-04654692) | Starvation-free pages | §1.5 |
| M7 | SSVDAGs (Villanueva et al.) | 2016 | Web3D | Değişken node boyutu | §1.2 |

---

## 3. Sonradan Eklenebilecek İyileştirmeler (Future Improvements)

Aşağıdaki iyileştirmeler mevcut sistem çalışır duruma geldikten sonra değerlendirilebilir. Her biri belirli tradeoff'lar içerir ve öncelik sırası proje ihtiyaçlarına göre belirlenmelidir.

### 3.1 SlabHash GPU Hash Table (Pacific Graphics 2024)

**Ne işe yarar:** Mevcut `GpuAtomicStack` yapısı yerine Molenaar & Eisemann'ın SlabHash tabanlı GPU hash table'ını kullanarak GPU'daki SVDAG editing performansını artırmak.

```rust
// Warp-synchronous (32 thread) GPU allocator
pub struct SlabHashTable {
    slabs: Vec<Slab>,           // Her slab 32 entry
    warp_mutex: AtomicU32,      // Warp başına spinlock
    allocator: SlabAlloc,       // GPU memory allocator (sabit boyutlu)
}
```

**Avantajları:**
- Warp içi kooperatif arama (32 thread = 1 warp, divergence yok)
- SlabAlloc sabit boyutlu allocation → heap fragmentation yok
- Mevcut GpuAtomicStack'ten ~%10-15 daha hızlı

**Dezavantajları:**
- Sabit boyut limitation: her node formatı (16B/24B/32B/36B/48B) için ayrı slab pool gerekir
- Warp-synchronous yaklaşım warp'lar arası load balancing'i zorlaştırır
- Mevcut GpuAtomicStack çalışır durumda — değişim için ek +500-800 satır kod

**Değer mi?:** Mevcut sistem çalışıyorsa düşük öncelik. Fark %10-15 mertebesinde.

### 3.2 Geometry/Color Ayrıştırması (Aokana yaklaşımı)

**Ne işe yarar:** SVDAG node'larında geometry (binary: voxel var/yok) ve color (renk/doku bilgisi) bilgisini ayırarak aynı geometrinin farklı renklerde paylaşılmasını sağlamak.

```
Chunk:
  ├── Geometry SVDAG (binary: voxel var/yok, child_mask sadece occupancy)
  └── Color Blocks (renk bilgisi, ayrı compressed array)
```

**Avantajları:**
- Aynı geometri farklı renklerde paylaşılabilir (ör: taş duvar, boyalı bloklar)
- Geometry SVDAG binary olduğu için daha küçük
- Renk değişiklikleri sadece color array'ini günceller, SVDAG'ı yeniden bake etmez

**Dezavantajları:**
- Color bilgisine erişmek için DFS order binary search → ekstra 1-2 memory access
- Strata’da canlı veri `SectorPalette` + `u8` indeks (`05` §14); geometry/color ayrımı LOD-0’da palet indeksini koruyarak kısmen mümkün, ancak unbake yolu `§1.8` ile çakışır.

**Değer mi?:** Düşük öncelik — §1.3 snapshot palet yolu yeterli; Aokana geometry/color ayrımı statik sahneler içindir.

### 3.3 Mesh Shader ile Hibrit Render

**Ne işe yarar:** Tier 1 (aktif bölge, 0-96m) için SVDAG ray marching yerine Task + Mesh Shader pipeline'ı ile doğrudan rasterization yapmak.

```
Task Shader:
  → Hangi voxel block'larının isosurface içerdiğini belirle
  → Frustum + occlusion cull uygula

Mesh Shader:
  → Sadece visible block'lar için geometry oluştur
  → Greedy mesh veya direct isosurface extraction
  → Rasterizer'a doğrudan besle
```

**Avantajları:**
- Tier 1'de mesh shader ile rasterization, SVDAG ray marching'ten potansiyel olarak daha hızlı
- Mevcut mesh-based grafik pipeline ile daha iyi entegrasyon

**Dezavantajları:**
- wgpu 0.29'da mesh shader desteği sınırlı: `VK_NV_mesh_shader` / `VK_EXT_mesh_shader` extension'larına bağlı, Apple Silicon (Metal) mesh shader'ı desteklemiyor
- İki render path (SVDAG ray marching + mesh shader) bakım maliyeti = 2× shader bakımı
- Tier 1→Tier 2 geçişinde ghost page table'ın mesh shader path'i için genişletilmesi gerekir

**Değer mi?:** Şu an için hayır. Aokana tamamen compute shader tabanlı ve 4.8× hızlanma sağlıyor. wgpu mesh shader desteği olgunlaştığında ve Tier 1 rendering bottleneck olursa tekrar değerlendirilebilir.

### 3.4 Segmentation SVDAG (VMV 2024) — Büyük Veri Setleri İçin

**Ne işe yarar:** Modding ile kullanıcı tarafından yüklenen büyük voxel modeller için her block-type/label'ı ayrı AABB grid'i + SVDAG olarak temsil etmek.

```rust
pub struct SegmentationSvdag {
    labels: Vec<LabelSvdag>,
    bvh: Bvh,                       // Tüm label'ların AABB'lerini indeksleyen BVH
}

pub struct LabelSvdag {
    label_id: u16,
    aabb_grid: Vec<AABB>,           // Her label için dolu bölgeleri temsil eden AABB'ler
    svdag: Svdag,                   // Her AABB içindeki geometri için SVDAG
}
```

**Avantajları:**
- 113 GB volume'ları 108 FPS'de path trace edebilir (lossless, 32 bounce, VMV 2024 sonucu)
- Her label bağımsız sıkıştırılır, sadece var olduğu bölgelerde yer kaplar

**Dezavantajları:**
- Strata'nın mevcut block modeliyle (32³ sector, u16 material ID) örtüşmez — büyük bir mimari değişiklik gerektirir
- Gerçek zamanlı edit için uygun değil (statik modeller için tasarlanmış)

**Değer mi?:** Strata'nın çekirdek voxel motoru için değil, modding sistemi ile yüklenen büyük statik modeller (ör: heykel, bina prefab) için düşünülebilir.

### 3.5 Chunk Boyutunu 256³'e Çıkarma (Aokana stili)

**Ne işe yarar:** Aokana LOD-0’da M³ dünya / 256³ grid kullanır ([arxiv:2505.02017](https://arxiv.org/abs/2505.02017)) — edit yok.

**Strata kararı (doğrulandı):** **Hayır.** `06` 32³ sektör + `§1.6` 8-sektör LOD aggregate. Tek 256³ bake edit’te ~512× maliyet; `SectorPalette` ve `GlobalBrickPool` 32³ için tasarlandı.

---

## 4. Önceliklendirme Özeti

| # | İyileştirme | Kazanç | Risk | Çaba | Öncelik |
|---|-----------|--------|------|------|---------|
| **1** | **Shallow SVDAG + LOD Streaming (§1.6)** | 4.8× hız, 9× VRAM azalması | Düşük | Orta | **YÜKSEK** |
| **2** | **Translation Matching (§1.4)** | Ek %20-35 sıkıştırma | Düşük | Düşük | **YÜKSEK** |
| **3** | **Hi-Z + Visibility Buffer Pipeline (§1.7)** | %50-70 overdraw azalması | Orta | Orta-Yüksek | **YÜKSEK** |
| 4 | SlabHash GPU Hash Table (§3.1) | ~%10-15 hız | Düşük | Orta | Düşük |
| 5 | Geometry/Color Ayrıştırması (§3.2) | Block tipine bağlı | Orta | Orta | İhtiyaç Analizi |
| 6 | Mesh Shader Hibrit Render (§3.3) | Potansiyel hız | Yüksek | Çok Yüksek | Beklemede |
| 7 | Segmentation SVDAG (§3.4) | Büyük modeller için | Yüksek | Yüksek | Beklemede |
| 8 | Chunk 256³ (§3.5) | N/A | Çok Yüksek | Çok Yüksek | **UYGUN DEĞİL** |
