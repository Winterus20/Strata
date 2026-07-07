# 13 — Aydınlatma Sistemi (KESİNLEŞMİŞ)

> **Durum:** 2026-07-06 tarihinde, altı bağımsız SOTA araştırma ajanıyla (L0/L1/L2/L3/L4/format+temporal/
> culling+NIV) doğrulanmış ve **kesinleşmiştir** (anayasa `01`–`15`). Tüm kod ve `16`+ planları bu
> belgeye uymak zorundadır. Çelişkide `01`–`14` önceliklidir. İkinci revizyon notları (JFA→DDA,
> SH L2→canonical DDGI, "10–25% SVDAG" iddiasının retraction'ı, NIV time-varying düzeltmesi vb.)
> bu belgenin ayrılmaz parçasıdır.

> **Revizyon Notu (2026-07-06):** Bu plan, güncel SOTA araştırmayla (Starlight TECHNICAL_DETAILS,
> 0fps WLP, voxel-light crate, ReSTIR GI, DDGI, Transform-Aware SVDAG 2025, JFA, Hillaire 2020,
> NIV 2026) gözden geçirildi. Kritik değişiklikler:
> 1. **WLP aritmetiği hatalı** (§1.2) — contiguous 4-bit paketleme + borrow guard uyumsuz. Düzeltildi.
> 2. **L3 clustered GI O(n²) pairwise hack** → **DDGI probe grid (SH L2)** (§1.5).
> 3. **L4 sabit 6-koni** → **ReSTIR GI reservoir** + LOD-anchored SVDAG march (§1.6).
> 4. **BFS:** `VecDeque` → **Dial 16-bucket**, pooled visited map, `IVec3`→`u64` anahtar (§1.3).
> 5. **Sky light:** tek max-height yerine **column-continuity** (overhang/mağara doğruluğu) (§1.4).
> 6. **Temporal:** voxel-keyed reprojection + variance-guided cleanup (§1.8).
> 7. **Day/night:** Hillaire 2020 atmosfer (plan 23 §4 ile hizalı).
> 8. Detaylar: `plans/research-lighting-gi-2026.md`.
>
> **Revizyon Notu (2026-07-06, ikinci geçiş — altı bağımsız subagent SOTA araştırması):** Ek
> doğrulamalar:
> - **L1:** Queue yapısı (Dial) ikincil; asıl kazanç **Starlight dual-queue (increase/decrease)
>   + origin-direction pruning**. SIMD mütevazı (1.3–2× flood core); per-voxel erişim layout'u
>   (tek coord resolve + tek id/rotasyon/emission lookup) SIMD'den önce gelmeli.
> - **L2:** **JFA sky-occlusion YANLIŞ ARAÇ** (distance-to-seed verir, light level değil; hata
>   overhang/mağara sınırında). Yerine **GPU upward column DDA** (XBrickMap brick mask reuse, exact,
>   tek pass). JFA yalnız "distance-to-open-sky" sanatsal falloff için JFA+1/+2 ile.
> - **L3:** SH L2 encoding **non-standard + leakage'i zayıflatır**. Canonical DDGI kullan
>   (`8×8` octahedral `R11G11B10F` irradiance + `16×16` `RG16F` mean/mean² distance, Chebyshev
>   moment visibility). **Moment distance field leakage'i önleyen en kritik parçadır.** Bellek
>   ~6KB/probe → **~0.4–3MB/sector** (planın 64–512KB iddiasıyla çelişir; imza öncesi uzlaştır).
>   4-tier streaming'i DDGI probe cascade'ine harala (Majercik 2021). LPV yalnız no-RT fallback;
>   uzun vadeli unified hedef = **Voxel Cone Tracing** (XBrickMap reuse).
> - **L4:** "Transform-Aware SVDAG 10–25% faster" İDDİASI YANLIŞ (paper statik, yalnız memory/geometry
>   reuse der). Yerine **"Encoding Occupancy in Memory Location" (CGF 2025)** — child-mask pointer'da,
>   fetch azaltır (gerçek incoherent-ray kazancı). SVDAG maliyeti ReSTIR'i çözmez → **iki-seviye
>   radyans cache** (ReSTIR GI = bounce 2; budget'li world-space radiance cache = bounce ≥3/far;
>   Bevy Solari: cache pass 1.42ms→0.09ms). Cone march'a **directional/SH radiance per node**
>   (anisotropic, leak önler) + low-roughness/emissive için **MIS with raw path-traced**.
> - **§1.2:** 16-bit (4-bit/ch) = *simulation/storage*; shading/GI'de 8-bit (`u32`) veya `r16f`'e
>   yükselt. `const generic` ile 4-bit↔8-bit switch (default 8-bit, HDR GI). Approach A compute
>   primitive, B (WLP) yalnız storage + bit-exact test oracle.
> - **§1.8:** İnvalidate yalnız `SectorLoaded/Unloaded` değil — `NeedsRemesh`, `NeedsSvdagBake` ve
>   **per-voxel block edit**'te de reset et (içerik değişince history fiziksel olarak yanlış). `α`'yı
>   global kamera hızı yerine per-key validity flag'den sür. Voxel-keyed reprojection **SOTA** (Ott
>   2025; NAADF 2026 — 32-frame history, quantized key ucuz); mümkünse NRD (REBLUR/RELAX) sar.
> - **§1.7:** slab/brick `u64` mask clustered-shading'e denk (XBrickMap reuse, ~O(1) branchless).
>   Construction'da **Morton-sorted 32-ary BVH** (Olsson 2012) + decoupled XY/Z + subgroup ballot +
>   normal-cone culling.
> - **§1.12:** NIV (arXiv:2602.12949) **zaten time-varying field destekler** — "runtime linear
>   day/night composite imkansız" iddiası YANLIŞ. Gerçek limit: training/bake + generalize. Day/night
>   analitik Hillaire sun+sky kanalında; NIV yalnız static distant indirect. **Primary distant =
>   DDGI / Godot-style SDFGI cascade** (voxel/SVDAG-native, no TensorCore); NIV experimental trait.

