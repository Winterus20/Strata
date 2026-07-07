# 10 — Render Pipeline

> **Olgunluk:** 🔒 Kesinleşti (`01-overview.md` §1.1, 2026-06-05). Anayasa `01`–`10`; `01`–`09` ile çelişirse önce anayasa güncellenir veya `10` revize edilir. `16`+ taslaklarla çelişirse **bu dosya** esas alınır.
> **Crate:** `render` (`02-implementation.md`)
> **Bağımlılıklar:** `06-xbrickmap.md` (32³ sektör, GPU feedback SSBO, branchless DDA, `PackedVertex`), `07-svdag.md` (SVDAG traversal, ghost page table, shallow LOD, `VisibilityBufferEntry`), `08-streaming.md` (4-tier mesafeler, GPU feedback öncelik), `09-meshing.md` (GigaBuffer + offset-allocator, PackedQuad, indirect draw, `NeedsRemesh`), `05-block-registry.md` (§18 Visibility LUT), `03-ecs-architecture.md` (Filter-First, sistem setleri)
> **Harici doğrulama (2026-06):** [Aokana I3D 2025](https://arxiv.org/abs/2505.02017) (shallow SVDAG, visibility buffer, tile-chunk pair, Hi-Z re-execution), [DOOM DtDA GPC 2025](https://graphicsprogrammingconference.com/2025/) (visibility buffer + deferred + VRCS), [Molenaar PG 2024](https://diglib.eg.org/handle/10.2312/pg20241310) (GPU edit, ghost page), [Laine & Karras 2010](https://dl.acm.org/doi/10.1145/1730804.1730814) (beam optimization)

---

## 1. Unified Visibility Buffer Render Pipeline

### 1.1 Genel Bakış

Strata'nın render pipeline'ı **GPU compute-driven**, **unified visibility buffer** yaklaşımına dayanır. Tüm tier'lar aynı 64-bit visibility buffer'a yazar; tek bir shading pass'inde tüm pikseller shade edilir. Bu yaklaşım:

- **Aokana** (I3D 2025): shallow SVDAG + tile-chunk pair + Hi-Z occlusion
- **DOOM: The Dark Ages** (GPC 2025): visibility buffer + deferred shading + VRCS
- **GigaVoxels DP** (HPG 2024): starvation-free ghost page geçişi

tekniklerini birleştirir.

### 1.2 Pass Sıralaması

```
┌──────────────────────────────────────────────────────────────────┐
│                      RENDER FRAME                                 │
├──────────────────────────────────────────────────────────────────┤
│ Pass 0: Depth Pre-Pass (Rasterize — dinamik mesh entity'ler: mob, player, item)│
│   → Entity depth buffer doldur (voxel mesh'leri GigaBuffer'dan gelir, burada değil)│
│   → Entity'ler için Hi-Z build (Pass 7 ile paylaşılır)           │
├──────────────────────────────────────────────────────────────────┤
│ Pass 1: Tile Selection + Hi-Z Occlusion (GPU Compute)            │
│   → Ekran 8×8 tile'lara bölünür                                  │
│   → Her tile'dan screen ray projekte → hangi sektörler katkıda?  │
│   → Önceki frame Hi-Z ile gizli tile'lar elenir                  │
│   → Çıktı: görünür Tile–Chunk pair listesi (indirect dispatch)   │
├──────────────────────────────────────────────────────────────────┤
│ Pass 2: Tier 1 — XBrickMap Ray Trace (GPU Compute)               │
│   → Beam optimization: Hi-Z'den başlangıç tahmini                │
│   → 4-level bitmask space skipping (branchless DDA, `06` §1.5)   │
│   → Ghost page check (WARM dual representation, `07` §1.5)       │
│   → 64-bit visibility buffer'a atomicMax write                   │
├──────────────────────────────────────────────────────────────────┤
│ Pass 3: Tier 2–3 — SVDAG Ray March (GPU Compute, indirect)       │
│   → Indirect dispatch: sadece görünür Tile–Chunk pairs           │
│   → Shallow SVDAG traversal (max depth 5, `07` §1.6)            │
│   → LOD blending (Aokana density=2 aggregate)                    │
│   → Aynı visibility buffer'a atomicMax (depth test otomatik)     │
├──────────────────────────────────────────────────────────────────┤
│ Pass 4: Hi-Z Re-Execution (GPU Compute)                          │
│   → Mevcut frame Hi-Z build (Pass 2–3 depth'inden)              │
│   → Pass 1'de culled tile'ları yeni Hi-Z ile tekrar test et     │
│   → Hatalı culling düzelt → eksik tile'ları yeniden ray march   │
├──────────────────────────────────────────────────────────────────┤
│ Pass 5: VRCS Color Resolve (GPU Compute)                         │
│   → Visibility buffer'dan piksel bilgisi çıkar                   │
│   → Fovea: tam çözünürlük (1 ray/pixel)                          │
│   → Mid: 2×2 tile = 1 shade + interpolate                        │
│   → Periferik: 4×4 tile = 1 shade + interpolate                  │
│   → SVDAG hit: DFS order + binary search ile renk bul            │
│   → G-buffer: albedo + normal + emissive + AO + light            │
├──────────────────────────────────────────────────────────────────┤
│ Pass 6: Mesh Entity Composite (Rasterize + Blend)                │
│   → Mob, player, item mesh'leri G-buffer üzerine composite       │
│   → Opaque entity'ler: depth test ile                            │
│   → Transparent entity'ler: depth write OFF, back-to-front sort  │
│   → Voxel transparent (su, cam): ayrı batch, sorted              │
├──────────────────────────────────────────────────────────────────┤
│ Pass 7: Deferred Lighting + HDR (GPU Compute)                    │
│   → Voxel GI (`13-lighting.md` ile entegre)                      │
│   → Tone mapping (ACES), bloom, exposure control                 │
│   → Final frame buffer                                           │
├──────────────────────────────────────────────────────────────────┤
│ Pass 8: Hi-Z Build (GPU Compute)                                 │
│   → Mevcut frame final depth → mipmap piramidi                   │
│   → Sonraki frame Pass 1 occlusion culling için                  │
└──────────────────────────────────────────────────────────────────┘
```

**Tradeoff — Neden 9 pass?**

| Karar | Alternatif | Tercih | Gerekçe |
|-------|-----------|--------|---------|
| Re-execution pass | Re-execution yok | **Re-execution var** | Hızlı kamera hareketinde yanlış culling → ghosting; maliyet ~0.2ms |
| Beam optimization | Rays from camera | **Hi-Z guided start** | Açık alanlarda %30-40 traversal hızı; Hi-Z zaten mevcut, ek maliyet ~0 |
| VRCS color resolve | Uniform shade | **Variable rate** | ~1-2ms GPU tasarrufu (DOOM DtDA kanıtlı); kalite kaybı periferikte algılanmaz |
| Entity composite ayrı pass | Entity'ler voxel ile aynı pass'ta | **Ayrı pass** | Mesh entity'ler farklı pipeline (rasterize); Aokana §5 mesh-voxel entegrasyonu |
| Depth pre-pass | Depth pre-pass yok | **Pre-pass var** | Entity Hi-Z'si olmadan entity occlusion culling imkânsız |

---

## 2. Visibility Buffer Layout (64-bit)

> **Anayasa uyumu:** Bu layout `07-svdag.md` §1.7 ile **birebir aynıdır**. Aokana (I3D 2025) Figure 7 canonical layout'u kullanılır.

### 2.1 Bit Dağılımı

| Bit Aralığı | İçerik | Açıklama |
|---|---|---|
| 0–23 (24 bit) | Voxel Pos | Voxel koordinatı (sector-içi, 8 bit × 3 eksen) |
| 24–36 (13 bit) | Sector ID | Hangi sector'den geldiği (max 8192 görünür sektör) |
| 37–39 (3 bit) | Normal | Axis-aligned normal: 0=X+, 1=X-, 2=Y+, 3=Y-, 4=Z+, 5=Z- |
| 40–63 (24 bit) | Depth | Reversed-Z: `0` = en uzak, `0xFFFFFF` = en yakın |

> **NOT:** Aokana Figure 7 ile birebir — depth **en yüksek** bitlerde. Bu sayede `atomicMax` ile en yakın piksel otomatik kazanır (reversed-Z: yakın = büyük değer, depth yüksek bitlerde → büyük u64 → atomicMax seçer).

> **13-bit sector_id gerekçesi (8192 görünür sektör limiti):** Aokana canonical layout'unun 13-bit sector_id alanı, tek frame'de ekranda **görünür** sektör sayısını sınırlar (max 8192). Strata'da 32³ sektör (32m kenar) ile: ACTIVE <96m → ~343 sektör, WARM 96–384m → ~15625 yüklü sektör (frustum + Hi-Z sonrası ~2000-4000 görünür), DISTANT 384–1536m → SVDAG ile özetlenmiş. Toplam görünür sektör frustum + Hi-Z culling sonrası pratikte <4000 kalır; 8192 limiti güvenli marjla karşılar. Tile selection pass (`§3.1`) `sector_count`'u 8192 ile clamplemeli — aşım durumunda en yakın sektörler önceliklendirilir (`08` §4 GPU feedback öncelik ile uyumlu).

```rust
/// 64-bit visibility buffer entry — Aokana (I3D 2025) Figure 7 layout.
/// `07` §1.7 ile birebir aynı.
/// atomicMax ile depth test: depth yüksek bitlerde olduğundan
/// daha yakın (daha büyük reversed-z) piksel otomatik kazanır.
pub struct VisibilityBufferEntry(pub u64);

impl VisibilityBufferEntry {
    /// Encode: voxel_pos (24-bit) + sector_id (13-bit) + normal (3-bit) + depth (24-bit, reversed-Z)
    ///
    /// **`depth` parametresi standard Z'dir** (0 = near plane, 1 = far plane).
    /// Fonksiyon dahili olarak `(1.0 - depth)` ile reversed-Z'ye çevirir:
    /// stored = 0xFFFFFF → en yakın (near), stored = 0x0 → en uzak (far).
    /// Bu sayede `atomicMax` otomatik olarak en yakın pikseli seçer.
    ///
    /// **DİKKAT:** `depth` zaten reversed-Z ise (1=near, 0=far), `(1.0 - depth)` tekrar
    /// tersine çevirir → yanlış sonuç. Her zaman standard Z (0=near, 1=far) verin.
    /// GPU ray trace sonucu `t / t_max` → standard Z; Hi-Z texture ise reversed-Z'dir.
    pub fn encode(depth: f32, normal: u8, sector_id: u16, voxel_pos: u32) -> u64 {
        let d = ((1.0 - depth) * 16_777_215.0) as u64; // standard Z → reversed-Z
        (voxel_pos as u64 & 0xFF_FFFF)                   // bit[0:23]
          | ((sector_id as u64 & 0x1FFF) << 24)          // bit[24:36]
          | ((normal as u64 & 0x7) << 37)                // bit[37:39]
          | (d << 40)                                    // bit[40:63]
    }

    /// Decode: visibility buffer'dan **standard Z** (0=near, 1=far) derinliğini çıkar.
    /// Reversed-Z storage'dan tekrar standard Z'ye çevrilir.
    pub fn decode_depth(entry: u64) -> f32 {
        1.0 - ((entry >> 40) as f32 / 16_777_215.0)
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

### 2.2 WGSL Atomic Write Stratejisi

64-bit atomic `atomicMax` native desteği wgpu'da `Features::SHADER_INT64_ATOMIC_MIN_MAX` ile kontrol edilir:

| Platform | Feature | Durum |
|----------|---------|-------|
| Vulkan | `VK_KHR_shader_atomic_int64` | RTX 20+, RDNA 2+ geniş destek |
| DX12 | SM 6.6+ | Windows modern GPU'larda mevcut |
| Metal | MSL 2.4+ | Apple Silicon tam destek |
| WebGPU | Proposal aşaması | Native fallback gerekir |

```rust
// wgpu feature kontrolü
let use_native_u64 = device.features()
    .contains(wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX);
```

**Native u64 path (öncelikli):**
```wgsl
@group(0) @binding(0)
var<storage, read_write> visibility_buffer: array<atomic<u64>>;

fn visibility_write(pixel_idx: u32, entry: u64) {
    // atomicMax: depth yüksek bitlerde → en yakın piksel otomatik kazanır
    // (reversed-Z: yakın = büyük değer, depth [40:63] → büyük u64 → atomicMax seçer)
    atomicMax(&visibility_buffer[pixel_idx], entry);
}
```

**Fallback path (native u64 yoksa):**
```wgsl
// İki 32-bit atomic ile emüle — Aokana layout: depth yüksek bitlerde
@group(0) @binding(0)
var<storage, read_write> vis_hi: array<atomic<u32>>; // bit[32:63] — depth burada
@group(0) @binding(1)
var<storage, read_write> vis_lo: array<atomic<u32>>; // bit[0:31]

fn visibility_write_emulated(pixel_idx: u32, entry: u64) {
    let lo = u32(entry & 0xFFFFFFFFu);
    let hi = u32(entry >> 32u);

    // Depth yüksek word'de (hi) → atomicMax ile en yakın piksel kazanılır
    let prev_hi = atomicMax(&vis_hi[pixel_idx], hi);

    // Hi kazanıldıysa (önceki değer daha küçüktü → daha uzaktı) lo'yu da güncelle
    if (prev_hi < hi) {
        atomicStore(&vis_lo[pixel_idx], lo);
    }
}
```

**Tradeoff:** Native u64 = 1 atomic op; emulated = 2 atomic op + branch. RTX 3060+ için native path ~%40 daha hızlı. Fallback yalnızca eski/düşük donanımda devreye girer. Her iki path'te de depth **yüksek bitlerde** olduğu için `atomicMax` kullanılır (Aokana Figure 7 ile tutarlı).

**⚠️ Emulated u64 Race Condition (Bilinen Tradeoff):**

Fallback path'te `atomicMax(vis_hi)` ve `atomicStore(vis_lo)` arasındaki pencerede race condition vardır:

```
  Thread A: atomicMax(vis_hi, hi_A) → prev_hi < hi_A → koşul doğru
  Thread B: atomicMax(vis_hi, hi_B) → hi_B > hi_A → prev_hi = hi_A → koşul doğru
  Thread B: atomicStore(vis_lo, lo_B) → doğru veri yazıldı ✓
  Thread A: atomicStore(vis_lo, lo_A) → lo_B'yi lo_A ile OVERWRITE ✗
```

**Sonuç:** `vis_hi` doğru (hi_B, en yakın), ama `vis_lo` yanlış (lo_A yerine lo_B olmalı). `lo` word: voxel_pos (24-bit) + sector_id alt 8-bit. Görsel etki: bir piksel yanlış sektör/voxel rengi gösterebilir — bir frame, bir piksel, nadiren.

**Neden kabul edilebilir:**
1. Modern GPU'larda (RTX 20+, RDNA 2+, Apple Silicon) native u64 atomic mevcut → fallback hiç devreye girmez.
2. Sadece WebGPU/legacy GPU'larda tetiklenir; aynı piksele aynı clock'ta iki thread'in yazma olasılığı düşük.
3. Depth test (`hi` word) her zaman doğru → z-fighting/ghosting yok; sadece renk hatası bir frame.
4. Alternatif CAS-döngüsü WGSL'de `atomicCompareExchangeWeak` ile mümkün ama 2× daha fazla atomic op → fallback tam amaçladığı düşük donanımı daha da yavaşlatır.

**Gelecek:** WebGPU `atomic<u64>` standardize olduğunda fallback tamamen kaldırılır.

---

## 3. Tile Selection + Hi-Z Occlusion Culling

> **Kaynak:** Aokana (I3D 2025) §3.5 — GPU-Driven Voxel Rendering Pipeline

### 3.1 Tile Selection Pass

Ekran 8×8 pixel tile'lara bölünür. Her tile için bir screen-space ray projekte edilir ve ray'in geçtiği sektörler belirlenir. Çıktı: **Tile–Chunk pair listesi**.

```wgsl
struct TileInfo {
    tile_x: u32,
    tile_y: u32,
    sector_id: u32,
    visible: u32,       // 0 = culled, 1 = visible
};

@group(0) @binding(0)
var<storage, read> sector_aabbs: array<SectorAabb>;
@group(0) @binding(1)
var<storage, read_write> tile_chunk_pairs: array<TileInfo>;
@group(0) @binding(2)
var hiz_texture: texture_2d<f32>;
@group(0) @binding(3)
var hiz_sampler: sampler;
@group(0) @binding(4)
var<uniform> params: CullingParams;

struct CullingParams {
    view_proj: mat4x4<f32>,
    screen_size: vec2<u32>,
    sector_count: u32,  // max 8192 (13-bit sector_id limiti, §2.1)
    tile_size: u32,     // 8
    hiz_valid: u32,     // 0 = ilk frame (Hi-Z yok)
};

@compute @workgroup_size(8, 8, 1)
fn tile_selection(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;
    let screen_w = params.screen_size.x / params.tile_size;
    let screen_h = params.screen_size.y / params.tile_size;

    if (tile_x >= screen_w || tile_y >= screen_h) { return; }

    // Tile merkezinden screen ray
    let tile_center = vec2<f32>(
        f32(tile_x * params.tile_size + params.tile_size / 2u),
        f32(tile_y * params.tile_size + params.tile_size / 2u),
    );
    let ray_dir = screen_to_world_ray(tile_center, params.view_proj);

    // Her sektör için frustum + Hi-Z testi
    for (var s = 0u; s < params.sector_count; s++) {
        let aabb = sector_aabbs[s];

        // 1. Frustum testi (AABB vs 6 plane — branchless select)
        let frustum_visible = frustum_test_aabb(aabb, params.view_proj);
        if (!frustum_visible) { continue; }

        // 2. Hi-Z occlusion testi
        var occluded = false;
        if (params.hiz_valid == 1u) {
            let screen_rect = project_aabb_to_screen(aabb, params.view_proj);
            let hiz_depth = sample_hiz(screen_rect, hiz_texture, hiz_sampler);
            // Reversed-Z: max depth = en yakın. AABB'nin en yakın noktası
            // (screen_rect.max_depth) yüzeyden uzaktaysa (daha küçük reversed-Z) → occluded
            occluded = screen_rect.max_depth < hiz_depth;
        }

        if (!occluded) {
            // sector_id = s, 13-bit: max 8192 (§2.1)
            // CPU tarafı sector_count'u min(visible_sectors, 8192) ile clamplemeli
            let pair_idx = atomicAdd(&tile_chunk_count, 1u);
            tile_chunk_pairs[pair_idx] = TileInfo(tile_x, tile_y, s, 1u);
        }
    }
}
```

### 3.2 Hi-Z Re-Execution Pass

Aokana'nın en önemli katkılarından biri: **önceki frame'in Hi-Z'i ile culled edilen tile'ları, mevcut frame'in Hi-Z'i ile tekrar test eder.** Hızlı kamera hareketinde yanlış culling'i düzeltir.

```
Re-execution mantığı:
  Frame N-1: Hi-Z eski → bazı tile'lar yanlışlıkla culled (hızlı hareket)
  Frame N:   Yeni Hi-Z (Pass 2-3 depth'inden) → culled tile'ları tekrar test
             → Hatalı culling düzeltilir → eksik tile'lar yeniden ray march
```

```wgsl
@compute @workgroup_size(8, 8, 1)
fn hiz_reexecution(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_x = gid.x;
    let tile_y = gid.y;

    // Bu tile Pass 1'de culled miydi?
    let was_culled = tile_was_culled_in_pass1(tile_x, tile_y);
    if (!was_culled) { return; } // Zaten render edildi, tekrar işleme

    // Mevcut frame Hi-Z ile tekrar test
    let screen_rect = tile_screen_rect(tile_x, tile_y);
    let new_hiz_depth = sample_hiz_current_frame(screen_rect);

    // Reversed-Z: max_depth = en yakın nokta.
    // Tile'ın en yakın noktası yüzeyin önündeyse (daha büyük reversed-Z) → visible → re-execute
    // Bu, occlusion test'in (§3.1, §9.2) tersidir: occluded = max_depth < hiz_depth
    if (screen_rect.max_depth >= new_hiz_depth) {
        // Yanlışlıkla culled! Yeniden ray march
        reexecute_tile_rays(tile_x, tile_y);
    }
}
```

**Tradeoff:**
| Metrik | Re-execution var | Re-execution yok |
|--------|-----------------|------------------|
| Ek GPU maliyeti | ~0.2ms | 0 |
| Hızlı hareket ghosting | **Yok** | Belirgin |
| Frame consistency | Yüksek | Düşük |

Sonuç: **Pozitif tradeoff** — 0.2ms maliyetle ghosting tamamen eliminasyon.

---

## 4. Tier 1: XBrickMap Ray Trace

### 4.1 Beam Optimization (Laine & Karras 2010)

Hi-Z buffer'dan düşük çözünürlüklü depth image kullanarak ray başlangıç noktasını tahmin et. Boş alanı atla.

```wgsl
fn ray_trace_xbrickmap(pixel: vec2<u32>, hiz_depth: f32) -> HitInfo {
    let ray = camera_get_ray(pixel);

    // Beam optimization: Hi-Z'den başlangıç tahmini
    // hiz_depth = önceki frame'in bu pikselinde en yakın yüzeyin reversed-Z derinliği
    // Ray trace t_deger → standard Z (0=near, 1=far); hiz_depth reversed-Z (1=near, 0=far)
    // Dönüşüm: t_start_standard = 1.0 - hiz_depth
    var t_start: f32;
    let hiz_standard_z = 1.0 - hiz_depth; // reversed-Z → standard Z
    if (hiz_standard_z > 0.0 && hiz_standard_z < 1.0) {
        // Önceki yüzeye %80 mesafeden başla (güvenlik marjı)
        t_start = hiz_standard_z * 0.8;
    } else {
        t_start = 0.0;
    }

    // Branchless DDA traversal (`06` §1.5 ile aynı)
    return traverse_xbrickmap(ray, t_start);
}
```

**Tradeoff:**
- Ek maliyet: ~0 (Hi-Z zaten mevcut)
- Kazanç: Açık alanlarda %30-40 traversal hızı (ray boş sektörleri atlar)
- Risk: Hızlı kamera hareketinde tahmin yanlış → conservative fallback (t_start = 0)
- **Karar: Pozitif — ücretsiz performans**

### 4.2 Ghost Page Entegrasyonu (WARM Tier)

WARM tier'da XBrickMap + SVDAG dual representation var (`08` §3.3). Ghost page state kontrolü branchless `select` ile yapılır:

```wgsl
fn trace_tier1_with_ghost(ray: Ray, sector_id: u32) -> HitInfo {
    let page_state = atomicLoad(&ghost_pages[sector_id / PAGES_PER_SECTOR]);

    // page_state: 0 = Ghost (SVDAG yok), 1 = Loading, 2 = Ready
    // Ghost/Loading durumunda XBrickMap'ten render (fallback yok)
    // Ready durumunda XBrickMap hâlâ render edilir (dual representation)
    // SVDAG Pass 3'te ayrıca trace edilir → visibility buffer atomicMax ile depth test

    return traverse_xbrickmap(ray, 0.0);
}
```

### 4.3 GPU Feedback SSBO (`06` Entegrasyonu)

`06-xbrickmap.md` §2.2 tanımladığı GPU feedback mekanizması: GPU, ray trace sırasında hangi sektörlere eriştiğini bir SSBO'ya atomic ile kaydeder. CPU bu listeyi okuyarak **sadece erişilen sektörleri** bellekte tutar.

```wgsl
// XBrickMap traversal sırasında — her sektör erişiminde
fn load_sector_with_feedback(coord: vec3<i32>) -> SectorData {
    let sector_idx = sector_hash_lookup(coord);

    // GPU feedback: bu sektöre erişildiğini kaydet
    atomicOr(&gpu_feedback_ssbo[sector_idx / 32u], 1u << (sector_idx % 32u));

    return sector_data[sector_idx];
}
```

CPU tarafı (`08` StreamingManager ile entegre):
```rust
/// GPU feedback SSBO'dan görünen sektörleri oku.
/// Sadece bu sektörler stream-in kuyruğuna eklenir (`08` §4).
pub fn process_gpu_feedback(
    feedback: &[u32],
    streaming: &mut StreamingManager,
) {
    for (word_idx, &word) in feedback.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            let bit = bits.trailing_zeros();
            bits &= bits - 1;
            let sector_idx = word_idx * 32 + bit as usize;
            streaming.prioritize_sector(sector_idx, GpuFeedbackPriority);
        }
    }
}
```

---

## 5. Tier 2–3: SVDAG Ray March

### 5.1 Indirect Dispatch (Tile–Chunk Pairs)

Pass 1'den gelen Tile–Chunk pair listesi kullanılarak **sadece görünür sektörler** ray march edilir. `09`'un multi-draw indirect prensibi ile aynı felsefe:

```wgsl
// Pass 3 — SVDAG Ray March (indirect dispatch)
// Dispatch count = Pass 1'den gelen görünür tile-chunk pair sayısı
@compute @workgroup_size(8, 8, 1)
fn svdag_ray_march(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pair_idx = gid.x;  // Tile-chunk pair index
    let pair = tile_chunk_pairs[pair_idx];

    if (pair.visible == 0u) { return; }

    let tile = vec2<u32>(pair.tile_x, pair.tile_y);
    let sector = pair.sector_id;

    // Pixel ray → SVDAG traversal
    let ray = tile_to_ray(tile);
    let svdag_root = sector_svdag_roots[sector];

    // Shallow SVDAG traversal (max depth 5, `07` §1.6)
    let hit = svdag_traverse(svdag_root, ray, SHALLOW_MAX_DEPTH);

    if (hit.found) {
        // hit.depth = ray t / t_max → standard Z (0=near, 1=far)
        // encode() dahili olarak (1.0 - depth) ile reversed-Z'ye çevirir (§2.1)
        let entry = VisibilityBufferEntry::encode(
            hit.depth, hit.normal, sector as u16, hit.voxel_pos
        );
        atomicMax(&visibility_buffer[tile_to_pixel(tile)], entry);
    }
}
```

> **Aokana Chunk Boyutu Adaptasyonu:** Aokana (I3D 2025) 256³ chunk'lar (8³ = 512 sektör eşdeğeri) kullanır; Strata 32³ sektörler ile çalışır (`07` §1.6). Farklar:
> 1. **Shallow SVDAG derinliği:** Aokana density=2 aggregate → ~5 seviye; Strata 32³ sektör başına max depth 5 (aynı), ancak daha fazla bağımsız SVDAG root → daha fazla `sector_svdag_roots` lookup.
> 2. **Tile–Chunk mapping:** Aokana'da 1 chunk = 256³; Strata'da 1 sektör = 32³. Tile–chunk pair listesi daha fazla entry içerir (8³ katı), ama her entry daha küçük SVDAG traverse eder → toplam traversal benzer.
> 3. **LOD aggregate:** Aokana 256³ → LOD-1 = 128³; Strata 32³ → LOD-1 = komşu 8 sektörün birleşimi (256³ equiv). Strata'nın LOD aggregate'i komşu sektörlerin SVDAG'larını birleştirerek Aokana'nın tek chunk LOD'uyla aynı sonucu verir (`07` §1.6 LOD blending).

### 5.2 LOD Blending (Tier Geçişlerinde)

Tier 2 (WARM) ve Tier 3 (DISTANT) arasında LOD geçişi:

```wgsl
fn svdag_traverse_with_lod(root: u32, ray: Ray, dist: f32) -> HitInfo {
    let base_lod = select_lod(dist);  // 0 = LOD-0 (32³), 1+ = aggregate

    // Sınır bölgesinde (tier eşiği ±16m, `08` §2 hysteresis)
    let blend = compute_tier_blend(dist);  // 0.0 = yakın tier, 1.0 = uzak tier

    if (blend > 0.0 && blend < 1.0) {
        // Geçiş bölgesi: her iki LOD'dan trace, depth-aware blend
        let hit_near = svdag_traverse_lod(root, ray, base_lod);
        let hit_far = svdag_traverse_lod(root, ray, base_lod + 1u);
        return blend_hits(hit_near, hit_far, blend);
    }

    return svdag_traverse_lod(root, ray, base_lod);
}
```

---

## 6. VRCS Color Resolve (Variable Rate Compute Shaders)

> **Kaynak:** DOOM: The Dark Ages (GPC 2025, id Software + Xbox ATG)
> **Kavram:** Compute shader'da hardware VRS kullanılamadığı için yazılımsal olarak fovea/periferik çözünürlük ayrımı.

### 6.1 Foveated Bölgeler

```rust
pub struct VrcsConfig {
    /// Fovea merkezi (genelde ekran ortası veya eye-tracker)
    pub fovea_center: Vec2,
    /// Fovea yarıçapı (normalize 0-1)
    pub fovea_radius: f32,         // default 0.15
    /// Orta bölge yarıçapı
    pub mid_radius: f32,           // default 0.35
    /// Fovea: her piksel için 1 thread
    /// Mid: 2×2 tile = 1 shade + interpolate
    /// Periferik: 4×4 tile = 1 shade + interpolate
    pub mid_tile: u32,             // 2
    pub peripheral_tile: u32,      // 4
}
```

| Bölge | Çözünürlük | Thread/Pixel | Ray Oranı | Kalite |
|-------|-----------|-------------|-----------|--------|
| **Fovea** | 1.0× | 1:1 | 1.0× | Tam |
| **Mid** | 0.5× | 1:4 | 0.25× | Yüksek |
| **Periferik** | 0.25× | 1:16 | 0.0625× | Kabul edilebilir |

### 6.2 Color Resolve Shader

```wgsl
@compute @workgroup_size(8, 8, 1)
fn vrcs_color_resolve(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.xy;
    let screen_size = vec2<f32>(params.screen_size);
    let pixel_norm = vec2<f32>(pixel) / screen_size;
    let dist_to_fovea = distance(pixel_norm, params.fovea_center);

    // VRCS rate seçimi
    var tile_size: u32;
    if (dist_to_fovea < params.fovea_radius) {
        tile_size = 1u;  // Fovea: tam çözünürlük
    } else if (dist_to_fovea < params.mid_radius) {
        tile_size = 2u;  // Mid: 2×2 tile
    } else {
        tile_size = 4u;  // Periferik: 4×4 tile
    }

    // Tile leader mı? (sadece tile'ın sol-üst pikseli shade yapar)
    let tile_origin = (pixel / tile_size) * tile_size;
    let is_leader = all(pixel == tile_origin);

    if (!is_leader) { return; }  // Early-exit: sadece leader shade yapar

    // Shade: visibility buffer'dan bilgi çıkar
    let entry = visibility_buffer[pixel.x + pixel.y * params.screen_size.x];
    let color = shade_visibility(entry);

    // Tile içindeki tüm piksellere aynı rengi yaz
    for (var dy = 0u; dy < tile_size; dy++) {
        for (var dx = 0u; dx < tile_size; dx++) {
            let target = tile_origin + vec2<u32>(dx, dy);
            if (all(target < params.screen_size)) {
                textureStore(output_color, target, color);
            }
        }
    }
}
```

### 6.3 SVDAG Color Lookup (DFS Order + Binary Search)

Aokana'nın color resolve stratejisi: SVDAG yaprağının rengini bulmak için **DFS sıra numarası** kullanılır.

```wgsl
fn resolve_svdag_color(sector_id: u32, voxel_pos: vec3<u32>) -> vec4<f32> {
    let root = sector_svdag_roots[sector_id];

    // Voxel pozisyonundan DFS order hesapla (traversal ile)
    let dfs_index = compute_dfs_order(root, voxel_pos);

    // Color array'de binary search (color data ayrı sıkıştırılmış array)
    let color_block = binary_search_color(color_arrays[sector_id], dfs_index);

    return decode_color(color_block);
}
```

**Performans (DOOM DtDA referansı):**

| Metrik | Uniform Shade | VRCS | Kazanç |
|--------|-------------|------|--------|
| Shade thread sayısı | 1.0× | **0.2-0.4×** | **-60-80%** |
| Frame time (color resolve) | ~1.5ms | **~0.5ms** | **-67%** |
| Toplam GPU savings | — | **~1-2ms** | Kritik (60Hz budget) |

**Tradeoff:** Periferik kalite kaybı insan gözünün peripheral vision sınırları içinde algılanmaz (DOOM DtDA kanıtlı). **Pozitif tradeoff.**

---

## 7. Seam/Crack Yönetimi (LOD Geçişleri)

**Problem:** Farklı tier/LOD seviyesindeki sektörler arasında geçişte çatlak (crack) ve z-fighting oluşur. Tier 1 (XBrickMap ray trace) ile Tier 2 (XBrickMap + SVDAG) arasındaki sınırda, ayrıca Tier 2 ile Tier 3 (SVDAG LOD+) arasında boşluklar ve örtüşmeler kaçınılmazdır.

### 7.1 Crack Türleri

| Crack Türü | Oluştuğu Yer | Görsel Etki | Şiddet |
|---|---|---|---|
| **Tier border crack** | Tier 1 ↔ Tier 2, Tier 2 ↔ Tier 3 sınırları | Boşluk / z-fighting | Yüksek |
| **LOD pop** | SVDAG LOD seviyesi değişimi | Ani geometri değişimi | Orta |
| **XBrickMap/SVDAG örtüşmesi** | WARM tier'da dual representation | Double hit, ghosting | Yüksek |
| **Mip discontinuity** | Farklı mip seviyesindeki komşu sektörler | Renk/yoğunluk farkı | Düşük |

### 7.2 Tier Border Crack Kapatma

**Strateji:** Her sektörün **border region**'ında (dış 1 voxel) komşu sektörün verisini **oku ama yazma**. Geçiş bölgesinde her iki tier da aynı veriyi görür.

```
┌─────────────┐   ┌─────────────┐
│ Sector A    │   │ Sector B    │
│ (Tier 1:    │   │ (Tier 2:    │
│  Ray Trace) │   │  SVDAG)     │
│  ┌───────┐  │   │  ┌───────┐  │
│  │ Inside│  │   │  │ Inside│  │
│  │ 30³   │  │   │  │ 30³   │  │
│  └───────┘  │   │  └───────┘  │
│  ← border →│   │  ← border →│  ← 1 voxel overlap
└─────────────┘   └─────────────┘
```

```wgsl
@compute @workgroup_size(8, 8)
fn border_aware_traversal(@builtin(global_invocation_id) id: vec3<u32>) {
    let ray = camera_get_ray(id.xy);
    let sector_coord = current_sector(ray);
    let local = ray.origin - vec3<f32>(sector_coord * 32u);

    // Border zone: dış 1 voxel
    let in_border = any(local < vec3<f32>(1.0)) || any(local > vec3<f32>(31.0));

    if (in_border && sector_has_neighbor(sector_coord, ray.direction)) {
        let neighbor_tier = get_neighbor_tier(sector_coord, ray.direction);

        // Komşu farklı tier'daysa, komşunun verisini oku
        if (neighbor_tier != current_tier) {
            // Komşu sektörün tier'ı ile traversal yap (seam-free)
            return traverse_at_tier(ray, sector_coord + neighbor_offset, neighbor_tier);
        }
    }

    return traverse_at_tier(ray, sector_coord, current_tier);
}
```

### 7.3 Geometri Fade (LOD Pop Azaltma)

```rust
pub struct TierBlendManager {
    /// Bevy Component olarak ECS üzerinde tutulur (DOD uyumlu, `03` Filter-First).
    /// Her sector entity'de `BlendFactor` component'ı var.
    /// Bu manager sadece update sistemini sağlar.
    pub fade_duration: u32, // ~10-15 frame = ~200ms
}

