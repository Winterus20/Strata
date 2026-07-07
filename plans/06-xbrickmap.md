# 06 — XBrickMap Veri Yapısı (Cubic Chunks)

## 1. XBrickMap — Aktif Alan Veri Yapısı

### 1.1 Hiyerarşik Yapı

XBrickMap, [VoxelRT](https://github.com/dubiousconst282/VoxelRT) karşılaştırmasında **eXtendedBrickMap** (3-level grid + 4³ occupancy bitmask + left-packed storage + `popcnt` random access) varyantına dayanır; ayrıntı: [voxel ray tracing guide](https://dubiousconst282.github.io/2024/10/03/voxel-ray-tracing/). Strata: 3 seviyeli hiyerarşik bitmask (Sector → Brick → Sub-brick) + **4-seviyeli palet zinciri** (SectorPalette → BrickPalette → SubBrick). Block type ID (`u16`) ve state variant **05-block-registry.md** ile paylaşılır; voxel bellekte yalnızca sektör-local `u8` indeks tutulur.

Sistem "Sınırsız Yükseklik" (Cubic Chunks) vizyonuyla tasarlanmıştır. Dünya, yatayda ve dikeyde **32×32×32** boyutlarında "Sektör"lere (Sector) bölünür. Bu boyut (32³), 8³ boyutundaki tuğlalardan (brick) tam 64 adet (4³) barındırdığı için tek bir `u64` maskesinin içine kusursuzca sığar.

```
Sector (32×32×32 = 32.768 voxel)
  ├── Sector Bitmask: u64
  │   └── 4³ = 64 brick'in doluluk bilgisi (her bit 1 brick)
  │       └── Left-packed: boş brick'ler dizide yer kaplamaz
  │
  ├── Brick[0..N] (N ≤ 64, sadece dolu olanlar)
  │   ├── Brick Bitmask: u64
  │   │   └── 4³ = 64 sub-brick'in doluluk bilgisi
  │   │       └── Her sub-brick = 2³ = 8 voxel
  │   │
  │   ├── Sub-brick[0..M] (M ≤ 64, sadece dolu olanlar)
  │   │   ├── Sub-brick Bitmask: 8-bit
  │   │   │   └── 2³ = 8 voxel'in doluluk bilgisi
  │   │   └── Palette Indices (8×8-bit = u64, sektör-local u8 indeks)
  │   │
  │   └── Brick Paleti (opsiyonel, max 16 materyal)
  │       └── Sadece brick 2+ farklı materyal içeriyorsa var
  │
  ├── SectorPalette (sektör başına, max 256 entry)
  │   └── u8 local_index → PaletteEntry { block_type: u16, variant: u16 }
  │       └── Bkz. 05-block-registry.md §14
  │
  ├── Sector LightMap (32×32×32 = 32KB)
  │   └── Her voxel için 4-bit skylight + 4-bit blocklight = 8-bit
  │       └── GPU'ya texture_3d (WGSL) olarak aktarılır
  │
   └── Sector Mip Chain (GPU texture_3d, sector seviyesinde LOD)
       ├── Mip-0: 32×32×32 (1³ = full detail)
       ├── Mip-1: 16×16×16 (2³ voxel blokları)
       ├── Mip-2: 8×8×8 (4³ voxel blokları)
       ├── Mip-3: 4×4×4 (8³ voxel blokları)
       ├── Mip-4: 2×2×2 (16³ voxel blokları)
       └── Mip-5: 1×1×1 (32³ = sector ortalaması)
```

**Mip ve Işık Neden Sector Seviyesinde?**
- Mip: brick traversal'da cache miss yaratmamak için sector seviyesinde. GPU'da hardware bilinear filter ile LOD okunur.
- Işık: geometriden çok daha sık değiştiği için (günbatımı, meşale ekleme) ayrı texture. GPU compute shader'da wavefront BFS ile güncellenir, `queue.write_buffer()` ile nokta atışı aktarılır.

**Neden 32×32×32 (Cubic)?**
- Minecraft gibi eski nesil oyunların aksine, dikeyde yükseklik sınırı (örn: 256, 384) yoktur. Oyuncu ne kadar yükseğe çıkarsa veya ne kadar derine inerse, sadece oradaki 32x32x32 sektörler yüklenir. Sonsuz gökyüzü ve devasa mağaralar tam performansla desteklenir.
- 32³ hacimde tam 64 adet (4³) brick (8³) bulunur. 64 sayısı tek bir 64-bit (u64) yazmaç (register) içine milimetrik olarak oturduğu için ray-tracing ışın hızı muazzamdır. 4 slab (dilim) hiyerarşisine gerek kalmaz, okuma süreci hızlanır.

### 1.2 Bellek Hesabı

| Bileşen | Boyut | Not |
|---|---|---|
| Sector metadata (struct) | 16 byte | `#[repr(C)]` u64(8B) + Option\<NonZeroU32\>(4B) + padding(4B) |
| Brick dizisi | 0-64 × ~56-80B | Left-packed, Paletli sıkıştırma ile |
| — Brick bitmask | 8 byte | Her brick için |
| — Sub-brick dizisi | 0-64 × ~16B | `u8 mask + u64 indices` (sektör-local u8) |
| — Brick Paleti | 0-16 byte | Sadece 2+ materyal varsa |
| SectorPalette | ~120-1024 B | Tipik 30-60 entry × 4B; max 256 entry (1 KB) |
| Sector LightMap | 32KB (dolu) | 32³ × 8-bit, sadece dolu sektörlerde |
| Sector Mip Chain | ~36.6KB (dolu) | 32³+16³+8³+4³+2³+1³ = ~36.6K, GPU'da generated |
| **Tam dolu sector** | ~70-80KB | 32.768 voxel (mip chain + light dahil) |
| **Ortalama arazi** | ~25-35KB | ~50% boşluk, tek-tip palet avantajı |
| **Boş sector** | 16 byte | Sadece Sector struct (LightMap yok) |

**Karşılaştırma:** Eski düz 16x256x16 Chunk 128KB kaplarken (içi boşken bile), Cubic XBrickMap boş gökyüzünü sadece 16 byte'a indirger. Dolu kısımlarda 4-seviyeli palet sayesinde tek-tip brick sadece 8 byte (bitmask) yer kaplar.

### 1.3 Veri Yapısı (Rust - SlotMap + Global SOA & Bevy ECS)

Her Sektörün kendi içinde `Vec` tutması, binlerce Sektör yüklendiğinde korkunç bir bellek parçalanmasına (Heap Fragmentation) yol açar. Bu yüzden brick verileri merkezi bir `GlobalBrickPool` (Bkz 2.5) içinde **SlotMap** ile tutulur. `SlotMap`, versiyonlu key'ler sayesinde dangling pointer olmadan O(1) insert/remove sağlar ve free-list ile sıfır heap fragmentation verir.

Sector'ün dünya koordinatı `SectorPosition(IVec3)` ayrı bir struct'tır. Sector'den pozisyona erişim `SectorMap` resource'u (Morton kodu ile HashMap) üzerinden yapılır. Bu SOA yaklaşımı sayesinde world query ile sektörler hem uzaysal konumlarıyla hem de brick verileriyle sorgulanabilir.

`Sector` ise sadece bir adres (pool_index) barındıran hafif (~16 Byte) bir struct'tır. Dünya koordinatındaki yeri ayrı bir `SectorPosition`'ta tutulur (SOA prensibi). Sector'den pozisyona erişim için `SectorMap` resource'u kullanılır.

ECS'de güncelleme takibi için **Bevy Change Detection** kullanılır: `Query<&Sector, Changed<Sector>>` ile sadece değişen sektörler işlenir.

### 1.4 `CompressedChunkData` (ECS / meshing snapshot)

`03-ecs-architecture.md` içindeki `SectorData(Arc<CompressedChunkData>)` bu tipe referans verir. Canlı dünya `Sector` + `GlobalBrickPool` + `SectorPalette` üzerinde kalır; meshing, network delta veya SVDAG bake **değişim anında** immutable snapshot alır.

```rust
/// Tek sektörün thread-safe, paylaşılabilir kopyası (Arc clone = refcount).
/// GlobalBrickPool'dan left-packed brick verisi + sektör paleti kopyalanır.
#[derive(Clone)]
pub struct CompressedChunkData {
    pub coord: IVec3,
    pub sector_mask: u64,
    /// Sektöre ait brick mask/sub-brick/palette dilimleri (pool'tan snapshot)
    pub bricks: Vec<BrickSnapshot>,
    pub palette: heapless::Vec<PaletteEntry, 256>,
    pub data_version: u64,
}

#[derive(Clone)]
pub struct BrickSnapshot {
    pub brick_mask: u64,
    pub sub_bricks: Vec<SubBrick>,
    pub brick_palette: Option<BrickPalette>,
}

impl CompressedChunkData {
    pub fn empty() -> Self {
        Self {
            coord: IVec3::ZERO,
            sector_mask: 0,
            bricks: Vec::new(),
            palette: heapless::Vec::new(),
            data_version: 0,
        }
    }

    /// `Sector` + pool + palette'ten snapshot (edit sonrası veya `NeedsRemesh` öncesi).
    pub fn snapshot_from_pool(
        coord: IVec3,
        sector: &Sector,
        pool: &GlobalBrickPool,
        palette: &SectorPalette,
        version: u64,
    ) -> Self { /* pack_sector_bricks + palette clone */ }
}
```

**Kurallar:** Snapshot oluşturma main thread veya tek-writer kanalından; mesh thread yalnızca okur. `data_version` ile `SectorMeshState` stale mesh tespiti (`03` §2.1).

```rust
use bevy::prelude::*;
use slotmap::{SlotMap, new_key_type};
use std::num::NonZeroU32;
use dashmap::DashMap;
use glam::IVec3;

new_key_type! { pub struct BrickKey; }

/// 32×32×32 voksellik, sınırsız yükseklik destekli kübik sektör
/// #[repr(C)] GPU SSBO'su ile layout uyumu için şart.
/// #[repr(C)] fixed layout ile unsafe pointer cast'ler güvenlidir.
/// bytemuck ile GPU'ya zero-copy aktarım mümkün.
/// Toplam: 16 byte = u64(8B) + Option<NonZeroU32>(4B, niche) + padding(4B)
#[derive(Component, Clone, Copy)]
#[repr(C)]
pub struct Sector {
    /// 64 adet 8x8x8 brick'in hangileri dolu (O(1) Popcnt)
    pub sector_mask: u64,
    /// Bu Sektörün alt verilerinin Global BrickPool'da başladığı indeks
    /// None = boş sector (hiç brick yok), niche sayesinde 4 byte
    pub pool_index: Option<NonZeroU32>,
}

/// Sektör-local palet — u8 voxel indeksi → (block_type, variant).
/// Sector struct'ı 16 byte tutulduğu için ayrı Component (SOA).
/// Bkz. 05-block-registry.md §14.2
#[derive(Component)]
pub struct SectorPalette {
    pub entries: heapless::Vec<PaletteEntry, 256>,
    /// Edit-time reverse lookup; save'de serialize edilmez.
    pub reverse: HashMap<PaletteEntry, u8>,
}

/// 05-block-registry.md ile aynı tip (crate `core::registry::palette`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PaletteEntry {
    pub block_type: u16,
    pub variant: u16,
}

/// Sector'ün dünya koordinatı (SOA: ayrı struct)
/// Sorgulama: Query<(&Sector, &SectorPosition)> ile
#[derive(Component, Clone, Copy)]
pub struct SectorPosition(pub IVec3);

/// Thread-safe spatial index: sector koordinatı → Entity
/// DashMap: sharded concurrent HashMap, read-heavy (%99 okuma) için optimize
/// Morton kodu (Z-order curve) ile uzaysal lokalite
/// 21-bit x 3 = 63-bit, u64'e sığar; 1 bit boş (flag olarak kullanılabilir)
///
/// Neden DashMap?
/// - RwLock<HashMap>: tüm okumalar tek kilitten geçer → contention
/// - DashMap: shard başına kilit, %99 okuma + %1 yazma için ideal
/// - crossbeam::SkipMap: lock-free ama %15 yavaş okuma
/// - Lock-free CAS: karmaşık, uğraştırmaya değmez (%5 contention altında)
#[derive(Resource)]
pub struct SectorMap {
    map: DashMap<u64, Entity>,
}

impl SectorMap {
    /// Signed → Unsigned bias: [-2²⁰, 2²⁰-1] → [0, 2²¹-1]
    /// Bitcast (two's complement) kullanılmaz çünkü -1 ve 0
    /// Morton uzayında zıt uçlara düşerek spatial locality'yi kırar.
    const BIAS: i64 = 1 << 20;

    /// Morton encode: 21-bit x,y,z → 63-bit interleaved key
    /// "Magic Bits" yöntemi: branchless, LUT'suz, ~1ns/call
    fn morton_encode(x: u32, y: u32, z: u32) -> u64 {
        fn split3(a: u64) -> u64 {
            let mut x = a & 0x1fffff;
            x = (x | x << 32) & 0x1f00000000ffff;
            x = (x | x << 16) & 0x1f0000ff0000ff;
            x = (x | x <<  8) & 0x100f00f00f00f00f;
            x = (x | x <<  4) & 0x10c30c30c30c30c3;
            x = (x | x <<  2) & 0x1249249249249249;
            x
        }
        split3(x as u64) | split3(y as u64) << 1 | split3(z as u64) << 2
    }

    /// IVec3 → u64 Morton key (bias ile branchless dönüşüm)
    /// Kapsama: 21-bit/axis = ±1,048,576 sektör = ±33M blok
    fn key(pos: IVec3) -> u64 {
        let x = (pos.x as i64 + Self::BIAS) as u64 & 0x1fffff;
        let y = (pos.y as i64 + Self::BIAS) as u64 & 0x1fffff;
        let z = (pos.z as i64 + Self::BIAS) as u64 & 0x1fffff;
        Self::morton_encode(x as u32, y as u32, z as u32)
    }
    pub fn get(&self, pos: IVec3) -> Option<Entity> {
        self.map.get(&Self::key(pos)).map(|e| *e)
    }
    pub fn insert(&self, pos: IVec3, entity: Entity) {
        self.map.insert(Self::key(pos), entity);
    }
    pub fn remove(&self, pos: IVec3) -> Option<Entity> {
        self.map.remove(&Self::key(pos)).map(|(_, e)| e)
    }
}

/// Tüm sektörlerin alt verilerini tutan SlotMap Havuzu
/// SlotMap: O(1) insert/remove, versiyonlu key, zero fragmentation
///
/// SOA (Structure of Arrays) performansı için SecondaryMap kullanılır:
/// - SlotMap<BrickKey, BrickData>: random access (AOS)
/// - SecondaryMap<BrickKey, T>: slot-indexed Vec (SOA), SlotMap ile otomatik senkron
///   Insert/remove'da manuel senkronizasyon GEREKMEZ — SecondaryMap,
///   SlotMap'ın slot indeksini kullanır, aynı versiyonlama ile korunur.
#[derive(Resource)]
pub struct GlobalBrickPool {
    // SlotMap: AOS, random access için optimize
    // Internal free-list + versioning ile zero fragmentation
    pub bricks: SlotMap<BrickKey, BrickData>,

    // SOA: SecondaryMap ile cache-friendly random access
    // SecondaryMap = slot-indexed Vec<V> + version check
    // SlotMap ile otomatik senkron: insert(key,val) / get(key) / get_mut(key)
    // NOT: remove gerekmez — versiyon eşleşmezse None döner
    // NOT: manuel free_list gerekmez — SlotMap internal yönetir
    pub brick_masks: SecondaryMap<BrickKey, u64>,
    pub sub_brick_offsets: SecondaryMap<BrickKey, u32>,
    pub sub_bricks: SecondaryMap<BrickKey, SubBrick>,
    pub palettes: SecondaryMap<BrickKey, BrickPalette>,
}

/// Brick başına data (SlotMap değeri olarak)
pub struct BrickData {
    pub brick_mask: u64,
    pub sub_brick_offset: u32,
}

impl GlobalBrickPool {
    /// Yeni brick ekle. İki aşamalı (tek adım mümkün değil — borrow checker):
    ///   SlotMap::insert_with_key(|key| { sm.insert(key, val); BrickData })
    ///   ❌ self.bricks (kapanış) + self.brick_masks (kapanış içi) aynı anda
    ///      alınamaz.
    ///   ✅ self.bricks.insert(...) → key al → self.brick_masks.insert(key, val)
    ///
    /// SecondaryMap::insert içsel olarak:
    ///   - slot indeksi > len ise Vec.extend() (amortized O(1))
    ///   - versiyon kontrolü (key versiyonu slot versiyonuyla eşleşir)
    ///   - varsa eski değeri replace, yoksa yeni slot oluştur
    /// İkinci adımın maliyeti: ~3 cycle (sadece slot.write + num_elems++)
    pub fn alloc_brick(&mut self) -> BrickKey {
        let key = self.bricks.insert(BrickData::default());
        self.brick_masks.insert(key, 0u64);
        self.sub_brick_offsets.insert(key, 0u32);
        key
    }

    /// Brick'i kaldır. SlotMap internal free-list'e ekler.
    /// SecondaryMap: remove'a gerek yok — versiyon eşleşmezse
    /// SecondaryMap::get(key) otomatik None döndürür.
    /// Slot reuse edildiğinde yeni key'in versiyonu eski key'den
    /// yüksek olacağı için SecondaryMap'teki eski değer otomatik
    /// "görünmez" olur.
    pub fn free_brick(&mut self, key: BrickKey) {
        self.bricks.remove(key);
    }

    /// Random access: SecondaryMap[key] ile SOA'dan direkt oku
    /// Maliyet: 1 bounds check + 1 version check = ~2 cycle
    pub fn get_mask(&self, key: BrickKey) -> Option<&u64> {
        self.brick_masks.get(key)
    }

    /// Mutable random access: mevcut brick'in mask'ini değiştir
    /// alloc_brick'te zaten initialize edildiği için her zaman Some döner
    pub fn set_mask(&mut self, key: BrickKey, mask: u64) {
        if let Some(m) = self.brick_masks.get_mut(key) {
            *m = mask;
        }
    }

    /// GPU upload için sector'a ait brick'leri paketle
    /// Sector'un u64 mask'ı hangi brick'lerin aktif olduğunu söyler
    /// Sadece aktif brick'lerin verisi GPU'ya upload edilir
    ///
    /// Hot path notu: SecondaryMap::get_mut() = bounds check + version check
    /// = ~2 cycle/call. 1000 brick için ~2000 cycle = noise.
    /// Eğer ileride 100K+ brick/frame olursa:
    ///   Kademe 1: get_unchecked_mut(key) ile version check'i atla
    ///   Kademe 2: slot index ile raw Vec (BrickSOA), bounds check de yok
    ///   Kademe 3: GPU compute shader'da direkt VRAM'den oku
    pub fn pack_sector_bricks(&self, sector: &Sector) -> Vec<u64> {
        // pool_index'den başlayarak sector_mask'taki set bit'leri tara
        let base = sector.pool_index.map(|nz| nz.get()).unwrap_or(0);
        let mut masks = Vec::with_capacity(sector.sector_mask.count_ones() as usize);
        let mut remaining = sector.sector_mask;
        while remaining != 0 {
            let bit = remaining.trailing_zeros();
            remaining &= remaining - 1;  // clear lowest set bit
            // BrickKey hesapla: base + bit sırası (left-packed)
            // ... (implementation detail)
        }
        masks
    }
}

/// 8 voxel = Sub-brick
pub struct SubBrick {
    pub voxel_mask: u8,
    /// 8 × 8-bit sektör-local palette index (SectorPalette'e bakar).
    /// Option<NonZeroU64>: None = tek tip (default local index 0 = AIR)
    pub indices: Option<NonZeroU64>,
}

/// Brick bazlı lokal palet (opsiyonel)
/// Sadece brick 2+ farklı materyal içeriyorsa oluşturulur
pub struct BrickPalette {
    /// SectorPalette local index'lerinin brick içi remap'i (max 16)
    pub materials: heapless::Vec<u8, 16>,
}

/// Render / RT için materyal özellikleri — BlockRegistry'den türetilir.
/// index = block_type_id (u16, max 65535). Voxel bellekte bu indeks tutulmaz.
/// Bkz. 05-block-registry.md §14.3
#[derive(Resource)]
pub struct GlobalPalette {
    pub materials: Vec<MaterialDef>,
}

pub struct MaterialDef {
    pub name: StringId,
    pub color: [u8; 3],
    pub emission: u8,
    pub opacity: u8,
}

/// Sector LightMap - ışık verisi geometriden ayrı
pub struct SectorLightMap {
    /// 32×32×32 = 32.768 voxel × 8-bit (4-bit sky + 4-bit block)
    pub data: Vec<u8>,
}

/// Sector Mip Chain — 6 seviyeli LOD (32→16→8→4→2→1)
/// Sadece %14 bellek overhead ile 6 seviye mip
/// GPU'da texture_3d array olarak tutulur, hardware bilinear filter ile seamless blend
///
/// LOD Mesafe Kuralları:
///   LOD-0 (32³): < 16 blok — full detail
///   LOD-1 (16³): 16-32 blok
///   LOD-2 (8³):  32-64 blok
///   LOD-3 (4³):  64-128 blok
///   LOD-4 (2³):  128-256 blok
///   LOD-5 (1³):  > 256 blok — sector ortalaması
pub struct SectorMip {
    pub levels: heapless::Vec<u8, { 32*32*32 + 16*16*16 + 8*8*8 + 4*4*4 + 2*2*2 + 1 }>,
    /// Mip generation: GPU compute shader'da 2×2×2 average pooling
    /// Emissive block'lar için MAX reduction kullanılır (feature preservation)
    /// CPU'da üretilmez — GPU feedback sonrası compute shader ile otomatik
}
```

### 1.4 Random Access (Popcnt ile O(1))

Bir Sector 32³, Brick 8³, SubBrick 2³ olduğu için bit kaydırma ve popcnt (`count_ones()`) ile ışık hızında (O(1)) erişim sağlanır. İşlemci donanım destekli bit-sayma algoritmaları kullanır.

### 1.5 Branchless (Dallanmasız) Ray Tracing (WGSL)

Ray tracing kodunda `if-else` kullanımı GPU warp'larını (wavefront) böldüğü için **execution divergence** yaratır. Bunun yerine WGSL'nin select fonksiyonu ve bit intrinsics (`firstTrailingBit`, `countLeadingZeros`) ile dallanmasız traversal yapılır. Shader'lar WGSL ile yazılır, wgpu tarafından derlenir.

**Temel prensipler:**
- `select(false_case, true_case, condition)` → dallanmasız seçim (if-else yerine)
- `firstTrailingBit(x)` → en düşük anlamlı 1-bit'in pozisyonu (sonraki dolu hücre)
- `countLeadingZeros(x)` → en yüksek anlamlı bitin pozisyonu

```wgsl
// Branchless DDA Traversal — Sabit Iterasyon + Active Flag
// ========================================================
// Strateji: 96 sabit iterasyon (L1 cache'e sığar), active=false
// olunca tüm işlemler select ile nop'e dönüşür. Warp divergence SIFIR.
//
// Neden 96?
//   - 3 seviyeli traversal'da (Sector→Brick→SubBrick) ortalama 15-40 iterasyon
//   - Maksimum teorik: ~200 (patolojik ışınlar)
//   - 96, tüm ışınların %99.9'unu kapsar
//   - RTX 4090 L1 cache (128KB): 96 iterasyon = 36KB/warp → tamamen L1'de
//   - 512 iterasyon = 192KB/warp → L2'ye/VRAM'e taşar (~1000× yavaş)
//   - Aşan ışınlar conservative fallback: en son bulunan yüzeye clamp

fn traverse_xbrickmap(ray: Ray) -> HitInfo {
    var t = 0.0;
    var hit = HitInfo(false, 0u, 0.0);
    var active = true;

    for (var i = 0u; i < 96u; i++) {
        let pos = ray.origin + ray.direction * t;
        let sector_coord = vec3<i32>(floor(pos / 32.0));
        let sector = load_sector(sector_coord);

        let step_size = select(1.0, 32.0, sector.sector_mask == 0u);

        let local = vec3<u32>(pos % 32.0);
        let brick_idx = (local.x / 8) + (local.z / 8) * 4 + (local.y / 8) * 16;
        let brick = load_brick(sector, brick_idx);

        let sub_idx = (local.x % 8 / 2) + (local.z % 8 / 2) * 4 + (local.y % 8 / 2) * 16;
        let sub_bit = (brick.brick_mask >> sub_idx) & 1u;
        let next_dense = firstTrailingBit(brick.brick_mask >> sub_idx);
        let skip = select(1.0, f32(next_dense), sub_bit == 0u);

        let voxel_local = vec3<u32>(local.x % 2, local.y % 2, local.z % 2);
        let voxel_bit_idx = voxel_local.x + voxel_local.z * 2 + voxel_local.y * 4;
        let sub = load_sub_brick(brick, sub_idx);
        let voxel_bit = (sub.voxel_mask >> voxel_bit_idx) & 1u;
        let mat = select(0u, load_material(sub, voxel_bit_idx), voxel_bit == 1u);

        hit.hit = select(hit.hit, true, mat != 0u && active);
        hit.material = select(hit.material, mat, mat != 0u && active);
        hit.distance = select(hit.distance, t, mat != 0u && active);

        let clamped_step = select(0.0, step_size + skip, active);
        t += clamped_step;

        active = select(active, false, mat != 0u);
    }

    return hit;
}
```

## 2. İleri Düzey Optimizasyonlar (SOTA - 2024)

### 2.1 Voxel Palette Compression (4-Seviyeli Palet Zinciri)

Bir Brick (8×8×8 = 512 voxel) içinde genelde 1–2 materyal bulunur. Her voxel için doğrudan `u16` block type tutmak yerine **sektör-local `u8` indeks** + çözümleme zinciri kullanılır (Minecraft section palette ile aynı felsefe; global ID ayrı tabloda).

**Seviye 0: BlockRegistry (dünya geneli, init-only)**
- `block_type: u16` (max 65535), özellikler SoA — **05-block-registry.md**
- Voxel bellekte **tutulmaz**; sadece sorgu hedefi

**Seviye 1: SectorPalette (sektör başına, max 256 entry)**
- `u8 local_index → PaletteEntry { block_type, variant }`
- Tipik sektör: 20–40 farklı `(type + variant)`; max 256 (aşımda fail-fast, Bkz. 05 §20)
- Entry 0 = AIR (`block_type=0`, `variant=0`)

**Seviye 2: BrickPalette (opsiyonel, max 16 remap)**
- Brick içinde 2+ farklı local index varsa: `u8 brick_local → u8 sector_local`
- Tek-tip brick: oluşturulmaz → 0 byte

**Seviye 3: SubBrick indeksleri**
- 8 voxel × 8-bit = `u64` packed indices
- Tek-tip: `indices = None` → sadece mask (8 byte)
- Çoklu: index → BrickPalette (varsa) → SectorPalette → BlockRegistry

**Çözümleme (runtime):**
```
SubBrick u8 → [BrickPalette] → SectorPalette → BlockRegistry.flags[block_type]
```

**GlobalPalette (render):** `MaterialDef` dizisi, `index = block_type_id`; BlockRegistry init'ten üretilir (05 §14.3).

**Sıkıştırma kazancı:**
| Durum | Eski (u16/voxel) | 4-seviyeli palet | Kazanç |
|---|---|---|---|
| Tek-tip brick (512×hava) | 1.024 B | 8 B (mask) | ~%99 |
| 2 materyal | 1.024 B | ~18 B | ~%98 |
| 16 materyal (brick max) | 1.024 B | ~56 B | ~%94 |

**Cross-ref:** State ownership (palet vs BlockEntity) → 03-ecs-architecture.md §10.6.1, 05-block-registry.md §10.5.

### 2.2 GPU Arena Allocator & Virtual Page Table
Oyuncu 32x32x32'lik Sektörde tek bir blok değiştirdiğinde tüm sektör vektörlerinin GPU'ya aktarılması bant genişliği (PCI-e bandwidth) darboğazı yaratır. Bunun için GPU VRAM'inde devasa bir **Page Table** (Sanal Sayfa Tablosu) (SSBO Arena) tahsis edilir. Kırılan bloğun olduğu "Page", GPU içindeki adresi bulunarak `queue.write_buffer()` ile nokta atışıyla güncellenir. CPU-GPU darboğazı tamamen çözülür.

### 2.3 Vertex Packing (4 Byte/Vertex)

Standart vertex formati ~24 byte/vertex (position + normal + texcoord + color) yerine **4 byte/vertex** kullanılır:

```rust
/// 4-byte packed vertex format
/// Vercidium tekniği: tüm veri tek u32'ye sığdırılır
struct PackedVertex(u32);

impl PackedVertex {
    fn new(local_pos: u8, tex_id: u8, normal: u8, ao: u8) -> Self {
        Self(
            (local_pos as u32)        | // 6 bit (0-63, 4x4x4 local pos)
            ((tex_id as u32) << 6)    | // 8 bit (0-255, texture ID)
            ((normal as u32) << 14)   | // 3 bit (0-5, 6 face yönü)
            ((ao as u32) << 17)         // 2 bit (0-3, ambient occlusion)
        )
    }

    fn local_pos(&self) -> u8 { (self.0 & 0x3F) as u8 }
    fn tex_id(&self) -> u8 { ((self.0 >> 6) & 0xFF) as u8 }
    fn normal(&self) -> u8 { ((self.0 >> 14) & 0x7) as u8 }
    fn ao(&self) -> u8 { ((self.0 >> 17) & 0x3) as u8 }
}
```

**WGSL Unpack Shader:**
```wgsl
struct PackedVertex {
    data: u32,
};

fn unpack_local_pos(v: PackedVertex) -> vec3<f32> {
    let idx = v.data & 0x3Fu;
    return vec3<f32>(f32(idx & 3u), f32((idx >> 2u) & 3u), f32((idx >> 4u) & 3u));
}

fn unpack_tex_id(v: PackedVertex) -> u32 {
    return (v.data >> 6u) & 0xFFu;
}

fn unpack_normal(v: PackedVertex) -> vec3<f32> {
    let n = (v.data >> 14u) & 0x7u;
    // 6 yön: +X, -X, +Y, -Y, +Z, -Z
    return vec3<f32>(
        select(0.0, select(-1.0, 1.0, n == 0u), n <= 1u),
        select(0.0, select(-1.0, 1.0, n == 2u), n >= 2u && n <= 3u),
        select(0.0, select(-1.0, 1.0, n == 4u), n >= 4u),
    );
}
```

**Kazanç:**

| Format | Boyut/Vertex | 1M Vertex | Tasarruf |
|--------|-------------|-----------|----------|
| Standart (pos+normal+uv+color) | 24 byte | 24 MB | — |
| **Packed** | **4 byte** | **4 MB** | **%83** |

**Kaynak:** [Vercidium — Voxel World Optimisations](https://vercidium.com/blog/voxel-world-optimisations), [Vercidium mesh generation code](https://github.com/Vercidium/voxel-mesh-generation)

---

### 2.4 Branchless WGSL DDA Traversal
WGSL ray-tracing döngülerindeki `if (mask == 0)` gibi dallanmalar, GPU warp'larını böldüğü için select fonksiyonu ve donanımsal bit intrinsics ile değiştirilir:

**Kullanılan WGSL built-in'leri:**
- `select(false_case, true_case, condition)` → dallanmasız seçim (if-else yerine)
- `firstTrailingBit(x)` → bir sonraki dolu biti atla (space skipping)
- `countLeadingZeros(x)` → ters yön traversal
- `extractBits(x, offset, count)` → bit alanı çıkarma

**Temel strateji:**
```wgsl
// if (mask == 0) { step = 32; } else { step = 1; }
// YUKARIDAKİ yerine:
step = select(1.0, 32.0, mask == 0u);

// if (bit == 0) { skip = next_set_bit_position; }
// YUKARIDAKİ yerine:
skip = select(1.0, f32(firstTrailingBit(mask >> idx)), bit == 0u);
```

Bu yaklaşım GPU performansını %20-30 artırır. **Not:** Erken çıkış (sabit iterasyon yerine) Volta+ mimarilerde `independent thread scheduling` sayesinde divergence cezasını minimize eder. Sabit iterasyon + `active` flag ile tam branchless traversal da mümkündür; bu durumda boş iterasyonlarda compute yapılır ama warp divergence sıfırdır. İkisi arasında seçim hedef donanıma bağlıdır. **Önemli:** 96 iterasyon L1 cache sınırı (< 128KB) için yeterlidir; 512 iterasyon VRAM'e taşar.

### 2.5 Renkli LOD (Sector Seviyesinde Mip-Mapping)
Uzaktaki manzaralar render edilirken boşlukları hızlı atlamak için mip verisi **sector seviyesinde** tutulur (brick seviyesinde değil). Bu sayede brick traversal'da extra cache miss oluşmaz.

**6 Seviyeli Mip Chain:**
```
Sector Mip Chain (GPU texture_3d array, hardware bilinear filter):
  Level | Res    | Voxel/Blok | Hafıza  | LOD Mesafe
  ------|--------|------------|---------|-----------
  Mip-0 | 32×32×32  | 1          | 32KB    | < 16 blok (full detail)
  Mip-1 | 16×16×16  | 2          | 4KB     | 16-32 blok
  Mip-2 | 8×8×8    | 4          | 512B    | 32-64 blok
  Mip-3 | 4×4×4    | 8          | 64B     | 64-128 blok
  Mip-4 | 2×2×2    | 16         | 8B      | 128-256 blok
  Mip-5 | 1×1×1    | 32         | 1B      | > 256 blok (sector avg)
  ------|--------|------------|---------|-----------
  Total |          |            | ~36.6KB | Sadece %14 overhead
```

8³ (Mip-2) ile 32³ (Mip-0) arasında çok büyük LOD boşluğu olduğu için 2 seviye yeterli değildir. 4³ ve altı seviyeler özellikle uzak mesafede (128+ blok) kritiktir — brick traversal tamamen atlanır, sadece texture lookup yapılır.

**GPU'da Mip Generation (CPU değil):**
```wgsl
// 2×2×2 average pooling — compute shader, her frame rebuild
// Emissive block'lar için MAX reduction (feature preservation)
@compute @workgroup_size(8, 8, 8)
fn build_mip(@builtin(global_invocation_id) id: vec3<u32>) {
    var sum = 0u; var count = 0u;
    for (var z = 0u; z < 2u; z++) {
        for (var y = 0u; y < 2u; y++) {
            for (var x = 0u; x < 2u; x++) {
                let v = textureLoad(src, vec3<i32>(id * 2u + vec3<u32>(x, y, z)), i32(level) - 1).r;
                if (v != 0u) { sum += v; count++; }
            }
        }
    }
    textureStore(dst, vec3<i32(id), vec4<i32>(select(0u, sum / count, count > 0u)));
}
```

**GPU'da kullanım:**
- Mip verisi `texture_3d<u8>` (WGSL) olarak GPU'ya aktarılır
- Hardware bilinear filter ile LOD seviyeleri arasında yumuşak geçiş
- Ray uzaklığı arttıkça daha düşük mip seviyesi kullanılır
- LOD-5 (1³) seviyesinde sadece 1 texture lookup → sektörün tamamı tek renk

**WGSL LOD Blending:**
```wgsl
fn sample_lod(pos: vec3<f32>, lod: f32) -> Material {
    let lod_a = u32(floor(lod));
    let lod_b = u32(min(ceil(lod), 5.0));
    let blend = lod - f32(lod_a);
    // Hardware bilinear filter ile seamless blend
    return mix(
        textureSampleLevel(tex, samp, pos, f32(lod_a)),
        textureSampleLevel(tex, samp, pos, f32(lod_b)),
        blend
    );
}
```

**Neden sector seviyesi?** Brick başına mip tutmak (64 brick × 16 byte = 1KB/sector) ekstra cache miss yaratır. Sector seviyesinde tek bir texture lookup ile LOD alınır.

### 2.6 Object Pooling (SlotMap + Free-List BrickPool)
Blokların sürekli eklenip kırılması, `Vec::push`/`remove` ile heap fragmentation ve stuttering yaratır. Çözüm: **SlotMap** (orlp/slotmap crate) + **Free-list** kombosu.

**SlotMap nedir?**
- Insert/remove O(1), versiyonlu key'ler ile dangling pointer yok
- Her slot: `(value, version)` çifti. Key'de de version bulunur.
- Eşleşme varsa erişim geçerli, yoksa güvenli hata
- 2³¹ insert/remove sonrası version wrap → teoride bile güvenli

**SlotMap varyant seçimi:**
| Tür | Random Access | Iterasyon | Insert/Remove | Kullanım |
|---|---|---|---|---|
| `SlotMap` | En hızlı | Yavaş (boş slotlar) | Hızlı | **Brick pool** (sık random access) |
| `HopSlotMap` | Aynı | Orta | 2x yavaş | Bulk iteration gereken yerler |
| `DenseSlotMap` | Yavaş (2 indirection) | Vec gibi hızlı | Orta | Sadece bulk işlemler |

**SlotMap vs SecondaryMap — SOA senkronizasyonu:**
`GlobalBrickPool`'da AOS (SlotMap) + SOA (SecondaryMap) ikisi de bulunur.
- `SlotMap<BrickKey, BrickData>`: random access (doku/traversal)
- `SecondaryMap<BrickKey, u64>`: slot-indexed Vec, boş slotları atlar
- **Senkronizasyon:** SecondaryMap, SlotMap'ın slot indeksini + versiyonunu kullanır.
  `insert(key)` / `remove(key)` otomatik senkronizedir — manuel güncelleme GEREKMEZ.
  `SecondaryMap::values()` sadece dolu slotları iterate eder (boş slotları atlar).
  Bu sayede SOA performansı (cache-friendly bulk iteration) ve AOS random access
  aynı anda elde edilir.

**Free-list entegrasyonu — SlotMap internal free-list:**
```rust
// SlotMap kendi free-list'ini internal yönetir — manuel free-list GEREKMEZ.
// Her insert: O(1) — ya boş slot'u reuse eder ya da yeni açar.
// Her remove: O(1) — slot'u vacant işaretler, versiyon+1 yapar.
// Toplam allocasyon = O(peak_brick), O(toplam_ekleme) değil.
//
// SecondaryMap::insert davranışı (kaynak koddan):
//   1. slot indeksi > len ise Vec.extend() (amortized O(1))
//   2. slot.versiyon == key.versiyon ise → replace, eskiyi döndür
//   3. slot boş ise → num_elems++, yeni Slot::Occupied yaz
//   4. slot.versiyon > key.versiyon (eski key) → None döndür (silent fail)
//
// Güvenlik: SlotMap remove → key versiyonu artar. Eski key ile
// SecondaryMap::insert çağrılırsa `is_older_version()` true döner,
// insert silent None döner. Yeni key (SlotMap::insert'ten) ile
// çağrılırsa versiyon eşleşir ve başarılı olur.

fn alloc_brick(pool: &mut GlobalBrickPool) -> BrickKey {
    let key = pool.bricks.insert(BrickData::default());
    // SecondaryMap::insert: versiyon kontrolü + slot oluşturma
    // Maliyet: extend(amortized O(1)) + atama(~1 cycle)
    pool.brick_masks.insert(key, 0u64);
    pool.sub_brick_offsets.insert(key, 0u32);
    key
}

fn free_brick(pool: &mut GlobalBrickPool, key: BrickKey) {
    pool.bricks.remove(key);
    // SecondaryMap remove GEREKMEZ — versiyon eşleşmezse
    // SecondaryMap::get(key) otomatik None döndürür.
}

fn get_mask(pool: &GlobalBrickPool, key: BrickKey) -> Option<&u64> {
    pool.brick_masks.get(key)
    // İçsel: 1 bounds check + 1 version compare = ~2 cycle
}

fn set_mask(pool: &mut GlobalBrickPool, key: BrickKey, val: u64) {
    if let Some(m) = pool.brick_masks.get_mut(key) {
        *m = val;
    }
}
```

**Kazanç:** SlotMap'in internal free-list + versioning mekanizması sayesinde heap fragmentation sıfır, dangling pointer imkansız. SecondaryMap ile SOA random access O(1) ve güvenli. alloc_brick iki adımda (SlotMap.insert + SecondaryMap.insert) toplam ~10 cycle. İkisi birlikte hem random access (traversal) hem bulk iteration (GPU upload) performansı optimize edilir.

### 2.7 GPU Feedback Loop (Visibility-Guided Upload)

GigaVoxels'ten esinlenen bu mekanizma, GPU'nun her frame sonunda **hangi sector'lere ihtiyacı olduğunu** CPU'ya bildirmesiyle çalışır. CPU sadece gerekli sector'leri upload eder, tüm dünyayı değil.

**Çalışma prensibi:**

```
Her frame:
  1. GPU compute shader çalışır
  2. Shader, ray'in değdiği her sector ID'sini bir SSBO'ya atomicAdd ile yazar
  3. Frame sonu: CPU, SSBO'daki sector ID'lerini okur (mapped memory)
  4. CPU, sadece bu sector'lerin güncel verisini `queue.write_buffer()` ile GPU'ya upload eder
  5. SSBO resetlenir (clear)
```

**WGSL feedback shader:**
```wgsl
// Feedback buffer: her sector için 1 uint32 (0 = gerekmez, 1 = gerekli)
@group(0) @binding(3)
var<storage, read_write> feedback_buffer: array<u32>;

// Ray traversal sırasında:
let sector_id = hash_sector_coord(sector_coord);
atomicMax(&feedback_buffer[sector_id], 1u);  // Bu sector lazım
```

**CPU tarafı (wgpu):**
```rust
pub struct FeedbackProcessor {
    // GPU ile CPU arasında mapped buffer (zero-copy)
    pub feedback_buffer: wgpu::Buffer,
    pub feedback_slice: wgpu::BufferSlice,
    pub needed_sectors: Vec<u32>,
}

impl FeedbackProcessor {
    pub fn collect_and_upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, pool: &GlobalBrickPool) {
        // Buffer mapping ile oku (PCIe round-trip yok)
        let feedback = self.feedback_buffer.slice(..);
        // MapAsync callback ile feedback verisini oku

        // Sadece set bit'leri işle
        for (id, &val) in feedback_data.iter().enumerate() {
            if val != 0 {
                self.needed_sectors.push(id as u32);
            }
        }

        // Sadece gerekli sector'leri upload et (queue.write_buffer)
        for &sector_id in &self.needed_sectors {
            let sector_data = pool.get_sector_gpu_data(sector_id);
            queue.write_buffer(&self.target_buffer, offset, &sector_data);
        }

        // Feedback buffer'ı sıfırla
        queue.write_buffer(&self.feedback_buffer, 0, &zeros);
    }
}
```

**Neden çalışır:** Bir sektör 32KB, PCIe 4.0 x16'da tek sector upload ~1μs. 100 sektör = 0.1ms. Tüm dünyayı (10K sector) upload etmek yerine sadece görünen ~100-200 sector upload edilir → **PCIe bandwidth tasarrufu ~%98**.

### 2.8 LOD-Bilinçli Branchless Traversal (GigaVoxels Tarzı)

GigaVoxels'in temel fikri: ışın kameradan uzaklaştıkça daha düşük detay seviyesinde traversal yap. XBrickMap'te bu, **sector mip seviyesinde durup brick/sub-brick detayına inmemek** anlamına gelir.

**Mantık:**
- Işın kameraya yakın (< 16 blok): Tam brick → sub-brick → voxel traversal (LOD-0)
- Işın yakın-orta (16-32 blok): Brick seviyesinde dur, sub-brick'e inme (LOD-1)
- Işın orta (32-64 blok): 8³ mip'te dur, brick traversal yok (LOD-2)
- Işın orta-uzak (64-128 blok): 4³ mip'te dur (LOD-3)
- Işın uzak (128-256 blok): 2³ mip'te dur (LOD-4)
- Işın çok uzak (> 256 blok): 1³ = sector ortalaması (LOD-5)

**WGSL'de branchless LOD seçimi (6 seviye):**
```wgsl
// LOD seviyesini dallanmasız hesapla
let dist = length(ray.origin - pos);
// Select ile branchless LOD: 0-5 arası
var lod = select(0u, 1u, dist > 16.0);
lod = select(lod, 2u, dist > 32.0);
lod = select(lod, 3u, dist > 64.0);
lod = select(lod, 4u, dist > 128.0);
lod = select(lod, 5u, dist > 256.0);

// LOD'a göre traversal derinliği
var max_depth = select(3u, 2u, lod >= 1u);
max_depth = select(max_depth, 1u, lod >= 2u);
max_depth = select(max_depth, 0u, lod >= 3u);
// lod >= 3: hiç traversal yok, direkt mip texture lookup
```

**Brick-only mod (LOD 1-2):** Sub-brick'e inme, brick'in ortalama materyalini kullan:
```wgsl
let brick_has_content = brick.brick_mask != 0u;
let brick_material = load_brick_material(sector, brick_idx);
let hit_mat = select(0u, brick_material, brick_has_content);
```

**Mip-only mod (LOD 3-5):** Sector mip texture'ından direkt oku, brick traversal yok:
```wgsl
// LOD-3: 4³ mip (32/8=4), LOD-4: 2³ mip (32/16=2), LOD-5: 1³ mip (32/32=1)
let mip_scale = select(8u, select(4u, 2u, lod >= 4u), lod >= 3u);
let mip_pos = vec3<u32>(floor(pos_local / f32(mip_scale)));
let mip_mat = textureLoad(sector_mip_tex, vec3<i32>(mip_pos), i32(lod) - 3).r;
let step_size = select(f32(mip_scale), 1.0, mip_mat != 0u);
```

**Kazanç:** Uzak mesafede brick traversal (64 bitmask check + sub-brick loads) tamamen atlanır. Sadece sector mip texture lookup + 1 material fetch. Görüş mesafesi 2-4 kat artar, FPS düşmez.

## 3. Donanım Ölçeklendirme ve Render Ayarları (Hardware Scaling)

Farklı ekran kartı donanımlarını desteklemek ve oyunun ayarlar menüsünde grafik kalitesini ölçeklendirebilmek adına sistem **dinamik render yolları (pipeline)** ile tasarlanmıştır. Motor, cihazın desteklediği teknolojiye göre en üst seviyeden başlayarak "Fallback" (geri dönüş) yapar.

### 3.1 Render Pipeline Yolları (Kademeli Sistem)

**KADEME 1: OMM (Opacity Micromaps) Modu - (En Üst Düzey)**
* **Hedef:** Yeni nesil (Örn: RTX 4000 Serisi) kartlara sahip kullanıcılar. Ayarlardan aktifleştirilir.
* **Sistem:** XBrickMap bitmask'leri doğrudan donanımın BVH ağacına şeffaflık maskesi olarak aktarılır. Özel bir `Intersection Shader` yazmaya dahi gerek kalmadan, donanımsal RT (Ray Tracing) çekirdekleri boşlukları (Air) silikon seviyesinde atlar. En pürüzsüz performansı verir.

**KADEME 2: Hardware RT (Donanımsal Işın İzleme) Modu - (Yüksek Düzey)**
* **Hedef:** DXR / Hardware RT destekleyen ancak OMM desteklemeyen (Örn: RTX 2000/3000 serisi) kartlar.
* **Sistem:** Dünyadaki 32³'lük Sektörler, ekran kartının `TLAS` (Top Level Acceleration Structure) ağacına Bounding Box (AABB) olarak aktarılır. Işın kutuya çarptığında, donanımsal RT pipeline'ındaki özel bir `Intersection Shader` tetiklenir ve o sektörün içindeki XBrickMap traversal algoritmasını donanım çekirdeğiyle hibrit olarak çalıştırır. **Not:** wgpu'da RT pipeline desteği deneyseldir (v28-29).

**KADEME 3: Compute Shader Modu - (Geri Dönüş / Fallback)**
* **Hedef:** Hardware RT desteklemeyen veya ışın izlemeyi kapatmak isteyen oyuncular (Örn: GTX serisi, APU'lar).
* **Sistem:** Standart `WGSL Compute Shader` kullanılarak ekran kartının genel işlem birimleri (ALU) ile ray marching yapılır. Mevcut planın ana iskeletini oluşturur ve oyunun her cihazda hatasız çalışmasını garanti eder.

### 3.2 Voxel Işıklandırma (3-Katmanlı Hibrit Sistem)

Voxel dünyasında ışıklandırma **üç bağımsız katmandan** oluşur. Her katman farklı bir amaca hizmet eder ve farklı bir algoritma ile hesaplanır:

```
final_color = direct_emissive * 0.4 + blocklight * 0.3 + restir_gi * 0.3
```

| Katman | Kaynak | Gecikme | Gürültü | Görevi |
|--------|--------|---------|---------|--------|
| **Direct Emissive** | Lava, glowstone, meşale (point lights) | Anında | Yok | Doğrudan aydınlatma |
| **Blocklight** | Blok ışık kaynakları (4-bit flood fill) | Anında | Yok | Küçük odalar, hızlı değişim |
| **ReSTIR GI** | Dolaylı aydınlatma (path traced) | 2-4 frame | Düşük | Renk bleeding, ambiyans |

**Neden üçü birden?** ReSTIR GI blocklight'ın yerini alamaz:
- Meşale küçük odada → Blocklight anında ve sessiz, ReSTIR gürültülü ve yavaş converge
- Kırmızıtaş lamba pulse → Blocklight anında, ReSTIR 2-4 frame gecikmeli
- Renk bleeding (lavdan kırmızı ışık) → Blocklight yapamaz, ReSTIR mükemmel
- Gün ışığı pencereden → İkisi de çalışır, ReSTIR daha kaliteli

#### 3.2.1 GPU Blocklight Propagation (Wavefront BFS)

Blok ışıklandırması (torch, glowstone, vs.) **GPU compute shader**'da wavefront BFS ile yapılır. CPU BFS (30+ sektörde 1-2ms) yerine GPU'da **~0.02ms/sector**.

**Neden GPU?**
| Yöntem | 1 Sektör (32³) | Tüm Dünya (100 sektör) |
|--------|---------------|----------------------|
| CPU BFS (rayon) | 0.5-1ms | 50-100ms |
| GPU Wavefront BFS | **0.02ms** | **1-5ms** |
| GPU Jump Flooding | 0.05ms | **0.3-2ms** |

**Wavefront BFS Algoritması:**
```
Her frame / block change:
  1. Işık kaynaklarını (emissive blocks) içeren bir queue oluştur
  2. Compute shader dispatch: 1 thread = 1 aktif voxel
  3. Her thread: 6 komşuya propagate (light - 1)
  4. Yeni aktif voxel'leri bir sonraki queue'ya ekle
  5. Queue boşalana kadar tekrarla (max 16 iterasyon, ışık menzili 15)
```

**WGSL Wavefront BFS:**
```wgsl
struct LightNode {
    pos: vec3<u32>,
    intensity: u32,
};

var<workgroup> shared_queue: array<LightNode, 256>;
var<workgroup> shared_count: u32;

@compute @workgroup_size(64)
fn propagate_blocklight(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= queue_len) { return; }

    let node = light_queue[idx];
    let current = textureLoad(blocklight_tex, vec3<i32>(node.pos), 0).r;
    if (current >= node.intensity) { return; }

    textureStore(blocklight_tex, vec3<i32>(node.pos), vec4<i32>(i32(node.intensity)));

    // 6 yönlü propagation
    for (var axis = 0u; axis < 3u; axis++) {
        for (var dir = 0u; dir < 2u; dir++) {
            var offset = vec3<u32>(0u);
            offset[axis] = select(0xFFFFFFFFu, 1u, dir == 0u); // -1 as unsigned
            let n = node.pos + offset;
            if (is_transparent(n) && node.intensity > 1u) {
                let qi = atomicAdd(&shared_count, 1u);
                if (qi < 256u) {
                    shared_queue[qi] = LightNode(n, node.intensity - 1u);
                }
            }
        }
    }
}
```

**LightMap yapısı:**
- 32×32×32 = 32.768 voxel
- Her voxel: 4-bit skylight + 4-bit blocklight = 8-bit → **32KB / sector**
- GPU'da `texture_3d<u8>` (WGSL) olarak, 2 ayrı texture (blocklight, skylight)
- Değişen kısımlar `queue.write_buffer()` ile nokta atışı güncellenir (GPU Arena ile entegre)

#### 3.2.2 Direct Emissive (Deferred Point Lights)

Emissive bloklar (lava, glowstone, meşale) aynı anda **iki rol** oynar:

1. **Direct light source**: Deferred shading'de point light olarak render edilir
2. **ReSTIR GI source**: Path tracing'de radiance kaynağı olarak kullanılır

```rust
pub struct EmissiveLight {
    pub position: IVec3,
    pub color: [f32; 3],
    pub intensity: f32,
    pub radius: f32,  // max 15 blok (oyun dengesi)
}

// Her frame: değişen bloklardan emissive listesini güncelle
// Bevy ECS: Query<(&BlockGrid, &SectorPosition)> ile sorgulama
pub fn collect_emissive_lights(query: Query<(&BlockGrid, &SectorPosition)>) -> Vec<EmissiveLight> {
    query.iter()
        .flat_map(|(grid, pos)| {
            grid.emissive_blocks().map(|local| EmissiveLight {
                position: pos.0 * 32 + local,
                color: MATERIALS[grid[local]].emission_color,
                intensity: MATERIALS[grid[local]].emission as f32 / 15.0,
                radius: 15.0,
            })
        })
        .collect()
}
```

#### 3.2.3 ReSTIR GI (Global Illumination)

ReSTIR GI **sadece dolaylı aydınlatma** içindir — direkt aydınlatma yukarıdaki iki katman tarafından karşılanır. Bu sayede:

- ReSTIR az sayıda örnekle (1-4 ray/pixel) çalışabilir
- Küçük odalarda blocklight gürültüyü maskeler
- Renk bleeding ve ambiyans ReSTIR'den gelir

```wgsl
// 3-katmanlı hybrid combine (WGSL)
fn combine_lighting(pos: vec3<u32>, normal: vec3<f32>) -> vec3<f32> {
    let emissive = sample_emissive(pos);
    let block = f32(textureLoad(blocklight_tex, vec3<i32>(pos), 0).r) / 15.0;
    let gi = textureLoad(gi_tex, vec3<i32>(pos), 0).rgb;

    return emissive * 0.4
         + vec3<f32>(block) * 0.3
         + gi * 0.3;
}
```

* Sadece RTX 2000+ serisi kartlarda aktifleştirilir
* Fallback: blocklight + emissive direct (ReSTIR olmadan)

---

## 4. Ultra Optimizasyonlar (Faz 2 — Beyond SOTA)

Bu bölüm, temel XBrickMap mimarisinin üzerine eklenebilecek 3 ileri düzey optimizasyonu kapsar. Her biri mevcut sisteme bağımsız olarak entegre edilebilir.

### 4.1 Ray-Guided LOD Selection (Hyperion Tarzı)

**Problem:** Mevcut LOD seçimi sadece ışın-nesne mesafesine bakar (`dist > 16`, `dist > 32`, ...). Oysa bir ışın yüzeye sıyırarak (grazing angle) geliyorsa, o yüzeyin detayı piksel altı seviyede kalır — yüksek LOD boşa kürek çekmektir.

**Çözüm:** Mesafeye ek olarak **geliş açısı** ve **voxel yoğunluğu** da LOD seçimine katılır:

```wgsl
fn select_lod(ray_dir: vec3<f32>, hit_pos: vec3<f32>, sector_mask: u64) -> u32 {
    let dist = length(ray_dir * (hit_pos - ray_origin));  // approximate
    let surface_normal = estimate_normal(hit_pos);         // from neighbor voxels
    let angle = abs(dot(normalize(ray_dir), surface_normal));
    let density = f32(countOneBits(sector_mask)) / 64.0;    // sector doluluk oranı

    // Baz LOD: mesafe tabanlı (mevcut mantık)
    var lod = select(0u, 1u, dist > 16.0);
    lod = select(lod, 2u, dist > 32.0);
    lod = select(lod, 3u, dist > 64.0);
    lod = select(lod, 4u, dist > 128.0);
    lod = select(lod, 5u, dist > 256.0);

    // Grazing angle bonus: sıyırma açısında +1-2 LOD (görsel kayıp yok)
    var angle_bonus = select(0u, 1u, angle > 0.85);   // ~32°
    angle_bonus = select(angle_bonus, 2u, angle > 0.96);  // ~16°

    // Yoğunluk penalty: çok yoğun sector'de detay kaybı daha belirgin
    // Seyrek sector'de daha agresif LOD atla
    let density_bonus = select(1u, 0u, density > 0.8);

    // Branchless clamp
    let raw = lod + angle_bonus + density_bonus;
    lod = select(raw, 5u, raw > 5u);
    return lod;
}
```

**Kazanç:** Uzaktaki dağ silsilelerinde, düz araziye sıyırarak gelen ışınlarda traversal step sayısı **%50 azalır**. Sıfır görsel kalite kaybı — çünkü grazing angle'da zaten piksel altı detail.

**Entegrasyon:** Mevcut WGSL LOD selection kodu (`06-xbrickmap.md:665-678`) tek fonksiyon değişikliği.

### 4.2 Async Copy & Compute Overlap (Triple-Buffered Streaming)

**Problem:** GPU feedback loop (Bölüm 2.6) sector ID'lerini CPU'ya bildirir, CPU `queue.write_buffer()` ile upload eder. Ama upload (PCIe DMA) sırasında compute **bekler**. Her frame ~0.1-0.5ms boşa harcanır.

**Çözüm:** 3 adet ping-pong buffer seti ile **bir frame ileriden** sector upload yapar. wgpu tek queue kullanır ama buffer setleri sayesinde upload ve compute farklı frame'lerde çalışır:

```
Frame N-1:                 Frame N:                  Frame N+1:
  Upload: Sector A          Upload: Sector B          Upload: Sector C
  Compute: Frame N-2 RT    Compute: Frame N-1 RT      Compute: Frame N RT
                            Render: Frame N-1 present  Render: Frame N present
```

```rust
/// Triple-buffered streaming: 3 adet ping-pong buffer seti
pub struct StreamingPipeline {
    // 3 frame'lik buffer havuzu
    staging_buffers: [wgpu::Buffer; 3],
    feedback_buffers: [wgpu::Buffer; 3],
    frame_index: usize,  // 0→1→2→0 döngüsü
}

impl StreamingPipeline {
    pub fn advance_frame(&mut self, queue: &wgpu::Queue) {
        let current = self.frame_index;
        let next = (current + 1) % 3;
        let prev = (current + 2) % 3;

        // Bir sonraki frame için sector'leri upload et
        // (feedback prev frame'den gelir, şimdiden upload et)
        let sectors_to_upload = self.read_feedback(prev);
        for (i, data) in sectors_to_upload.iter().enumerate() {
            queue.write_buffer(&self.staging_buffers[next], offset, data);
        }

        // Mevcut frame'de compute shader staging_buffers[current]'ı kullanır
        // Upload bir sonraki frame'de hazır olacak

        self.frame_index = next;
    }
}
```

**Kazanç:** Upload bir frame önce yapıldığı için compute hiç beklemez. Toplam frame süresi ~%3-5 kısalır (orta/yüksek sektör değişim senaryolarında).

**Entegrasyon:** Mevcut `FeedbackProcessor` (Bölüm 2.6) triple-buffer wrapper ile sarılır.

### 4.3 Voxel → Meshlet Hybrid Rendering (Nanite Tarzı)

**Problem:** Ray tracing voxel'leri, özellikle yakın mesafede (her piksel 1-4 ray) pahalıdır. Her ray DDA traversal yapar, çoğu boş alanı geçer. Oysa yakın mesafede **geleneksel rasterization** çok daha hızlıdır.

**Çözüm:** **3-Tier hibrit render pipeline:**

```
Tier 1 — Yakın (< 32 blok): Meshlet Rasterization
  GPU Compute Shader: Marching Cubes → meshlet buffer
  Hiç ray tracing yok, direkt rasterization
  Meshlet boyutu: 8³ voxel → ~5-20 triangle, Nanite cluster'a benzer

Tier 2 — Orta (32-128 blok): XBrickMap Ray Trace (mevcut)
  Mevcut branchless DDA traversal
  LOD-1 ila LOD-3 arası

Tier 3 — Uzak (> 128 blok): Mip Texture Lookup
  Sadece sector mip texture'dan oku
  Brick traversal tamamen atlanır
```

**GPU Meshlet Generation (WGSL Compute Shader):**
```wgsl
// Her 8³ brick için: meshlet üretimi
// Watertight Marching Cubes: vertex pozisyonlarını bit mask'tan çıkar
struct Meshlet {
    vertex_count: u32,
    triangle_count: u32,
    vertices: array<vec3<f32>, 64>,  // max 64 vertex
    indices: array<u32, 192>,        // max 64 tri × 3
    visibility_mask: u64,            // HDR: voxel var/yok
}

@compute @workgroup_size(8, 8, 8)
fn generate_meshlets(@builtin(global_invocation_id) id: vec3<u32>, sector_coord: vec3<i32>) {
    // 1. Sector'ün voxel verisini oku (bitmask + palette)
    // 2. Her 8³ brick için Marching Cubes çalıştır
    // 3. Meshlet buffer'a yaz (visibility_mask + vertex buffer)
    // 4. Visibility mask: hangi meshlet'ler boş → skip

    // NOT: Sadece değişen brick'lerde rebuild
    // Değişmeyen brick'lerde meshlet önbellekte kalır
}
```

**Visibility Buffer Culling (Nanite-style, WGSL):**
```wgsl
// Meshlet'leri visibility buffer'a göre cull et
// Her meshlet'in 64-bit visibility_mask'i:
//   Bit = 0 → boş, rasterization yapma
//   Bit = 1 → dolu, normal rasterization

// 8-bit cluster cull: 8 meshlet = 1 cluster
// Cluster'daki tüm meshlet'ler boşsa → cluster'ı atla
// Duvarlar, zeminler gibi büyük düz yüzeylerde %90 skip oranı
```

**Pipeline entegrasyonu (`10-render-pipeline.md` — 6-pass pipeline, taslak):**
```
Pass 0: Meshlet Generation (Compute) — sadece Tier 1 sector'ler
Pass 1: Meshlet Visibility Cull (Compute) — cluster bazlı early out
Pass 2: Meshlet Rasterization (Vertex/Fragment) — depth prepass
Pass 3: XBrickMap Ray Trace (Compute) — Tier 2 sector'ler
Pass 4: Mip Lookup (Compute) — Tier 3 sector'ler
Pass 5: Hybrid Composite — 3 tier'ı birleştir, depth test
```

**Kazanç:**
| Senaryo | Sadece RT | Meshlet Hybrid | İyileşme |
|---------|-----------|----------------|----------|
| Yakın planda (0-10 blok) | 4 ray/pixel × 40 step = 160 op | 1 meshlet raster = 1 op | **~160×** |
| Kapalı alan (oda, mağara) | ~80 step/ray | ~5 tri/pixel raster | **~16×** |
| Açık alan (dağ, vadi) | ~30 step/ray | %50 meshlet skip | **~2-4×** |

**Entegrasyon:** Mevcut Greedy Mesher trait'i (Plan 11.1) abstract. Yeni `GpuMeshletMesher` implementasyonu eklenir, 6-pass pipeline'a Pass 0-2 olarak yerleşir. OMM/Hardware RT faaliyeti varsa, meshlet pass otomatik devre dışı kalır (zaten donanım halleder).

---

## 5. Araştırma Doğrulamaları ve Öneriler (2026-06)

> **Kaynak:** 5 worker ile 40+ WebSearch sorgusu, SIGGRAPH/akademik paper'lar, voxel motor karşılaştırmaları.

### 5.1 Doğrulanan Kararlar

| Karar | Doğrulama |
|-------|-----------|
| 3-level bitmask hiyerarşi | VoxelRT eXtendedBrickMap varyantı, SOTA aligned |
| 4-seviyeli palet zinciri | Minecraft section palette ile aynı felsefe, validated |
| GlobalBrickPool + SlotMap | Zero heap fragmentation, O(1) alloc/dealloc |
| Branchless WGSL ray trace | GPU wavefront division eliminasyonu, ~%20-30 performans artışı |
| GPU feedback loop | GigaVoxels inspired, PCIe bandwidth %98 tasarrufu |

### 5.2 P2 — AADF/NAADF Cache Eklentesi

**Problem:** XBrickMap bitmask traversal'da boş alanlar atlanır ama her adımda memory access gerekir.

**Çözüm:** Bitmask'e parallel AADF (Adaptive Anti-aliased Distance Field) cache eklentesi. Ray tracing'de boş alanlarda **~10× hızlanma** sağlar.

**Çalışma prensibi:**
- Her brick için precomputed minimum boşluk mesafesi (1 byte)
- Ray boş alana girerse, AADF cache'den direkt atlama mesafesi okunur
- Mevcut bitmask approach ile paralel çalışır, ek bellek ~%5

```rust
// Brick başına AADF cache entry
pub struct BrickAadfCache {
    /// 8×8×8 grid için minimum boşluk mesafesi (0-7)
    pub distances: [u8; 8],
}
```

**Etki:** Boş alan traversal'da ~10× hızlanma. Karmaşık geometride minimal etki. **Phase 2-3** — benchmark gerekli.

### 5.3 P2 — Full Occupancy Encoding (Modisett & Billeter CGF 2025)

**Problem:** SVDAG node'larında her traversal adımında child_mask için ek memory access.

**Çözüm:** SVDAG node'larında occupancy metadata pointer'ın üst bitlerine gömülür. Her traversal adımında **1 memory access azalması** → %10-15 traversal speed gain.

**Entegrasyon:** `07-svdag.md` §1.2'deki OccupancyNode formatı ile uyumlu. XBrickMap tarafında etki yok — sadece SVDAG traversal.

### 5.4 P2 — Geometry/Color Separation (LOD 1+)

**Problem:** LOD 1+ aggregate SVDAG'larda geometry ve color aynı DAG'da → renk değişikliği tüm DAG'ı rebuild gerektirir.

**Çözüm:** Geometry ve color ayrı DAG'larda saklanır:

```
Geometry DAG → shape (dolu/boş)
Color DAG → material (block_type/variant)
```

**Etki:** %5-15 VRAM tasarrufu (geometry dedup daha agresif). **Phase 2-3** — SVDAG bake pipeline'da değerlendir.