## 1. Aydınlatma Sistemi — 5-Kademeli Hybrid Mimari

### 1.1 Genel Bakış

| Kademe | Ad | Yöntem | Frekans | Performans |
|---|---|---|---|---|
| **L0** | Direct Light | Analytic (sun, point lights) | Her frame | ~0.1ms |
| **L1** | Block Light (BFS) | CPU SIMD flood-fill + two-phase removal | Değişiklikte | <100µs/torch |
| **L2** | Sky Light | Column-first + column-continuity (Starlight-style) | Sector load/değişiklik | <0.5ms/sector |
| **L3** | Indirect GI (near) | **DDGI probe grid (SH L2)** + visibility buffer | ACTIVE: N frame, WARM: M frame | <1ms/sector |
| **L4** | Indirect GI (far) | **ReSTIR GI reservoir** + SVDAG ray march + Hi-Z | Her 10 frame | <2ms |

### Temel Prensipler

- **L0 = Direct:** Anlık, maliyetsiz — mesh'e doğrudan bake
- **L1 = Block:** BFS zaten gerekli, SIMD ile ultra-hızlı — mesh vertex color'a bake
- **L2 = Sky:** Starlight-style column-first + column-continuity, XBrickMap slab bitmask'inden O(1) sky-source
- **L3 = Indirect near:** DDGI probe grid — oyuncuya yakın alanlarda doğru, sızıntısız GI
- **L4 = Indirect far:** ReSTIR GI reservoir (emissive voxel = area light) + SVDAG ray march

### 1.1.1 Resmi Öneri: Hibrit GI Mimarisi (Near=DDGI, Far=ReSTIR)

> **KARAR (2026-07-06):** Strata'nın GI katmanı için **tek yöntem değil, iki katmanlı hibrit**
> benimsenir. Gerekçe: tek yöntem Strata'nın çelişen kısıtlarını (editing-heavy + sınırsız dünya +
> GPU-driven + distal kalite) tek başına karşılayamaz.

- **Near (ACTIVE + WARM):** **DDGI probe grid** (SH L2, moment-based visibility).
  - Artı: anlık güncellenir, edit'e dost, ray-trace gerektirmez (SVDAG yalnız trace target).
  - Alternatif: **LPV (SH radiance propagation)** — saf voxel, SVDAG trace'i hiç gerektirmez;
    "SVDAG bağımlılığı istemiyorum" durumunda fallback. (Kaplanyan & Dachsbacher 2010;
    Cosin Ayerbe 2022 clustered voxel GI.)
- **Far (DISTANT):** **ReSTIR GI reservoir** over SVDAG. Emissive voxel'lar (lava/glowstone/torch)
  area light olarak örneklenir; visibility buffer `voxel_coord`+normal G-buffer'ı bedava verir.
- **ARCHIVE:** Render edilmez; NIV (Faz 6) distant-only opsiyonel.

**Neden hibrit en iyisi:**
| Yalnızca... | Eksik |
|---|---|
| DDGI | distalde pahalı / az doğru |
| ReSTIR PT (tam) | her şeyi ray-trace eder, edit'te stall |
| LPV | distalde enerji kaybı / leak |
| **Near=DDGI + Far=ReSTIR** | **her iki kısıtı da karşılar — önerilen** |

> Not: LPV(SH) fallback'i `indirect/lpv.rs` olarak eklenebilir; trait arkası sayesinde DDGI ile
> çalışma zamanında swap edilebilir. NIV yalnız ARCHIVE/distant için experimental backend.
>
> **DÜZELTME (ikinci revizyon, L0/Day-Night):** Hillaire 2020 atmosphère doğru (Unreal standart,
> 2D multi-scatter LUT, runtime hava değişimi). Strata kübik dünya → güneş yönü keyfi; per-voxel
> **sky-exposure/aperture** (güneşe DDA/oklüzyon konisi) ayrı bir directional field olsun, yalnız
> güneş açısı eşiği geçince re-project et. **Gün/geceyi bloğa bake ETME** — 0fps iki-kanal prensibiyle
> ayrı zaman-değişken `sky` kanalı tut (day/night = uniform update, flood değil). Hillaire aerial
> perspective'i DISTANT/WARM tier'a (SVDAG uzak görünüm) bağla.

---

### 1.2 Light Data Formatı (16-bit Packed)

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

**WLP (Weighted Lexicographic Packing) — DÜZELTME:** Orijinal plandaki `wlp_*` sabitleri
(`COMPONENT_MASK=0x0F0F0F0F`, `BORROW_GUARD=0x08080808`) contiguous 4-bit paketleme ile
**uyumsuzdur** ve cross-channel borrow corruption'a yol açar (örn. sıfır kanalı yanlış şekilde
"aydınlatır"). İki doğru yaklaşım:

- **(A) Önerilen — one channel per SIMD lane:** `sky,R,G,B` ayrı `u8` (veya `u32x4` lane) olarak
  tutulur; per-lane native `max`/`sub`/`compare` ile WLP tamamen gereksizleşir, hem doğru hem
  daha hızlı (voxelize PR #93/#95/#97: kanalları eşzamanlı flood etmek bit-uyumlu + 1.29–1.48×).
- **(B) Guard-separated nibbles (0fps 2018 canonical):** Hesaplama için 32-bit'e açılır — kanal
  yüksek nibble'da, düşük nibble = 0 guard. Tam 0fps sabitleri kullanılır:

```rust
// 0fps canonical: kanal YÜKSEK nibble'da, düşük nibble guard=0
// SADECE 32-bit ara register'da kullan; 16-bit'e pack etmeden ÖNCE isolate et.
const COMPONENT_MASK: u32 = 0xF0F0F0F0;   // her byte'ın yüksek nibble'ı
const BORROW_GUARD:  u32 = 0x20202020;    // her yüksek nibble'ın üst biti
const CARRY_MASK:    u32 = 0x10101010;    // her byte'ın bit 4'ü (borrow sınırı)

#[inline]
pub fn wlp_less_than(a: u32, b: u32) -> u32 {
    let d = (((a & COMPONENT_MASK) | BORROW_GUARD) - (b & COMPONENT_MASK)) & CARRY_MASK;
    (d | (d >> 3) | (d >> 4)) & COMPONENT_MASK
}
```

> **Kritik:** `LightData` 16-bit storage'da kanallar contiguous (bits 0-3,4-7,...) kalır; WLP
> arithmetic YALNIZCA (B)'deki gibi guard-separated 32-bit formda yapılır, sonra `pack_16`
> ile izole edilir. **(A) tercih edilir** — SIMD lane başına 1 kanal borrow riskini tamamen eler.

```rust
#[repr(transparent)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LightData(pub u16);

impl LightData {
    #[inline]
    pub fn sky(&self) -> u8 { (self.0 & 0xF) as u8 }

    #[inline]
    pub fn block_r(&self) -> u8 { ((self.0 >> 4) & 0xF) as u8 }

    #[inline]
    pub fn block_g(&self) -> u8 { ((self.0 >> 8) & 0xF) as u8 }

    #[inline]
    pub fn block_b(&self) -> u8 { ((self.0 >> 12) & 0xF) as u8 }

    #[inline]
    pub fn new(sky: u8, r: u8, g: u8, b: u8) -> Self {
        Self(
            (sky & 0xF) as u16
            | ((r & 0xF) as u16) << 4
            | ((g & 0xF) as u16) << 8
            | ((b & 0xF) as u16) << 12,
        )
    }

    // --- YAKLAŞIM (A): SIMD lane başına tek kanal — borrow yok ---
    // Her lane bir kanalın N voxelini taşır; max/sub native. WLP gereksiz.
    #[inline]
    pub fn max_channel(a: u32, b: u32) -> u32 { a.max(b) }   // per-lane

    // --- YAKLAŞIM (B): guard-separated 32-bit WLP (referans/CPU fallback) ---
    #[inline]
    pub fn wlp_max(a: u32, b: u32) -> u32 {
        let lt = Self::wlp_less_than(a, b);
        a ^ ((a ^ b) & lt)
    }

    #[inline]
    pub fn wlp_less_than(a: u32, b: u32) -> u32 {
        const COMPONENT_MASK: u32 = 0xF0F0F0F0;
        const BORROW_GUARD: u32 = 0x20202020;
        const CARRY_MASK: u32 = 0x10101010;
        let d = (((a & COMPONENT_MASK) | BORROW_GUARD) - (b & COMPONENT_MASK)) & CARRY_MASK;
        (d | (d >> 3) | (d >> 4)) & COMPONENT_MASK
    }
}

// --- ZORUNLU property test (WLP DÜZELTMESİNİ DOĞRULAR) ---
// Tüm 4 kanal için scalar referansla random input (sıfır kanal + 15 sınırı dahil) eşitliği iddia et.
// Colored-removal over-zero (aşağıdaki §1.3) için ayrı test.
//
// > **DÜZELTME (ikinci revizyon) — İKİ FORMAT AYRIMI:** 16-bit (4-bit/ch) YALNIZ *simulation/
// > storage* formatıdır (Minecraft gibi, 2 byte/voxel ucuz). Shading/GI'de 4-bit banding + HDR
// > yetersiz → `u32` (8-bit/ch) veya `r16f`'e yükselt. `LightData` bir `const generic` olsun
// > (default 8-bit HDR GI hedefi). Approach A (one-lane-per-channel) **compute primitive**;
// > Approach B (WLP) yalnız **storage encoding + bit-exact test oracle**. Pool/storage'da 16-bit,
// > propagation kernel'a `u32x4` aç, native max/sub_sat, geri pack et.


**Bellek Hesabı:**
- Sub-brick başına: 8 voxel × 16 bit = 16 byte
- Brick başına: ~64 sub-brick × ~16 byte = ~1KB (left-packed)
- Slab başına: ~64 brick × ~1KB = ~64KB (sparse)
- Sector: ~128-256KB (ortalama arazi)

---

### 1.3 L1 — Block Light (SIMD BFS Flood-Fill)

#### Propagation (Işık Yerleştirme)

```rust
// PERFORMANS NOTLARI (2026 revizyon):
// - VecDeque yerine **Dial 16-bucket queue** (level 0..15 → 16 bucket, O(1) dequeue,
//   doğal wavefront, SIMD-bucket dostu). voxel-light crate'i referans al.
// - visited map PER-CALL ayrılmaz: **pooled** generation-stamped bitset / HashMap kullan.
//   IVec3 hash yavaştır → koordinatı u64'e pack et (morton veya linear sector offset).
// - Voxel erişimi: komşu başına TEK XBrickMap resolve (voxelize PR #93: 2.1–2.5×).
// - 4 kanalı eşzamanlı flood et (rayon + private copy + overlay-merge) → bit-uyumlu 1.29–1.48×.
// - Büyük editlerde runUpdates(maxSteps) bütçeleme: işi frame'lere yay (mountain edit stall önleme).
//
// ÖNCELIK (ikinci revizyon, 2026-07): Queue yapısı (Dial) İKİNCİL. Asıl doğruluk+perf kazancı
// Starlight'tan: **ayrı increase-queue / decrease-queue** + her entry'de **origin-direction mask**
// (back-edge'i atla). Bu, removal'ı tüm sektörü yeniden flood etmeden doğru yapar.
// `voxel-light`'ın tek-kuyruklu two-phase'i bunun yaklaşığı; dual-queue'ya yükselt (az over-zero).
// SIMD mütevazı (flood core 1.3–2×, 10–15× yalnız bitwise-bitmap); per-voxel ERİŞİM LAYOUT'U
// (tek coord resolve + tek id+rotasyon+emission lookup, non-emitter emission okumasını atla)
// SIMD'den ÖNCE gelmeli — çoğu durumda SIMD'den daha çok kazandırır.

pub struct BlockLightEngine {
    buckets: [Vec<BfsNode>; 16],     // Dial bucket queue
    visited: PackedVisited,          // pooled, generation-stamped
    buffer_pool: BufferPool,
}

#[repr(C)]
pub struct BfsNode {
    pub pos: UVec3Packed,            // u64 packed coord
    pub light_level: u8,
}

impl BlockLightEngine {
    pub fn place_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        level: u8,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        updates.push(LightUpdate { pos, light: level, color });

        self.queue.clear();
        self.queue.push_back(BfsNode { pos, light_level: level });
        self.visited.clear();
        self.visited.insert(pos, level);

        while let Some(node) = self.queue.pop_front() {
            let current_level = node.light_level;
            if current_level <= 1 { continue; }

            for dir in DIRECTIONS_6 {
                let neighbor = node.pos + dir;
                if self.visited.contains_key(&neighbor) { continue; }
                if sector.is_opaque(neighbor) { continue; }

                let new_level = current_level - 1;
                let existing = sector.get_light(neighbor);

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

#### Two-Phase Removal (Işık Kaldırma)

> **DÜZELTME (colored over-zero):** Orijinal `zero_dependents` HERHANGİ bir kanal eski level'in
> altındaysa bağımlı sayar ve fazla sıfırlar (farklı renk aralıklarına sahip örtüşen colored
> source'larda doğru değil). Düzeltme: `voxel-light` tarzı **boundary-source + overlay** yaklaşımı —
> sadece bu kaynağa bağımlı olanları sıfırla, sonra boundary source'lardan re-propagate et; "başka
> kaynaktan daha parlak komşu"yu yeniden doğrula (provenance) ki yanlış ışık çalmayasın.

```rust
impl BlockLightEngine {
    pub fn remove_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        let boundary_sources = self.zero_dependents(sector, pos, color, &mut updates);

        for source in boundary_sources {
            let new_updates = self.place_light(sector, source.pos, source.level, color);
            updates.extend(new_updates);
        }

        updates
    }

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
                    updates.push(LightUpdate { pos: neighbor, light: 0, color });
                    self.queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: neighbor_level,
                    });
                } else if neighbor_level >= node.light_level {
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

#### SIMD Acceleration (15x Hızlanma)

```rust
// ÖNERİLEN SIMD: lane başına TEK kanal (sky/R/G/B), her lane N voxel.
// WLP'ye GEREK YOK — native per-lane max/sub. (A) yaklaşımı, §1.2 ile hizalı.
use wide::u32x8;

// Her kanal ayrı register'da; 8 voxel/lane.
pub fn propagate_simd_channels(
    sky: &mut [u32x8], r: &mut [u32x8], g: &mut [u32x8], b: &mut [u32x8],
    queue: &mut BfsQueue,
) {
    while let Some(node) = queue.pop() {
        // tek kanal max/combine — borrow riski yok
        // current_level - 1, neighbor < current → store, push
    }
}
```

> **NOT:** Orijinal `wlp_less_than_simd` / `wlp_decrement_simd` (§1.2'deki bozuk sabitlerle) KULLANILMAZ.
> Yerine yukarıdaki one-channel-per-lane form. `std::simd` (portable_simd, stable 1.80+) `wide`
> yerine tercih edilebilir keyfi lane genişliği için. `packed_simd` ÖLÜ — KULLANMA.

**Performans (Ryzen 9 7900, voxel-light crate):**

| Operasyon | Level 7 | Level 10 | Level 14 |
|---|---|---|---|
| Propagation (scalar) | 17µs | 60µs | 174µs |
| Propagation (SIMD) | ~5µs | ~18µs | ~52µs |
| Removal (tek kaynak) | 105µs | — | 432µs |
| Full place+remove cycle | — | — | ~300µs (SIMD) |

---

### 1.4 L2 — Sky Light (Column-First + Heightmap)

#### Sky Source Setup — Column-Continuity (Overhang/Mağara Doğruluğu)

> **DÜZELTME:** Tek `max-height` heightmap, overhang (yukarı-çık-aşağı-in) altındaki yüzeyleri
> kaçırır → ışık sızıntısı/kaybı. Vanilla 1.20 `ChunkSkyLightSources` gibi **her (x,z) kolonunda
> en üstten aşağı tarayıp ilk opak bloğa kadar süren opaklık-0 koşusunu** bul; boşluklu kolonlarda
> opak bloğun ALTINDAKİ yüzeyleri de sky-kaynağı olarak işaretle. XBrickMap slab/brick maskeleri
> zaten mevcut → "tümü opaklık-0" bitmask'i O(1) sky-source seed'i için yeniden kullan.

```rust
impl Sector {
    // Her kolonda sky-erişilebilir Y yüzeylerini döndürür (overhang altındakiler dahil).
    pub fn build_sky_sources(&self) -> Vec<i16> {
        let mut sources = Vec::new();
        for sx in 0..32 {
            for sz in 0..32 {
                let mut y = 31i16;
                // en üstten aşağı: opaklık-0 koşusunun bittiği (ilk opak) seviyeyi bul
                while y >= 0 && self.is_opacity0(sx as i32, y as i32, sz as i32) {
                    y -= 1;
                }
                sources.push(y); // y = ilk opak bloğun Y'si; altı sky-erişimli yüzey
            }
        }
        sources
    }
}
```

#### Column-First Propagation

```rust
impl SkyLightEngine {
    pub fn propagate_sky(&mut self, sector: &Sector) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        let sources = sector.build_sky_sources();

        for sx in 0..32 {
            for sz in 0..32 {
                let top_opaque_y = sources[sx + sz * 32];
                // sky-erişilebilir her yüzeyi 15 ile doldur (overhang altı dahil)
                for y in (0..=top_opaque_y).rev() {
                    if sector.is_opacity0(sx as i32, y, sz as i32) {
                        updates.push(LightUpdate {
                            pos: IVec3::new(sx as i32, y, sz as i32),
                            light: 15,
                            color: LightColor::Sky,
                        });
                        if sx == 0 || sx == 31 || sz == 0 || sz == 31 {
                            self.horizontal_queue.push_back(BfsNode {
                                pos: IVec3::new(sx as i32, y, sz as i32),
                                light_level: 14,
                            });
                        }
                    }
                }
            }
        }

        self.horizontal_bfs(sector, &mut updates);
        updates
    }

    fn horizontal_bfs(&mut self, sector: &Sector, updates: &mut Vec<LightUpdate>) {
        while let Some(node) = self.horizontal_queue.pop_front() {
            if node.light_level == 0 { continue; }

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
- **~7x az queue işlemi** (Starlight level-propagation + direction-pruning'ten; benchmark gerekir, internal tahmin)
- En büyük O(1) kazanç: sky-source seed'lerini XBrickMap slab/brick mask'lerinden çıkar (Vanilla
  `ChunkSkyLightSources`'ın 32³ granüllü hali — sector tavan slab mask'i + sub-brick mask'leri).

> **DÜZELTME (ikinci revizyon):** "GPU JFA sky-occlusion" **YANLIŞ ARAÇ**. JFA yalnız
> distance-to-open-sky verir (light level değil), ve hata tam overhang/mağara sınırlarında oluşur
> (mob spawn için riskli). Yerine **GPU upward column DDA** (her x,z için tek yukarı ışın, mevcut
> XBrickMap brick mask'lerini yeniden kullanır, exact, tek pass). JFA yalnız ileride "yumuşak
> distance-to-open-sky" sanatsal falloff istenirse JFA+1/+2 ile gündeme gelir. Column-first BFS
> CPU'da kalır (doğru attenuation için authoritative path).

---

### 1.5 L3 — Indirect GI (DDGI Probe Grid)

> **DÜZELTME:** Orijinal "clustered pairwise GI" (O(clusters²) Bresenham, `1/(1+d²)` normalize,
> enerji korunması yok, ışık sızıntısı) gerçek GI değildi. **DDGI** (Majercik 2019, JCGT) ile
> değiştirildi — oyunlarda çalışan modern standart. `#[repr(C)]` struct + `1/(1+d²)` normalize DROP edildi.

> **DÜZELTME (ikinci revizyon):** **SH L2 encoding NON-STANDARD ve leakage'i zayıflatır.** Canonical
> DDGI kullan: `8×8` octahedral **irradiance** (`R11G11B10F`) + `16×16` octahedral `RG16F`
> **mean/mean² distance** (Chebyshev moment visibility). **Moment distance field, leakage'i önleyen
> en kritik parçadır** — SH L2'ye trade etme (SH L2 yalnız glossy aynı probe'dan sürülüyorsa gerekir).
> 4-tier streaming'i DDGI probe **cascade**'ine harala (Majercik 2021 multiresolution cascaded volumes):
> ACTIVE yüksek çözünürlük, WARM kaba. Edit'te yalnız o sektörün probe'ları dirty (Adaptive DDGI).
> LPV yalnız no-RT/no-SVDAG fallback; uzun vadeli unified near+far hedefi = **Voxel Cone Tracing**
> (XBrickMap/SVDAG reuse). IS-DDGI (MIS + adaptive temporal reuse) drop-in eklenebilir (1.27–2.47×).

#### Probe Grid Yapısı (per-sector, sparse)

```rust
// 32³ sector → probe spacing 4-8 voxel → 64..512 probe/sector.
// Canonical DDGI: 8×8 octahedral irradiance (R11G11B10F ≈ 2KB) +
//                 16×16 octahedral RG16F mean/mean² distance (Chebyshev) ≈ 4KB.
// ~6KB/probe → ~0.4–3MB/sector (aşağıdaki bellek notuna bak).
pub struct DdgiProbeGrid {
    pub spacing: u8,                  // 4 veya 8
    pub probe_count: UVec3,           // 32/spacing
    pub irradiance: Vec<OctahedralTex>,   // 8×8 R11G11B10F
    pub distance:  Vec<OctahedralTex>,    // 16×16 RG16F mean/mean² (leakage önler)
}


// Probe güncelleme: rotating schedule — birkaç sector/frame (streaming tier ile hizalı).
// ACTIVE: her N frame; WARM: her M frame; DISTANT: coarser probe veya NIV (§1.12).
impl DdgiProbeGrid {
    pub fn update(&mut self, trace_target: &SvdagRoot) {
        // Her probe: yarıçap içi voxel'ları SVDAG ile ışınla (emissive voxel = area light),
        // octahedral radiance + visibility moment birikimi. Moment-based interpolant sızıntıyı eler.
    }
}
```

**Avantaj / Performans:**
- ~**1.0 ms/frame** diffuse GI (RTX 2080 Ti, 1080p), IS-DDGI ile 1.27–2.47× daha hızlı.
- **~0.4–3 MB/sector** probe verisi (canonical ~6KB/probe × 64–512; planın 64–512KB iddiası
  SH L2 + tek moment ile mümkün ama leakage riski taşır — streaming VRAM bütçesiyle uzlaştır).
- Yalnız ACTIVE/WARM sector'da canlı probe gerekir; streaming event'leriyle rotate (cascade).
- ReSTIR GI (§1.6) ile birleşir; SVDAG trace target olarak kullanılır.

---

### 1.6 L4 — Indirect GI (SVDAG + ReSTIR Reservoir)

> **DÜZELTME:** Sabit 6-koni yerine **ReSTIR GI reservoir** (Ouyang 2021; RTXDI SDK). Emissive
> voxel'lar (lava/glowstone/torch) area light olarak örneklenir; visibility buffer `hit.voxel_coord`
> + normal G-buffer'ı bedava verir. Eğer koni izi kalacaksa **LOD-anchored** olmalı (aşağıda).
>
> > **DÜZELTME (ikinci revizyon):** "Transform-Aware SVDAG 2025 ile 10-25% daha hızlı incoherent
> > ray" İDDİASI YANLIŞ — paper (PACMCGIT 2025) bunu söylemez; yalnız *memory/geometry-reuse*
> > (mirror/rotation/translation/axis-permutation matching + variable-length pointer) der ve
> > **statiktir** (edit'te rebake gerekir). Yerine **"Encoding Occupancy in Memory Location"**
> > (Modisett & Billeter, CGF 2025) — child-mask pointer bitlerinde, intersection'da ek fetch
> > gerektirmez; gerçek incoherent-ray kazancı bu. SVDAG maliyeti ReSTIR'i çözmez (incoherent
> > secondary ray'e düşman, SIMD divergence). Bu yüzden **iki-seviye radyans cache** şart: ReSTIR
> > GI = bounce 2 resampler; budget'li **world-space radiance cache** (Occupancy DAG ayrı, Radiance
> > DAG cached/budget'li) = bounce ≥3 / far (Bevy Solari modeli: cache pass 1.42ms→0.09ms, GI ray
> > mesafesi ~4m kapalı + LOD-tiered). NIV uzak backstop (§1.12).

#### ReSTIR GI Reservoir (WGSL sketch)

```wgsl
// Her pixel: reservoir (sample position, radiance, weight, M).
// Spatial + temporal resample; unbiased/controlled-bias; emissive voxel'ları gerçek emitter olarak örnekler.
@compute @workgroup_size(64, 1, 1)
fn restir_gi(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.xy;
    let hit = visibility_buffer_load(pixel);
    if (hit.depth == MAX_DEPTH) { return; }

    // BRDF-sampled secondary bounce → "light sample"; reservoir'a ekle.
    // Hemisferi UNIFORM örnekle (cosine-weighted kaçının), blue-noise desen.
    var reservoir: Reservoir;
    for (var i = 0u; i < NUM_SAMPLES; i++) {
        let dir = sample_uniform_hemisphere(hit.normal, blue_noise(pixel, i));
        let radiance = svdag_trace_emitter(hit.position, dir, svdag_root); // SVDAG march
        reservoir.add_sample(dir, radiance, 1.0);
    }
    // spatial + temporal resample (komşu pixel + geçen frame)
    restir_spatial_temporal(reservoir, pixel);
    let irradiance = reservoir.estimate();
    irradiance_cache_store(hit.voxel_coord, irradiance);
}
```

#### LOD-Anchored SVDAG March (koni izi korunursa)

```wgsl
fn svdag_cone_march(origin: vec3<f32>, direction: vec3<f32>, aperture: f32, root: u32) -> vec3<f32> {
    var t = 0.0;
    var radiance = vec3<f32>(0.0);
    var cone_width = aperture;

    // DURDURMA: cone_width > node_size olduğunda alt ağacı atla, pre-filtered node radiance oku.
    // (Encoding Occupancy in Memory Location 2025: child-mask pointer'da, fetch azaltır.)
    for (var i = 0u; i < 64u; i++) {
        let node = svdag_query_lod(root, origin + direction * t, cone_width);
        if (node.is_leaf || cone_width > get_node_size(node.lod)) {
            radiance += node.radiance * node.opacity;
            break;
        }
        t += get_node_size(node.lod);     // step = node size (LOD-anchored)
        cone_width = aperture * t;        // cone footprint ≈ pixel footprint
    }
    return radiance;
}
```

**Notlar:**
- Thin geometry ışık sızıntısı → opacity-weighted stop + DDGI moment visibility ile hafiflet.
- Yüksek frekanslı emissive düşük LOD'da söner → ReSTIR gerçek emitter'ı örnekler, bunu eler.
- Occupancy DAG (XBrickMap, ucuz) ile Radiance DAG (pahalı, cached, budget'li güncelleme) AYRILMALI.
- **DÜZELTME:** Cone march **scalar radiance yerine directional/SH radiance per node** (anisotropic,
  DXE tarzı) saklamalı — aksi halde LOD-boundary leak + over-blur. `lod = log2(coneDiameter/nodeSize)`
  standard stop kuralı (Crassin 2011). Low-roughness/emissive bloklarda ReSTIR'e **MIS with raw
  path-traced input** ekle; **biased ReSTIR** varyantı framerate için tercih edilir.

---

### 1.7 Hierarchical Light Culling

```rust
pub struct LightCullingMask {
    pub slab_light_mask: u64,
    pub brick_light_mask: u64,
    pub sorted_lights: Vec<LightSource>,
}

impl LightCullingMask {
    pub fn sort_lights_morton(&mut self) {
        self.sorted_lights.sort_by_key(|l| {
            morton_encode_3d(
                l.pos.x as u32,
                l.pos.y as u32,
                l.pos.z as u32,
            )
        });
    }

    #[inline]
    pub fn slab_has_light(&self, brick_index: usize) -> bool {
        self.slab_light_mask & (1 << brick_index) != 0
    }

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

> **DÜZELTME (ikinci revizyon):** slab/brick `u64` mask'leri **clustered-shading'e denk** ve
> XBrickMap mask'lerini sıfır ekstra bellekle yeniden kullanır (~O(1) branchless). Construction
> pass'te **Morton-sorted 32-ary BVH** (Olsson 2012) kullan; **decoupled XY/Z** (themaister/Persson:
> per-brick `u64` bitmask = XY, sorted Z-index range) → O(N) memory scaling 10K+ light. **Subgroup
> ballot** ile branchless mask üret; **normal-cone culling** (Olsson) ile yanlış pozitifleri azalt.
> Mask'lar `GlobalBrickPool` slotlarında tut (heap fragmentation yasağı, plan 06).

---

### 1.8 Temporal Accumulation (Voxel-Keyed Reprojection)

> **DÜZELTME:** Tek `mix()` blend'i ghosting/yumuşatma yapar. **Voxel-keyed reprojection**
> (Ott 2025 voxel-world TAA; NAADF 2026 — 32-frame history, quantized key ucuz): history anahtarı
> olarak visibility buffer `voxel_pos`/`sector_id` kullan (float hatası yok, keskin geometri).
> **Voxel-keyed reprojection SOTA'dır** — camera teleport/streaming'de history hayatta kalır.
> Variance-guided spatial cleanup **SVGF** (Schied 2017) veya mümkünse **NRD** (REBLUR/RELAX, `SH`
> mode) sar; per-key 1st/2nd moment variance → à-trous, disocclusion'da `α→1` / reservoir `→0`.

```wgsl
@compute @workgroup_size(8, 8, 1)
fn temporal_accumulate(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.xy;
    let current = current_frame_irradiance(pixel);
    let hit = visibility_buffer_load(pixel);

    // Voxel-stable history key (depth/normal/voxel_id uyumsuzsa reject)
    let history_sample = history_buffer_sample_voxel(hit.voxel_coord, hit.sector_id);
    let valid = history_valid(hit);
    let variance = estimate_variance(pixel);

    // Adaptive alpha: statik & valid → düşük; edit/disocclusion → yüksek
    let blend = select(HIGH_ALPHA, adaptive_alpha(variance), valid);
    let result = mix(history_sample, current, blend);

    final_irradiance_store(pixel, result);
    history_buffer_store_voxel(hit.voxel_coord, hit.sector_id, result);
}

// Edit/load invalidation: SectorLoaded/Unloaded (plan 08) event'i → ilgili history texel'i anında
// geçersiz kıl. AYRICA içerik değişince history fiziksel olarak yanlış olur:
//   - NeedsRemesh, NeedsSvdagBake event'leri → ilgili sector history'si reset.
//   - per-voxel block edit → o texel'in α'sı →1 / reservoir confidence →0.
// Adaptive α per-key validity flag'den sürülür (global kamera-hızı heuristic DEĞİL).
```

---

### 1.9 Mesh'e Light Bake

```rust
pub fn bake_light_to_mesh(
    mesh: &mut MeshData,
    sector: &Sector,
    face_vertices: &[IVec3],
) {
    for vertex in face_vertices {
        let light_samples = [
            sector.get_light(*vertex),
            sector.get_light(*vertex + IVec3::new(1, 0, 0)),
            sector.get_light(*vertex + IVec3::new(0, 1, 0)),
            sector.get_light(*vertex + IVec3::new(1, 1, 0)),
        ];

        let smooth_light = smooth_lighting(light_samples);
        vertex.color = light_to_color(smooth_light);
    }
}

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

### 1.10 Tier-Bazlı Lighting Stratejisi

| Tier | Yöntem | Güncelleme | Not |
|---|---|---|---|
| **ACTIVE** (0-96m) | L0+L1+L2+L3 (CPU BFS + **DDGI** probe) | Her değişiklikte | En doğru, mesh'e baked |
| **WARM** (96-384m) | L0+L1+L2+L3 (DDGI, seyrek güncelleme) | Her M frame | Yumuşak geçiş |
| **DISTANT** (384m-1.5km) | L0+L4 (**ReSTIR GI** over SVDAG) | Her 10 frame | Yaklaşık GI |
| **ARCHIVE** (1.5km+) | L0 sadece (NIV opsiyonel, Faz 6) | — | Render edilmez |

> Hibrit karar (§1.1.1): Near=DDGI, Far=ReSTIR. LPV(SH) DDGI'ye trait-arkası fallback.

---

### 1.11 GPU Lighting Pipeline

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
│ Pass 4: DDGI Probe Update (GPU Compute) — L3 near                    │
│   → Probe trace (SVDAG trace target)                                 │
│   → Octahedral radiance + moment visibility birikimi                 │
│   → Rotating schedule (ACTIVE N frame, WARM M frame)                │
├──────────────────────────────────────────────────────────────────────┤
│ Pass 5: ReSTIR GI (GPU Compute) — L4 far                            │
│   → Reservoir sample (emissive voxel = area light)                  │
│   → Hi-Z occlusion + SVDAG march                                    │
│   → Spatial + temporal resample → irradiance cache                  │
│   → Temporal accumulation (voxel-keyed reprojection)                 │
├──────────────────────────────────────────────────────────────────────┤
│ Pass 6: Light → Mesh Bake (CPU)                              │
│   → Smooth lighting (4-vertex average)                       │
│   → Vertex color write                                       │
└──────────────────────────────────────────────────────────────┘
```

---

### 1.12 Neural Irradiance Volume (Faz 6 — Distant-Only, Opsiyonel)

> **Güncelleme:** NIV artık 2026 (Eurographics, arXiv:2602.12949). **Phase-6 distant-only
> opsiyonel backend** olarak tut — çekirdek tier değil. Training SVDAG path-traced baker
> gerektirir (önce §1.4/§1.5 validasyonu için inşa edilmeli).
>
> **DÜZELTME (ikinci revizyon):** "Neural temsiller runtime'da lineer composite edilemez
> (day/night blend'i bozulur)" İDDİASI YANLIŞ — NIV (arXiv:2602.12949) **zaten time-varying
> irradiance field destekler** (yüksek boyutlu field, ek runtime maliyet olmadan). Gerçek limit:
> training/bake maliyeti + generalize (infinite-world ölçeğinde 1–5MB "medium scene" iddiası
> doğrulanmamış). Day/night'i yine de **analitik Hillaire sun + sky kanalında** tut (NIV yalnız
> static distant indirect). **Primary distant backend = DDGI / Godot-style SDFGI cascade**
> (voxel/SVDAG-native, no TensorCore, 60 FPS GTX 1060). NIV'i aynı `IrradianceBackend` trait'inde
> experimental (requires ML-capable GPU) tut. Aktivision "Neural Light Grid" (SIGGRAPH 2024) prod
> referansı.

**Adobe NIV (2024/2026)** tekniği — uzun vadeli optimizasyon:

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

### 1.13 Crate Organizasyonu (Aydınlatma)

```
crates/
  lighting/
    ├── mod.rs                  ← Lighting plugin entry point
    ├── light_data.rs           ← 16-bit packed light data (+ WLP düzeltmesi §1.2)
    ├── engine.rs               ← LightEngine (orchestrator)
    ├── direct/
    │   ├── mod.rs              ← Direct lighting
    │   ├── sun.rs              ← Directional sun light (Hillaire 2020 atmosfer sürülür)
    │   └── point.rs            ← Point/spot lights
    ├── block/
    │   ├── mod.rs              ← Block light
    │   ├── bfs_cpu.rs          ← CPU Dial 16-bucket BFS (VecDeque yerine)
    │   ├── bfs_simd.rs         ← One-channel-per-lane SIMD BFS
    │   ├── removal.rs          ← Two-phase removal (boundary-source + overlay, over-zero düzeltmesi)
    │   └── colored.rs          ← RGB channel propagation (eşzamanlı flood)
    ├── sky/
    │   ├── mod.rs              ← Sky light system
    │   ├── column_first.rs     ← Column-continuity propagation (overhang doğruluğu)
    │   ├── sky_sources.rs      ← Column-continuity sky source setup
    │   ├── jfa_occlusion.rs    ← Opsiyonel GPU JFA sky-occlusion field (büyük editler)
    │   └── day_night.rs        ← Day/night cycle (Hillaire 2020)
    ├── indirect/
    │   ├── mod.rs              ← Indirect GI system (trait: DDGI | LPV)
    │   ├── ddgi.rs             ← DDGI probe grid (SH L2, moment visibility) — L3 (varsayılan)
    │   ├── lpv.rs              ← LPV (SH radiance propagation) — DDGI fallback (SVDAG bağımsız)
    │   ├── restir_gi.rs        ← ReSTIR GI reservoir — L3/L4 sampler
    │   ├── svdag_trace.rs      ← LOD-anchored SVDAG march (Transform-Aware SVDAG 2025)
    │   ├── irradiance_cache.rs ← Per-voxel irradiance cache
    │   └── baker.rs            ← SVDAG path-traced offline/async baker (NIV eğitimi + validasyon)
    ├── culling/
    │   ├── mod.rs              ← Light culling system
    │   ├── hierarchical.rs     ← Hierarchical bitmask
    │   ├── morton.rs           ← Morton Z-order sorting
    │   └── priority.rs         ← Light update priority queue (budget'li runUpdates)
    ├── mesh_bake.rs            ← Light data → vertex color
    ├── tier.rs                 ← Tier-bazlı lighting stratejisi
    └── gpu/
        ├── mod.rs              ← GPU lighting pipelines
        ├── ddgi_update.rs      ← DDGI probe trace/update
        ├── restir.rs           ← ReSTIR GI passes
        ├── hi_z.rs             ← Hi-Z occlusion for lighting
        ├── temporal.rs         ← Voxel-keyed temporal accumulation (§1.8)
        └── neural_irradiance.rs← Neural Irradiance Volume (Faz 6, distant-only, trait arkası)
```

---

### 1.14 Performans Hedefleri (Aydınlatma)

| Metrik | Hedef | Not |
|---|---|---|
| Tek torch propagation (SIMD, one-lane/ch) | <100µs | Level-14, Dial bucket + pooled visited |
| Torch removal + re-propagate | <300µs | Two-phase + SIMD (over-zero düzeltmesi) |
| Sector skylight (açık arazi) | <0.5ms | Column-continuity + column-first |
| DDGI probe update (near) | <1ms/sector | SH L2 + moment visibility, rotating schedule |
| ReSTIR GI + SVDAG trace (far) | <2ms/frame | Reservoir + Transform-Aware SVDAG |
| Light culling (10K lights) | <0.5ms | Hierarchical bitmask + Morton |
| Light → mesh bake | <2ms/sector | Smooth lighting (4-vertex avg) |
| Temporal accumulation | <1ms/frame | Voxel-keyed reprojection + var-guide cleanup |
| Bellek (light data) | 16 bit/voxel | Sky 4-bit + RGB 4×4-bit |
| GPU irradiance cache | <1ms/frame | Temporal accumulation |