/// Bevy Component — her sector entity'ye attach edilir.
#[derive(Component)]
pub struct BlendFactor(pub f32); // 0.0 = yakın tier, 1.0 = uzak tier

impl TierBlendManager {
    pub fn update_system(
        player_pos: Vec3,
        mut query: Query<(&SectorCoord, &Tier, &mut BlendFactor)>,
    ) {
        let inv_duration = 1.0 / self.fade_duration as f32;
        for (coord, target_tier, mut blend) in &mut query {
            let target = match target_tier {
                Tier::Active => 0.0,
                Tier::Warm => 0.5,
                Tier::Distant => 1.0,
                Tier::Archive => 1.0,
            };
            // Smooth lerp — ani geçiş yok (`08` §2 hysteresis ile uyumlu)
            blend.0 += (target - blend.0) * inv_duration;
        }
    }
}
```

### 7.4 XBrickMap/SVDAG Dual Representation (WARM Tier)

WARM tier'da hem XBrickMap hem SVDAG mevcut. Visibility buffer'da atomicMax ile depth test otomatik olarak **daha yakın olanı** seçer. Ghost page ile starvation-free geçiş (`07` §1.5):

```wgsl
// WARM tier: XBrickMap trace (Pass 2) + SVDAG trace (Pass 3)
// Her ikisi de aynı visibility buffer'a yazar
// atomicMax → depth test → en yakın piksel otomatik kazanır (depth yüksek bitlerde)
// Ghost page loading sırasında XBrickMap hâlâ render → starvation yok
```

### 7.5 Mip Discontinuity — Geçiş Düzeltme

```wgsl
fn sample_mip_safe(pos: vec3<f32>, sector_coord: vec3<i32>, dist: f32) -> vec4<f32> {
    let base_lod = compute_lod(dist);

    // Sector sınırına uzaklık (0-1 arası)
    let local = pos - vec3<f32>(sector_coord * 32);
    let border_dist = min(min(local.x, 32.0 - local.x),
                        min(min(local.y, 32.0 - local.y),
                            min(local.z, 32.0 - local.z)));

    // Sınıra yaklaştıkça LOD'u düşürt (komşuyla uyum)
    let border_bias = select(0.0, 1.0, border_dist < 2.0);
    let lod = min(base_lod - border_bias, 5.0);

    return textureSampleLevel(sector_mip_tex, mip_sampler, pos / 32.0, lod);
}
```

### 7.6 Test ve Validasyon

| Test | Yöntem | Kabul Kriteri |
|------|--------|---------------|
| **Tier border crack** | Düz zeminde 2 tier arası kamera geçişi | Hiçbir frame'de boşluk/çizgi yok |
| **LOD pop** | Dağlık arazide hızlı uzaklaşma | Anlık geometri değişimi yok (fade smooth) |
| **XBrickMap/SVDAG örtüşmesi** | İnce nesnede (çit) tier geçişi | Çift görüntü, ghosting yok |
| **Mip discontinuity** | İki farklı mip seviyesinde sector sınırı | Renk farkı < %2 |

---

## 8. Transparent & Cutout Rendering

### 8.1 Voxel Transparent (Su, Cam)

`09-meshing.md` §11b tanımladığı `NonGreedyMesher` çıktısı ayrı render edilir:

```
Transparent render sırası:
1. Opaque pass (XBrickMap RT + SVDAG RM + mesh entity opaque)
2. Transparent voxel pass:
   - Depth write OFF
   - Back-to-front sort (sector distance)
   - Alpha blend
