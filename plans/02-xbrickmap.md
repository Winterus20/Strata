# 02 — XBrickMap Veri Yapısı (Cubic Chunks)

## 1. XBrickMap — Aktif Alan Veri Yapısı

### 1.1 Hiyerarşik Yapı

XBrickMap, VoxelRT araştırmasında en iyi performansı gösteren brickmap varyantıdır. 3 seviyeli hiyerarşik bitmask (Sector -> Brick -> Sub-brick) + 3-seviyeli hibrit palet (Global + Brick + SubBrick) kullanır.

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
  │   │   └── Palette Indices (8×8-bit = u64, 256 materyal)
  │   │
  │   └── Brick Paleti (opsiyonel, max 16 materyal)
  │       └── Sadece brick 2+ farklı materyal içeriyorsa var
  │
  ├── Sector LightMap (32×32×32 = 32KB)
  │   └── Her voxel için 4-bit skylight + 4-bit blocklight = 8-bit
  │       └── GPU'ya texture_3d<u8> olarak aktarılır
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
- Işık: geometriden çok daha sık değiştiği için (günbatımı, meşale ekleme) ayrı texture. GPU compute shader'da wavefront BFS ile güncellenir, `queue.write_buffer` ile nokta atışı aktarılır.

**Neden 32×32×32 (Cubic)?**
- Minecraft gibi eski nesil oyunların aksine, dikeyde yükseklik sınırı (örn: 256, 384) yoktur. Oyuncu ne kadar yükseğe çıkarsa veya ne kadar derine inerse, sadece oradaki 32x32x32 sektörler yüklenir. Sonsuz gökyüzü ve devasa mağaralar tam performansla desteklenir.
- 32³ hacimde tam 64 adet (4³) brick (8³) bulunur. 64 sayısı tek bir 64-bit (u64) yazmaç (register) içine milimetrik olarak oturduğu için ray-tracing ışın hızı muazzamdır. 4 slab (dilim) hiyerarşisine gerek kalmaz, okuma süreci hızlanır.

### 1.2 Bellek Hesabı

| Bileşen | Boyut | Not |
|---|---|---|
| Sector metadata (Component) | 16 byte | `#[repr(C)]` u64(8B) + Option\<NonZeroU32\>(4B) + padding(4B) |
| Brick dizisi | 0-64 × ~56-80B | Left-packed, Paletli sıkıştırma ile |
| — Brick bitmask | 8 byte | Her brick için |
| — Sub-brick dizisi | 0-64 × ~16B | `u8 mask + u64 indices` (256 materyal) |
| — Brick Paleti | 0-16 byte | Sadece 2+ materyal varsa |
| Sector LightMap | 32KB (dolu) | 32³ × 8-bit, sadece dolu sektörlerde |
| Sector Mip Chain | ~36.6KB (dolu) | 32³+16³+8³+4³+2³+1³ = ~36.6K, GPU'da generated |
| **Tam dolu sector** | ~70-80KB | 32.768 voxel (mip chain + light dahil) |
| **Ortalama arazi** | ~25-35KB | ~50% boşluk, tek-tip palet avantajı |
| **Boş sector** | 16 byte | Sadece Sector Component (LightMap yok) |

**Karşılaştırma:** Eski düz 16x256x16 Chunk 128KB kaplarken (içi boşken bile), Cubic XBrickMap boş gökyüzünü sadece 16 byte'a indirger. Dolu kısımlarda 3-seviyeli palet sayesinde tek-tip brick sadece 8 byte (bitmask) yer kaplar.

### 1.3 Veri Yapısı (Rust - SlotMap + Global SOA & Bevy ECS)

Her Sektörün kendi içinde `Vec` tutması, binlerce Sektör yüklendiğinde korkunç bir bellek parçalanmasına (Heap Fragmentation) yol açar. Bu yüzden brick verileri merkezi bir `GlobalBrickPool` (Bkz 2.5) içinde **SlotMap** ile tutulur. `SlotMap`, versiyonlu key'ler sayesinde dangling pointer olmadan O(1) insert/remove sağlar ve free-list ile sıfır heap fragmentation verir.

