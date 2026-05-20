# Chunk Architecture — Strata

> **Son güncelleme:** 2026-05-20 (Optimizasyonlar eklendi: SOA+SIMD, Transform-Aware SVDAG, Shallow SVDAG, Vertex Pool, Foveated Rendering, Network Quantization, AOI, Content-Defined Chunking)
> **Durum:** Onaylanmış — Uygulama aşamasına hazır
> **Not:** Mevcut `Vec<u16>` chunk sistemi tamamen değiştirilecek. Bu doküman yeni mimarinin tek kaynağıdır.

---

## 1. Genel Bakış

Strata, 4-kademeli (tier) hiyerarşik bir voxel veri sistemi kullanır. Her kademe oyuncuya olan mesafeye göre farklı bir veri temsil formatı kullanır. Bu yaklaşım, **edit hızı**, **render performansı**, **bellek verimliliği** ve **streaming** arasında Pareto-optimal dengeyi sağlar.

### Temel Prensipler

- **Yakın = Brickmap (XBrickMap):** O(1) edit, 4-level ray skip, doğrudan fizik
- **Orta = Brickmap + SVDAG birlikte:** Pop-in olmadan yumuşak geçiş
- **Uzak = SVDAG:** Deduplication, LOD, GPU ray march
- **Çok uzak = Sıkıştırılmış SVDAG:** Disk, zstd + rkyv, lazy streaming

### Kanıtlanmış Referanslar

| Bileşen | Kaynak |
|---|---|
| XBrickMap | dubiousconst282/VoxelRT (2024) — en hızlı ray trace yöntemlerinden |
| SVDAG + GPU Editing | GPU-SVDAG-Editing, Pacific Graphs 2024 |
| Aokana Framework | Fang et al., ACM SIGGRAPH 2025 — 4.8x hız, 9x VRAM azalması |
| Hybrid Voxel Formats | Molenaar & Eisemann, Eurographics 2024 |
| Transform-Aware SVDAG | Molenaar & Eisemann, SIGGRAPH 2025 — %20-45 ek deduplication |
| Shallow SVDAG Streaming | Fang et al., Aokana, SIGGRAPH 2025 — %5 VRAM, 2-4× hız |
| Vertex Pooling | Nick McDonald — %40 frame time, %25 meshing time azalması |
| Foveated Rendering | SIGGRAPH 2025 — %60-80 ray/pixel azalması, %99.3 periferik animasyon |
| Rapier Voxels Shape | dimforge/rapier 0.32+ / parry3d 0.26+ — native sparse voxel collider, ghost collision free |
| Teardown | Tuxedo Labs — 8³ brick + MIP ray tracing |
| WGSL 64-bit Atomics | wgpu PR #5383 (2024) — SHADER_INT64_ATOMIC_ALL_OPS / MIN_MAX feature flags |
| GearHash Chunking | HuggingFace Xet — content-defined chunking, BLAKE3 Merkle tree |
| Delta Compression | Network quantization — smallest-three quaternion, varint delta |
| AOI / Interest Management | Spatial partitioning — %80-90 bant genişliği azalması |
| BFS Flood-Fill Lighting | Seed of Andromeda (2015), voxel-light crate (2026) — ~174µs/level-14 torch |
| Starlight Propagation | PaperMC/Starlight — Vanilla'dan 28x hızlı, level propagation |
| SIMD Flood-Fill | atrufulgium.net (2024) — 128 voxel/iterasyon, 15x hızlanma |
| Clustered Voxel GI | Ayerbe & Patow, CGF 2022 — 100x az visibility test |
| Hierarchical Bitmask Culling | SCITEPRESS 2024 — Morton Z-order + light culling |
| TU Wien RGI | Ott et al., 2025 — voxel-specific TAA, noise-free path tracing |
| Neural Irradiance Volume | Adobe, 2024 — 1-5MB, ~1ms inference, noise-free GI |

---

## 2. Dünya Organizasyonu

```
World
  ├── HashMap<IVec3, Sector>     ← spatial hash, O(1) erişim
  │   └── Sector (32×128×32 = 131.072 voxel)
  │       ├── XBrickMap          ← 4-level hiyerarşik brick yapısı (slab → brick → sub-brick)
  │       ├── SVDAG              ← uzak LOD için (Tier 2'den itibaren)
  │       ├── LightData[131072]  ← 16-bit packed light (sky 4-bit + RGB 4×4-bit)
  │       ├── LightCullingMask   ← Hierarchical bitmask (Morton Z-order)
  │       ├── Tier               ← aktif kademe bilgisi
  │       └── Dirty              ← değişiklik bayrağı
  │
  ├── Shared Node Pool           ← tüm SVDAG'lar için global havuz
  │   └── lock-free slab allocator
  │
  ├── Light Engine               ← 5-kademeli hybrid aydınlatma
  │   ├── L0: Direct Light       ← Analytic (sun, point lights)
  │   ├── L1: Block Light        ← CPU SIMD BFS flood-fill
  │   ├── L2: Sky Light          ← Column-first + heightmap
  │   ├── L3: Clustered GI       ← Near indirect (GPU compute)
  │   └── L4: SVDAG Cone Trace   ← Far indirect (GPU ray march)
  │
  ├── Streaming Manager          ← tier geçişlerini yönetir
  │   ├── Predictive predictor   ← hareket vektörüne göre preload
  │   └── Priority queue         ← yükleme sırası
  │
  └── Render Pipeline            ← unified visibility buffer
      ├── Frustum Culling Pass
      ├── XBrickMap Ray Trace Pass
      ├── SVDAG Ray March Pass
      ├── Color Resolve Pass
      └── Build Hi-Z Pass
```

### Sector Koordinat Sistemi

```rust
/// 32×128×32 voksellik bir sektörün dünya koordinatlarındaki konumu.
/// X/Z eksenleri 32 blok, Y ekseni 128 blok adımında ilerler.
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
pub struct SectorCoord(pub IVec3);

impl SectorCoord {
    /// Dünya pozisyonundan sektör koordinatına dönüşüm.
    pub fn from_world(pos: IVec3) -> Self {
        Self(IVec3::new(
            pos.x.div_euclid(32),
            pos.y.div_euclid(128),
            pos.z.div_euclid(32),
        ))
    }

    /// Sektörün dünya uzayındaki minimum köşesi.
    pub fn world_origin(&self) -> IVec3 {
        IVec3::new(
            self.0.x * 32,
            self.0.y * 128,
            self.0.z * 32,
        )
    }
}
```

---

## 3. XBrickMap — Aktif Alan Veri Yapısı

### 3.1 Hiyerarşik Yapı

XBrickMap, VoxelRT benchmark'da **en iyi** performansı gösteren brickmap varyantıdır. 4 seviyeli hiyerarşik bitmask + left-packed materyal dizisi kullanır.

Sector boyutu **32×128×32** (Y ekseni 128 voxel — mağara/tavan desteği için). Bu boyut, tek bir u64 bitmask ile 4³=64 brick'i adreslemenin ötesinde olduğu için **4 slab** yapısı kullanılır:

```
Sector (32×128×32 = 131.072 voxel)
  ├── 4 Slab (her biri 32×32×32 = 32.768 voxel)
  │   │   Y=0..32, Y=32..64, Y=64..96, Y=96..128
  │   │
  │   ├── Slab Bitmask: u64
  │   │   └── 4³ = 64 brick'in doluluk bilgisi (her bit 1 brick)
  │   │       └── Left-packed: boş brick'ler dizide yer kaplamaz
  │   │
  │   ├── Brick[0..N] (N ≤ 64, sadece dolu olanlar)
  │   │   ├── Brick Bitmask: u64
  │   │   │   └── 4³ = 64 sub-brick'in doluluk bilgisi
  │   │   │       └── Her sub-brick = 2³ = 8 voxel
  │   │   │
  │   │   ├── Sub-brick[0..M] (M ≤ 64, sadece dolu olanlar)
  │   │   │   ├── Sub-brick Bitmask: 8-bit
  │   │   │   │   └── 2³ = 8 voxel'in doluluk bilgisi
  │   │   │   └── Materials: [u16; K] (K ≤ 8, left-packed)
  │   │   │
  │   │   └── Mip Levels (space skipping için)
  │   │       ├── mip_half: 64-bit (4³ = 64 voxel'in ortalama doluluğu)
  │   │       └── mip_quarter: 64-bit (2³ = 8 voxel'in ortalama doluluğu)
  │   │
  │   └── SVDAG Root Index (Tier 2'den itibaren, Option<u32>)
  │
  └── Slab metadata: [SlabMeta; 4]
```

**Neden 4 slab?**
- 32×128×32 = 131.072 voxel → 256 brick (8³ boyutunda)
- 256 brick > 64 bit → tek u64 bitmask yetmez
- 4 slab × 64 brick = 256 brick, her slab kendi u64'üne sahip
- Ray tracing: Y koordinatından slab index'i O(1) (`y / 32`)

### 3.2 Bellek Hesabı

| Bileşen | Boyut | Not |
|---|---|---|
| Slab metadata (4 slab) | 4 × 8 byte = 32B | Her slab'ın u64 bitmask'i |
| Brick dizisi (per slab) | 0-64 × ~1.2KB | Left-packed, boş brick yok |
| — Brick bitmask | 8 byte | Her brick için |
| — Sub-brick dizisi | 0-64 × ~24B | Left-packed |
| — Materials | 0-512 × 2B | Sadece dolu voxel'ler |
| — Mip levels | 16 byte | 2 × 64-bit per brick |
| **Tam dolu sector** | ~312KB | 131.072 voxel (4 × ~78KB) |
| **Ortalama arazi** | ~120-160KB | ~50% boşluk varsayımı |
| **Boş sector** | 32 byte | Sadece 4 slab bitmask |

**Karşılaştırma:** Eski 16×256×16 `Vec<u16>` chunk = 128KB. Yeni XBrickMap boş sector = 32B, dolu sector = ~312KB. Sparse arazi için ortalama ~120-160KB, `Vec<u16>`'dan daha verimli (boşluklardan tasarruf).

### 3.3 Veri Yapısı (Rust)

```rust
/// 32×128×32 voksellik bir sektörün XBrickMap temsili.
/// 4 dikey slab'a bölünmüştür (her slab 32×32×32).
pub struct Sector {
    /// 4 dikey slab. Her biri 32×32×32 voksellik bağımsız brickmap.
    pub slabs: [Slab; 4],

    /// SVDAG root node index'i (Tier 2+ için).
    /// None = SVDAG henüz oluşturulmamış.
    pub svdag_root: Option<u32>,

    /// Bu sector'de değişiklik yapıldı mı?
    /// true = SVDAG artık stale, yeniden bake gerekli.
    pub dirty: bool,

    /// Son bake zamanı.
    pub last_bake_time: Instant,
}

/// 32×32×32 voksellik bir dikey slab.
/// Orijinal 3-level brickmap yapısını korur.
pub struct Slab {
    /// 64 brick'in (4³) doluluk bitmask'i.
    /// Bit i set ise, brick i dizide mevcut.
    pub slab_mask: u64,

    /// Dolu brick'ler. slab_mask'teki set bitlere göre sıralı.
    pub bricks: Vec<Brick>,
}

/// 8³ voksellik bir brick.
pub struct Brick {
    /// 64 sub-brick'in (4³) doluluk bitmask'i.
    pub brick_mask: u64,

    /// Dolu sub-brick'ler. brick_mask'teki set bitlere göre sıralı.
    pub sub_bricks: Vec<SubBrick>,

    /// 4³ çözünürlükte mip level (space skipping).
    /// Her bit, 2×2×2 voksellik bloğun ortalama doluluğunu temsil eder.
    pub mip_half: u64,

    /// 2³ çözünürlükte mip level.
    pub mip_quarter: u64,
}

/// 2³ = 8 voksellik bir sub-brick.
pub struct SubBrick {
    /// 8 voxel'in doluluk bitmask'i.
    pub voxel_mask: u8,

    /// Dolu voxel'lerin materyal ID'leri.
    /// voxel_mask'teki set bitlere göre sıralı, left-packed.
    pub materials: Vec<u16>,
}
```

### 3.4 Random Access (Popcnt ile O(1))

```rust
impl Sector {
    /// Y koordinatından slab index'i hesapla.
    #[inline]
    fn slab_index(y: i32) -> usize {
        (y >> 5) as usize // y / 32, 0..3 arası
    }

    /// (x, y, z) pozisyonundaki blok ID'sini getir.
    /// Koordinatlar sector içinde [0, 32) × [0, 128) × [0, 32) aralığında olmalı.
    pub fn get_block(&self, pos: IVec3) -> Option<u16> {
        let slab_idx = Self::slab_index(pos.y);
        let slab = &self.slabs[slab_idx];
        let local_y = pos.y & 31; // slab içindeki Y (0..31)

        let bx = pos.x / 8;
        let by = local_y / 8;
        let bz = pos.z / 8;
        let brick_index = bx + bz * 4 + by * 16; // 4³ grid

        // Brick bu slab'de var mı?
        if slab.slab_mask & (1 << brick_index) == 0 {
            return None; // Boş alan
        }

        // Brick dizisindeki index = popcnt(slab_mask & ((1 << brick_index) - 1))
        let brick_offset = (slab.slab_mask & ((1u64 << brick_index) - 1)).count_ones() as usize;
        let brick = &slab.bricks[brick_offset];

        let sx = pos.x % 8 / 2;
        let sy = local_y % 8 / 2;
        let sz = pos.z % 8 / 2;
        let sub_index = sx + sz * 4 + sy * 16;

        if brick.brick_mask & (1 << sub_index) == 0 {
            return None;
        }

        let sub_offset = (brick.brick_mask & ((1u64 << sub_index) - 1)).count_ones() as usize;
        let sub = &brick.sub_bricks[sub_offset];

        let vx = pos.x % 2;
        let vy = local_y % 2;
        let vz = pos.z % 2;
        let v_index = (vx + vz * 2 + vy * 4) as u8;

        if sub.voxel_mask & (1 << v_index) == 0 {
            return None;
        }

        let v_offset = (sub.voxel_mask & ((1u8 << v_index) - 1)).count_ones() as usize;
        Some(sub.materials[v_offset])
    }

    /// (x, y, z) pozisyonuna blok koy/kaldır.
    pub fn set_block(&mut self, pos: IVec3, block_id: Option<u16>) {
        let slab_idx = Self::slab_index(pos.y);
        // ... ilgili slab'in bitmask güncelleme + materials ekleme/çıkarma
        // ... parent mip level'ları güncelle
        // ... dirty = true
        self.dirty = true;
    }
}
```

### 3.5 Ray Tracing (4-Level Space Skipping)

```wgsl
// XBrickMap ray marching (compute shader)
// 4-level: sector → slab → brick → sub-brick → voxel
@compute @workgroup_size(64)
fn xbrickmap_ray_trace(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let ray = camera_get_ray(pixel);

    var t: f32 = 0.0;
    var step: f32 = 1.0;

    for (var i = 0u; i < 512u; i++) {
        let pos = ray.origin + ray.direction * t;
        let sector_coord = floor(pos / vec3f(32.0, 128.0, 32.0));

        // 1. Sector seviyesi kontrol
        let sector_bitmask = sector_query_bitmask(sector_coord);
        if (sector_bitmask == 0u) {
            step = 128.0; // tüm sector boş
            t += step;
            continue;
        }

        // 2. Slab seviyesi (Y'ye göre)
        let slab_idx = u32(floor(pos.y / 32.0)) & 3u;
        let slab_bitmask = slab_load(sector_coord, slab_idx);
        if (slab_bitmask == 0u) {
            step = 32.0; // tüm slab boş
            t += step;
            continue;
        }

        let local_y = fract(pos.y / 32.0) * 32.0;
        let brick_pos = vec3f(fract(pos.x / 8.0) * 8.0, fract(local_y / 8.0) * 8.0, fract(pos.z / 8.0) * 8.0);
        let brick_index = compute_brick_index(brick_pos);

        // 3. Brick seviyesi kontrol
        if ((slab_bitmask & (1u << brick_index)) == 0u) {
            step = 8.0;
            t += step;
            continue;
        }

        let sub_pos = fract(pos / 2.0) * 2.0;
        let sub_index = compute_sub_index(sub_pos);

        // 4. Sub-brick seviyesi kontrol
        let brick_data = brick_load(sector_coord, slab_idx, brick_index);
        if ((brick_data.brick_mask & (1u << sub_index)) == 0u) {
            step = 2.0;
            t += step;
            continue;
        }

        // 5. Voxel seviyesi — kesin kontrol
        let voxel_pos = floor(pos);
        let voxel_id = voxel_load(sector_coord, slab_idx, brick_index, sub_index, voxel_pos);
        if (voxel_id != AIR) {
            visibility_buffer_write(pixel, t, pos, voxel_id);
            return;
        }

        step = 1.0;
        t += step;
    }
}
```

### 3.6 SOA Layout + SIMD Optimizasyonu

Mevcut AOS (Array of Structures) layout — `Vec<Brick>`, `Vec<SubBrick>` — **pointer chasing** yaratır. CPU cache miss oranı yüksek, SIMD kullanılamaz. **SOA (Structure of Arrays)** layout ile bu sorun çözülür.

#### 3.6.1 AOS → SOA Dönüşümü

```rust
// AOS (kötü) — her brick ayrı allocation, cache unfriendly
pub struct Slab_AOS {
    pub slab_mask: u64,
    pub bricks: Vec<Brick>, // her Brick kendi Vec'lerine sahip
}

pub struct Brick_AOS {
    pub brick_mask: u64,
    pub sub_bricks: Vec<SubBrick>,
    pub mip_half: u64,
    pub mip_quarter: u64,
}

// SOA (iyi) — tüm veriler bitişik, SIMD ile işlenebilir
pub struct Slab {
    /// 64 brick'in doluluk bitmask'i.
    pub slab_mask: u64,

    /// Tüm brick bitmask'leri bitişik — SIMD popcnt ile paralel işlenebilir.
    /// 64-bit aligned, `wide` crate ile 4×64-bit aynı anda.
    pub brick_masks: Vec<u64>,

    /// Her brick'in sub-brick başlangıç offset'i (brick_masks'e göre index).
    pub sub_brick_offsets: Vec<u32>,

    /// Tüm sub-brick'ler tek allocation'da.
    pub sub_bricks: Vec<SubBrick>,

    /// Tüm materyaller bitişik — texture-like access pattern.
    pub materials: Vec<u16>,

    /// Tüm mip level'ları bitişik — SIMD popcnt ile paralel.
    pub mip_half: Vec<u64>,
    pub mip_quarter: Vec<u64>,
}
```

#### 3.6.2 SIMD Popcnt ile Paralel Bitmask İşleme

`wide` crate ile 4×64-bit bitmask aynı anda işlenir:

```rust
use wide::u64x4;

impl Slab {
    /// 4 brick'in popcnt'sini aynı anda hesapla (SIMD).
    /// Geleneksel: 4× count_ones() = 4 işlem
    /// SIMD: 1× u64x4 popcnt = 1 işlem
    #[inline]
    pub fn popcnt_4_bricks(&self, indices: [usize; 4]) -> [u32; 4] {
        let masks = u64x4::new([
            self.brick_masks[indices[0]],
            self.brick_masks[indices[1]],
            self.brick_masks[indices[2]],
            self.brick_masks[indices[3]],
        ]);

        // SIMD popcnt — tüm 64-bit değerler aynı anda
        let result = masks.count_ones();

        [result[0], result[1], result[2], result[3]]
    }

    /// Ray tracing için 4 brick'i aynı anda kontrol et.
    /// Space skipping'te 4× hız artışı.
    #[inline]
    pub fn check_4_bricks(&self, indices: [usize; 4]) -> u64x4 {
        let masks = u64x4::new([
            self.brick_masks[indices[0]],
            self.brick_masks[indices[1]],
            self.brick_masks[indices[2]],
            self.brick_masks[indices[3]],
        ]);
        masks
    }
}
```