3. Transparent mesh entity pass:
   - Aynı depth write OFF + sort
```

### 8.2 Cutout (Alpha Test — Yaprak, Çit)

Cutout bloklar opaque pass'ta `alpha > threshold` testi ile render edilir. Depth write **açık** kalır.

```wgsl
// Cutout fragment shader
fn fs_cutout(in: FragmentInput) -> vec4<f32> {
    let tex_color = textureSample(albedo_tex, samp, in.uv);
    if (tex_color.a < 0.5) { discard; }
    return tex_color;
}
```

---

## 9. Frustum Culling & Culling Pipeline

### 9.1 Frustum (CPU-Side Build)

View-projection matrix'ten 6 plane frustum. **Sadece frustum build** CPU'da yapılır (~0.05ms). Test GPU'da.

```rust
/// View frustum — 6 plane ile tanımlanır.
#[derive(Clone)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    pub fn from_matrix(view_proj: Mat4) -> Self {
        let rows = view_proj.to_cols_array_2d();
        let planes = [
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][0], rows[1][3] + rows[1][0], rows[2][3] + rows[2][0]),
                rows[3][3] + rows[3][0],
            ),
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][0], rows[1][3] - rows[1][0], rows[2][3] - rows[2][0]),
                rows[3][3] - rows[3][0],
            ),
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][1], rows[1][3] + rows[1][1], rows[2][3] + rows[2][1]),
                rows[3][3] + rows[3][1],
            ),
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][1], rows[1][3] - rows[1][1], rows[2][3] - rows[2][1]),
                rows[3][3] - rows[3][1],
            ),
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][2], rows[1][3] + rows[1][2], rows[2][3] + rows[2][2]),
                rows[3][3] + rows[3][2],
            ),
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][2], rows[1][3] - rows[1][2], rows[2][3] - rows[2][2]),
                rows[3][3] - rows[3][2],
            ),
        ];
        Self { planes }
    }

    pub fn contains_aabb(&self, aabb: &Aabb) -> FrustumIntersection {
        let mut result = FrustumIntersection::Inside;
        for plane in &self.planes {
            let p_vertex = plane.positive_vertex(aabb);
            let n_vertex = plane.negative_vertex(aabb);
            if plane.distance(p_vertex) < 0.0 {
                return FrustumIntersection::Outside;
            }
            if plane.distance(n_vertex) < 0.0 {
                result = FrustumIntersection::Intersect;
            }
        }
        result
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrustumIntersection { Outside, Intersect, Inside }

#[derive(Clone, Copy)]
pub struct Plane { pub normal: Vec3, pub distance: f32 }

impl Plane {
    pub fn from_vecs(normal: Vec3, distance: f32) -> Self {
        let len = normal.length();
        Self { normal: normal / len, distance: distance / len }
    }
    pub fn distance(&self, point: Vec3) -> f32 { self.normal.dot(point) + self.distance }
    pub fn positive_vertex(&self, aabb: &Aabb) -> Vec3 {
        Vec3::new(
            if self.normal.x > 0.0 { aabb.max.x } else { aabb.min.x },
            if self.normal.y > 0.0 { aabb.max.y } else { aabb.min.y },
            if self.normal.z > 0.0 { aabb.max.z } else { aabb.min.z },
        )
    }
    pub fn negative_vertex(&self, aabb: &Aabb) -> Vec3 {
        Vec3::new(
            if self.normal.x < 0.0 { aabb.max.x } else { aabb.min.x },
            if self.normal.y < 0.0 { aabb.max.y } else { aabb.min.y },
            if self.normal.z < 0.0 { aabb.max.z } else { aabb.min.z },
        )
    }
}

/// AABB — `09-meshing.md` ile aynı tip.
pub struct Aabb { pub min: Vec3, pub max: Vec3 }
```

### 9.2 GPU Frustum + Hi-Z Culling

Frustum ve Hi-Z testleri **tamamen GPU compute**'da çalışır (Pass 1). CPU sadece frustum plane'ları uniform'a yazar.

```wgsl
struct SectorAabb { min: vec3<f32>, max: vec3<f32> };
struct FrustumUniform { planes: array<vec4<f32>, 6>, sector_count: u32 };

@group(0) @binding(0) var<storage, read> sector_aabbs: array<SectorAabb>;
@group(0) @binding(1) var<storage, read_write> sector_visible: array<u32>;
@group(0) @binding(2) var<uniform> frustum: FrustumUniform;

@compute @workgroup_size(256, 1, 1)
fn frustum_cull(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= frustum.sector_count) { return; }
    let aabb = sector_aabbs[idx];
    var visible = true;
    for (var i = 0u; i < 6u; i++) {
        let plane = frustum.planes[i];
        let normal = plane.xyz;
        var p_vertex: vec3<f32>;
        p_vertex.x = select(aabb.min.x, aabb.max.x, normal.x > 0.0);
        p_vertex.y = select(aabb.min.y, aabb.max.y, normal.y > 0.0);
        p_vertex.z = select(aabb.min.z, aabb.max.z, normal.z > 0.0);
        let dist = dot(normal, p_vertex) + plane.w;
        if (dist < 0.0) { visible = false; break; }
    }
    sector_visible[idx] = select(0u, 1u, visible);
}
```

**Hi-Z Occlusion (aynı compute pass içinde):**

```wgsl
@group(0) @binding(3) var hiz_texture: texture_2d<f32>;
@group(0) @binding(4) var hiz_sampler: sampler;
@group(0) @binding(5) var<uniform> occ_params: OcclusionParams;