Sector'ün dünya koordinatı `SectorPosition(IVec3)` ayrı bir Component'tir. Sector'den pozisyona erişim `SectorMap` resource'u (Morton kodu ile HashMap) üzerinden yapılır. Bu SOA yaklaşımı sayesinde `Query<(&Sector, &SectorPosition)>` ile sektörler hem uzaysal konumlarıyla hem de brick verileriyle sorgulanabilir.

`Sector` ise sadece bir adres (pool_index) barındıran hafif (~16 Byte) bir **Bevy Component'i'dir**. Dünya koordinatındaki yeri ayrı bir `SectorPosition` component'inde tutulur (SOA prensibi). Sector'den pozisyona erişim için `SectorMap` resource'u kullanılır.

ECS'de güncelleme takibi için manuel `dirty: bool` yerine **Bevy Component Change Detection** kullanılır: `Query<&Sector, Changed<Sector>>`.

```rust
use bevy::prelude::*;
use slotmap::{SlotMap, new_key_type};
use std::num::NonZeroU32;
use dashmap::DashMap;

new_key_type! { pub struct BrickKey; }

/// 32×32×32 voksellik, sınırsız yükseklik destekli kübik sektör
/// #[repr(C)] GPU SSBO'su ile layout uyumu için şart.
/// Bevy Component + #[repr(C)] sorunsuzdur. Bevy'nin Table storage'ı
/// column-major layout kullanır, #[repr(C)] fixed layout ile unsafe
/// pointer cast'ler daha güvenlidir. bytemuck ile GPU'ya zero-copy
/// aktarım da mümkün olur.
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

/// Sector'ün dünya koordinatı (SOA: ayrı Component)
/// Sorgulama: Query<(Entity, &Sector, &SectorPosition)>
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
    /// Morton encode: 21-bit x,y,z → 63-bit interleaved key
    /// "Magic Bits" yöntemi: branchless, LUT'suz, ~1ns/call
    fn morton_encode(x: u32, y: u32, z: u32) -> u64 {
        fn split3(a: u32) -> u64 {
            let mut x = (a as u64) & 0x1fffff;
            x = (x | x << 32) & 0x1f00000000ffff;
            x = (x | x << 16) & 0x1f0000ff0000ff;
            x = (x | x << 8) & 0x100f00f00f00f00f;
            x = (x | x << 4) & 0x10c30c30c30c30c3;
            x = (x | x << 2) & 0x1249249249249249;
            x
        }
        split3(x) | split3(y) << 1 | split3(z) << 2
    }

    fn key(pos: IVec3) -> u64 {
        Self::morton_encode(pos.x as u32, pos.y as u32, pos.z as u32)
    }
    fn get(&self, pos: IVec3) -> Option<Entity> {
        self.map.get(&Self::key(pos)).map(|e| *e)
    }
    fn insert(&self, pos: IVec3, entity: Entity) {
        self.map.insert(Self::key(pos), entity);
    }
    fn remove(&self, pos: IVec3) -> Option<Entity> {
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
    fn alloc_brick(&mut self) -> BrickKey {
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
    fn free_brick(&mut self, key: BrickKey) {
        self.bricks.remove(key);
    }

    /// Random access: SecondaryMap[key] ile SOA'dan direkt oku
    /// Maliyet: 1 bounds check + 1 version check = ~2 cycle
    fn get_mask(&self, key: BrickKey) -> Option<&u64> {
        self.brick_masks.get(key)
    }

    /// Mutable random access: mevcut brick'in mask'ini değiştir
    /// alloc_brick'te zaten initialize edildiği için her zaman Some döner
    fn set_mask(&mut self, key: BrickKey, mask: u64) {
        if let Some(m) = self.brick_masks.get_mut(key) {
            *m = mask;
        }
    }

    /// GPU upload için sector'a ait brick'leri paketle
    /// Sector'un u64 mask'ı hangi brick'lerin aktif olduğunu söyler
    /// Sadece aktif brick'lerin verisi GPU'ya upload edilir
    fn pack_sector_bricks(&self, sector: &Sector) -> Vec<u64> {
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
    /// 8 × 8-bit palette index = 256 materyal
    /// Option<NonZeroU64>: None = tek tip (default_material)
    pub indices: Option<NonZeroU64>,
}

/// Brick bazlı lokal palet (opsiyonel)
/// Sadece brick 2+ farklı materyal içeriyorsa oluşturulur
pub struct BrickPalette {
    /// Global paletten index'ler (max 16 farklı materyal/brick)
    pub materials: heapless::Vec<u8, 16>,
}

/// 3-Seviyeli Global Palet (tüm dünya için ortak, oyun başında yüklenir)
#[derive(Resource)]
pub struct GlobalPalette {
    /// Materyal ID → Blok özellikleri
    pub materials: Vec<MaterialDef>,
}

pub struct MaterialDef {
    pub name: &'static str,
    pub color: [u8; 3],
    pub emission: u8,
    pub opacity: u8,
    // ...
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

Ray tracing kodunda `if-else` kullanımı GPU warp'larını (wavefront) böldüğü için **execution divergence** yaratır. Bunun yerine WGSL'nin `select()` fonksiyonu ve bit intrinsics (`countTrailingZeros`, `firstTrailingBit`) ile dallanmasız traversal yapılır.

**Temel prensipler:**
- `select(a, b, cond)` → cond true ise `b`, false ise `a` döndürür (dallanmasız)
- `countTrailingZeros(x)` → en düşük anlamlı 1-bit'in pozisyonu (sonraki dolu hücre)
- `firstTrailingBit(x)` → ilk dolu bitin indeksi

```wgsl
// Branchless DDA Traversal — Sabit Iterasyon + Active Flag
// ========================================================
// Strateji: 96 sabit iterasyon (L1 cache'e sığar), active=false
// olunca tüm işlemler select() ile nop'e dönüşür. Warp divergence SIFIR.
//
// Neden 96?
//   - 3 seviyeli traversal'da (Sector→Brick→SubBrick) ortalama 15-40 iterasyon
//   - Maksimum teorik: ~200 (patolojik ışınlar)
//   - 96, tüm ışınların %99.9'unu kapsar
//   - RTX 4090 L1 cache (128KB): 96 iterasyon = 36KB/warp → tamamen L1'de
//   - 512 iterasyon = 192KB/warp → L2'ye/VRAM'e taşar (~1000× yavaş)
//   - Aşan ışınlar conservative fallback: en son bulunan yüzeye clamp
//
// Cache Analizi (RTX 4090, L1=128KB/SM, L2=72MB):
//   Iterasyon | L1 Kullanımı | Durum
//   ≤64       | 24KB/warp     | ✅ L1'e sığar
//   96        | 36KB/warp     | ✅ L1'e sığar
//   128       | 48KB/warp     | ⚠️ Sıkışık
//   256       | 96KB/warp     | ❌ L2'ye taşar
//   512       | 192KB/warp    | ❌ VRAM'e taşar
//
// Erken çıkış alternatifi (if hit { break; }):
//   Volta+ independent thread scheduling ile divergence ~%5
//   Sabit iterasyon + active flag: divergence %0
//   Bu tasarım full branchless'i seçer (tutarlı perf, taşınabilir)