#### 3.6.3 SOA Bellek Hesabı

| Bileşen | AOS (bytes) | SOA (bytes) | Fark |
|---|---|---|---|
| Slab header | 16 | 56 | +40 (pointer'lar) |
| Brick dizisi (64 brick) | 64 × 48 = 3072 | 64 × 8 = 512 (masks) | **-2560** |
| Sub-brick dizisi | 64 × 24 = 1536 | 64 × 8 = 512 | **-1024** |
| Materials | 512 × 2 = 1024 | 512 × 2 = 1024 | 0 |
| Mip levels | 64 × 16 = 1024 | 64 × 16 = 1024 | 0 |
| **Toplam (full slab)** | **~6672B** | **~3128B** | **-53%** |

**Ek avantajlar:**
- **Prefetcher friendly:** CPU hardware prefetcher ardışık bellek erişimlerini otomatik prefetch eder
- **Vectorization:** Derleyici otomatik SIMD üretebilir (auto-vectorization)
- **Cache line verimliliği:** 64-byte cache line'da daha fazla anlamlı veri

#### 3.6.4 Object Pooling

`Vec<Brick>`, `Vec<SubBrick>` için **object pool** — allocation/deallocation cost yok, GC churn yok:

```rust
use slotmap::SlotMap;

/// Object pool — brick ve sub-brick allocation'ları için.
pub struct BrickPool {
    /// Brick slot map — O(1) alloc/free, no fragmentation.
    bricks: SlotMap<BrickKey, BrickData>,

    /// Serbest liste — hızlı reuse.
    free_list: Vec<BrickKey>,
}

pub struct BrickData {
    pub brick_mask: u64,
    pub sub_brick_start: u32,
    pub sub_brick_count: u32,
    pub mip_half: u64,
    pub mip_quarter: u64,
}
```

---

## 4. SVDAG — Uzak Alan Veri Yapısı

### 4.1 Shared Node Pool

Tüm sektörlerin SVDAG'ları **tek bir global node havuzunu** paylaşır. Bu, aynı geometrinin (örn. düz zemin, su seviyesi) birden fazla sector'de **tek node** olarak saklanmasını sağlar.

```rust
/// Global SVDAG node havuzu.
/// Thread-safe: CPU tarafı parking_lot RwLock, GPU tarafı 32-bit atomic allocator.
pub struct SharedNodePool {
    /// Node verileri: [child_mask: u8, child_indices: [u32; 8]]
    nodes: Vec<SvdagNode>,

    /// Serbest slot listesi (GC sonrası yeniden kullanım).
    free_slots: Vec<u32>,

    /// Referans sayacı (her node kaç sector tarafından kullanılıyor).
    ref_counts: Vec<u32>,

    /// GPU buffer: atomic allocator için.
    /// 32-bit atomic<u32> kullanır (Metal'da 64-bit atomicAdd/CAS yok).
    gpu_free_head: wgpu::Buffer,

    /// Maksimum node kapasitesi.
    /// 256K node × 40B = ~10MB GPU buffer.
    capacity: u32,
}

/// Tek bir SVDAG node'u.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvdagNode {
    /// Hangi child'ların mevcut olduğu (8 bit).
    pub child_mask: u8,

    /// Child node index'leri. Sadece child_mask'te set olanlar geçerli.
    pub child_indices: [u32; 8],

    /// Leaf ise materyal ID'si, değilse 0xFFFF.
    pub material: u16,
}
```

**GPU Allocator (32-bit atomic):**

Metal'da `SHADER_INT64_ATOMIC_ALL_OPS` desteklenmediğinden, node pool allocator `atomic<u32>` kullanır. Node index'leri zaten u32'ye sığar:

```wgsl
// GPU node pool allocator
struct NodePool {
    free_head: atomic<u32>,       // Sonraki serbest slot
    capacity: u32,                // Maksimum node sayısı
    nodes: array<SvdagNode, 262144>, // 256K slot
}

/// Yeni node allocate et (lock-free, atomik).
fn node_alloc(pool: ptr<storage, NodePool>) -> u32 {
    let idx = atomicAdd(&pool.free_head, 1u);
    if (idx >= pool.capacity) {
        return 0xFFFFFFFFu; // Pool dolu
    }
    return idx;
}

/// Node'u serbest bırak (ref count sıfır olunca).
fn node_free(pool: ptr<storage, NodePool>, idx: u32) {
    // free_head'i atomik olarak geri çekme — GC pass'te toplu yapılır
    // Bireysel free: sadece ref count sıfırla, GC pass'te free_slots'a ekle
}
```

### 4.2 Node Bellek Hesabı

| Bileşen | Boyut |
|---|---|
| child_mask | 1 byte |
| child_indices | 32 byte (8 × u32) |
| material | 2 byte |
| Padding | 5 byte (16-byte alignment) |
| **Toplam** | **40 byte** |

32×128×32 sector için tipik SVDAG:
- Boş/homojen alan: **~8-12KB** (çok az node)
- Karmaşık arazi: **~25-40KB**
- Deduplication ile: **%20-30 ek tasarruf** (global paylaşım)

**GPU Node Pool Kapasitesi:**
- 256K node × 40B = **~10MB** GPU buffer
- 256K node, yüzlerce sector'ün SVDAG'ını barındırmaya yeterli (deduplication ile)

### 4.3 Brick → SVDAG Bake (GPU Compute)

```rust
/// Brickmap'ten SVDAG'e dönüşüm (GPU compute shader ile).
/// GPU-SVDAG-Editing (Pacific Graphs 2024) yaklaşımı.
pub struct SvdagBaker {
    /// GPU ring buffer: pending editler.
    edit_buffer: GpuRingBuffer<EditOp>,

    /// GPU hash table: SVDAG node lookup.
    hash_table: GpuHashTable,

    /// Node pool: global SVDAG node havuzu.
    node_pool: GpuNodePool,
}

/// Tek bir voxel düzenleme işlemi.
#[repr(C)]
pub struct EditOp {
    pub sector: IVec3,
    pub pos: IVec3,
    pub old_material: u16,
    pub new_material: u16,
}
```

**Bake Pipeline:**

```
1. Brickmap'ten voxel array çıkar (CPU → GPU upload, ~2ms)
2. GPU compute: Geçici SVO oluştur (bottom-up, ~5ms)
3. GPU compute: Mevcut SVDAG'e merge et (HashDAG algorithm, ~5ms)
4. GPU compute: Duplicate node'ları temizle (hash-based, ~3ms)
5. CPU: Node pool'dan root index al, sector'a ata
   ⚠️ GPU → CPU readback (map_read_async) pipeline stall yaratır
   → Çözüm: Readback'ı frame sonuna schedule et, sonraki frame'de result'ı al
6. Eski node'ların ref count'unu azalt (GC için işaretle)
```

**Toplam süre: ~15ms** (CPU'daki 200ms'lik süreye kıyasla)

**Pipeline Stall Mitigasyonu:**
- GPU compute dispatch'leri async olarak başlat
- `map_read_async` ile readback'ı frame sonuna planla
- Sonucu sonraki frame'de işle (1 frame gecikme kabul edilebilir, bake zaten arka planda)
- Kademeli bake: Her frame sadece M sector'ün bake'ını başlat

### 4.5 Transform-Aware Deduplication (SIGGRAPH 2025)

Mevcut SVDAG sadece **birebir aynı** geometriyi dedup eder. **Transform-Aware SVDAG** (Molenaar & Eisemann, SIGGRAPH 2025) simetri ve dönüşümleri kullanarak ek **%20-45** tasarruf sağlar.

#### 4.5.1 Simetri Tipleri

| Simetri | Açıklama | Tasarruf |
|---|---|---|
| **Mirror X/Y/Z** | Eksenlerde ayna | %10-15 |
| **Rotation 90°/180°/270°** | Y ekseni etrafında dönüş | %10-20 |
| **Translation** | Öteleme ile eşleştirme | %5-10 |
| **Kombinasyonlar** | Mirror + Rotation | %20-45 (kümülatif) |

#### 4.5.2 Transform-Aware Node Yapısı

```rust
/// SVDAG node'daki geometrinin transform bilgisi.
/// Deduplication sırasında bu transform'lar dikkate alınır.
#[repr(u8)]
pub enum SvdagTransform {
    Identity = 0,
    MirrorX = 1,
    MirrorY = 2,
    MirrorZ = 3,
    MirrorXY = 4,
    MirrorXZ = 5,
    MirrorYZ = 6,
    MirrorXYZ = 7,
    Rotate90 = 8,
    Rotate180 = 9,
    Rotate270 = 10,
    Rotate90MirrorX = 11,
    Rotate180MirrorX = 12,
    Rotate270MirrorX = 13,
    // ... toplam 48 kombinasyon (octahedral symmetry group)
}

/// Transform-aware SVDAG node.
pub struct SvdagNode {
    /// Hangi child'ların mevcut olduğu (8 bit).
    pub child_mask: u8,

    /// Child node index'leri.
    pub child_indices: [u32; 8],

    /// Leaf ise materyal ID'si, değilse 0xFFFF.
    pub material: u16,

    /// Bu node'un transform'u (deduplication için).
    /// Aynı geometri farklı transform'larla paylaşılabilir.
    pub transform: SvdagTransform,
}
```

#### 4.5.3 Transform-Aware Hash Lookup

```rust
/// Transform-aware SVDAG hash table.
/// Aynı geometriyi farklı transform'larla eşleştirir.
pub struct TransformAwareHashTable {
    /// Normal hash: geometry_hash → node_index
    normal_map: HashMap<u64, u32>,

    /// Transform hash: (geometry_hash, transform) → node_index
    /// Aynı geometri 48 farklı transform'la aranır.
    transform_map: HashMap<(u64, SvdagTransform), u32>,
}

impl TransformAwareHashTable {
    /// Geometriyi tüm transform'larla ara.
    /// İlk eşleşmeyi döndür — en az transform maliyetli olanı tercih et.
    pub fn lookup_with_transforms(&self, geometry_hash: u64) -> Option<(u32, SvdagTransform)> {
        // 1. Önce identity dene (en ucuz)
        if let Some(&idx) = self.normal_map.get(&geometry_hash) {
            return Some((idx, SvdagTransform::Identity));
        }

        // 2. Transform'ları sırayla dene (maliyet sırasına göre)
        let transform_order = [
            SvdagTransform::MirrorX,
            SvdagTransform::MirrorY,
            SvdagTransform::MirrorZ,
            SvdagTransform::Rotate90,
            SvdagTransform::Rotate180,
            SvdagTransform::Rotate270,
            // ... diğerleri
        ];

        for transform in transform_order {
            if let Some(&idx) = self.transform_map.get(&(geometry_hash, transform)) {
                return Some((idx, transform));
            }
        }

        None
    }

    /// Yeni geometriyi tüm transform'larıyla kaydet.
    pub fn insert_with_all_transforms(&mut self, geometry_hash: u64, node_index: u32) {
        self.normal_map.insert(geometry_hash, node_index);

        // Tüm transform varyantlarını kaydet
        for transform in SvdagTransform::all() {
            let transformed_hash = Self::compute_transformed_hash(geometry_hash, transform);
            self.transform_map.insert((transformed_hash, transform), node_index);
        }
    }
}
```

#### 4.5.4 GPU Ray March'te Transform Uygulama

```wgsl
/// SVDAG ray march — transform-aware node traversal.
/// Node'un transform'u biliniyorsa, ray'i transform et ve normal node'dan devam et.
fn svdag_ray_march_transformed(
    ray: Ray,
    node: SvdagNode,
    transform: SvdagTransform,
) -> HitResult {
    // 1. Ray'i ters transform et (world → node local)
    let local_ray = apply_inverse_transform(ray, transform);

    // 2. Normal node traversal (transform uygulanmamış gibi)
    let hit = svdag_ray_march_normal(local_ray, node);

    // 3. Hit normal'ini forward transform et (local → world)
    let world_normal = apply_forward_transform(hit.normal, transform);

    return HitResult {
        t: hit.t,
        position: hit.position,
        normal: world_normal,
        material: hit.material,
    };
}
```

**Performans:** Transform lookup O(1) (hash table), transform uygulama O(1) (lookup table). Ek maliyet minimal, tasarruf **%20-45** bellek.

---

### 4.6 Shallow SVDAG Streaming (Aokana, SIGGRAPH 2025)

### 4.6 Shallow SVDAG Streaming (Aokana, SIGGRAPH 2025)

Derin SVDAG traversal = çoklu indirect jump = GPU cache miss. **Aokana** yaklaşımı (Fang et al., SIGGRAPH 2025) bu sorunu **sığ SVDAG'lar + streaming** ile çözer.

#### 4.6.1 Temel Fikir

| Özellik | Derin SVDAG (mevcut) | Shallow SVDAG (Aokana) |
|---|---|---|
| **Max depth** | 8-12 level | 4-5 level |
| **Traversal** | Çoklu indirect jump | Az indirect jump, daha fazla linear |
| **VRAM kullanımı** | Tüm sahne | Sadece **%5** (view-dependent) |
| **32K+ çözünürlük** | Yavaş (cache miss) | **2-4× daha hızlı** |
| **Streaming** | Yok | View-dependent, LOD bazlı |

#### 4.6.2 Shallow SVDAG Yapısı

```rust
/// Shallow SVDAG — max depth 4-5, streaming-friendly.
pub struct ShallowSvdag {
    /// Root node'lar (birden fazla sığ ağaç).
    /// Her root, bir "tile"ı temsil eder.
    roots: Vec<ShallowSvdagRoot>,

    /// Node pool — tüm shallow SVDAG'lar için ortak.
    node_pool: SharedNodePool,

    /// Streaming state — hangi tile'lar yüklü.
    streaming_state: SvdagStreamingState,
}

/// Tek bir shallow SVDAG root'u (bir tile).
pub struct ShallowSvdagRoot {
    /// Tile koordinatı (dünya uzayında).
    pub tile_coord: IVec3,

    /// Root node index (node pool'da).
    pub root_index: u32,

    /// Bu tile'ın LOD seviyesi.
    pub lod_level: u8,

    /// Yüklü mü? (streaming state).
    pub loaded: bool,

    /// Öncelik skoru (streaming priority).
    pub priority: f32,
}
```

#### 4.6.3 View-Dependent Streaming

```rust
/// Shallow SVDAG streaming manager.
/// Sadece görünür tile'ları yükler, geri kalanı disk'te tutar.
pub struct SvdagStreamingManager {
    /// Yüklü tile'lar.
    loaded_tiles: HashMap<IVec3, ShallowSvdagRoot>,

    /// Bekleyen yükleme kuyruğu (öncelik sırası).
    load_queue: PriorityQueue<IVec3, f32>,

    /// Disk'teki tile index'i.
    disk_index: SvdagDiskIndex,
}

impl SvdagStreamingManager {
    /// Her frame çağrılır — görünür tile'ları belirle.
    pub fn update(&mut self, camera: &Camera, frustum: &Frustum) {
        // 1. Frustum'daki tile'ları belirle
        let visible_tiles = self.frustum_query(frustum);

        // 2. Öncelik hesapla (mesafe + görüş yönü)
        for tile in visible_tiles {
            let priority = self.compute_priority(tile, camera);
            self.load_queue.push(tile, priority);
        }

        // 3. Yükleme kuyruğundan tile'ları yükle (budget içinde)
        self.load_tiles_from_queue(Budget::VRAM_5_PERCENT);

        // 4. Görünmez tile'ları unload et
        self.unload_invisible_tiles();
    }

    /// VRAM budget: sahnenin sadece %5'i yüklü.
    const VRAM_BUDGET: f32 = 0.05;
}
```

#### 4.6.4 GPU Ray March ile Entegrasyon

```wgsl
// Shallow SVDAG ray march — streaming-friendly.
// Sadece yüklü tile'lar için ray march yapılır.
@compute @workgroup_size(64)
fn shallow_svgdag_ray_march(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let ray = camera_get_ray(pixel);

    // 1. Ray'in geçtiği tile'ları belirle
    let tile = compute_tile_from_ray(ray);

    // 2. Tile yüklü mü kontrol et
    if (!is_tile_loaded(tile)) {
        // Tile yüklü değil — fallback (düşük LOD veya skip)
        let fallback_result = ray_march_fallback(ray, tile);
        visibility_buffer_write(pixel, fallback_result);
        return;
    }

    // 3. Tile yüklü — shallow SVDAG traversal (max 4-5 level)
    let root = get_tile_root(tile);
    let hit = svdag_ray_march_shallow(ray, root);

    visibility_buffer_write(pixel, hit);
}
```

**Performans (Aokana sonuçları):**
- **4.8× hız artışı** (derin SVDAG'e kıyasla)
- **9× VRAM azalması** (sadece %5 yüklü)
- **32K+ çözünürlük** HashDAG'den 2-4× daha hızlı
- **Streaming overhead:** <1ms/frame (async load)

---

### 4.7 SVDAG → Brick Unbake

Oyuncu bir sector'e yaklaştığında:

```
1. SVDAG root node'dan başla
2. GPU compute: SVDAG → voxel array (top-down traversal, ~3ms)
3. CPU: Voxel array → Brickmap (bitmask + materials, ~2ms)
4. GPU: Node pool'dan ref count azalt
5. Sector.dirty = false
```

**Toplam süre: ~5ms**

---

## 5. 4-Tier Streaming Sistemi

### 5.1 Kademe Tanımları

| Tier | Ad | Mesafe | Veri Formatı | Render | Fizik |
|---|---|---|---|---|---|
| **1** | ACTIVE | 0-96m (~3 sector) | XBrickMap | Ray trace / Greedy mesh | Rapier Voxels collider |
| **2** | WARM | 96-384m (~3-12 sector) | XBrickMap + SVDAG | Brick öncelikli, SVDAG fallback | Rapier Voxels collider |
| **3** | DISTANT | 384m-1.5km | SVDAG only | GPU ray march | Yaklaşık collider |
| **4** | ARCHIVE | 1.5km+ | Compressed SVDAG (disk) | Render edilmez | Yok |

**Mesafe bazları:** Sector köşegeni ~132m. Tier 1 = 3×3×3 sector (yakın, düzenlenebilir). Tier 2 = yumuşak geçiş bölgesi.

### 5.2 Tier Geçiş Kuralları

```rust
/// Bir sector'un tier'ını belirle.
pub fn determine_tier(sector_pos: IVec3, camera: &Camera) -> Tier {
    let dist = (sector_pos - camera.position).length();

    if dist < 96.0 {
        Tier::Active
    } else if dist < 384.0 {
        Tier::Warm
    } else if dist < 1536.0 {
        Tier::Distant
    } else {
        Tier::Archive
    }
}
```

### 5.3 Yumuşak Geçiş (Tier 2)

Tier 2'de **her iki representation birlikte** bulunur. Bu, pop-in'ı tamamen ortadan kaldırır:

```
Oyuncu uzaklaşıyor:
  Tier 1 → Tier 2:
    1. Brickmap hâlâ aktif (render + fizik)
    2. Arka planda GPU bake başlat (Brick → SVDAG)
    3. Bake bitti → sector.svdag_root = Some(root_index)
    4. Sector artık Tier 2'ye geçti

Oyuncu daha da uzaklaşıyor:
  Tier 2 → Tier 3:
    1. Brickmap bellekten serbest bırak
    2. SVDAG aktif (render)
    3. Fizik: SVDAG'den yaklaşık collider oluştur

Oyuncu yaklaşıyor:
  Tier 3 → Tier 2:
    1. SVDAG → Brickmap unbake (arka plan)
    2. Unbake bitti → her iki representation mevcut
    3. Brickmap aktif (render + fizik)

  Tier 2 → Tier 1:
    1. SVDAG bellekten serbest bırak (ref count azalt)
    2. Sadece Brickmap kalır
```

### 5.4 Predictive Streaming

```rust
/// Oyuncunun hareketine göre sector önceliklerini hesapla.
pub struct StreamingPredictor {
    velocity: Vec3,
    acceleration: Vec3,
    look_direction: Vec3,
}

impl StreamingPredictor {
    /// 2 saniye sonraki tahmini pozisyon.
    pub fn predict_position(&self, current: Vec3) -> Vec3 {
        current + self.velocity * 2.0 + self.acceleration * 1.0
    }

    /// Öncelikli sector'ları belirle (yükleme sırası).
    pub fn priority_sectors(&self, current: IVec3) -> Vec<(SectorCoord, f32)> {
        let predicted = self.predict_position(current.as_vec3());
        let predicted_sector = SectorCoord::from_world(predicted.as_ivec3());

        // Bakış yönündeki sector'lara öncelik ver
        let mut sectors = Vec::new();
        for offset in SECTOR_RADIUS.iter() {
            let candidate = predicted_sector.0 + offset;
            let to_candidate = (candidate - predicted_sector.0).as_vec3().normalize();
            let alignment = to_candidate.dot(self.look_direction);

            // Mesafe + bakış yönüne göre skor
            let dist = (candidate - predicted_sector.0).length();
            let score = alignment * 0.6 + (1.0 - dist / MAX_RADIUS) * 0.4;

            sectors.push((SectorCoord(candidate), score));
        }

        sectors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        sectors
    }
}
```

---

## 6. Unified Visibility Buffer Render Pipeline

### 6.1 Genel Akış

Tüm tier'lar **aynı 64-bit visibility buffer'a** yazar. Bu, iki farklı render pipeline'ını tek bir shading pass'inde birleştirir.

```
┌──────────────────────────────────────────────────────────────┐
│                    RENDER FRAME                               │
├──────────────────────────────────────────────────────────────┤
│ Pass 1: Frustum Culling (GPU Compute)                        │
│   → Görünür sector'ları belirle, tier'lara göre sınıfla      │
│   → Çıktı: sector_list (buffer)                              │
├──────────────────────────────────────────────────────────────┤
│ Pass 2: Tier 1 — XBrickMap Ray Trace (GPU Compute)           │
│   → 4-level bitmask space skipping                           │
│   → Visibility buffer'a yaz (depth + normal + sector + voxel)│
├──────────────────────────────────────────────────────────────┤
│ Pass 3: Tier 2 — XBrickMap + SVDAG (GPU Compute)             │
│   → Brick varsa brick'ten, yoksa SVDAG'den                   │
│   → Aynı visibility buffer'a (depth test otomatik)           │
├──────────────────────────────────────────────────────────────┤
│ Pass 4: Tier 3 — SVDAG Ray March (GPU Compute)               │
│   → Hi-Z occlusion culling ile                               │
│   → Aynı visibility buffer'a                                 │
├──────────────────────────────────────────────────────────────┤
│ Pass 5: Color Resolve (GPU Compute)                          │
│   → Visibility buffer'dan tüm pikselleri tek seferde shade et│
│   → G-buffer → final frame buffer                            │
├──────────────────────────────────────────────────────────────┤
│ Pass 6: Build Hi-Z (GPU Compute)                             │
│   → Depth buffer'dan hierarchical Z-buffer oluştur           │
│   → Sonraki frame occlusion culling için                     │
└──────────────────────────────────────────────────────────────┘
```

### 6.2 Visibility Buffer Layout (64-bit)

| Bit Aralığı | İçerik | Açıklama |
|---|---|---|
| 0-23 (24 bit) | Depth | Z-depth, 16M+ hassasiyet |
| 24-26 (3 bit) | Normal | Axis-aligned normal (X+/X-/Y+/Y-/Z+/Z-) |
| 27-39 (13 bit) | Sector ID | Hangi sector'den geldi |
| 40-63 (24 bit) | Voxel Pos | Voxel koordinatı (sector içinde) |

#### WGSL 64-bit Atomik Stratejisi

Visibility buffer depth test için `atomicMin` gerekir. wgpu'da 64-bit atomik desteği platforma göre değişir:

| Platform | Feature | Depth Write |
|---|---|---|
| Vulkan (VK_KHR_shader_atomic_int64) | `SHADER_INT64_ATOMIC_ALL_OPS` | `atomic<u64>` native |
| DX12 (SM 6.6+) | `SHADER_INT64_ATOMIC_ALL_OPS` | `atomic<u64>` native |
| Metal (Apple8+) | `SHADER_INT64_ATOMIC_MIN_MAX` | `atomic<vec2<u32>>` + `atomicStoreMin` |

**Karar:** Device oluşturulurken feature check yapılır. `SHADER_INT64_ATOMIC_ALL_OPS` varsa native `atomic<u64>` kullanılır. Yoksa `atomic<vec2<u32>>` + `atomicStoreMin` fallback'e düşülür. Depth test için return value gerekmez — sadece minimum depth'u yazmak yeterlidir.

```rust
// Rust tarafında feature check
let use_native_u64 = adapter.features().contains(
    wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS
);

let required_features = if use_native_u64 {
    wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS
} else {
    wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX
};
```

```wgsl
// Path A: Native u64 atomic (Vulkan, DX12)
#ifdef NATIVE_U64_ATOMIC
struct VisibilityEntry {
    depth_and_normal: u64,  // 24-bit depth + 3-bit normal + padding
    sector_and_voxel: u64,  // 13-bit sector + 24-bit voxel + padding
}

fn visibility_depth_write(entry: ptr<storage, atomic<u64>, read_write>, new_depth: u32) {
    atomicMin(entry, u64(new_depth));
}

// Path B: vec2<u32> fallback (Metal)
#else
struct VisibilityEntry {
    depth_and_normal: vec2<u32>,  // [0]: 24-bit depth + padding, [1]: normal + sector
    sector_and_voxel: vec2<u32>,  // [0]: 13-bit sector + padding, [1]: 24-bit voxel
}

fn visibility_depth_write(entry: ptr<storage, atomic<vec2<u32>>, read_write>, new_depth: u32) {
    // Sadece depth component'ını güncelle (vec2'nin ilk elemanı)
    let packed = vec2<u32>(new_depth, 0u);
    atomicStoreMin(entry, packed);
}
#endif
```

**Tradeoff:** `vec2<u32>` fallback'te `atomicStoreMin` return value vermez — ama visibility buffer için bu sorun değil. Eski depth değerini okumak gerekmez, sadece yeni depth daha yakınsa yazmak yeterlidir.

#### SVDAG Node Pool Allocator

Node pool allocator `atomicAdd`/`atomicCAS` gerektirir. Bu operasyonlar Metal'da 64-bit için mevcut değildir. **Çözüm:** Allocator 32-bit `atomic<u32>` index'ler kullanır — node index'leri zaten u32'ye sığar (4B × 256M node = 1GB max pool).

```wgsl
// Node pool allocator — her zaman 32-bit atomic
struct NodePool {
    free_list_head: atomic<u32>,  // Serbest slot index'i
    nodes: array<SvdagNode, 262144>, // 256K node = ~10MB
}

fn alloc_node(pool: ptr<storage, NodePool>) -> u32 {
    return atomicAdd(&pool.free_list_head, 1u);
}
```

### 6.3 Hi-Z Occlusion Culling

```wgsl
// Pass 4: SVDAG ray march — Hi-Z ile gereksiz ray'leri atla
@compute @workgroup_size(8, 8)
fn svdag_ray_march_hiz(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let tile = id.xy;
    let tile_depth = hiz_buffer_load(tile);

    // Eğer bu tile zaten tamamen kapalıysa, ray march yapma
    if (tile_depth == MAX_DEPTH) {
        return;
    }

    // Tile'ın ortalama derinliğine göre LOD seç
    let lod_level = select_lod(tile_depth);

    // SVDAG ray march — sadece görünür tile'lar için
    svdag_ray_march(tile, lod_level);
}
```

---

### 6.4 Vertex Pooling (Nick McDonald)

Her sector için ayrı mesh buffer = yüksek driver overhead, sık VBO recreate. **Vertex Pool** yaklaşımı ile tüm mesh'ler tek büyük buffer'da yönetilir.

#### 6.4.1 Temel Yapı

```rust
/// Global vertex pool — tüm sector mesh'leri tek buffer'da.
/// Mesh rebuild = sadece pool'a yaz (VBO recreate yok).
pub struct VertexPool {
    /// GPU vertex buffer (büyük, önceden allocate).
    gpu_buffer: wgpu::Buffer,

    /// CPU staging buffer (meshing sonuçları buraya yazılır).
    staging_buffer: Vec<u8>,

    /// Her sector'ün pool'daki slice'ı.
    sector_slices: HashMap<SectorCoord, VertexSlice>,

    /// Serbest alan yönetimi (bump allocator + free list).
    allocator: PoolAllocator,

    /// Toplam kapasite (vertex cinsinden).
    capacity: u32,
}

/// Bir sector'ün vertex pool'daki yeri.
pub struct VertexSlice {
    /// Başlangıç offset (vertex cinsinden).
    pub offset: u32,

    /// Vertex sayısı.
    pub count: u32,

    /// Index buffer offset (ayrı pool).
    pub index_offset: u32,
    pub index_count: u32,

    /// Bu slice'ın versiyonu (mesh rebuild'de artar).
    pub version: u32,
}
```

#### 6.4.2 Pool Allocator

```rust
/// Vertex pool allocator — bump allocator + free list hybrid.
pub struct PoolAllocator {
    /// Bump pointer — sonraki serbest offset.
    bump_offset: u32,

    /// Serbest slice'lar (eski mesh'lerden).
    free_list: Vec<(u32, u32)>, // (offset, count)

    /// High water mark (en yüksek kullanılan offset).
    high_water_mark: u32,
}

impl PoolAllocator {
    /// Yeni vertex slice allocate et.
    pub fn alloc(&mut self, vertex_count: u32) -> Option<u32> {
        // 1. Free list'den uygun yer ara (first fit)
        for (i, &(offset, count)) in self.free_list.iter().enumerate() {
            if count >= vertex_count {
                self.free_list.remove(i);
                // Kalan kısmı free list'e geri koy
                if count > vertex_count {
                    self.free_list.push((offset + vertex_count, count - vertex_count));
                }
                return Some(offset);
            }
        }

        // 2. Free list'de yer yok — bump allocator
        if self.bump_offset + vertex_count <= self.capacity {
            let offset = self.bump_offset;
            self.bump_offset += vertex_count;
            self.high_water_mark = self.high_water_mark.max(self.bump_offset);
            return Some(offset);
        }

        None // Pool dolu
    }

    /// Bir slice'ı serbest bırak.
    pub fn free(&mut self, offset: u32, count: u32) {
        self.free_list.push((offset, count));
        // Free list'i merge et (bitişik slice'ları birleştir)
        self.merge_free_list();
    }
}
```

#### 6.4.3 Mesh Rebuild Pipeline

```rust
impl VertexPool {
    /// Bir sector'ün mesh'ini rebuild et.
    pub fn rebuild_sector_mesh(
        &mut self,
        queue: &wgpu::Queue,
        sector: &SectorCoord,
        mesh_data: &MeshData,
    ) {
        // 1. Eski slice'ı serbest bırak
        if let Some(old_slice) = self.sector_slices.remove(sector) {
            self.allocator.free(old_slice.offset, old_slice.count);
        }

        // 2. Yeni slice allocate et
        let offset = self.allocator.alloc(mesh_data.vertex_count);
        let slice = VertexSlice {
            offset,
            count: mesh_data.vertex_count,
            index_offset: 0, // index pool'dan
            index_count: mesh_data.index_count,
            version: old_slice.version.map_or(0, |v| v + 1),
        };

        // 3. Staging buffer'a yaz
        self.write_to_staging(&slice, mesh_data);

        // 4. GPU'ya upload (buffer copy)
        queue.write_buffer(
            &self.gpu_buffer,
            slice.offset as u64 * std::mem::size_of::<Vertex>() as u64,
            &self.staging_buffer,
        );

        self.sector_slices.insert(*sector, slice);
    }
}
```

#### 6.4.4 Performans

| Metrik | Ayrı VBO | Vertex Pool | Fark |
|---|---|---|---|
| **Frame time** | 16.7ms | **10.0ms** | **-40%** |
| **Meshing time** | 8.3ms | **6.2ms** | **-25%** |
| **Driver overhead** | Yüksek (her sector ayrı) | Düşük (tek buffer) | **-60%** |
| **GPU memory** | Fragmented | Contiguous | **-15%** |
| **Rebuild cost** | VBO create + upload | Sadece upload | **-50%** |

---

### 6.5 Foveated Rendering

İnsan gözünün **peripheral vision** sınırlarını kullanarak render maliyetini düşürür. Merkez (fovea) tam çözünürlük, kenarlar düşük çözünürlük.

#### 6.5.1 Fovea Bölgeleri

```rust
/// Foveated rendering konfigürasyonu.
pub struct FoveatedConfig {
    /// Fovea merkezi (ekran koordinatlarında, normalized 0-1).
    pub fovea_center: Vec2,

    /// Fovea yarıçapı (ekranın yüzde kaçı tam çözünürlük).
    pub fovea_radius: f32, // 0.1 = ekranın %10'u

    /// Orta bölge yarıçapı.
    pub mid_radius: f32, // 0.3 = ekranın %30'u

    /// Bölge çözünürlük oranları.
    pub fovea_scale: f32,    // 1.0 (tam çözünürlük)
    pub mid_scale: f32,      // 0.5 (yarım çözünürlük)
    pub peripheral_scale: f32, // 0.25 (çeyrek çözünürlük)
}
```

| Bölge | Çözünürlük | Kapsam | Ray/Pixel Oranı |
|---|---|---|---|
| **Fovea** | 1.0× (tam) | Merkez %10 | 1.0× |
| **Orta** | 0.5× (yarım) | %10-30 | 0.25× |
| **Periferik** | 0.25× (çeyrek) | %30-100 | 0.0625× |

#### 6.5.2 GPU Compute Entegrasyonu

```wgsl
// Foveated ray march — bölgeye göre LOD seç.
@compute @workgroup_size(8, 8)
fn foveated_ray_march(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let screen_size = vec2f(textureDimensions(visibility_buffer));

    // 1. Pikselin fovea'ya uzaklığını hesapla
    let pixel_norm = vec2f(pixel) / screen_size;
    let dist_to_fovea = distance(pixel_norm, fovea_center);

    // 2. Bölgeye göre ray step size belirle
    var step_multiplier: f32;
    if (dist_to_fovea < fovea_radius) {
        step_multiplier = 1.0; // Tam çözünürlük
    } else if (dist_to_fovea < mid_radius) {
        step_multiplier = 2.0; // Yarım çözünürlük (2× step)
    } else {
        step_multiplier = 4.0; // Çeyrek çözünürlük (4× step)
    }

    // 3. Ray march — adaptive step size ile
    let ray = camera_get_ray(pixel);
    let hit = ray_march_adaptive(ray, step_multiplier);

    visibility_buffer_write(pixel, hit);
}
```

#### 6.5.3 Peripheral Animasyon Durdurma

SIGGRAPH 2025 çalışması: **Periferik bölgedeki animasyonları durdurmak** ek **%99.3** azalma sağlar (insan gözü peripheral'de hareket algılamada zayıftır, özellikle düşük kontrastta).

```rust
/// Foveated animation throttling.
pub struct FoveatedAnimationController {
    /// Animasyonlu entity'ler.
    animated_entities: Vec<AnimatedEntity>,
}

impl FoveatedAnimationController {
    /// Her entity'nin animasyon frekansını fovea uzaklığına göre ayarla.
    pub fn update(&mut self, fovea_center: Vec2, screen_positions: &HashMap<Entity, Vec2>) {
        for entity in &mut self.animated_entities {
            let screen_pos = screen_positions[&entity.id];
            let dist = distance(screen_pos, fovea_center);

            // Animasyon update frekansı
            entity.update_hz = if dist < 0.1 {
                60.0 // Fovea: her frame
            } else if dist < 0.3 {
                20.0 // Orta: her 3 frame
            } else {
                5.0 // Periferik: her 12 frame
            };
        }
    }
}
```

#### 6.5.4 Performans

| Metrik | Uniform Rendering | Foveated | Azalma |
|---|---|---|---|
| **Ray/pixel sayısı** | 1.0× | **0.2-0.4×** | **-60-80%** |
| **Animasyon update** | 60Hz tüm ekran | **Adaptive** | **-99.3%** (periferik) |
| **Frame time** | 16.7ms | **6-10ms** | **-40-65%** |
| **GPU power** | 100% | **30-50%** | **-50-70%** |

---

## 7. Fizik Entegrasyonu (Rapier + Custom)

### 7.1 Rapier Voxels Shape — Güncel API

**Versiyon:** `rapier3d 0.32+` / `parry3d 0.26+` / `bevy_rapier3d 0.30+`

Rapier, **genel amaçlı rigid-body fizik motorları arasında** açıkça voxel destekleyen **ilk** motordur (parry#336). Dedicated voxel shape, şu avantajları sağlar:

| Avantaj | Açıklama |
|---|---|
| **Düşük bellek** | Her voxel ~1 byte (neighborhood info) |
| **Ghost collision yok** | Internal edge tracking — komşu voxel yüzeyleri "internal" olarak işaretlenir, kayma sırasında takılma olmaz |
| **Otomatik blok gruplama** | Bitmask-based neighbor lookup, O(1) |
| **Sparse storage** | Boş bölgeler minimum bellek kaplar |
| **Incremental edit** | `set_voxel()` + `propagate_voxel_change()` |

#### 7.1.1 Temel API

```rust
use bevy_rapier3d::prelude::*;
use glam::{IVec3, Vec3};

/// XBrickMap'ten Rapier Voxels collider oluştur.
/// Sector'ün dolu voxel'lerini topla ve sparse collider'a dönüştür.
pub fn sector_to_voxels(sector: &Sector, voxel_size: Vec3) -> Collider {
    let occupied: Vec<IVec3> = sector
        .iter_occupied()
        .map(|p| IVec3::new(p.x, p.y, p.z))
        .collect();

    ColliderBuilder::voxels(voxel_size, &occupied).build()
}
```

#### 7.1.2 VoxelState ve VoxelType (Parry 0.26)

Parry'ın `Voxels` shape'i her voxel için **neighborhood bilgisi** tutar:

```rust
/// Rapier'ın sağladığı voxel sınıflandırması.
/// Her voxel, komşularına göre otomatik sınıflandırılır.
pub enum VoxelType {
    /// Tüm 6 komşu dolu — internal face, collision check atlanır
    Internal,
    /// 1-5 komşu dolu — surface voxel, collision check gerekli
    Surface,
    /// Köşe veya kenar — detaylı collision check
    Feature,
}

/// Voxel durumu — dolu/boş + neighborhood bilgisi.
pub struct VoxelState {
    pub filled: bool,
    pub free_faces: u8, // 6 bit — hangi yüzler açık
    pub voxel_type: VoxelType,
}
```

**Ghost Collision Önleme:**

Rapier'ın Voxels shape'i, **internal edge** problemini otomatik çözer. İki komşu voxel arasındaki yüzey "internal" olarak işaretlenir ve collision detection bu yüzeyi yok sayar. Bu, karakterin düz zeminde kayarken takılmasını önler.

#### 7.1.3 Desteklenen İşlemler

| İşlem | API | Kompleksite |
|---|---|---|
| Voxel ekle/kaldır | `set_voxel(key, is_filled)` | O(log N) |
| Neighborhood güncelle | `propagate_voxel_change()` | O(1) lokal |
| Voxel durumu sorgula | `voxel_state(key)` | O(log N) |
| AABB'deki voxel'ler | `voxels_intersecting_local_aabb()` | O(K) |
| Mesh'e dönüştür | `to_trimesh()` | O(N) |
| Outline'a dönüştür | `to_outline()` | O(N) |
| Bölge kırp | `crop(mins, maxs)` | O(N) |
| AABB'ye böl | `split_with_box(aabb)` | O(N) |

#### 7.1.4 Gerçek Sınırlamalar (2026 Ocak itibarıyla)

| Özellik | Durum | Not |
|---|---|---|
| Static kinematic collider | ✅ Tam destek | Terrain için ideal |
| Dynamic rigid-body | ⚠️ Kısmi | Mass/inertia manuel hesaplanmalı |
| Voxels vs Capsule/Ball/Cuboid | ✅ Tam destek | Oyuncu vs terrain |
| Voxels vs Voxels | ⚠️ Parry 0.26'da düzeltildi | Force calculation arası çalışıyor |
| Voxels vs TriMesh | ✅ Çalışıyor | |
| Shape-casting (CCD) | ❌ Desteklenmiyor | |
| `set_voxel()` incremental edit | ✅ Çalışıyor | |
| `propagate_voxel_change()` | ✅ Çalışıyor | Neighbor sync için |
| `combine_voxel_states()` | ✅ Çalışıyor | Sector boundary merge |

**Strateji:** Aktif alan (Tier 1/2) için Rapier Voxels kullan. Oyuncu vs terrain için Voxels vs Capsule yeterli. Voxel vs Voxel (düşen kum vs zemin) Parry 0.26'da düzeltildi ama production-ready değil — bu durumlar için **custom physics layer** (Bölüm 7.5) kullan.

---

### 7.2 Broad-Phase Acceleration

Rapier 0.27.0'dan itibaren broad-phase **Dynamic BVH** tabanlı (parry#361). Bu, eski Hierarchical Sweep-and-Prune algoritmasını tamamen değiştirdi.

#### 7.2.1 Yeni BVH Özellikleri

```rust
/// Rapier'ın yeni BVH broad-phase'i.
/// SIMD-accelerated tree traversal + otomatik rebalancing.
pub struct BvhBroadPhase {
    /// Dynamic AABB tree — collider'ların hiyerarşik düzeni.
    tree: Qbvh<ColliderHandle>,

    /// Query pipeline — BVH'den türetilir, ayrı update gerektirmez.
    query_pipeline: QueryPipeline,
}
```

**Avantajlar:**
- **SIMD-accelerated traversal** — `wide` crate ile vectorized
- **Otomatik rebalancing** — collider hareket ettiğinde tree kendini dengeler
- **Tek acceleration structure** — broad-phase + scene queries aynı BVH'yi kullanır
- **Persistent islands** (rapier#895) — simulation islands frame'ler arası persist olur, connected component re-extraction maliyeti yok

#### 7.2.2 Sector-Level Spatial Hashing

BVH'ye ek olarak, **sector bazlı spatial hashing** ile geniş alan query'leri optimize edilir:

```rust
/// Sector bazlı spatial hash grid.
/// BVH broad-phase'i complement eder — hangi sector'ların fizik update'i
/// gerektiğini O(1) belirler.
pub struct PhysicsSpatialHash {
    /// Hücre boyutu = 1 sector (32×128×32 voxel).
    cell_size: IVec3,

    /// Sector → fizik entity listesi.
    cells: HashMap<SectorCoord, Vec<Entity>>,

    /// Aktif collision pair'leri (persistent islands'dan türetilir).
    active_pairs: HashSet<(Entity, Entity)>,
}
```

#### 7.2.3 Tier-Bazlı Broad-Phase Frekansı

| Tier | Broad-Phase | Frekans | Not |
|---|---|---|---|
| **ACTIVE** | Tam BVH traversal | Her frame (60Hz) | Tüm collider'lar |
| **WARM** | BVH + spatial hash prune | Her 3 frame (20Hz) | Sadece dinamik entity'ler |
| **DISTANT** | Sadece oyuncu query | Her 10 frame (6Hz) | Oyuncu vs terrain AABB |
| **ARCHIVE** | Yok | — | |

---

### 7.3 Incremental Collider Güncelleme

Brickmap değiştiğinde **tüm collider'ı yeniden oluşturmaya gerek yok**. Rapier'ın `set_voxel()` ve `propagate_voxel_change()` metodları O(1) lokal güncelleme sağlar.

#### 7.3.1 3-Kademeli Güncelleme Stratejisi

```rust
impl Sector {
    /// Brickmap değişikliklerini fizik collider'ına yansıt.
    /// Değişiklik sayısına göre optimal strateji seçilir.
    pub fn update_collider(
        &mut self,
        collider: &mut Collider,
        changes: &[VoxelChange],
    ) {
        match changes.len() {
            0 => {}

            // 1-8 voxel: Incremental set_voxel + propagate
            1..=8 => {
                if let Some(voxels) = collider.as_voxels_mut() {
                    for change in changes {
                        voxels.set_voxel(change.grid_pos, change.is_filled);
                        voxels.propagate_voxel_change(change.grid_pos);
                    }
                }
            }

            // 9-64 voxel: Bölgesel rebuild (crop + merge)
            9..=64 => {
                self.rebuild_region(collider, changes);
            }

            // 64+ voxel: Tam rebuild
            _ => {
                *collider = Self::build_full_collider(self);
            }
        }
    }

    /// Bölgesel rebuild — sadece etkilenen alanı güncelle.
    fn rebuild_region(&self, collider: &mut Collider, changes: &[VoxelChange]) {
        // 1. Etkilenen bölgenin AABB'sini hesapla
        let aabb = Self::compute_changes_aabb(changes);

        // 2. Mevcut collider'ı bu AABB'ye göre böl
        if let Some(voxels) = collider.as_voxels_mut() {
            let (inside, outside) = voxels.split_with_box(&aabb);

            // 3. Inside kısmını yeni brickmap verisiyle yeniden oluştur
            let new_inside = Self::build_region_voxels(self, &aabb);

            // 4. Outside + new_inside'i birleştir
            // (combine_voxel_states ile boundary sync)
        }
    }
}
```

#### 7.3.2 Sector Boundary Sync

Komşu sector'ların collider'ları boundary'lerde **birleşik** çalışır:

```rust
/// İki komşu sector'un voxel collider'larını boundary'de birleştir.
/// Rapier'ın combine_voxel_states metodu:
/// - Her iki sector'un neighborhood bilgilerini merge eder
/// - Boundary'lerde internal edge'leri doğru işaretler
/// - Ghost collision'ı boundary'de de önler
pub fn sync_sector_boundaries(
    sector_a: &mut Collider,
    sector_b: &mut Collider,
    offset: IVec3,
) {
    if let (Some(a), Some(b)) = (
        sector_a.as_voxels_mut(),
        sector_b.as_voxels_mut(),
    ) {
        a.combine_voxel_states(b, offset);
    }
}
```

**Sync stratejisi:**
- **Tier 1 ↔ Tier 1:** Her frame sync (aktif boundary)
- **Tier 1 ↔ Tier 2:** Her 5 frame sync
- **Tier 2 ↔ Tier 2:** Her 15 frame sync
- **Tier 3+:** Sync yok (yaklaşık collider kullanılır)

---

### 7.4 Character Controller Entegrasyonu

Rapier'ın **Kinematic Character Controller** API'si, voxel terrain ile optimize edilmiş şekilde kullanılır.

#### 7.4.1 Temel Kurulum

```rust
use bevy_rapier3d::prelude::*;

/// Oyuncu karakter controller setup.
pub fn setup_character(mut commands: Commands) {
    commands
        .spawn(RigidBody::KinematicPositionBased)
        .insert(Collider::capsule_y(0.4, 0.8)) // Capsule shape önerilir
        .insert(Transform::default())
        .insert(KinematicCharacterController {
            // Yerden küçük boşluk — numerical stability için
            offset: CharacterLength::Absolute(0.01),

            // Dikey yön
            up: Vec3::Y,

            // Maksimum tırmanılabilir eğim (45°)
            max_slope_climb_angle: 45_f32.to_radians(),

            // Otomatik kayılacak minimum eğim (30°)
            min_slope_slide_angle: 30_f32.to_radians(),

            // Otomatik merdiven tırmanma
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(1.0),   // 1 blok yükseklik
                min_width: CharacterLength::Absolute(0.6),    // Minimum genişlik
                include_dynamic_bodies: true,
            }),

            // Yere yapışma (merdiven inişi + yokuş aşağı)
            snap_to_ground: Some(CharacterLength::Absolute(0.5)),

            // Dinamik cisimlere impulse uygula
            apply_impulse_to_dynamic_bodies: true,

            ..default()
        });
}
```

#### 7.4.2 XBrickMap-Optimize Ground Check

Rapier'ın shape query'sine ek olarak, **XBrickMap'in 4-level space skipping**'i ile ultra-hızlı zemin tespiti:

```rust
impl CharacterController {
    /// XBrickMap kullanarak zemin tespiti.
    /// Rapier'ın ray-cast'inden 3-5x daha hızlı (4-level skip).
    pub fn ground_check_xbrickmap(
        &self,
        sector: &Sector,
        pos: Vec3,
        foot_radius: f32,
    ) -> GroundState {
        let grid_pos = Self::world_to_grid(pos);

        // 1. Slab seviyesi kontrol (Y/32) — boş slab ise erken çık
        let slab_idx = (grid_pos.y >> 5) as usize;
        if sector.slabs[slab_idx].slab_mask == 0 {
            return GroundState::Air; // Alt tamamen boş
        }

        // 2. Brick seviyesi kontrol
        let brick_idx = Self::compute_brick_index(grid_pos);
        if sector.slabs[slab_idx].slab_mask & (1 << brick_idx) == 0 {
            return GroundState::Air;
        }

        // 3. Sub-brick seviyesi kontrol
        // 4. Kesin voxel kontrol

        // Foot radius içindeki voxel'ları kontrol et (disk query)
        let grounded = self.check_foot_contact(sector, grid_pos, foot_radius);

        if grounded {
            let slope = self.compute_slope_angle(sector, grid_pos);
            GroundState::Grounded { slope_angle: slope }
        } else {
            GroundState::Air
        }
    }
}
```

#### 7.4.3 Character Output Okuma

```rust
/// Character controller output'unu oku ve game logic'e uygula.
fn read_character_output(
    mut controllers: Query<(Entity, &KinematicCharacterControllerOutput)>,
) {
    for (entity, output) in controllers.iter() {
        // Zemin teması
        if output.grounded {
            // Zemin normal'i
            let ground_normal = output.ground_normal;

            // Efektif hareket (engellere göre ayarlanmış)
            let effective_movement = output.effective_translation;

            // Çarpışma bilgisi
            for collision in &output.collisions {
                // Hangi engele çarpıldı
                let hit_entity = collision.entity;
                let hit_position = collision.hit_pos;
            }
        }
    }
}
```

---

### 7.5 Custom Physics Layer

Rapier'ın desteklemediği durumlar için **custom sparse voxel physics** katmanı.

#### 7.5.1 Kapsam

| Durum | Çözüm |
|---|---|
| Voxel vs Voxel collision | Custom spatial hash |
| Falling sand / gravel | Custom particle simulation |
| Explosion debris | Custom rigid-body spawn |
| Structural integrity | Custom stability check |
| Fluid simulation | Custom cellular automata |

#### 7.5.2 Falling Sand / Gravel

```rust
/// Falling particle simulation — kum, çakıl, su.
pub struct FallingParticleSystem {
    /// Aktif parçacıklar.
    particles: Vec<FallingParticle>,

    /// Spatial hash grid (hücre = 1 voxel).
    spatial_grid: SparseGrid<CellInfo>,

    /// Sleep state — hareketsiz bölgeleri uyut.
    sleep_manager: SleepManager,
}

/// Tek bir falling particle.
pub struct FallingParticle {
    pub grid_pos: IVec3,
    pub velocity: Vec3,
    pub block_id: u16,
    pub mass: f32,
    pub settled: bool,
    pub settle_timer: f32, // Hareketsiz kalma süresi
}

impl FallingParticleSystem {
    /// Bir fizik tick'i simüle et.
    pub fn simulate(&mut self, dt: f32, sector: &Sector) {
        // 1. Uyku kontrolü — hareketsiz parçacıkları uyut
        self.sleep_manager.update(&mut self.particles, dt);

        // 2. Aktif parçacıkları güncelle
        for particle in self.particles.iter_mut() {
            if particle.settled {
                continue;
            }

            // Yerçekimi uygula
            particle.velocity.y -= 9.81 * dt;

            // Hedef pozisyonu hesapla
            let target_pos = particle.grid_pos
                + (particle.velocity * dt).as_ivec3();

            // Hedef boş mu kontrol et (XBrickMap'ten O(1))
            if sector.is_empty(target_pos) && self.spatial_grid.is_empty(target_pos) {
                // Boş — hareket et
                particle.grid_pos = target_pos;
                particle.settled = false;
                particle.settle_timer = 0.0;
            } else {
                // Dolu — dur
                particle.velocity = Vec3::ZERO;
                particle.settled = true;
                particle.settle_timer += dt;

                // Yeterince süredir hareketsizse sleep'e al
                if particle.settle_timer > 2.0 {
                    self.sleep_manager.sleep(particle);
                }
            }
        }

        // 3. Spatial grid'i güncelle
        self.spatial_grid.rebuild(&self.particles);
    }
}
```

#### 7.5.3 Spatial Hash (Custom)

```rust
/// Sparse spatial hash grid — custom collision detection için.
/// Teschner et al. "Optimized Spatial Hashing for Collision Detection"
/// yaklaşımını takip eder.
pub struct SparseSpatialHash<T> {
    /// Hücre boyutu (voxel boyutuna eşit).
    cell_size: f32,

    /// Hash table: (cell_x, cell_y, cell_z) → entity listesi.
    cells: HashMap<IVec3, Vec<T>>,
}

impl<T> SparseSpatialHash<T> {
    /// Hash fonksiyonu — 3D koordinatı 1D index'e map eder.
    fn hash(pos: IVec3) -> u64 {
        // Large prime numbers ile çarp — collision minimizasyonu
        let x = pos.x as u64;
        let y = pos.y as u64;
        let z = pos.z as u64;
        (x * 73856093 ^ y * 19349663 ^ z * 83492791) % 0xFFFFFFFF
    }

    /// Bir pozisyondaki komşu hücreleri sorgula.
    pub fn query_neighbors(&self, pos: IVec3) -> impl Iterator<Item = &T> {
        let mut results = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let neighbor = pos + IVec3::new(dx, dy, dz);
                    if let Some(entities) = self.cells.get(&neighbor) {
                        results.extend(entities);
                    }
                }
            }
        }
        results.into_iter()
    }
}
```

---

### 7.6 Destruction & Fracture Sistemi

Teardown'dan ilhamla **Voronoi-based fracture** sistemi.

#### 7.6.1 Hasar Birikimi

```rust
/// Voxel hasar sistemi — patlama, darbe, ateş hasarını biriktirir.
pub struct DamageSystem {
    /// Her voxel için hasar birikimi (sparse grid).
    damage_grid: SparseGrid<f32>,

    /// Hasar eşiği — bu değeri aşan voxel'ler kırılır.
    fracture_threshold: f32,

    /// Hasar yayılımı (komşu voxel'lere hasar transferi).
    damage_propagation: f32,
}

impl DamageSystem {
    /// Bir patlama hasarını uygula.
    pub fn apply_explosion(
        &mut self,
        sector: &mut Sector,
        center: Vec3,
        radius: f32,
        intensity: f32,
    ) {
        // Patlama alanındaki tüm voxel'ları bul
        let grid_center = Self::world_to_grid(center);
        let grid_radius = (radius / VOXEL_SIZE).ceil() as i32;

        for dx in -grid_radius..=grid_radius {
            for dy in -grid_radius..=grid_radius {
                for dz in -grid_radius..=grid_radius {
                    let pos = grid_center + IVec3::new(dx, dy, dz);
                    let dist = (pos.as_vec3() - grid_center.as_vec3()).length();

                    if dist <= grid_radius as f32 {
                        // Mesafeye göre hasar (inverse square)
                        let damage = intensity / (1.0 + dist * dist);

                        // Hasarı biriktir
                        let current = self.damage_grid.get(pos).unwrap_or(0.0);
                        self.damage_grid.insert(pos, current + damage);

                        // Eşik aşıldı — fracture'a işaretle
                        if current + damage >= self.fracture_threshold {
                            self.mark_for_fracture(sector, pos);
                        }
                    }
                }
            }
        }
    }
}
```

#### 7.6.2 Voronoi Fracture

```rust
/// Voronoi-based voxel fracture — Teardown yaklaşımı.
pub struct VoronoiFracture {
    /// Voronoi noktaları (random seed ile üretilir).
    voronoi_points: Vec<VoronoiPoint>,

    /// Fragment havuzu (object pooling).
    fragment_pool: ObjectPool<Fragment>,
}

/// Tek bir Voronoi noktası.
pub struct VoronoiPoint {
    pub position: Vec3,
    pub seed: u64,
}

/// Parçalanmış bir fragment — dinamik rigid-body'ye dönüşür.
pub struct Fragment {
    pub voxel_bounds: BoundingBox,    // Fragment'in AABB'si
    pub voxel_count: u32,             // Kaç voxel içeriyor
    pub mass: f32,                    // Toplam kütle
    pub center_of_mass: Vec3,         // Kütle merkezi
    pub inertia_tensor: Mat3,         // Eylemsizlik tensörü
    pub collider: Option<Collider>,   // Rapier collider (spawn sonrası)
}

impl VoronoiFracture {
    /// Bir bölgeyi Voronoi ile parçala.
    pub fn fracture_region(
        &mut self,
        sector: &mut Sector,
        region_aabb: BoundingBox,
        intensity: f32,
    ) -> Vec<Fragment> {
        // 1. Voronoi noktaları oluştur (yoğunluk = intensity ile orantılı)
        let num_points = (intensity * 10.0) as usize;
        self.generate_voronoi_points(&region_aabb, num_points);

        // 2. Flood-fill ile fragment'leri ayır
        let fragments = self.flood_fill_fragments(sector, &region_aabb);

        // 3. Her fragment için fizik özelliklerini hesapla
        let mut result = Vec::new();
        for fragment in fragments {
            // Küçük fragment'ler (< 8 voxel) → particle'a dönüştür
            if fragment.voxel_count < 8 {
                self.spawn_debris_particles(&fragment);
                continue;
            }

            // Büyük fragment'ler → rigid-body hesapla
            let physics_fragment = self.compute_physics(&fragment);
            result.push(physics_fragment);
        }

        // 4. Orijinal sector'dan kırılan voxel'leri kaldır
        self.remove_fractured_voxels(sector, &result);

        result
    }

    /// Fragment için kütle, CoM ve inertia tensor hesapla.
    fn compute_physics(&self, fragment: &RawFragment) -> Fragment {
        let voxel_mass = VOXEL_SIZE.powi(3) * MATERIAL_DENSITY;
        let total_mass = fragment.voxel_count as f32 * voxel_mass;

        // Kütle merkezi
        let com = fragment.voxel_positions.iter().sum::<Vec3>()
            / fragment.voxel_count as f32;

        // Inertia tensor (parallel axis theorem)
        let mut inertia = Mat3::ZERO;
        for pos in &fragment.voxel_positions {
            let r = *pos - com;
            let r2 = r.dot(r);
            inertia += voxel_mass * (Mat3::from_diagonal(r2) - r * r.transpose());
        }

        Fragment {
            voxel_bounds: fragment.bounding_box,
            voxel_count: fragment.voxel_count,
            mass: total_mass,
            center_of_mass: com,
            inertia_tensor: inertia,
            collider: None,
        }
    }
}
```

#### 7.6.3 Fragment → Rigid-Body Spawn

```rust
/// Parçalanmış fragment'leri Rapier rigid-body olarak spawn et.
pub fn spawn_fragments_as_rigidbodies(
    mut commands: Commands,
    fragments: Vec<Fragment>,
) {
    for fragment in fragments {
        // Voxel koordinatlarından occupied listesi çıkar
        let occupied = fragment.voxel_positions
            .iter()
            .map(|p| {
                // Fragment local → grid coords
                IVec3::new(
                    ((p.x - fragment.voxel_bounds.min.x) / VOXEL_SIZE) as i32,
                    ((p.y - fragment.voxel_bounds.min.y) / VOXEL_SIZE) as i32,
                    ((p.z - fragment.voxel_bounds.min.z) / VOXEL_SIZE) as i32,
                )
            })
            .collect::<Vec<_>>();

        // Voxels collider oluştur
        let collider = ColliderBuilder::voxels(
            Vec3::splat(VOXEL_SIZE),
            &occupied,
        ).build();

        // Rigid-body oluştur
        commands
            .spawn(RigidBody::Dynamic)
            .insert(collider)
            .insert(Transform::from_translation(fragment.center_of_mass))
            .insert(Velocity::default())
            .insert(FragmentMetadata {
                mass: fragment.mass,
                voxel_count: fragment.voxel_count,
                lifetime: 30.0, // 30 saniye sonra cleanup
            });
    }
}
```

---

### 7.7 Physics Tier Management

Streaming tier'larına göre fizik güncelleme stratejisi:

| Tier | Fizik Detayı | Güncelleme Frekansı | Collider Tipi |
|---|---|---|---|
| **ACTIVE** (0-96m) | Tam Voxels + custom physics | Her frame (60Hz) | Rapier Voxels (full) |
| **WARM** (96-384m) | Voxels (static only) | Her 3 frame (20Hz) | Rapier Voxels (static) |
| **DISTANT** (384m-1.5km) | Yaklaşık AABB | Her 10 frame (6Hz) | Rapier Cuboid (AABB) |
| **ARCHIVE** (1.5km+) | Fizik yok | — | Collider yok |

#### 7.7.1 Tier Geçişi Sırasında Fizik

```rust
impl Sector {
    /// Tier değiştiğinde fizik collider'ını güncelle.
    pub fn update_physics_for_tier(
        &mut self,
        old_tier: Tier,
        new_tier: Tier,
        physics_world: &mut PhysicsWorld,
    ) {
        match (old_tier, new_tier) {
            // Uzaklaşıyor: detaylı → basit
            (Tier::Active, Tier::Warm) => {
                // Collider'ı koru ama dynamic entity'leri freeze et
                self.freeze_dynamic_colliders(physics_world);
            }
            (Tier::Warm, Tier::Distant) => {
                // Voxels collider'ı AABB cuboid'e değiştir
                self.simplify_to_aabb(physics_world);
            }
            (Tier::Distant, Tier::Archive) => {
                // Collider'ı tamamen kaldır
                self.remove_collider(physics_world);
            }

            // Yaklaşıyor: basit → detaylı
            (Tier::Archive, Tier::Distant) => {
                // AABB cuboid collider oluştur
                self.create_aabb_collider(physics_world);
            }
            (Tier::Distant, Tier::Warm) => {
                // AABB → Voxels (arka planda bake)
                self.rebuild_voxels_collider(physics_world);
            }
            (Tier::Warm, Tier::Active) => {
                // Static → full physics (dynamic entity'leri aktif et)
                self.activate_dynamic_colliders(physics_world);
            }
        }
    }
}
```

---

### 7.8 GPU Physics Vizyonu

Dimforge'un 2026 hedefi: **rust-gpu ile GPU physics**. Strata bu vizyona hazır olmalı.

#### 7.8.1 Mevcut Durum (2026 Ocak)

Dimforge, 2025 boyunca GPU physics üzerinde çalıştı:

| Proje | Açıklama | Durum |
|---|---|---|
| **wgmath** | WGSL matematik kütüphanesi | ✅ Tamamlandı |
| **wgrapier** | WGSL tabanlı Rapier subset (GPU) | ✅ Demo çalışıyor |
| **wgsparkl** | WGSL MPM simulation | ✅ Demo çalışıyor |
| **Slosh** | Slang port (wgsparkl) | 🔄 Devam ediyor |
| **rust-gpu** | Rust → SPIR-V/CUDA compiler | 🎯 2026 hedefi |

**wgrapier demo performansı:**
- 93.000 body + 120.000 joint (GPU)
- 34.000 plank stack (GPU)
- BVH-based broad-phase + Soft-TGS constraint solver

#### 7.8.2 Strata GPU Physics Hazırlığı

```rust
/// GPU physics için hazırlık katmanı.
/// CPU physics ile aynı interface'i kullanır — runtime'da seçilir.
pub trait PhysicsBackend {
    /// Broad-phase collision detection.
    fn broad_phase(&mut self, dt: f32);

    /// Narrow-phase collision detection.
    fn narrow_phase(&mut self, dt: f32);

    /// Constraint solving.
    fn solve_constraints(&mut self, dt: f32);

    /// Position integration.
    fn integrate(&mut self, dt: f32);
}

/// CPU backend (mevcut — Rapier).
pub struct CpuPhysicsBackend {
    physics_pipeline: PhysicsPipeline,
    broad_phase: BvhBroadPhase,
    // ...
}

/// GPU backend (gelecek — rust-gpu tabanlı).
pub struct GpuPhysicsBackend {
    /// Broad-phase GPU compute (BVH traversal).
    broad_phase_compute: wgpu::ComputePipeline,

    /// Contact generation GPU compute.
    contact_gen_compute: wgpu::ComputePipeline,

    /// Constraint solver GPU (Soft-TGS).
    solver_compute: wgpu::ComputePipeline,

    /// GPU buffer'ları.
    body_buffer: wgpu::Buffer,
    collider_buffer: wgpu::Buffer,
    contact_buffer: wgpu::Buffer,
}

impl PhysicsBackend for GpuPhysicsBackend {
    fn broad_phase(&mut self, dt: f32) {
        // GPU BVH traversal compute dispatch
        // wgrapier yaklaşımı
    }

    fn narrow_phase(&mut self, dt: f32) {
        // GPU contact generation
    }

    fn solve_constraints(&mut self, dt: f32) {
        // GPU Soft-TGS solver
    }

    fn integrate(&mut self, dt: f32) {
        // GPU position integration
    }
}
```

#### 7.8.3 CPU/GPU Tradeoff

| Metrik | CPU Physics | GPU Physics |
|---|---|---|
| **Determinizm** | ✅ Tam deterministik (enhanced-determinism feature) | ⚠️ Floating point non-determinizm |
| **Gecikme** | Düşük (<1ms) | Yüksek (GPU dispatch + readback) |
| **Throughput** | ~5.000 body @ 60Hz | ~100.000+ body @ 60Hz |
| **Dinamik nesne** | Az sayıda (oyuncu, araçlar) | Çok sayıda (debris, particles) |
| **Network sync** | ✅ Ideal (deterministik) | ⚠️ Zor (non-deterministik) |

**Strata stratejisi:**
- **CPU physics:** Oyuncu, araçlar, dinamik entity'ler (deterministik, düşük gecikme)
- **GPU physics (gelecek):** Patlama debris, falling sand, büyük yığınlar (yüksek throughput)

---

### 7.9 Performans Hedefleri

| Metrik | Hedef | Not |
|---|---|---|
| **Collider güncelleme (tek voxel)** | <0.1ms | `set_voxel` + `propagate_voxel_change` |
| **Collider güncelleme (bölgesel)** | <1ms | `split_with_box` + rebuild |
| **Collider güncelleme (tam rebuild)** | <5ms | 32×128×32 sector için |
| **Boundary sync (2 sector)** | <0.5ms | `combine_voxel_states` |
| **Character ground check** | <0.05ms | XBrickMap 4-level skip |
| **Broad-phase (ACTIVE)** | <2ms | BVH traversal, 100+ sector |
| **Falling sand (1K particle)** | <3ms | Custom spatial hash |
| **Fracture (patlama)** | <10ms | Voronoi + flood-fill + rigid-body spawn |
| **GPU physics (gelecek)** | <5ms | 100K+ body, rust-gpu |

---

### 7.10 Crate Organizasyonu (Fizik)

```
crates/
  physics/
    ├── mod.rs              ← Physics plugin entry point
    ├── collider.rs         ← Sector → Voxels collider conversion
    ├── broad_phase.rs      ← BVH + spatial hash complement
    ├── incremental.rs      ← Incremental collider update
    ├── boundary.rs         ← Sector boundary sync
    ├── character/
    │   ├── mod.rs          ← Character controller
    │   ├── ground_check.rs ← XBrickMap-optimized ground detection
    │   └── movement.rs     ← Movement + slope handling
    ├── custom/
    │   ├── mod.rs          ← Custom physics layer
    │   ├── falling_sand.rs ← Falling particle simulation
    │   ├── spatial_hash.rs ← Sparse spatial hash grid
    │   └── fluids.rs       ← Cellular automata fluids
    ├── destruction/
    │   ├── mod.rs          ← Destruction system
    │   ├── damage.rs       ← Damage accumulation
    │   ├── voronoi.rs      ← Voronoi fracture
    │   └── fragment.rs     ← Fragment → rigid-body spawn
    ├── tier.rs             ← Physics tier management
    └── gpu/
        ├── mod.rs          ← GPU physics abstraction
        ├── backend.rs      ← PhysicsBackend trait
        └── compute.rs      ← GPU compute pipelines (gelecek)
```

---

## 8. Aydınlatma Sistemi — 5-Kademeli Hybrid Mimari

Strata, **5-kademeli hybrid aydınlatma** sistemi kullanır. Her kademe farklı bir ışık türünü ve hesaplama yöntemini temsil eder. Bu yaklaşım, **doğruluk**, **performans**, **bellek verimliliği** ve **dinamik güncelleme** arasında Pareto-optimal dengeyi sağlar.

### 8.1 Genel Bakış

| Kademe | Ad | Yöntem | Frekans | Performans |
|---|---|---|---|---|
| **L0** | Direct Light | Analytic (sun, point lights) | Her frame | ~0.1ms |
| **L1** | Block Light (BFS) | CPU SIMD flood-fill + two-phase removal | Değişiklikte | <100µs/torch |
| **L2** | Sky Light | Column-first + heightmap (Starlight-style) | Chunk load/değişiklik | <0.5ms/sector |
| **L3** | Indirect GI (near) | Clustered Voxel GI + visibility buffer | Her 5 frame | <3ms |
| **L4** | Indirect GI (far) | SVDAG ray march + Hi-Z occlusion | Her 10 frame | <2ms |

### Temel Prensipler

- **L0 = Direct:** Anlık, maliyetsiz — mesh'e doğrudan bake
- **L1 = Block:** BFS zaten gerekli, SIMD ile ultra-hızlı — mesh vertex color'a bake
- **L2 = Sky:** Starlight-style column-first, XBrickMap slab bitmask'inden heightmap O(1)
- **L3 = Indirect near:** Clustered GI — oyuncuya yakın alanlarda doğru GI
- **L4 = Indirect far:** SVDAG ray march — uzaktaki alanlarda yaklaşık GI

### Kanıtlanmış Referanslar

| Bileşen | Kaynak |
|---|---|
| BFS Flood-Fill | Seed of Andromeda (2015), voxel-light crate (2026) |
| Starlight Propagation | PaperMC/Starlight — Vanilla'dan 28x hızlı |
| SIMD Flood-Fill | atrufulgium.net (2024) — 128 voxel/iterasyon, 15x hızlanma |
| Two-Phase Removal | voxel-light crate — correct removal + re-propagation |
| Column-First Sky | Starlight heightmap optimization — ~300 queued entry (vs ~2000+) |
| Word-Level Parallelism | 0fps.net (mikolalysenko) — 8 kanal tek u32'de, bitwise ops |
| Clustered Voxel GI | Ayerbe & Patow, CGF 2022 — 100x az visibility test |
| Hierarchical Bitmask | SCITEPRESS 2024 — Morton Z-order + hierarchical light culling |
| SVDAG Cone Tracing | NVidia VXGI, Crassin & Green — hierarchical LOD cone sampling |
| Aokana GPU Pipeline | Fang et al., SIGGRAPH 2025 — 4.8x hız, 9x VRAM azalması |
| TU Wien RGI | Ott et al., 2025 — voxel-specific TAA, noise-free path tracing |
| Neural Irradiance Volume | Adobe, 2024 — 1-5MB, ~1ms inference, noise-free |

---

### 8.2 Light Data Formatı (16-bit Packed)

Her voxel için **16-bit packed** light data. XBrickMap'in left-packed materials dizisine paralel olarak saklanır.

```
┌─────────────────────────────────────────┐
│ Light Data (16 bit per voxel)           │
├─────────────────────────────────────────┤
│ Bits 0-3:   Sky Light (0-15)            │
│ Bits 4-7:   Block Light R (0-15)        │
│ Bits 8-11:  Block Light G (0-15)        │
│ Bits 12-15: Block Light B (0-15)        │
└─────────────────────────────────────────┘
```

```rust
/// 16-bit packed light data — 4 kanal × 4-bit.
/// XBrickMap SubBrick'e paralel olarak saklanır.
#[repr(transparent)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LightData(pub u16);

impl LightData {
    /// Sky light seviyesi (0-15).
    #[inline]
    pub fn sky(&self) -> u8 { (self.0 & 0xF) as u8 }

    /// Block light R kanalı (0-15).
    #[inline]
    pub fn block_r(&self) -> u8 { ((self.0 >> 4) & 0xF) as u8 }

    /// Block light G kanalı (0-15).
    #[inline]
    pub fn block_g(&self) -> u8 { ((self.0 >> 8) & 0xF) as u8 }

    /// Block light B kanalı (0-15).
    #[inline]
    pub fn block_b(&self) -> u8 { ((self.0 >> 12) & 0xF) as u8 }

    /// Tüm kanalları tek seferde ayarla.
    #[inline]
    pub fn new(sky: u8, r: u8, g: u8, b: u8) -> Self {
        Self(
            (sky & 0xF) as u16
            | ((r & 0xF) as u16) << 4
            | ((g & 0xF) as u16) << 8
            | ((b & 0xF) as u16) << 12,
        )
    }

    /// Word-Level Parallelism: 4-bit component-wise max.
    /// 0fps.net (mikolalysenko) tekniği — 8 kanal tek u32'de.
    #[inline]
    pub fn wlp_max(a: u32, b: u32) -> u32 {
        let lt = Self::wlp_less_than(a, b);
        a ^ ((a ^ b) & lt)
    }

    /// Word-Level Parallelism: 4-bit component-wise less-than.
    #[inline]
    pub fn wlp_less_than(a: u32, b: u32) -> u32 {
        const COMPONENT_MASK: u32 = 0x0F0F0F0F;
        const BORROW_GUARD: u32 = 0x08080808;
        const CARRY_MASK: u32 = 0x10101010;
        let d = (((a & COMPONENT_MASK) | BORROW_GUARD) - (b & COMPONENT_MASK)) & CARRY_MASK;
        (d | (d >> 3) | (d >> 4)) & COMPONENT_MASK
    }

    /// Word-Level Parallelism: 4-bit component-wise decrement + saturate.
    #[inline]
    pub fn wlp_decrement(x: u32) -> u32 {
        let d = ((x & 0x0F0F0F0F) | 0x02020202) - 0x01010101;
        let b = d & 0x10101010;
        (d + (b >> 4)) & 0x0F0F0F0F
    }
}
```

**Bellek Hesabı:**
- Sub-brick başına: 8 voxel × 16 bit = 16 byte
- Brick başına: ~64 sub-brick × ~16 byte = ~1KB (left-packed)
- Slab başına: ~64 brick × ~1KB = ~64KB (sparse)
- Sector: ~128-256KB (ortalama arazi)

---

### 8.3 L1 — Block Light (SIMD BFS Flood-Fill)

**Algoritma:** Starlight-style level propagation + two-phase removal + SIMD acceleration.

#### 8.3.1 Propagation (Işık Yerleştirme)

```
1. Light source'u BFS queue'ya ekle (level L)
2. Queue'dan node çıkar
3. 6 komşuyu kontrol et:
   - Opaque blok mu? → atla
   - Komşu light level + 2 <= mevcut level? → komşuya level-1 yaz, queue'ya ekle
4. Queue boş olana kadar tekrarla
```

```rust
/// BFS flood-fill light propagation — Starlight-style level propagation.
/// SIMD-accelerated: 128 voxel/iterasyon (wide crate).
pub struct BlockLightEngine {
    /// BFS queue — reusable buffer (allocation yok).
    queue: VecDeque<BfsNode>,

    /// Visited set — ahash ile ~2x hızlı (voxel-light crate).
    visited: AHashMap<IVec3, u8>,

    /// Reusable internal buffers (GC churn yok).
    buffer_pool: BufferPool,
}

/// BFS queue node'u.
#[repr(C)]
pub struct BfsNode {
    pub pos: IVec3,
    pub light_level: u8,
}

impl BlockLightEngine {
    /// Yeni ışık kaynağı yerleştir ve propagate et.
    pub fn place_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        level: u8,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        // 1. Source'u ayarla
        let mut updates = Vec::new();
        updates.push(LightUpdate { pos, light: level, color });

        // 2. BFS queue'ya ekle
        self.queue.clear();
        self.queue.push_back(BfsNode { pos, light_level: level });
        self.visited.clear();
        self.visited.insert(pos, level);

        // 3. BFS propagation
        while let Some(node) = self.queue.pop_front() {
            let current_level = node.light_level;
            if current_level <= 1 { continue; } // Minimum level

            // 6 komşuyu kontrol et
            for dir in DIRECTIONS_6 {
                let neighbor = node.pos + dir;
                if self.visited.contains_key(&neighbor) { continue; }

                // Opaque blok mu?
                if sector.is_opaque(neighbor) { continue; }

                let new_level = current_level - 1;
                let existing = sector.get_light(neighbor);

                // Starlight kuralı: neighbor + 2 <= current → güncelle
                if existing + 2 <= new_level {
                    self.visited.insert(neighbor, new_level);
                    self.queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: new_level,
                    });
                    updates.push(LightUpdate {
                        pos: neighbor,
                        light: new_level,
                        color,
                    });
                }
            }
        }

        updates
    }
}
```

#### 8.3.2 Two-Phase Removal (Işık Kaldırma)

**voxel-light crate (2026)** algoritması:

```
Phase 1: BFS ile kaldırılan kaynağa bağımlı tüm voxel'leri sıfırla
  - Komşu light < eski level → bağımlı, sıfırla
  - Komşu light >= eski level → bağımsız kaynak, boundary source olarak kaydet

Phase 2: Boundary source'lardan yeniden propagate et
  - Overlay kullan (Phase 1'de sıfırlananlar için zero döndür)
```

```rust
impl BlockLightEngine {
    /// Işık kaynağını kaldır — two-phase removal.
    pub fn remove_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        let mut updates = Vec::new();

        // Phase 1: Bağımlı voxel'leri sıfırla
        let boundary_sources = self.zero_dependents(sector, pos, color, &mut updates);

        // Phase 2: Boundary source'lardan yeniden propagate et
        for source in boundary_sources {
            let new_updates = self.place_light(sector, source.pos, source.level, color);
            updates.extend(new_updates);
        }

        updates
    }

    /// Phase 1: Bağımlı voxel'leri sıfırla, boundary source'ları kaydet.
    fn zero_dependents(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        color: LightColor,
        updates: &mut Vec<LightUpdate>,
    ) -> Vec<BoundarySource> {
        let old_level = sector.get_light_at(pos, color);
        let mut boundary_sources = Vec::new();

        self.queue.clear();
        self.queue.push_back(BfsNode { pos, light_level: old_level });

        while let Some(node) = self.queue.pop_front() {
            for dir in DIRECTIONS_6 {
                let neighbor = node.pos + dir;
                let neighbor_level = sector.get_light_at(neighbor, color);

                if neighbor_level < node.light_level {
                    // Bağımlı → sıfırla
                    updates.push(LightUpdate {
                        pos: neighbor,
                        light: 0,
                        color,
                    });
                    self.queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: neighbor_level,
                    });
                } else if neighbor_level >= node.light_level {
                    // Bağımsız kaynak → boundary source
                    boundary_sources.push(BoundarySource {
                        pos: neighbor,
                        level: neighbor_level,
                    });
                }
            }
        }

        boundary_sources
    }
}
```

#### 8.3.3 SIMD Acceleration (15x Hızlanma)

**atrufulgium.net (2024)** SIMD flood-fill tekniği:

```rust
use wide::{u32x4, u64x4};

/// SIMD-accelerated light propagation.
/// 128 voxel/iterasyon işler (4×32-bit × 4 depth).
pub fn propagate_simd(
    slab: &mut Slab,
    light_data: &mut [LightData],
    queue: &mut BfsQueue,
) {
    // X ekseni: 32-bit içinde bitwise shift (32 voxel paralel)
    // Y ekseni: 8 × u32x4 (32 voxel paralel)
    // Z ekseni: 32 u32x4 kopyalama (doğrudan SIMD)

    while let Some(node) = queue.pop() {
        let level = node.light_level;

        // 6 komşuyu SIMD ile kontrol et
        let current = u32x4::from([level as u32; 4]);
        let neighbor = load_neighbor_light_simd(light_data, node.pos);

        // Component-wise: neighbor + 2 <= current ?
        let should_update = wlp_less_than_simd(neighbor + 2, current);

        if should_update.any() {
            let new_level = wlp_decrement_simd(current);
            store_neighbor_light_simd(light_data, node.pos, new_level);
            queue.push_bulk(neighbor_positions(should_update));
        }
    }
}
```

**Performans (Ryzen 9 7900, voxel-light crate):**

| Operasyon | Level 7 | Level 10 | Level 14 |
|---|---|---|---|
| Propagation (scalar) | 17µs | 60µs | 174µs |
| Propagation (SIMD) | ~5µs | ~18µs | ~52µs |
| Removal (tek kaynak) | 105µs | — | 432µs |
| Full place+remove cycle | — | — | ~300µs (SIMD) |

Level-14 torch ~11.500 voxel'e dokunuyor.

---

### 8.4 L2 — Sky Light (Column-First + Heightmap)

**Algoritma:** Starlight-style column-first propagation + XBrickMap heightmap.

#### 8.4.1 Heightmap'ten Sky Source Setup (O(1))

XBrickMap'in **slab bitmask'leri** doğal heightmap olarak kullanılır:

```rust
impl Sector {
    /// Slab bitmask'lerinden sky source heightmap'i oluştur.
    /// Boş slab = tüm sütun açık, tek seferde level-15 ata.
    pub fn build_sky_heightmap(&self) -> [i16; 32 * 32] {
        let mut heightmap = [128i16; 32 * 32]; // Varsayılan: en üst

        for (slab_idx, slab) in self.slabs.iter().enumerate().rev() {
            if slab.slab_mask == 0 {
                // Tüm slab boş → bu slab ve altındaki tüm sütunlar açık
                continue;
            }

            // Dolu brick'leri kontrol et
            for brick_idx in slab.slab_mask.iter_ones() {
                let bx = brick_idx % 4;
                let bz = (brick_idx / 4) % 4;
                let by = brick_idx / 16;

                let world_x = bx * 8;
                let world_z = bz * 8;
                let world_y = slab_idx * 32 + by * 8;

                // Bu brick'in sütunundaki heightmap'i güncelle
                for dx in 0..8 {
                    for dz in 0..8 {
                        let sx = (world_x + dx) as usize;
                        let sz = (world_z + dz) as usize;
                        let idx = sx + sz * 32;
                        if world_y as i16 < heightmap[idx] {
                            heightmap[idx] = world_y as i16;
                        }
                    }
                }
            }
        }

        heightmap
    }
}
```

#### 8.4.2 Column-First Propagation

```rust
impl SkyLightEngine {
    /// Column-first sky light propagation.
    /// 1. Heightmap'ten sky source'ları belirle (O(1))
    /// 2. Dikey sütunları level-15 ile doldur
    /// 3. Yatay BFS spread (overhang/mağara kenarları)
    pub fn propagate_sky(&mut self, sector: &Sector) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        let heightmap = sector.build_sky_heightmap();

        // 1. Dikey sütunları doldur
        for sx in 0..32 {
            for sz in 0..32 {
                let sky_y = heightmap[sx + sz * 32];

                for y in (0..sky_y).rev() {
                    updates.push(LightUpdate {
                        pos: IVec3::new(sx as i32, y, sz as i32),
                        light: 15,
                        color: LightColor::Sky,
                    });

                    // Kenar sütunları → yatay BFS queue'ya ekle
                    if sx == 0 || sx == 31 || sz == 0 || sz == 31 {
                        self.horizontal_queue.push(BfsNode {
                            pos: IVec3::new(sx as i32, y, sz as i32),
                            light_level: 14, // Bir seviye azalarak başla
                        });
                    }
                }
            }
        }

        // 2. Yatay BFS spread
        self.horizontal_bfs(sector, &mut updates);

        updates
    }

    /// Yatay BFS spread — overhang ve mağara kenarları için.
    fn horizontal_bfs(&mut self, sector: &Sector, updates: &mut Vec<LightUpdate>) {
        while let Some(node) = self.horizontal_queue.pop_front() {
            if node.light_level == 0 { continue; }

            // Sadece yatay komşular (±X, ±Z)
            for dir in DIRECTIONS_4_HORIZONTAL {
                let neighbor = node.pos + dir;
                if sector.is_opaque(neighbor) { continue; }

                let existing = sector.get_sky_light(neighbor);
                if existing + 2 <= node.light_level {
                    let new_level = node.light_level - 1;
                    updates.push(LightUpdate {
                        pos: neighbor,
                        light: new_level,
                        color: LightColor::Sky,
                    });
                    self.horizontal_queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: new_level,
                    });
                }
            }
        }
    }
}
```

**Performans:**
- Açık arazi (çöl): ~300 queued entry (Starlight optimizasyonu)
- Vanilla: ~2000+ queued entry
- **~7x az queue işlemi**

---

### 8.5 L3 — Indirect GI (Clustered Voxel GI)

**Algoritma:** Clustered Voxel GI (Ayerbe & Patow, CGF 2022) + XBrickMap mip level clustering.

#### 8.5.1 Mip Level'den Cluster Oluşturma

XBrickMap'in **mip_half** (4³) ve **mip_quarter** (2³) level'ları doğal cluster candidate'larıdır:

```rust
/// XBrickMap mip level'larından light cluster oluştur.
/// Aynı normal'e sahip voxel'leri grupla → visibility test sayısı azalır.
pub struct LightCluster {
    pub center: Vec3,
    pub normal: Vec3,
    pub lit_voxel_count: u32,
    pub accumulated_irradiance: Vec3,
    pub visible_from_camera: bool,
}

impl Sector {
    /// Mip level'larından cluster oluştur.
    pub fn build_light_clusters(&self) -> Vec<LightCluster> {
        let mut clusters = Vec::new();

        for (slab_idx, slab) in self.slabs.iter().enumerate() {
            for (brick_idx, brick) in slab.bricks.iter().enumerate() {
                // mip_quarter: 2³ = 8 voxel'lik gruplar
                for group in brick.mip_quarter.iter_ones() {
                    let center = brick_quarter_center(brick_idx, group);
                    let normal = estimate_cluster_normal(brick, group);
                    let lit_count = count_lit_voxels(brick, group);

                    if lit_count > 0 {
                        clusters.push(LightCluster {
                            center,
                            normal,
                            lit_voxel_count: lit_count,
                            accumulated_irradiance: Vec3::ZERO,
                            visible_from_camera: false,
                        });
                    }
                }
            }
        }

        // Normal-benzeri cluster'ları birleştir
        clusters.sort_by(|a, b| a.normal.dot(b.normal).partial_cmp(&0.5).unwrap());
        clusters
    }
}
```

#### 8.5.2 Visibility Test (3D Bresenham)

```rust
/// 3D Bresenham line algorithm ile cluster visibility test.
/// Lazy evaluation: sadece kameradan görünür voxel'ler için hesapla.
pub fn test_cluster_visibility(
    cluster: &LightCluster,
    camera_pos: Vec3,
    sector: &Sector,
) -> bool {
    // Cluster merkezinden kamera pozisyonuna ray cast
    let dir = (camera_pos - cluster.center).normalize();
    let steps = (cluster.center.distance(camera_pos)).ceil() as i32;

    // 3D Bresenham
    let mut pos = cluster.center.as_ivec3();
    for _ in 0..steps {
        if sector.is_opaque(pos) {
            return false; // Occluded
        }
        pos += dir.as_ivec3();
    }

    true // Visible
}
```

#### 8.5.3 Irradiance Gathering

```rust
impl IndirectGIEngine {
    /// Clustered irradiance gathering.
    /// Her görünür cluster için lit voxel'leri topla.
    pub fn gather_irradiance(
        &mut self,
        clusters: &mut [LightCluster],
        sector: &Sector,
    ) {
        for cluster in clusters.iter_mut() {
            if !cluster.visible_from_camera { continue; }

            // Bu cluster'ın gördüğü lit cluster'ları bul
            let mut total_irradiance = Vec3::ZERO;
            let mut visible_count = 0;

            for other in clusters.iter() {
                if other.lit_voxel_count == 0 { continue; }

                // Visibility test (3D Bresenham)
                if is_visible(cluster.center, other.center, sector) {
                    let dist = cluster.center.distance(other.center);
                    let attenuation = 1.0 / (1.0 + dist * dist);
                    total_irradiance += other.accumulated_irradiance * attenuation;
                    visible_count += 1;
                }
            }

            if visible_count > 0 {
                cluster.accumulated_irradiance = total_irradiance / visible_count as f32;
            }
        }
    }
}
```

**Avantaj:** 131.072 voxel → ~500-1000 cluster → **100x daha az visibility test**.

---

### 8.6 L4 — Indirect GI (SVDAG Cone Tracing)

**Algoritma:** Voxel cone tracing (Crassin & Green) + SVDAG hierarchical LOD + Hi-Z occlusion.

#### 8.6.1 SVDAG Cone March (WGSL)

```wgsl
// SVDAG üzerinden voxel cone tracing — Tier 3/4 için.
@compute @workgroup_size(64)
fn svdag_cone_trace(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let ray = camera_get_ray(pixel);

    // Visibility buffer'dan hit bilgisi al
    let hit = visibility_buffer_load(pixel);
    if (hit.depth == MAX_DEPTH) { return; }

    // 6 yöne cone trace (diffuse GI)
    var irradiance: vec3f = vec3f(0.0);
    let directions = get_hemisphere_directions(hit.normal);

    for (var i = 0u; i < 6u; i++) {
        let cone_dir = directions[i];
        let cone_aperture = 0.5; // diffuse için geniş cone

        // SVDAG üzerinden cone march
        let radiance = svdag_cone_march(
            hit.position,
            cone_dir,
            cone_aperture,
            svdag_root,
        );
        irradiance += radiance;
    }

    irradiance /= 6.0;

    // Irradiance cache'e yaz
    irradiance_cache_store(hit.voxel_coord, irradiance);
}

// SVDAG cone march — hiyerarşik LOD ile
fn svdag_cone_march(
    origin: vec3f,
    direction: vec3f,
    aperture: f32,
    root: u32,
) -> vec3f {
    var t: f32 = 0.0;
    var radiance: vec3f = vec3f(0.0);
    var cone_width: f32 = aperture;

    for (var i = 0u; i < 64u; i++) {
        let pos = origin + direction * t;

        // SVDAG'den en uygun LOD node'u bul
        let (node, lod) = svdag_query_lod(root, pos, cone_width);

        if (node.is_leaf) {
            radiance += node.radiance * node.opacity;
            break;
        }

        // Cone genişliği ile LOD seç
        // Geniş cone = düşük LOD (daha hızlı)
        // Dar cone = yüksek LOD (daha doğru)
        t += get_node_size(lod);
        cone_width *= 1.5; // cone genişler
    }

    return radiance;
}
```

#### 8.6.2 Hi-Z Occlusion for Lighting

```wgsl
// Hi-Z occlusion ile gereksiz cone trace'leri atla
@compute @workgroup_size(8, 8)
fn lighting_hiz_cull(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let tile = id.xy;
    let tile_depth = hiz_buffer_load(tile);

    // Eğer bu tile zaten tamamen kapalıysa, lighting yapma
    if (tile_depth == MAX_DEPTH) {
        return;
    }

    // Tile'ın ortalama derinliğine göre LOD seç
    let lod_level = select_lighting_lod(tile_depth);

    // Sadece görünür tile'lar için GI hesapla
    compute_indirect_lighting(tile, lod_level);
}
```

---

### 8.7 Hierarchical Light Culling

**Algoritma:** Hierarchical Bitmask Implicit Grids (SCITEPRESS 2024) + Morton Z-order.

```rust
/// Hierarchical light culling bitmask'i.
/// XBrickMap'in mevcut bitmask yapısına ek katman.
pub struct LightCullingMask {
    /// Slab level: 64-bit — hangi brick'lerde light var.
    pub slab_light_mask: u64,

    /// Brick level: 64-bit — hangi sub-brick'lerde light var.
    pub brick_light_mask: u64,

    /// Morton Z-order ile sıralı light source'lar.
    pub sorted_lights: Vec<LightSource>,
}

impl LightCullingMask {
    /// Light source'ları Morton Z-order'a göre sırala.
    /// Bu, hierarchical bitmask'in etkililiğini artırır.
    pub fn sort_lights_morton(&mut self) {
        self.sorted_lights.sort_by_key(|l| {
            morton_encode_3d(
                l.pos.x as u32,
                l.pos.y as u32,
                l.pos.z as u32,
            )
        });
    }

    /// Boş slab'ı O(1) kontrol et.
    #[inline]
    pub fn slab_has_light(&self, brick_index: usize) -> bool {
        self.slab_light_mask & (1 << brick_index) != 0
    }

    /// Boş brick'i O(1) kontrol et.
    #[inline]
    pub fn brick_has_light(&self, sub_index: usize) -> bool {
        self.brick_light_mask & (1 << sub_index) != 0
    }
}
```

**Avantaj:**
- Boş slab → tüm light propagation atla (O(1))
- Boş brick → 64 voxel atla
- Morton order → nearby light'lar aynı bitmask sector'de
- 10.000+ light için bile etkili

---

### 8.8 Temporal Accumulation (TAA-Style)

**Algoritma:** TU Wien RGI (2025) — voxel-specific temporal anti-aliasing.

```wgsl
// Temporal accumulation — önceki frame'lerle birleştir
@compute @workgroup_size(8, 8)
fn temporal_accumulate(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;

    let current = current_frame_irradiance(pixel);
    let history = history_buffer_load(pixel);

    // Motion vector ile history sample
    let motion = motion_vector_buffer_load(pixel);
    let prev_pixel = pixel - motion;
    let history_sample = history_buffer_sample(prev_pixel);

    // Voxel-specific blending factor
    // Keskin köşelerde daha az blending (ghosting önleme)
    let blend_factor = compute_voxel_blend_factor(pixel);

    let result = mix(history_sample, current, blend_factor);

    // Output
    final_irradiance_store(pixel, result);
    history_buffer_store(pixel, result);
}
```

**Avantaj:** Noise-free GI, düşük sample count ile yüksek kalite, voxel köşelerinde ghosting yok.

---

### 8.9 Mesh'e Light Bake

Light data → greedy mesh vertex color entegrasyonu:

```rust
/// Light data'yı greedy mesh vertex color'a bake et.
/// Smooth lighting: her vertex 4 komşu bloğun light ortalaması.
pub fn bake_light_to_mesh(
    mesh: &mut MeshData,
    sector: &Sector,
    face_vertices: &[IVec3],
) {
    for vertex in face_vertices {
        // 4 komşu bloğun light değerlerini al
        let light_samples = [
            sector.get_light(*vertex),
            sector.get_light(*vertex + IVec3::new(1, 0, 0)),
            sector.get_light(*vertex + IVec3::new(0, 1, 0)),
            sector.get_light(*vertex + IVec3::new(1, 1, 0)),
        ];

        // Smooth: sıfır olmayan değerlerin ortalaması
        let smooth_light = smooth_lighting(light_samples);

        // Vertex color'a yaz (sky + block RGB)
        vertex.color = light_to_color(smooth_light);
    }
}

/// Smooth lighting — sıfır olmayan değerlerin ortalaması.
/// Harsh geçişleri önler.
fn smooth_lighting(samples: [LightData; 4]) -> LightData {
    let mut sum = 0u32;
    let mut count = 0u32;

    for sample in samples {
        if sample.sky() > 0 || sample.block_r() > 0 {
            sum += sample.0 as u32;
            count += 1;
        }
    }

    if count == 0 {
        LightData::default()
    } else {
        LightData((sum / count) as u16)
    }
}
```

---

### 8.10 Tier-Bazlı Lighting Stratejisi

| Tier | Yöntem | Güncelleme | Not |
|---|---|---|---|
| **ACTIVE** (0-96m) | L0+L1+L2+L3 (CPU BFS + Clustered GI) | Her değişiklikte | En doğru, mesh'e baked |
| **WARM** (96-384m) | L0+L1+L2 (CPU BFS) | Her 3 frame | Yumuşak geçiş |
| **DISTANT** (384m-1.5km) | L0+L4 (SVDAG cone trace) | Her 10 frame | Yaklaşık GI |
| **ARCHIVE** (1.5km+) | L0 sadece | — | Render edilmez |

#### 8.10.1 Tier Geçişi Sırasında Lighting

```rust
impl Sector {
    /// Tier değiştiğinde lighting'i güncelle.
    pub fn update_lighting_for_tier(&mut self, old_tier: Tier, new_tier: Tier) {
        match (old_tier, new_tier) {
            // Uzaklaşıyor: detaylı → basit
            (Tier::Active, Tier::Warm) => {
                // Clustered GI'yi durdur, sadece BFS tut
                self.indirect_gi_active = false;
            }
            (Tier::Warm, Tier::Distant) => {
                // BFS'i durdur, SVDAG cone trace'e geç
                self.block_light_active = false;
                self.svdag_lighting_active = true;
            }
            (Tier::Distant, Tier::Archive) => {
                // Tüm lighting'i durdur
                self.svdag_lighting_active = false;
            }

            // Yaklaşıyor: basit → detaylı
            (Tier::Archive, Tier::Distant) => {
                // SVDAG cone trace'i aktif et
                self.svdag_lighting_active = true;
            }
            (Tier::Distant, Tier::Warm) => {
                // SVDAG → BFS (arka planda unbake)
                self.svdag_lighting_active = false;
                self.rebuild_block_light();
            }
            (Tier::Warm, Tier::Active) => {
                // Clustered GI'yi aktif et
                self.indirect_gi_active = true;
            }
        }
    }
}
```

---

### 8.11 GPU Lighting Pipeline (wgpu)

```
┌──────────────────────────────────────────────────────────────┐
│                    LIGHTING FRAME                             │
├──────────────────────────────────────────────────────────────┤
│ Pass 1: Direct Light (CPU)                                   │
│   → Sun, point lights — analytic, mesh'e bake                │
├──────────────────────────────────────────────────────────────┤
│ Pass 2: Block Light BFS (CPU SIMD)                           │
│   → Dirty sector'lar için BFS flood-fill                     │
│   → Two-phase removal + re-propagation                       │
├──────────────────────────────────────────────────────────────┤
│ Pass 3: Sky Light (CPU)                                      │
│   → Column-first + heightmap (O(1) source setup)             │
│   → Yatay BFS spread (overhang/mağara)                       │
├──────────────────────────────────────────────────────────────┤
│ Pass 4: Clustered GI (GPU Compute)                           │
│   → Cluster build (mip level'den)                            │
│   → Visibility test (3D Bresenham)                           │
│   → Irradiance gathering                                     │
├──────────────────────────────────────────────────────────────┤
│ Pass 5: SVDAG Cone Trace (GPU Compute)                       │
│   → Hi-Z occlusion culling                                   │
│   → Hierarchical LOD cone march                              │
│   → Temporal accumulation                                    │
├──────────────────────────────────────────────────────────────┤
│ Pass 6: Light → Mesh Bake (CPU)                              │
│   → Smooth lighting (4-vertex average)                       │
│   → Vertex color write                                       │
└──────────────────────────────────────────────────────────────┘
```

---

### 8.12 Neural Irradiance Volume (Faz 6 Vision)

**Adobe NIV (2024)** tekniği — uzun vadeli optimizasyon:

```
Neural Irradiance Volume:
  - Pre-computed irradiance field (MLP ile sıkıştırılmış)
  - 1-5MB bellek (geleneksel probe'lardan 10x küçük)
  - ~1ms inference (consumer GPU, 1080p)
  - G-buffer input (position + normal)
  - Noise-free, ray tracing/denoising gerektirmez

Strata Entegrasyonu:
  - Tier 3 (Distant) için NIV kullan
  - SVDAG'den training data üret (offline)
  - Runtime'da G-buffer → NIV inference → indirect diffuse
  - Dynamic objeler için de çalışır (unseen objects)
```

---

### 8.13 Crate Organizasyonu (Aydınlatma)

```
crates/
  lighting/
    ├── mod.rs                  ← Lighting plugin entry point
    ├── light_data.rs           ← 16-bit packed light data (sky + RGB)
    ├── engine.rs               ← LightEngine (orchestrator)
    │
    ├── direct/
    │   ├── mod.rs              ← Direct lighting (sun, point lights)
    │   ├── sun.rs              ← Directional sun light (day/night cycle)
    │   └── point.rs            ← Point/spot lights (analytic)
    │
    ├── block/
    │   ├── mod.rs              ← Block light (emissive blocks)
    │   ├── bfs_cpu.rs          ← CPU BFS flood-fill (Starlight-style)
    │   ├── bfs_simd.rs         ← SIMD-accelerated BFS (wide crate)
    │   ├── removal.rs          ← Two-phase removal (voxel-light style)
    │   └── colored.rs          ← RGB channel propagation (packed)
    │
    ├── sky/
    │   ├── mod.rs              ← Sky light system
    │   ├── column_first.rs     ← Column-first propagation (Starlight)
    │   ├── heightmap.rs        ← Slab bitmask'ten heightmap (O(1))
    │   └── day_night.rs        ← Day/night cycle (ambient shift)
    │
    ├── indirect/
    │   ├── mod.rs              ← Indirect GI system
    │   ├── clustered.rs        ← Clustered Voxel GI (CGF 2022)
    │   ├── cone_trace.rs       ← Voxel cone tracing (SVDAG)
    │   ├── irradiance_cache.rs ← Per-face irradiance cache
    │   └── visibility.rs       ← 3D Bresenham visibility test
    │
    ├── culling/
    │   ├── mod.rs              ← Light culling system
    │   ├── hierarchical.rs     ← Hierarchical bitmask implicit grids
    │   ├── morton.rs           ← Morton Z-order sorting
    │   └── priority.rs         ← Light update priority queue
    │
    ├── mesh_bake.rs            ← Light data → vertex color (greedy mesh)
    ├── tier.rs                 ← Tier-bazlı lighting stratejisi
    │
    └── gpu/
        ├── mod.rs              ← GPU lighting pipelines
        ├── svdag_light.rs      ← SVDAG cone tracing (Tier 3/4)
        ├── hi_z.rs             ← Hi-Z occlusion for lighting
        ├── temporal.rs         ← Temporal accumulation (TAA-style)
        └── neural_irradiance.rs← Neural Irradiance Volume (Faz 6)
```

---

### 8.14 Performans Hedefleri (Aydınlatma)

| Metrik | Hedef | Not |
|---|---|---|
| Tek torch propagation (SIMD) | <100µs | Level-14, wide crate |
| Torch removal + re-propagate | <300µs | Two-phase + SIMD |
| Sector skylight (açık arazi) | <0.5ms | Heightmap O(1) + column-first |
| Clustered GI (near) | <3ms | 100x az visibility test |
| SVDAG cone trace (far) | <2ms | Hi-Z + hierarchical LOD |
| Light culling (10K lights) | <0.5ms | Hierarchical bitmask + Morton |
| Light → mesh bake | <2ms/sector | Smooth lighting (4-vertex avg) |
| Temporal accumulation | <1ms/frame | Voxel-specific TAA |
| Bellek (light data) | 16 bit/voxel | Sky 4-bit + RGB 4×4-bit |
| GPU irradiance cache | <1ms/frame | Temporal accumulation |

---

## 9. Network Senkronizasyonu (Replicon/Renet2)

### 8.1 Tier-Bazlı Delta Sync

| Tier | Sync Yöntemi | Paket Boyutu | Frekans |
|---|---|---|---|
| **ACTIVE** | Brick delta (sparse) | 10-50 byte/değişiklik | Anlık |
| **WARM** | Brick delta + SVDAG root | 10-50B + 4B | Anlık + periyodik |
| **DISTANT** | SVDAG root index | 4 byte | Snapshot |
| **ARCHIVE** | Compressed SVDAG | 1-5KB | Lazy load |

### 8.2 Brick Delta Formatı

```rust
/// Tek bir brick değişikliği (network için optimize).
#[repr(C)]
pub struct BrickDelta {
    /// Sector koordinatı (3 × i16 = 6 byte).
    pub sector: I16Vec3,

    /// Brick index (0-63, 1 byte).
    pub brick_index: u8,

    /// Değişen sub-brick'ler (bitmask, 1 byte).
    pub changed_sub_bricks: u8,

    /// Yeni materyal verisi (değişen voxel'ler için).
    pub new_materials: Vec<u16>,
}

/// Ortalama değişiklik: ~10-20 byte
/// 100 değişiklik/saniye = ~1-2 KB/s bant genişliği
```

### 8.3 SVDAG Snapshot Sync

```rust
/// Uzak sector için SVDAG snapshot gönder.
pub fn send_sector_snapshot(sector: &Sector, peer: &mut Peer) {
    if let Some(root_index) = sector.svdag_root {
        // Sadece root node index'i gönder (4 byte)
        // Alıcı taraf kendi node pool'undan resolve eder
        // VEYA tüm SVDAG subtree'yi gönder (ilk bağlantı)
        peer.send(SectorSnapshot {
            sector: sector.coord,
            root_index: root_index,
            subtree_data: node_pool.export_subtree(root_index),
        });
    }
}
```

---

### 8.4 Delta Compression + Quantization

Mevcut brick delta formatı ham veri gönderiyor. **Quantization + delta encoding** ile bant genişliği **%85-90** azaltılır.

#### 8.4.1 Position Quantization

```rust
/// Pozisyon quantization — Vec3 (12 byte) → 3×short (6 byte).
/// 1cm hassasiyet (voxel engine için yeterli).
#[repr(C)]
pub struct QuantizedPosition {
    pub x: i16, // -32768..32767 → -327.68m..327.67m (1cm step)
    pub y: i16,
    pub z: i16,
}

impl QuantizedPosition {
    pub fn from_vec3(pos: Vec3) -> Self {
        Self {
            x: (pos.x * 100.0) as i16, // 1cm = 0.01m → 100 unit/m
            y: (pos.y * 100.0) as i16,
            z: (pos.z * 100.0) as i16,
        }
    }

    pub fn to_vec3(&self) -> Vec3 {
        Vec3::new(
            self.x as f32 / 100.0,
            self.y as f32 / 100.0,
            self.z as f32 / 100.0,
        )
    }
}
```

#### 8.4.2 Quaternion Compression

```rust
/// Quaternion compression — 4 float (16 byte) → smallest-three (8 byte).
/// En büyük component'i çıkar, 3 component'ı normalize et.
#[repr(C)]
pub struct CompressedQuaternion {
    /// Hangi component en büyük (2 bit).
    pub largest_index: u8,

    /// Kalan 3 component (her biri 16-bit fixed point).
    pub a: i16,
    pub b: i16,
    pub c: i16,

    /// Padding (2 byte).
    _padding: u8,
}

impl CompressedQuaternion {
    pub fn from_quat(q: Quat) -> Self {
        // En büyük component'i bul
        let abs = [q.x.abs(), q.y.abs(), q.z.abs(), q.w.abs()];
        let largest = abs.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;

        // Kalan 3 component'ı al ve normalize et
        let mut components = [q.x, q.y, q.z, q.w];
        let sign = if components[largest] >= 0.0 { 1.0 } else { -1.0 };

        Self {
            largest_index: largest as u8,
            a: (components[(largest + 1) % 4] * sign * 32767.0) as i16,
            b: (components[(largest + 2) % 4] * sign * 32767.0) as i16,
            c: (components[(largest + 3) % 4] * sign * 32767.0) as i16,
            _padding: 0,
        }
    }
}
```

#### 8.4.3 Delta Encoding

```rust
/// Delta encoding — mutlak değer yerine değişim gönder.
pub struct DeltaEncoder {
    /// Son gönderilen pozisyon (her entity için).
    last_positions: HashMap<Entity, QuantizedPosition>,

    /// Son gönderilen rotation.
    last_rotations: HashMap<Entity, CompressedQuaternion>,
}

impl DeltaEncoder {
    /// Bir entity'nin state'ini encode et.
    pub fn encode_entity(&mut self, entity: Entity, pos: Vec3, rot: Quat) -> Vec<u8> {
        let quantized_pos = QuantizedPosition::from_vec3(pos);
        let compressed_rot = CompressedQuaternion::from_quat(rot);

        let mut buffer = Vec::new();

        // Position delta
        if let Some(last_pos) = self.last_positions.get(&entity) {
            let dx = quantized_pos.x - last_pos.x;
            let dy = quantized_pos.y - last_pos.y;
            let dz = quantized_pos.z - last_pos.z;

            // Küçük delta → varint ile encode
            if dx.abs() < 128 && dy.abs() < 128 && dz.abs() < 128 {
                buffer.push(0x01); // Delta flag
                buffer.push(dx as u8);
                buffer.push(dy as u8);
                buffer.push(dz as u8);
            } else {
                buffer.push(0x02); // Full position flag
                buffer.extend_from_slice(&quantized_pos.x.to_le_bytes());
                buffer.extend_from_slice(&quantized_pos.y.to_le_bytes());
                buffer.extend_from_slice(&quantized_pos.z.to_le_bytes());
            }
        } else {
            buffer.push(0x00); // İlk mesaj — full position
            buffer.extend_from_slice(&quantized_pos.x.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.y.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.z.to_le_bytes());
        }

        // Rotation (sadece değiştiyse gönder)
        if let Some(last_rot) = self.last_rotations.get(&entity) {
            if compressed_rot != *last_rot {
                buffer.push(0x01); // Rotation changed
                buffer.extend_from_slice(&std::slice::from_ref(&compressed_rot.largest_index));
                buffer.extend_from_slice(&compressed_rot.a.to_le_bytes());
                buffer.extend_from_slice(&compressed_rot.b.to_le_bytes());
                buffer.extend_from_slice(&compressed_rot.c.to_le_bytes());
            }
        }

        // State'i güncelle
        self.last_positions.insert(entity, quantized_pos);
        self.last_rotations.insert(entity, compressed_rot);

        buffer
    }
}
```

#### 8.4.4 Bant Genişliği Karşılaştırması

| Veri | Ham (byte) | Quantized (byte) | Delta (byte) |
|---|---|---|---|
| **Position** | 12 (Vec3) | 6 (3×i16) | 1-3 (varint delta) |
| **Rotation** | 16 (Quat) | 8 (smallest-three) | 0-8 (sadece değişim) |
| **Velocity** | 12 (Vec3) | 6 (3×i16) | 1-3 (varint delta) |
| **Toplam/entity/frame** | **40** | **20** | **2-14** |

**Sonuç:** 100KB/s → **10-15KB/s** (600+ oyuncu desteklenir).

---

### 8.5 Interest Management / AOI (Area of Interest)

Her oyuncu tüm sector güncellemelerini alıyor. **AOI sistemi** ile her oyuncu sadece yakınındaki sector'ları alır.

#### 8.5.1 Spatial Partitioning

```rust
/// Area of Interest (AOI) sistemi.
/// Her oyuncu sadece belirli yarıçaptaki sector'ları alır.
pub struct InterestManager {
    /// Grid tabanlı spatial partition.
    grid: SpatialGrid,

    /// Her oyuncunun AOI yarıçapı.
    aois: HashMap<Entity, f32>,

    /// Her oyuncunun abonelik listesi (hangi sector'ları alıyor).
    subscriptions: HashMap<Entity, HashSet<SectorCoord>>,
}

/// Grid tabanlı spatial partition.
pub struct SpatialGrid {
    /// Hücre boyutu (AOI yarıçapına göre).
    cell_size: f32,

    /// Hücre → entity listesi.
    cells: HashMap<IVec2, Vec<Entity>>,

    /// Entity → hücre koordinatı.
    entity_cells: HashMap<Entity, IVec2>,
}
```

#### 8.5.2 AOI Update

```rust
impl InterestManager {
    /// Her oyuncunun AOI'sini güncelle.
    pub fn update(&mut self, dt: f32) {
        for (entity, aois) in &self.aois {
            let current_pos = self.get_entity_position(*entity);
            let current_cell = self.pos_to_cell(current_pos);

            // Eski abonelikleri kontrol et
            let old_subscriptions = self.subscriptions.entry(*entity).or_default().clone();

            // Yeni abonelikleri hesapla
            let mut new_subscriptions = HashSet::new();
            let radius_cells = (aois / self.grid.cell_size).ceil() as i32;

            for dx in -radius_cells..=radius_cells {
                for dz in -radius_cells..=radius_cells {
                    let cell = current_cell + IVec2::new(dx, dz);
                    let dist = (cell - current_cell).as_vec2().length() * self.grid.cell_size;

                    if dist <= *aois {
                        // Bu hücredeki tüm sector'ları aboneliğe ekle
                        if let Some(entities) = self.grid.cells.get(&cell) {
                            for e in entities {
                                if let Some(sector) = self.get_entity_sector(*e) {
                                    new_subscriptions.insert(sector);
                                }
                            }
                        }
                    }
                }
            }

            // Değişen abonelikleri gönder
            let added: Vec<_> = new_subscriptions.difference(&old_subscriptions).collect();
            let removed: Vec<_> = old_subscriptions.difference(&new_subscriptions).collect();

            if !added.is_empty() || !removed.is_empty() {
                self.send_subscription_updates(*entity, &added, &removed);
            }

            self.subscriptions.insert(*entity, new_subscriptions);
        }
    }
}
```

#### 8.5.3 Performans

| Metrik | AOI Yok | AOI (50-100m) | Azalma |
|---|---|---|---|
| **Bant genişliği** | 100KB/s/oyuncu | **10-20KB/s/oyuncu** | **-80-90%** |
| **Network packet** | Tüm sector'lar | Sadece yakın sector'lar | **-85%** |
| **Maks oyuncu** | ~100 | **600+** | **6×** |
| **Server CPU** | Yüksek (herkes için process) | Düşük (sadece ilgili) | **-70%** |

---

## 10. Depolama — Hybrid Tiered Storage

### 9.1 Genel Bakış

Strata, **3-kademeli hybrid depolama mimarisi** kullanır. Streaming tier'ları ile depolama tier'ları birebir eşleşir. Bu yaklaşım, **I/O verimliliği**, **disk footprint minimizasyonu** ve **crash safety** arasında optimal dengeyi sağlar.

```
┌──────────────────────────────────────────────────────────────────────┐
│                    HYBRID TIERED STORAGE                             │
├──────────────────────────────────────────────────────────────────────┤
│                                                                       │
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
│  │                               └──────────────────────────┘   │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                                                       │
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

### 9.2 Region File Formatı

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

### 9.3 Content-Addressable Deduplication

Aynı içeriğe sahip sector'ler (düz arazi, su seviyesi, mağara tavanları) **tek payload** olarak saklanır.

```rust
/// Content-addressable deduplication.
/// Aynı içeriğe sahip sector'ler tek payload olarak saklanır.
pub struct DedupTable {
    /// content_hash → region_file_offset
    index: HashMap<u64, u64>,

    /// Hash → referans sayısı (GC için)
    ref_counts: HashMap<u64, u32>,
}

impl DedupTable {
    /// Sector'ü kaydet. Aynı hash varsa mevcut payload'u kullan.
    pub fn store_sector(
        &mut self,
        region: &mut RegionFile,
        coord: SectorCoord,
        payload: &[u8],
    ) -> Result<u64> {
        let hash = xxhash64(payload);

        if let Some(&offset) = self.index.get(&hash) {
            // Aynı içerik zaten var, referans sayısını artır
            *self.ref_counts.get_mut(&hash).unwrap() += 1;
            return Ok(offset);
        }

        // Yeni payload yaz
        let offset = region.append_payload(payload)?;
        self.index.insert(hash, offset);
        self.ref_counts.insert(hash, 1);
        Ok(offset)
    }
}
```

**Beklenen tasarruf:** Tekrarlayan geometri için **%30-60** disk tasarrufu.

### 9.4 Async I/O Stratejisi (Windows-optimize)

**mmap kullanmıyoruz** — page fault async thread'i bloklar (async hazard). Windows'ta **unbuffered I/O + multi-thread** en iyi sonucu verir (11.3 GB/s NVMe SSD'de).

```rust
/// Windows-optimize async I/O stratejisi.
/// mmap kullanmıyoruz (page fault = blocking I/O).
pub struct AsyncStorageBackend {
    /// Write thread pool (tokio blocking).
    write_pool: tokio::runtime::Handle,

    /// Read thread pool (ayrı, yüksek öncelikli).
    read_pool: tokio::runtime::Handle,

    /// Flush scheduler (batch write-back).
    flush_scheduler: FlushScheduler,

    /// Prefetch manager (predictive read-ahead).
    prefetch: PrefetchManager,
}

impl AsyncStorageBackend {
    /// Sector yükle (unbuffered I/O, sector-aligned).
    pub async fn load_sector(&self, coord: SectorCoord) -> Result<Sector> {
        // 1. Önce cache'e bak
        if let Some(cached) = self.cache.get(&coord) {
            return Ok(cached);
        }

        // 2. Prefetch kuyruğuna ekle
        self.prefetch.enqueue(coord);

        // 3. Region file'dan oku (tokio::task::spawn_blocking)
        let data = tokio::task::spawn_blocking(move || {
            // Unbuffered read (FILE_FLAG_NO_BUFFERING)
            // 4KB aligned, doğrudan SSD'den
            region.read_sector_aligned(coord)
        }).await??;

        // 4. Decompress + deserialize
        let sector = self.decompress_and_deserialize(&data)?;

        // 5. Cache'e ekle
        self.cache.insert(coord, sector.clone());

        Ok(sector)
    }

    /// Sector kaydet (write-back, batch flush).
    pub fn mark_dirty(&self, coord: SectorCoord, sector: Arc<Sector>) {
        self.cache.insert(coord, sector);
        self.flush_scheduler.schedule(coord);
    }
}
```

### 9.5 SQLite Metadata Schema

```sql
-- Sector metadata (hızlı lookup, range query)
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

-- GC candidates (silinmesi gereken sector'ler)
CREATE TABLE gc_candidates (
    content_hash INTEGER PRIMARY KEY,
    ref_count    INTEGER NOT NULL,
    marked_at    INTEGER NOT NULL
);

-- World config
CREATE TABLE world_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

### 9.6 Write-Back Pipeline

```rust
/// Batch write-back scheduler.
pub struct FlushScheduler {
    /// Flush bekleyen dirty sector'ler.
    dirty_queue: VecDeque<(SectorCoord, Arc<Sector>)>,

    /// Aktif flush'lar.
    in_flight: HashMap<SectorCoord, JoinHandle<()>>,

    /// Threshold'lar.
    max_batch_size: usize,
    max_wait_time: Duration,
    flush_interval: Duration,
}

impl FlushScheduler {
    /// Periyodik flush döngüsü.
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

    /// Batch flush (region'a göre grupla).
    async fn flush_batch(&mut self, batch: Vec<(SectorCoord, Arc<Sector>)>) {
        // 1. Region'lara göre grupla
        let by_region = self.group_by_region(&batch);

        // 2. Her region için paralel compress + write
        let tasks: Vec<_> = by_region.into_iter().map(|(region, sectors)| {
            tokio::task::spawn_blocking(move || {
                // a. Compress (zstd, adaptive level)
                // b. Dedup check (content hash)
                // c. Region file'a yaz (append, 4KB aligned)
                // d. SQLite metadata güncelle (WAL transaction)
            })
        }).collect();

        // 3. Tüm region'lar bitene kadar bekle
        for task in tasks {
            task.await.unwrap();
        }

        // 4. Dirty flag'leri temizle
        self.clear_dirty_flags(&batch);
    }
}
```

### 9.7 Tier-Bazlı Compression Stratejisi

| Tier | Compression | Hedef | Beklenen Oran |
|---|---|---|---|
| **WARM (cache)** | zstd level 1 | Hız > boyut | 3:1 |
| **DISTANT** | zstd level 3 | Denge | 8:1 |
| **ARCHIVE** | zstd level 19 | Boyut > hız | 15:1 |
| **Dedup payload** | zstd level 3 + dedup | Tekrar eden geometri | 20:1+ |

### 9.8 Garbage Collection & Compaction

```rust
/// Periyodik GC döngüsü.
pub struct GarbageCollector {
    db: rusqlite::Connection,
    dedup_table: DedupTable,
}

impl GarbageCollector {
    /// Ref count sıfıra düşen payload'ları temizle.
    pub async fn run_gc(&mut self) {
        // 1. SQLite'dan GC candidate'leri al
        let candidates = self.db.prepare(
            "SELECT content_hash FROM gc_candidates WHERE ref_count = 0"
        ).unwrap();

        // 2. Region file'lardan payload'ları sil
        for hash in candidates {
            self.dedup_table.remove_payload(hash);
        }

        // 3. Region file compaction (fragmentation temizle)
        self.compact_regions().await;

        // 4. SQLite VACUUM (WAL checkpoint)
        self.db.execute("PRAGMA wal_checkpoint(TRUNCATE)", []).unwrap();
    }

    /// Region file compaction.
    async fn compact_regions(&mut self) {
        // Her region file için:
        // 1. Canlı payload'ları yeni dosyaya kopyala
        // 2. Eski dosyayı sil, yenisini rename et
        // 3. SQLite offset'leri güncelle (transaction)
    }
}
```

### 9.9 Performans Hedefleri

| Metrik | Hedef | Not |
|---|---|---|
| **Hot load (cache hit)** | <0.1ms | RAM'den doğrudan |
| **Warm load (cache miss)** | <2ms | Decompress + deserialize |
| **Cold load (disk)** | <5ms | Unbuffered I/O + decompress |
| **Batch save (64 sector)** | <50ms | Paralel compress + SQLite WAL |
| **Write throughput** | >500MB/s | Multi-thread unbuffered |
| **Dedup tasarrufu** | %30-60 | Tekrarlayan geometri |
| **Crash recovery** | <100ms | SQLite WAL replay |
| **GC cycle** | <200ms | Periyodik, background |

---

### 9.10 Content-Defined Chunking (GearHash)

Sabit sector sınırları = deduplication verimsiz. **Content-defined chunking** (HuggingFace Xet yaklaşımı) ile sınır içerik hash'ine göre belirlenir, aynı içerik farklı sector'lerde bile dedup edilir.

#### 9.10.1 Gear Hash ile Sınır Belirleme

```rust
/// Content-defined chunking — GearHash ile sınır belirleme.
/// Sabit sector sınırları yerine, içerik hash'ine göre chunk sınırları belirlenir.
pub struct ContentDefinedChunker {
    /// Gear rolling hash state.
    gear_state: u64,

    /// Minimum chunk boyutu (byte).
    min_chunk_size: u32,

    /// Maksimum chunk boyutu (byte).
    max_chunk_size: u32,

    /// Hedef chunk boyutu (ortalama).
    target_chunk_size: u32,

    /// Chunk boundary mask (hash'in hangi bit'leri kontrol edilir).
    boundary_mask: u64,
}

impl ContentDefinedChunker {
    /// Gear rolling hash ile chunk sınırı kontrol et.
    pub fn should_split(&mut self, byte: u8) -> bool {
        // Gear rolling hash güncelle
        self.gear_state = (self.gear_state << 1) ^ GEAR_TABLE[byte as usize];

        // Boundary mask ile kontrol (örneğin son 13 bit 0 ise split)
        (self.gear_state & self.boundary_mask) == 0
    }

    /// Bir sector'ü content-defined chunk'lara böl.
    pub fn chunk_sector(&mut self, sector_data: &[u8]) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let mut chunk_start = 0;
        let mut chunk_size = 0;

        for (i, &byte) in sector_data.iter().enumerate() {
            chunk_size += 1;

            // Minimum chunk boyutuna ulaşıldıysa split kontrol et
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

        // Son chunk (kalan veri)
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

/// Gear hash lookup table (256 entry, random 64-bit değerler).
const GEAR_TABLE: [u64; 256] = [
    // 256 random 64-bit değer (pre-computed)
    0x1234567890ABCDEF, 0xFEDCBA0987654321, // ...
];
```

#### 9.10.2 MerkleHash ile Integrity Verification

```rust
/// Merkle tree — chunk integrity verification.
/// Her chunk'ın hash'i bir Merkle tree'de birleştirilir.
pub struct MerkleTree {
    /// Leaf node'lar (chunk hash'leri).
    leaves: Vec<[u8; 32]>,

    /// Internal node'lar.
    nodes: Vec<[u8; 32]>,

    /// Root hash (tüm ağacın özeti).
    root: [u8; 32],
}

impl MerkleTree {
    /// Chunk hash'lerinden Merkle tree oluştur.
    pub fn from_chunks(chunks: &[Chunk]) -> Self {
        let mut leaves: Vec<[u8; 32]> = chunks.iter().map(|c| c.hash).collect();

        // Bottom-up Merkle tree
        let mut level = leaves.clone();
        let mut nodes = Vec::new();

        while level.len() > 1 {
            let mut next_level = Vec::new();
            for chunk in level.chunks(2) {
                let parent = if chunk.len() == 2 {
                    blake3::hash(&[chunk[0], chunk[1]].concat()).into()
                } else {
                    chunk[0] // Tek leaf, olduğu gibi yukarı taşı
                };
                next_level.push(parent);
                nodes.push(parent);
            }
            level = next_level;
        }

        Self {
            leaves,
            nodes,
            root: level[0],
        }
    }

    /// Bir chunk'ın integrity'sini doğrula.
    pub fn verify_chunk(&self, chunk_index: usize, chunk_data: &[u8]) -> bool {
        let expected_hash = self.leaves[chunk_index];
        let actual_hash = blake3::hash(chunk_data).into();
        expected_hash == actual_hash
    }
}
```

#### 9.10.3 Deduplication ile Entegrasyon

```rust
/// Content-defined chunking + deduplication.
/// Aynı chunk'lar farklı sector'lerde bile tek kez saklanır.
pub struct ChunkedDedupStorage {
    /// Chunk store — content-addressable.
    chunk_store: HashMap<[u8; 32], ChunkData>,

    /// Sector → chunk listesi mapping.
    sector_chunks: HashMap<SectorCoord, Vec<[u8; 32]>>,

    /// Chunk referans sayacı (GC için).
    chunk_ref_counts: HashMap<[u8; 32], u32>,
}

impl ChunkedDedupStorage {
    /// Sector'ü chunk'lara böl ve kaydet.
    pub fn store_sector(&mut self, coord: SectorCoord, data: &[u8]) {
        // 1. Content-defined chunking
        let mut chunker = ContentDefinedChunker::new();
        let chunks = chunker.chunk_sector(data);

        // 2. Her chunk'ı dedup ile kaydet
        let mut chunk_hashes = Vec::new();
        for chunk in &chunks {
            let hash = chunk.hash;

            // Chunk zaten var mı?
            if !self.chunk_store.contains_key(&hash) {
                // Yeni chunk — kaydet
                self.chunk_store.insert(hash, ChunkData {
                    data: data[chunk.offset as usize..(chunk.offset + chunk.size) as usize].to_vec(),
                    size: chunk.size,
                });
                self.chunk_ref_counts.insert(hash, 1);
            } else {
                // Mevcut chunk — referans sayısını artır
                *self.chunk_ref_counts.get_mut(&hash).unwrap() += 1;
            }

            chunk_hashes.push(hash);
        }

        // 3. Sector → chunk mapping kaydet
        self.sector_chunks.insert(coord, chunk_hashes);
    }

    /// Sector'ü yükle (chunk'lardan birleştir).
    pub fn load_sector(&self, coord: &SectorCoord) -> Option<Vec<u8>> {
        let chunk_hashes = self.sector_chunks.get(coord)?;

        // Chunk'lardan sector verisini birleştir
        let mut data = Vec::new();
        for hash in chunk_hashes {
            let chunk = self.chunk_store.get(hash)?;
            data.extend_from_slice(&chunk.data);
        }

        Some(data)
    }
}
```

#### 9.10.4 Performans

| Metrik | Sabit Sector | Content-Defined | Fark |
|---|---|---|---|
| **Dedup oranı** | %30-60 | **%50-80** | **+20-30%** |
| **Storage efficiency** | Sector boundary waste | **Zero waste** | **+15-25%** |
| **Integrity check** | xxHash64 (sector) | **BLAKE3 Merkle** | **Güvenli** |
| **Chunk overhead** | Yok | ~4 byte/chunk | Minimal |

---

## 11. Performans Hedefleri

### 10.1 Render

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

### 10.2 Fizik

| Metrik | Hedef | Not |
|---|---|---|
| Collider güncelleme (tek voxel) | <0.1ms | `set_voxel` + `propagate_voxel_change` |
| Collider güncelleme (bölgesel) | <1ms | `split_with_box` + rebuild |
| Collider güncelleme (tam rebuild) | <5ms | 32×128×32 sector için |
| Boundary sync (2 sector) | <0.5ms | `combine_voxel_states` |
| Character ground check | <0.05ms | XBrickMap 4-level skip |
| Broad-phase (ACTIVE) | <2ms | BVH traversal, 100+ sector |
| Falling sand (1K particle) | <3ms | Custom spatial hash |
| Fracture (patlama) | <10ms | Voronoi + flood-fill + rigid-body spawn |

### 10.3 Network

| Metrik | Hedef | Not |
|---|---|---|
| Delta sync (ham) | <2KB/s/oyuncu | Brick delta |
| Delta sync (quantized) | **<200B/s/oyuncu** | Position quantization + delta encoding |
| Snapshot | 1-5KB/sector | SVDAG compressed |
| TPS | 20+ | Server-authoritative |
| AOI bant genişliği | **10-20KB/s/oyuncu** | Sadece yakın sector'lar |
| Maks oyuncu | **600+** | AOI + quantization ile |

### 10.4 Streaming

| Metrik | Hedef | Not |
|---|---|---|
| Bake süresi | <15ms | GPU compute (pipeline stall mitigate) |
| Unbake süresi | <5ms | SVDAG → Brickmap |
| Pop-in | Yok | Tier 2 yumuşak geçiş |
| Predictive preload | %80 azalma | Hareket vektörü tahmini |
| Shallow SVDAG VRAM | **%5** | Sadece görünür tile'lar |
| Shallow SVDAG hız | **2-4×** | Derin SVDAG'e kıyasla |

### 10.5 Storage

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

## 12. Crate Organizasyonu

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

## 13. Uygulama Sırası

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

## 14. Riskler ve Mitigasyon

| Risk | Olasılık | Etki | Mitigasyon |
|---|---|---|---|
| GPU SVDAG bake süresi >15ms | Orta | Yüksek | Kademeli bake (her frame küçük bölüm) |
| Visibility buffer 64-bit yetersiz | Düşük | Orta | 128-bit'e genişlet (2× u64) |
| WGSL 64-bit atomik desteği eksik (Metal) | Yüksek | Düşük | `vec2<u32>` + `atomicStoreMin` fallback (depth test için yeterli) |
| Node pool allocator Metal'da çalışmıyor | Orta | Orta | 32-bit `atomic<u32>` allocator kullan (node index'leri u32'ye sığar) |
| Rapier Voxels deneysel sınırlamalar | Orta | Orta | Parry 0.26'da Voxels vs Voxels düzeltildi; custom physics layer fallback olarak hazır |
| SVDAG node pool fragmentasyonu | Düşük | Yüksek | Periyodik compact + defrag |
| Tier 2'de çift bellek kullanımı | Yüksek | Orta | Sadece gerekli sector'larda Tier 2 |
| Network snapshot boyutu büyük | Orta | Orta | Delta compression + LOD bazlı gönderim |
| mmap async thread blokluyor (page fault) | Yüksek | Orta | Unbuffered I/O + spawn_blocking kullan (mmap'ten kaçın) |
| SQLite WAL dosyası büyüyor | Düşük | Düşük | Periyodik wal_checkpoint(TRUNCATE) |
| Dedup hash collision | Çok düşük | Yüksek | xxHash64 yeterli (collision prob ~10⁻¹⁹) |
| Region file fragmentasyonu | Orta | Orta | Periyodik compaction (GC cycle) |
| SOA layout migration cost | Orta | Düşük | AOS→SOA geçiş incremental, runtime'da seçilebilir |
| Transform-aware SVDAG hash overhead | Düşük | Düşük | 48 transform lookup O(1), lookup table ile |
| Shallow SVDAG streaming stutter | Orta | Orta | Async preload + budget management |
| Vertex pool fragmentation | Orta | Orta | Free list merge + periyodik defrag |
| Foveated rendering artefact | Düşük | Orta | Smooth transition between zones, eye tracking (opsiyonel) |
| GearHash chunk boundary instability | Düşük | Düşük | Min/max chunk size ile stabilize |
| BFS queue overflow (çok ışık) | Orta | Orta | Max queue size + priority-based pruning |
| SIMD desteği eksik (eski CPU) | Düşük | Düşük | Scalar fallback (15x yavaş ama çalışır) |
| Colored light removal over-zero | Düşük | Düşük | Per-channel boundary tracking (voxel-light limitation) |
| Clustered GI cluster explosion | Orta | Orta | Max cluster count + LOD-based merge |
| SVDAG cone tracing noise | Orta | Orta | Temporal accumulation + TAA-style blending |
| Day/night cycle stutter | Düşük | Orta | Gradual ambient shift (per-frame delta) |
| Neural Irradiance training time | Yüksek | Düşük | Offline training, runtime sadece inference |

---

## 15. Alternatifler ve Neden Reddedildi

| Alternatif | Neden Reddedildi |
|---|---|
| **Saf SVO** | Edit cost çok yüksek, cache performansı kötü, network sync karmaşık |
| **Clipmap** | Multiplayer'da her oyuncu için ayrı clipmap = kaos, mağara için uygun değil |
| **Tree64** | Hâlâ chunk hierarchy kullanıyor, edit zor |
| **Flat Vec<u16>** | Bellek verimsiz, LOD/ ray tracing doğal değil |
| **Global SVDAG** | Derin traversal, çoklu indirect jump, GPU cache miss |
| **WGSL native u64 her yerde** | Metal'da `SHADER_INT64_ATOMIC_ALL_OPS` yok → `vec2<u32>` fallback gerekli |
| **64-bit atomic node allocator** | Metal'da `atomicAdd<u64>` yok → 32-bit `atomic<u32>` allocator yeterli (node index u32) |
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

---

## 16. Sözlük

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
| **Visibility Buffer** | 64-bit, tüm render pass'lerinin ortak yazdığı buffer (native u64 veya vec2<u32> fallback) |
| **Hi-Z** | Hierarchical Z-buffer, occlusion culling için |
| **Bake** | Brickmap → SVDAG dönüşümü |
| **Unbake** | SVDAG → Brickmap dönüşümü |
| **Left-packed** | Boş entry'lerin atlandığı, sıkıştırılmış dizi düzeni |
| **Popcnt** | Population count — set bit sayma işlemi |
| **Region File** | 32×32×1 sector grubu içeren binary dosya (.strata) |
| **Content-Addressable** | İçerik hash'i ile adresleme, deduplication için |
| **Write-Back** | Lazy flush stratejisi, dirty cache'ten arka plan yazma |
| **WAL** | Write-Ahead Logging, SQLite crash recovery mekanizması |
| **Unbuffered I/O** | OS cache bypass, doğrudan SSD'den okuma/yazma (FILE_FLAG_NO_BUFFERING) |
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
| **Quantization** | Veri boyutunu azaltmak için hassasiyet düşürme (position: 12B→6B, quaternion: 16B→8B) |
| **Delta Encoding** | Mutlak değer yerine değişim gönderme — network bant genişliği optimizasyonu |
| **AOI** | Area of Interest — her oyuncu sadece yakınındaki sector'ları alır |
| **Content-Defined Chunking** | GearHash ile içerik bazlı chunk boundary belirleme (sabit sınır yerine) |
| **Merkle Tree** | BLAKE3 hash'leri ile chunk integrity verification |
| **GearHash** | Rolling hash fonksiyonu — content-defined chunking için |
| **BFS Flood-Fill** | Breadth-First Search ile ışık yayılımı — her voxel'i bir kez ziyaret |
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