struct OcclusionParams { view_proj: mat4x4<f32>, texture_size: vec2<u32> };

fn project_to_screen(pos: vec3<f32>, vp: mat4x4<f32>) -> vec4<f32> {
    let clip = vp * vec4<f32>(pos, 1.0);
    return clip / clip.w;
}

fn read_hiz_depth(min_uv: vec2<f32>, max_uv: vec2<f32>) -> f32 {
    let size = max_uv - min_uv;
    let max_dim = max(size.x, size.y);
    let lod = u32(log2(max_dim * f32(textureDimensions(hiz_texture, 0).x)));
    let center = (min_uv + max_uv) * 0.5;
    return textureSampleLevel(hiz_texture, hiz_sampler, center, f32(lod)).r;
}

@compute @workgroup_size(64, 1, 1)
fn hiz_occlusion_cull(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= occ_params.sector_count) { return; }
    if (sector_visible[idx] == 0u) { return; }  // Frustum'dan geçmiş

    let aabb = sector_aabbs[idx];
    let min_screen = project_to_screen(aabb.min, occ_params.view_proj);
    let max_screen = project_to_screen(aabb.max, occ_params.view_proj);
    let hiz_depth = read_hiz_depth(min_screen.xy, max_screen.xy);

    // AABB'nin en yakın noktasının NDC derinliği (reversed-Z: büyük = yakın)
    // min/max corner'ların project edilmiş Z'sinden en büyüğünü al = en yakın nokta
    // NOT: Tam konservatif test 8 köşe gerektirir; 2 köşe (min/max) yaklaşık
    let aabb_near_ndc = max(min_screen.z, max_screen.z);

    // Reversed-Z: aabb_near_ndc < hiz_depth → AABB yüzeyin arkasında → occluded
    if (aabb_near_ndc < hiz_depth) {
        sector_visible[idx] = 0u;  // Occluded
    }
}
```

### 9.3 Culling Pipeline Özeti

```
Render Frame Culling:
  ┌─────────────────────────────────────────┐
  │ 1. CPU: Frustum plane'ları build        │
  │    (view-projection → 6 plane, ~0.05ms) │
  ├─────────────────────────────────────────┤
  │ 2. GPU: Tile Selection (Pass 1)         │
  │    → Frustum + Hi-Z → Tile–Chunk pairs  │
  ├─────────────────────────────────────────┤
  │ 3. GPU: Indirect dispatch               │
  │    → Sadece görünür tile-chunk pairs    │
  ├─────────────────────────────────────────┤
  │ 4. GPU: Hi-Z Re-Execution (Pass 4)      │
  │    → Yanlış culling düzelt              │
  └─────────────────────────────────────────┘