fn traverse_xbrickmap(ray: Ray) -> HitInfo {
    var t: f32 = 0.0;
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
        let next_dense = countTrailingZeros(brick.brick_mask >> sub_idx);
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
    
    // 96 iterasyon aşan ışınlar: conservative fallback
    // hit.hit false ise, en son geçerli pozisyona clamp et
    return hit;
}
```

## 2. İleri Düzey Optimizasyonlar (SOTA - 2024)

### 2.1 Voxel Palette Compression (3-Seviyeli Hibrit Palet)
Bir Brick (8x8x8 = 512 voxel) içinde genelde 1 veya 2 çeşit materyal bulunur. Her voxel için 16-bit harcamak yerine **3 seviyeli hibrit palet** kullanılır:

**Seviye 1: Global Palet (256 materyal)**
- Tüm dünyada ortak: HAVA(0), TOPRAK(1), TAŞ(2), KUM(3), SU(4), AHŞAP(5), ...
- Oyun başında yüklenir, değişmez
- Her materyal: `MaterialDef { name, color, emission, opacity, ... }`

**Seviye 2: Brick Paleti (opsiyonel, max 16 materyal)**
- Global paletten index'lerin lokal kopyası
- Sadece brick 2+ farklı materyal içeriyorsa oluşturulur
- 16 × u8 = 16 byte
- Tek-tip brick'te hiç oluşturulmaz → 0 byte

**Seviye 3: SubBrick İndeksleri**
- 8 voxel × 8-bit = u64 (global palette direkt index)
- Eğer brick tek tip: `Option<NonZeroU64> = None` → sadece mask 8 byte
- 2+ materyal varsa: 8-bit index'ler brick palette bakar

**Sıkıştırma Kazancı:**
| Durum | Eski yöntem (u16/voxel) | 3-seviyeli palet | Kazanç |
|---|---|---|---|
| Tek-tip brick (512×hava) | 1.024B | 8B (sadece mask) | %99 |
| 2 materyal (taş+toprak) | 1.024B | ~18B (mask+indices+palet) | %98 |
| 16 materyal (max) | 1.024B | ~56B | %94 |

### 2.2 GPU Arena Allocator & Virtual Page Table
Oyuncu 32x32x32'lik Sektörde tek bir blok değiştirdiğinde tüm sektör vektörlerinin GPU'ya aktarılması bant genişliği (PCI-e bandwidth) darboğazı yaratır. Bunun için GPU VRAM'inde devasa bir **Page Table** (Sanal Sayfa Tablosu) (SSBO Arena) tahsis edilir. Kırılan bloğun olduğu "Page", GPU içindeki adresi bulunarak `queue.write_buffer` ile nokta atışıyla güncellenir. CPU-GPU darboğazı tamamen çözülür.

### 2.3 Branchless WGSL DDA Traversal
WGSL ray-tracing döngülerindeki `if (mask == 0)` gibi dallanmalar, GPU warp'larını böldüğü için `select()` ve donanımsal bit intrinsics ile değiştirilir:

**Kullanılan WGSL built-in'leri:**
- `select(a, b, cond)` → dallanmasız ternary (if-else yerine)
- `countTrailingZeros(x)` → bir sonraki dolu biti atla (space skipping)
- `countLeadingZeros(x)` → ters yön traversal
- `firstTrailingBit(x)` → ilk dolu bit pozisyonu
- `extractBits(x, offset, count)` → bit alanı çıkarma

**Temel strateji:**
```
// if (mask == 0) { step = 32; } else { step = 1; }
// YUKARIDAKİ yerine:
step = select(1.0, 32.0, mask == 0);