```

---

## 10. Hi-Z Buffer Build

```wgsl
@group(0) @binding(0) var depth_texture: texture_2d<f32>;
@group(0) @binding(1) var depth_sampler: sampler;
@group(0) @binding(2) var hiz_output: texture_storage_2d<r32float, write>;

struct HiZParams { level: u32, src_size: vec2<u32> };
@group(0) @binding(3) var<uniform> hiz_params: HiZParams;

@compute @workgroup_size(16, 16, 1)
fn hiz_build(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= hiz_params.src_size.x || id.y >= hiz_params.src_size.y) { return; }

    // 2×2 bloktan maximum depth al (reversed-Z: max = en yakın)
    let offset = vec2<i32>(id.xy) * 2;
    var max_depth: f32 = 0.0;  // reversed-Z: 0 = en uzak

    for (var dy = 0; dy < 2; dy++) {
        for (var dx = 0; dx < 2; dx++) {
            let sample_pos = offset + vec2<i32>(dx, dy);
            let sample_uv = vec2<f32>(sample_pos) / vec2<f32>(hiz_params.src_size * 2u);
            let depth = textureSampleLevel(depth_texture, depth_sampler, sample_uv, 0.0).r;
            max_depth = max(max_depth, depth);
        }
    }

    textureStore(hiz_output, vec2<i32>(id.xy), vec4<f32>(max_depth, 0.0, 0.0, 0.0));
}
```

> **NOT:** Reversed-Z kullanıldığı için Hi-Z **maximum** depth alır (en yakın = en büyük değer). Occlusion test (`§9.2`): `aabb_near_ndc < hiz_depth` = occluded — AABB'nin en yakın NDC derinliği Hi-Z'den küçükse, AABB yüzeyin arkasındadır.

---

## 11. HDR Rendering

### 11.1 HDR Pipeline

```rust
pub struct HdrPipeline {
    pub hdr_texture: wgpu::Texture,
    pub hdr_view: wgpu::TextureView,
    pub bloom_texture: wgpu::Texture,
    pub bloom_view: wgpu::TextureView,
    pub exposure: f32,
    pub tone_mapper: ToneMappingMode,
}

pub enum ToneMappingMode { Aces, Reinhard, Uncharted2, Linear }

pub struct ExposureController {
    pub mode: ExposureMode,
    pub manual_exposure: f32,
    pub auto_min: f32,
    pub auto_max: f32,
    pub auto_speed: f32,
    pub current_exposure: f32,
}

pub enum ExposureMode { Manual, Auto }
```

### 11.2 Tone Mapping + Bloom (WGSL)

```wgsl
// ACES tone mapping (Pass 7 — Deferred Lighting sonrası)
fn aces_tone_map(color: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((color * (a * color + b)) / (color * (c * color + d) + e), vec3(0.0), vec3(1.0));
}

@compute @workgroup_size(8, 8, 1)
fn hdr_resolve(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.xy;
    let hdr_color = textureLoad(hdr_texture, pixel, 0).rgb;

    // Exposure
    let exposed = hdr_color * params.exposure;

    // Tone mapping
    let tone_mapped = aces_tone_map(exposed);

    // Bloom ekle
    let bloom = textureSampleLevel(bloom_texture, bloom_sampler, vec2<f32>(pixel) / screen_size, 0.0).rgb;
    let final_color = tone_mapped + bloom * params.bloom_intensity;

    textureStore(output_texture, pixel, vec4<f32>(final_color, 1.0));
}
```

### 11.3 Bloom Pass

```rust
pub struct BloomPass {
    pub threshold: f32,       // Parlaklık eşiği (default: 1.0)
    pub intensity: f32,       // Bloom şiddeti (default: 0.3)
    pub blur_iterations: u32,   // Gaussian blur iterasyonu (default: 5)
    pub blur_radius: f32,     // Blur yarıçapı (default: 4.0)
}
```

---

## 12. Async Compute Fırsatları

Render pipeline'daki GPU compute pass'leri arasında **async compute queue** ile örtüştürülebilecek fırsatlar. Modern GPU'lar (RTX 20+, RDNA 2+) graphics ve compute queue'larını paralel yürütebilir.

| Fırsat | Örtüşen Pass'ler | Tahmini Kazanç | Not |
|--------|-----------------|----------------|-----|
| **Hi-Z Build + XBrickMap RT** | Pass 8 (N-1 frame) ∥ Pass 2 (N frame) | ~0.2ms | Önceki frame Hi-Z, Pass 1 başlamadan hazır olmalı — tam frame gecikme kabul edilebilir |
| **SVDAG Bake + Ray March** | `07` SVDAG bake (compute) ∥ Pass 3 SVDAG RM | ~1-2ms | Ghost page ile zaten paralel; async compute ile bake daha da hızlanır |
| **VRCS Color Resolve + Hi-Z Build** | Pass 5 ∥ Pass 8 | ~0.2ms | Hi-Z build bağımsız buffer'a yazar; VRCS okuması etkilenmez |
| **Mesh Upload + Culling** | `09` GigaBuffer upload ∥ Pass 1 | ~0.3ms | Upload ayrı queue'da, culling bağımsız veri okur |

**Uygulama notu:** wgpu'da async compute `wgpu::Queue::submit` ile birden fazla command buffer farklı `CommandEncoder`'dan gönderilerek sağlanır. Bevy'nin render scheduler (`RenderStage`) ile entegrasyon `04-plugin-api.md` §2 SubApp'te yapılır.

> **DİKKAT:** Pass 4 (Hi-Z Re-Execution) Pass 2-3'ün tamamlanmasını beklemeli (barrier). Async compute bu bağımlılığı ihlal etmemeli. Pipeline barrier yerleşimi: `Pass 1 → [Pass 2 ∥ Pass 3] → barrier → Pass 4 → Pass 5 → ...`

---

## 13. Performans Hedefleri

| Metrik | Hedef | Not |
|--------|-------|-----|
| Tile selection + Hi-Z culling (GPU) | <0.5ms | 1000+ sektör, Aokana referans |
| Hi-Z re-execution | <0.2ms | Yanlış culling düzeltme |
| XBrickMap ray trace (Tier 1) | ~2-3ms | Beam optimization ile |
| SVDAG ray march (Tier 2-3) | ~2-4ms | Indirect dispatch, shallow depth |
| VRCS color resolve | <0.5ms | %60-80 thread azalması |
| Entity composite | <0.3ms | Mesh entity'ler |
| HDR + lighting | ~1ms | ACES + bloom |
| Hi-Z build | <0.2ms | Mipmap piramidi |
| **Toplam voxel pipeline** | **~6-10ms** | 60Hz hedef (16.6ms budget); 144Hz için ~6.9ms — dar, optimizasyon kritik |
| Frustum culling (CPU) | <0.05ms | Sadece plane build |
| Culling oranı | %60-80 | Görünür/Toplam sektör |
| GPU feedback hit rate | >90% | Sadece erişilen sektörler yüklü |

---

## 14. Crate Organizasyonu

```
crates/
  render/
    ├── mod.rs                  ← RenderPlugin, pass orkestrasyon
    ├── pipeline/
    │   ├── mod.rs              ← Render frame coordinator
    │   ├── tile_selection.rs   ← Pass 1: Tile–Chunk pairs + Hi-Z cull
    │   ├── xbrickmap_rt.rs     ← Pass 2: XBrickMap ray trace + beam opt
    │   ├── svdag_rm.rs         ← Pass 3: SVDAG ray march (indirect)
    │   ├── hiz_reexec.rs       ← Pass 4: Hi-Z re-execution
    │   ├── vrcs_resolve.rs     ← Pass 5: VRCS color resolve
    │   ├── entity_composite.rs ← Pass 6: Mesh entity composite
    │   └── hdr_lighting.rs     ← Pass 7: Deferred lighting + HDR
    ├── visibility/
    │   ├── mod.rs              ← Visibility buffer yönetimi
    │   ├── buffer.rs           ← 64-bit encode/decode
    │   └── atomic.rs           ← Native u64 / emulated fallback
    ├── culling/
    │   ├── mod.rs              ← Culling sistemi
    │   ├── frustum.rs          ← Frustum, Plane
    │   ├── gpu_cull.rs         ← GPU frustum + Hi-Z culling
    │   └── hiz.rs              ← Hi-Z buffer build
    ├── seam/
    │   ├── mod.rs              ← Seam/crack yönetimi
    │   ├── border.rs           ← Border-aware traversal
    │   └── blend.rs            ← TierBlendManager
    ├── vrcs/
    │   ├── mod.rs              ← VRCS config
    │   └── fovea.rs            ← Foveated bölge hesaplama
    └── hdr/
        ├── mod.rs
        ├── tone_mapping.rs
        ├── bloom.rs
        └── exposure.rs