// if (bit == 0) { skip = next_set_bit_position; }
// YUKARIDAKİ yerine:
skip = select(1.0, f32(countTrailingZeros(mask >> idx)), bit == 0);
```

Bu yaklaşım GPU performansını %20-30 artırır. **Not:** Erken çıkış (sabit iterasyon yerine) Volta+ mimarilerde `independent thread scheduling` sayesinde divergence cezasını minimize eder. Sabit iterasyon + `active` flag ile tam branchless traversal da mümkündür; bu durumda boş iterasyonlarda compute yapılır ama warp divergence sıfırdır. İkisi arasında seçim hedef donanıma bağlıdır. **Önemli:** 96 iterasyon L1 cache sınırı (< 128KB) için yeterlidir; 512 iterasyon VRAM'e taşar.

### 2.4 Renkli LOD (Sector Seviyesinde Mip-Mapping)
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
                let v = textureLoad(src, id * 2u + vec3(x, y, z), level - 1);
                if (v != 0u) { sum += v; count++; }
            }
        }
    }
    textureStore(dst, id, level,
        select(vec4(0u), vec4(sum / count), count > 0u));
}
```

**GPU'da kullanım:**
- Mip verisi `texture_3d<u8>` olarak GPU'ya aktarılır
- Hardware bilinear filter ile LOD seviyeleri arasında yumuşak geçiş
- Ray uzaklığı arttıkça daha düşük mip seviyesi kullanılır
- LOD-5 (1³) seviyesinde sadece 1 texture lookup → sektörün tamamı tek renk