```

---

## 15. Araştırma Doğrulamaları ve Öneriler (2026-06)

> **Kaynak:** 5 worker ile 40+ WebSearch sorgusu, SIGGRAPH/akademik paper'lar, GPU vendor best practices.

### 15.1 Doğrulanan Kararlar

| Karar | Doğrulama |
|-------|-----------|
| Aokana visibility buffer (64-bit) | 2024-2026 literatüründe voxel-specific alternatif bulunamadı — hâlâ SOTA |
| Hi-Z occlusion + re-execution | Aokana ghosting eliminasyonu ~0.2ms, production validated |
| VRCS foveated shading | DOOM DtDA GPC 2025 kanıtlı, ~%60-80 thread azalması |
| Tile-chunk pairs | Aokana §4, indirect dispatch ile GPU-driven rendering |
| HDR FP16 + ACES + bloom | Industry standard, Bevy uyumlu |

### 15.2 P2 — R64Uint Texture Atomic Visibility Buffer

**Problem:** Mevcut visibility buffer `storage` buffer olarak kullanılır. GPU texture cache'den faydalanamaz.

**Çözüm:** Visibility buffer'ı `R64Uint` texture olarak kullan. GPU texture cache daha iyi locality sağlar.

**Etki:** ~%15-30 cache locality iyileştirmesi.

```rust
// Eski (storage buffer):
let vis_buffer = device.create_texture(&wgpu::TextureDescriptor {
    format: wgpu::TextureFormat::Rg32Uint, // 2×32-bit emulated
    usage: wgpu::TextureUsages::STORAGE_BINDING,
    ..
});