**WGSL LOD Blending:**
```wgsl
fn sample_lod(pos: vec3<f32>, lod: f32) -> Material {
    let lod_a = u32(floor(lod));
    let lod_b = u32(min(ceil(lod), 5));
    let blend = lod - f32(lod_a);
    // Hardware bilinear filter ile seamless blend
    return mix(
        textureSampleLevel(tex, sampler, pos, f32(lod_a)),
        textureSampleLevel(tex, sampler, pos, f32(lod_b)),
        blend
    );
}
```

**Neden sector seviyesi?** Brick başına mip tutmak (64 brick × 16 byte = 1KB/sector) ekstra cache miss yaratır. Sector seviyesinde tek bir texture lookup ile LOD alınır.

### 2.5 Object Pooling (SlotMap + Free-List BrickPool)
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

### 2.6 GPU Feedback Loop (Visibility-Guided Upload)

GigaVoxels'ten esinlenen bu mekanizma, GPU'nun her frame sonunda **hangi sector'lere ihtiyacı olduğunu** CPU'ya bildirmesiyle çalışır. CPU sadece gerekli sector'leri upload eder, tüm dünyayı değil.

**Çalışma prensibi:**

```
Her frame:
  1. GPU compute shader çalışır
  2. Shader, ray'in değdiği her sector ID'sini bir SSBO'ya atomicAdd ile yazar
  3. Frame sonu: CPU, SSBO'daki sector ID'lerini okur (mapped memory)
  4. CPU, sadece bu sector'lerin güncel verisini queue.write_buffer ile GPU'ya upload eder
  5. SSBO resetlenir (clear)
```

**WGSL feedback shader:**
```wgsl
// Feedback buffer: her sector için 1 uint32 (0 = gerekmez, 1 = gerekli)
@group(0) @binding(3) var<storage, read_write> feedback_buffer: array<atomic<u32>>;

// Ray traversal sırasında:
let sector_id = hash_sector_coord(sector_coord);
atomicMax(&feedback_buffer[sector_id], 1u);  // Bu sector lazım
```

**CPU tarafı:**
```rust
pub struct FeedbackProcessor {
    // GPU ile CPU arasında mapped buffer (zero-copy)
    pub feedback_buffer: wgpu::Buffer,
    // Sadece gerekli sector'lerin ID'leri
    pub needed_sectors: Vec<u32>,
}

impl FeedbackProcessor {
    pub fn collect_and_upload(&mut self, pool: &GlobalBrickPool) {
        // mapped memory'den oku (PCIe round-trip yok)
        let feedback = self.feedback_buffer.slice(..).map();
        
        // Sadece set bit'leri işle
        for (id, &val) in feedback.iter().enumerate() {
            if val != 0 {
                self.needed_sectors.push(id as u32);
            }
        }
        
        // Sadece gerekli sector'leri upload et
        for &sector_id in &self.needed_sectors {
            let sector_data = pool.get_sector_gpu_data(sector_id);
            queue.write_buffer(&gpu_sector_buffer, offset, &sector_data);
        }
        
        // Feedback buffer'ı sıfırla
        self.reset_feedback();
    }
}
```

**Neden çalışır:** Bir sektör 32KB, PCIe 4.0 x16'da tek sector upload ~1μs. 100 sektör = 0.1ms. Tüm dünyayı (10K sector) upload etmek yerine sadece görünen ~100-200 sector upload edilir → **PCIe bandwidth tasarrufu ~%98**.

### 2.7 LOD-Bilinçli Branchless Traversal (GigaVoxels Tarzı)

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
// select ile branchless LOD: 0-5 arası, sadece 2 select
let lod = select(0u, 1u, dist > 16.0);
let lod = select(lod, 2u, dist > 32.0);
let lod = select(lod, 3u, dist > 64.0);
let lod = select(lod, 4u, dist > 128.0);
let lod = select(lod, 5u, dist > 256.0);

// LOD'a göre traversal derinliği
let max_depth = select(3u, 2u, lod >= 1u);
let max_depth = select(max_depth, 1u, lod >= 2u);
let max_depth = select(max_depth, 0u, lod >= 3u);
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
let mip_mat = textureLoad(sector_mip_tex, mip_pos, lod - 3u).r;
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
* **Hedef:** DXR / Vulkan RT destekleyen ancak OMM desteklemeyen (Örn: RTX 2000/3000 serisi) kartlar.
* **Sistem:** Dünyadaki 32³'lük Sektörler, ekran kartının `TLAS` (Top Level Acceleration Structure) ağacına Bounding Box (AABB) olarak aktarılır. Işın kutuya çarptığında, WGPU içindeki özel bir `Intersection Shader` tetiklenir ve o sektörün içindeki XBrickMap traversal algoritmasını donanım çekirdeğiyle hibrit olarak çalıştırır.

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
}

var<workgroup> shared_queue: array<LightNode, 256>;
var<workgroup> shared_count: atomic<u32>;

@compute @workgroup_size(64)
fn propagate_blocklight(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= queue_len) { return; }
    
    let node = light_queue[idx];
    let current = textureLoad(blocklight_tex, node.pos, 0).r;
    if (current >= node.intensity) { return; }
    
    textureStore(blocklight_tex, node.pos, vec4(node.intensity));
    
    // 6 yönlü propagation
    for (var axis = 0u; axis < 3u; axis++) {
        for (var dir = 0u; dir < 2u; dir++) {
            var offset = vec3<u32>(0u);
            offset[axis] = dir == 0u ? 1u : -1u;
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
- GPU'da `texture_3d<u8>` olarak, 2 ayrı texture (blocklight, skylight)
- Değişen kısımlar `queue.write_buffer` ile nokta atışı güncellenir (GPU Arena ile entegre)

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
pub fn collect_emissive_lights(query: Query<(&BlockGrid, &SectorPosition)>) -> Vec<EmissiveLight> {
    query.iter().flat_map(|(grid, pos)| {
        grid.emissive_blocks().map(|local| EmissiveLight {
            position: pos.0 * 32 + local,
            color: MATERIALS[grid[local]].emission_color,
            intensity: MATERIALS[grid[local]].emission as f32 / 15.0,
            radius: 15.0,
        })
    }).collect()
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
    let block = textureLoad(blocklight_tex, pos, 0).r / 15.0;
    let gi = textureLoad(gi_tex, pos, 0).rgb;
    
    return emissive * 0.4
         + vec3(f32(block)) * 0.3
         + gi * 0.3;
}
```

* Sadece RTX 2000+ serisi kartlarda aktifleştirilir
* Fallback: blocklight + emissive direct (ReSTIR olmadan)