// Yeni (R64Uint texture):
let vis_buffer = device.create_texture(&wgpu::TextureDescriptor {
    format: wgpu::TextureFormat::R64Uint, // native 64-bit
    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
    ..
});
```

**Gereksinim:** `Features::TEXTURE_INT64` desteği (Vulkan: `VK_EXT_shader_image_atomic_int64`, DX12: SM 6.6+).

**Entegrasyon:** Phase 2-3 — feature detection ile graceful fallback.

### 15.3 P2 — SPD Single-Pass Hi-Z Build

**Problem:** Mevcut Hi-Z build multi-pass mipmap generation (~0.3ms).

**Çözüm:** AMD SPD (Single Pass Downsampler) ile tek dispatch'te tüm mip seviyeleri.

**Etki:** ~0.15ms tasarruf.

```wgsl
// AMD SPD — single dispatch Hi-Z build
// https://gpuopen.com/fidelityfx-spd/
@compute @workgroup_size(256, 1, 1)
fn spd_downsample(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    // 6 mip seviyesi tek dispatch'te
    // Shared memory + atomic operations ile parallel reduction
    // Her seviye: 2×2 min/max pooling
}
```

**Entegrasyon:** Pass 8 (Hi-Z Build) güncellenmeli. AMD SPD reference implementation wgpu'ya port edilmeli.

### 15.4 P3 — AgX Tonemapper

**Problem:** Mevcut ACES tonemapper emissive bloklarda hue shift yapar (mavi→mor, kırmızı→pembe).

**Çözüm:** AgX tonemapper — hue-preserving highlights.

```wgsl
// AgX tonemapper — hue-preserving
fn agx_tonemap(color: vec3<f32>) -> vec3<f32> {
    let agx = agx_look(color * agx_exposure);
    return srgb_to_linear(agx);
}

fn agx_look(color: vec3<f32>) -> vec3<f32> {
    // AgX Default look — minimal contrast, hue korunur
    let saturation = 1.0;
    let contrast = 1.0;
    // ...
}
```

**Avantaj:** Lava, glowstone, neon bloklarda doğal renk geçişleri. **Phase 5+** — ACES ile birlikte seçenek olarak sunulmalı.

### 15.5 P3 — VRCS Deblocking Filter

**Problem:** VRCS foveated shading 4×4 periferik tile'larda block artifacts oluşturur.

**Çözüm:** Fovea/mid boundary'de deblocking filter — edge smoothing.

```wgsl
// VRCS deblocking — fovea boundary'de smoothing
fn vrcs_deblock(pos: vec2<f32>, fovea_radius: f32) -> vec3<f32> {
    let dist = length(pos - screen_center);
    let boundary = smoothstep(fovea_radius - 0.1, fovea_radius + 0.1, dist);
    // Periferik tile boundary'de bilinear interpolation
    return mix(high_res, low_res, boundary);
}
```

**Etki:** Periferikte pürüzsüz geçiş. **Phase 5+** — profiling sonrası gerekirse eklenmeli.
