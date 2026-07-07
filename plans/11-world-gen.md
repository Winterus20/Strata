# 11 — World Generation Sistemi

> **Olgunluk:** 📝 Taslak (`01-overview.md` §1.1). Anayasa `01`–`10`; bu dosya `01`–`10` ile çelişirse **anayasa** önceliklidir.
> **Crate:** `world-gen` (`02-implementation.md`)
> **Bağımlılıklar:** `06-xbrickmap.md` (32³ sektör, `GlobalBrickPool`, `SectorPalette`), `05-block-registry.md` (`BlockRegistry`, `PaletteEntry`), `03-ecs-architecture.md` (Bevy ECS, `WorldSystems::Generation`, `AsyncComputeTaskPool`), `08-streaming.md` (`LoadSource::WorldGen`, `StreamingEvent`), `32-modding.md` (`worldgen-hooks` WIT)
> **Temel felsefe:** Density-function tabanlı, sektör-bağımsız, deterministik hash, cubic-chunks native

---

## 1. Genel Bakış

Strata'nın prosedürel dünya üretimi **density-function** tabanlı, **biome-driven** ve **structure-aware** bir sistemdir. Minecraft 1.18+'ın noise router yaklaşımından esinlenilmiş, ancak Strata'nın kübik sektör (32³) ve XBrickMap (`06`) mimarisine uyarlanmıştır.

### Temel Prensipler

- **Density-function tabanlı:** Her voxel için `f(x,y,z) -> density` hesaplanır. `density > 0` = katı, `density < 0` = hava. Sütun iterasyonu YOK — sınırsız yükseklik doğal olarak desteklenir.
- **Sektör-bağımsız:** Her 32³ sektör, yalnızca `(coord, seed)` fonksiyonu olarak üretilir. Komşu sektör verisine ihtiyaç duymaz.
- **Deterministik hash:** `wyhash(coord, seed)` nokta sorguları, `PCG32` akış RNG. Aynı seed + aynı koordinat = aynı sonuç.
- **Biome-driven:** Whittaker diyagramı (sıcaklık + nem) ile biome belirlenir; biome, density fonksiyonunun parametrelerini kontrol eder.
- **ECS-native:** `WorldGenPlugin` (Bevy Plugin), `AsyncComputeTaskPool` ile paralel üretim (`03` §9.5).
- **Streaming-entegre:** WorldGen, `LoadSource::WorldGen` olarak `08-streaming` §7.1'de kaynak.
- **Modding-friendly:** `worldgen-hooks` WIT arayüzü (`32-modding.md`).

### Eski Taslaktan Farklar (Reddedilenler)

| Eski Taslak | Sorun | Yeni Yaklaşım |
|-------------|-------|---------------|
| Sütun iterasyonu (y: 0..128) | Sabit yükseklik, cubic chunks ile uyumsuz (`06`) | Density function: voxel başına `f(x,y,z)` |
| `BiomeMap` sabit `Vec` grid | Sonsuz dünya ile uyumsuz | Prosedürel Whittaker lookup |
| `sector.set_block()` doğrudan | `GlobalBrickPool` + `SectorPalette` bypass (`06`) | `SectorPalette::get_or_insert` ile pool yazımı |
| `tokio::spawn_blocking` | Bevy ECS scheduler ile uyumsuz (`03` §9.5) | `AsyncComputeTaskPool` |
| LCG RNG (Java tarzı) | Zayıf kalite, 3D pattern | PCG32 + wyhash |
| Basit 3D noise threshold mağara | "İsviçre peyniri" mağaralar | Hibrit: 3D noise isosurface + worm noise tünel |

---

## 2. Seed & Determinism

```rust
use std::num::Wrapping;

/// Dünya seed'i ve üretim parametreleri.
#[derive(Clone, Copy, Debug)]
pub struct WorldSeed(pub u64);

/// PCG32 — hızlı, kaliteli, splittable RNG.
/// LCG'nin zayıf bit kalitesini permütasyon ile düzeltir.
/// Kaynak: https://www.pcg-random.org/
#[derive(Clone)]
pub struct Pcg32Rng {
    state: u64,
    inc: u64,
}

impl Pcg32Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self { state: 0, inc: (seed << 1) | 1 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Split — alt-thread'ler için bağımsız RNG oluştur.
    /// Her split farklı bir stream üretir (LCG increment değişikliği).
    pub fn split(&mut self) -> Self {
        let new_inc = (self.next_u64() << 1) | 1;
        Self {
            state: self.state,
            inc: new_inc,
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let old_state = self.state;
        self.state = old_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc);
        let xorshifted = (((old_state >> 18) ^ old_state) >> 27) as u32;
        let rot = (old_state >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    pub fn next_u64(&mut self) -> u64 {
        (self.next_u32() as u64) << 32 | self.next_u32() as u64
    }

    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// wyhash — koordinat bazlı deterministik hash (stream RNG değil, tek seferlik).
/// Her (seed, x, y, z) kombinasyonu için benzersiz, yüksek kaliteli u64 üretir.
/// Kullanım: yapı yerleşimi, ağaç pozisyonu, biome nokta sorguları.
#[inline]
pub fn wyhash_coord(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut h = seed;
    h ^= (x as u64).wrapping_mul(0x9E3779B97F4A7C15);
    h ^= (y as u64).wrapping_mul(0x517CC1B727220A95);
    h ^= (z as u64).wrapping_mul(0x6C62272E07BB0142);
    h = h.wrapping_mul(0x9E3779B97F4A7C15);
    h ^= h >> 32;
    h
}
```

**Neden PCG32 + wyhash?**
- PCG32: Stream RNG — ardışık çağrılar bağımsız değerler üretir (terrain iterasyonu, ağaç dallanma).
- wyhash: Nokta sorgusu — `(x,y,z)` koordinatından doğrudan deterministik hash (yapı "burada olmalı mı?" kontrolü).
- İkisi birlikte: stream ve point-query senaryolarını kapsar.

---

## 3. Noise Pipeline

`fastnoise2` crate (SIMD destekli, C++ FFI) kullanılır (`01-overview.md` §6.1).

```rust
use fastnoise2::FastNoise2;

/// Noise pipeline — seed'den oluşturulan, birden fazla noise kanalı.
pub struct NoisePipeline {
    /// 3D FBM — temel arazi yoğunluğu (density function çekirdeği).
    pub terrain_density: FastNoise2,

    /// 2D FBM — sıcaklık haritası (biome için).
    pub temperature: FastNoise2,

    /// 2D FBM — nem haritası (biome için).
    pub moisture: FastNoise2,

    /// 3D Simplex — mağara oyma (isosurface threshold).
    pub cave_noise: FastNoise2,

    /// 3D Cellular — worm/tünel modifikatörü.
    pub worm_noise: FastNoise2,

    /// 2D — yüzey detay noise'u (mikro varyasyon).
    pub detail: FastNoise2,

    /// 3D — erozyon modülasyonu (eğim bazlı).
    pub erosion: FastNoise2,
}

impl NoisePipeline {
    /// Seed'den noise pipeline oluştur.
    /// Her kanal bağımsız sub-seed alır (PCG32 split).
    pub fn from_seed(seed: WorldSeed) -> Self {
        let mut rng = Pcg32Rng::new(seed.0);

        Self {
            terrain_density: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.005)
                .fractal_type(fastnoise2::FractalType::FBM)
                .octaves(6)
                .lacunarity(2.0)
                .gain(0.5)
                .build(),

            temperature: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.002)
                .fractal_type(fastnoise2::FractalType::FBM)
                .octaves(4)
                .build(),

            moisture: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.002)
                .fractal_type(fastnoise2::FractalType::FBM)
                .octaves(4)
                .build(),

            cave_noise: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.02)
                .noise_type(fastnoise2::NoiseType::OpenSimplex2)
                .fractal_type(fastnoise2::FractalType::FBM)
                .octaves(3)
                .build(),

            worm_noise: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.01)
                .noise_type(fastnoise2::NoiseType::Cellular)
                .cellular_distance_function(
                    fastnoise2::CellularDistanceFunction::Hybrid
                )
                .build(),

            detail: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.05)
                .noise_type(fastnoise2::NoiseType::OpenSimplex2)
                .build(),

            erosion: FastNoise2::from_seed(rng.next_u64())
                .frequency(0.015)
                .noise_type(fastnoise2::NoiseType::OpenSimplex2)
                .fractal_type(fastnoise2::FractalType::FBM)
                .octaves(3)
                .build(),
        }
    }
}
```

**Not:** `fastnoise2` Rust crate'i C++ `FastNoise2` kütüphanesini FFI ile sarar. Builder API'si crate versiyonuna göre değişebilir; implementasyon aşamasında güncel API doğrulanmalıdır.

### 3.1 Batch Evaluation — KRİTİK PERFORMANS KURALI

FastNoise2 benchmark'ları ve wiki'si açıkça belirtiyor: **`GenSingle` (tekil `get_noise_3d`/`get_noise_2d`) ÇOK YAVAŞ — asla döngüde kullanma.** SIMD lane'leri underutilize olur. FastNoise2 (AVX2) 3D Simplex'te **~268 Mpts/s** (batch) elde ederken, tekil çağrıda bu hızın çok altına düşülür.

**YASAK:** Aşağıdaki gibi inner-loop tekil çağrı yapılmaz:

```rust
// 🚫 YANLIŞ — SIMD heba olur, ~5-8× yavaş
for ly in 0..32 {
    for lz in 0..32 {
        for lx in 0..32 {
            let n = noise.terrain_density.get_noise_3d(wx as f32, wy as f32, wz as f32);
            // ...
        }
    }
}
```

**DOĞRU:** Tüm sektör için bir `Vec<f32>` tahsis et, tek `GenUniformGrid3D` çağrısıyla doldur:

```rust
/// Bir sektörün tüm 3D noise kanalını tek SIMD çağrısıyla doldur.
/// GenUniformGrid3D: positions internal olarak üretilir, batch SIMD.
pub fn fill_sector_3d(
    gen: &FastNoise2,
    out: &mut [f32; 32 * 32 * 32],
    origin: IVec3,
    seed: u64,
) {
    gen.gen_uniform_grid_3d(
        out,                       // &mut [f32]
        origin.x as f32, origin.y as f32, origin.z as f32,
        32, 32, 32,               // sample counts
        1.0, 1.0, 1.0,            // step sizes (1 block)
        seed,
    );
}
```

**Alternatif — daha da hızlı:** `GenPositionArray` ile kendi position buffer'ını bir kez tahsil edip tüm çağrılarda yeniden kullan (offset ile kaydır). En yüksek throughput için önerilir.

**2D noise (biome/temperature/moisture):** Bunlar sadece X/Z'ye bağlı. `GenUniformGrid2D` ile `32×32` slice üret (Y boyutu 1). Sonra column cache'e yaz (bkz. §4.2).

**Performans Kazancı:** Batch API + column caching → sektör üretim hızı tahmini **~10×** artar (FastNoise2 SIMD + 32× azalan biome sorgusu).

---

## 4. Biome Sistemi

### 4.1 Whittaker Diyagramı

Sabit `Vec<BiomeId>` grid yerine **prosedürel biome lookup**: sıcaklık ve nem noise değerlerinden Whittaker diyagramı ile biome belirleme.

```rust
/// Biome ID (u8 = 256 max biome).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BiomeId(pub u8);

/// Whittaker diyagramı — sıcaklık × nem grid'i.
/// Her hücre bir BiomeId tutar. Grid çözünürlüğü yapılandırılabilir.
pub struct WhittakerDiagram {
    /// Sıcaklık ekseni boyutu (default 16).
    pub temp_steps: u32,
    /// Nem ekseni boyutu (default 16).
    pub moist_steps: u32,
    /// Biome grid (temp_steps × moist_steps).
    pub grid: Vec<BiomeId>,
}

impl WhittakerDiagram {
    /// Normalize sıcaklık (0.0-1.0) ve nem (0.0-1.0) → BiomeId.
    pub fn lookup(&self, temperature: f32, moisture: f32) -> BiomeId {
        let t = (temperature.clamp(0.0, 1.0) * (self.temp_steps - 1) as f32) as usize;
        let m = (moisture.clamp(0.0, 1.0) * (self.moist_steps - 1) as f32) as usize;
        self.grid[m * self.temp_steps as usize + t]
    }

    /// Default Whittaker diyagramı (vanilla biome'ler).
    pub fn default_diagram() -> Self {
        // 16×16 grid, bilinen biome pattern'leri
        // Sol-alt: soğuk+kuru = tundra
        // Sağ-üst: sıcak+nemli = jungle
        // ... (TOML'den yüklenir, burada default)
        todo!("Load from TOML presets")
    }
}
```

### 4.2 Biome Sorgulama (Prosedürel, Grid-Free)

```rust
/// Dünya koordinatından biome sorgula — sabit grid YOK.
/// 4 nokta örnekleme ile smooth blending.
pub fn query_biome(
    world_x: i32,
    world_z: i32,
    noise: &NoisePipeline,
    diagram: &WhittakerDiagram,
) -> BiomeSample {
    // 4 nokta: (x,z), (x+BIOME_SAMPLE_STEP, z), (x, z+BIOME_SAMPLE_STEP), (x+step, z+step)
    const STEP: i32 = 64; // Biome örnekleme adımı (blok)
    let base_x = (world_x / STEP) * STEP;
    let base_z = (world_z / STEP) * STEP;

    let mut samples = [BiomeSample::default(); 4];
    let offsets = [(0i32, 0i32), (STEP, 0), (0, STEP), (STEP, STEP)];

    for (i, (dx, dz)) in offsets.iter().enumerate() {
        let sx = base_x + dx;
        let sz = base_z + dz;
        let temp = noise.temperature.get_noise_2d(sx as f32, sz as f32) * 0.5 + 0.5;
        let moist = noise.moisture.get_noise_2d(sx as f32, sz as f32) * 0.5 + 0.5;
        let biome = diagram.lookup(temp, moist);
        samples[i] = BiomeSample { biome, temp, moist };
    }

    // Bilinear interpolation ağırlıkları
    let fx = (world_x - base_x) as f32 / STEP as f32;
    let fz = (world_z - base_z) as f32 / STEP as f32;

    BiomeSample::blend(&samples, fx, fz)
}

pub struct BiomeSample {
    pub biome: BiomeId,
    pub temp: f32,
    pub moist: f32,
}

impl BiomeSample {
    /// Bilinear blend — 4 köşe örneğinden ağırlıklı ortalama.
    /// Dominant biome = en yüksek ağırlıklı örnek.
    pub fn blend(samples: &[BiomeSample; 4], fx: f32, fz: f32) -> Self {
        let w = [
            (1.0 - fx) * (1.0 - fz),
            fx * (1.0 - fz),
            (1.0 - fx) * fz,
            fx * fz,
        ];
        // Dominant biome seç (en yüksek ağırlık)
        let max_idx = w.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap().0;
        Self {
            biome: samples[max_idx].biome,
            temp: samples.iter().zip(w.iter()).map(|(s, &w)| s.temp * w).sum(),
            moist: samples.iter().zip(w.iter()).map(|(s, &w)| s.moist * w).sum(),
        }
    }
}

### 4.2.1 Column Caching (KRİTİK OPTİMİZASYON)

2D noise (temperature, moisture, biome) sadece X/Z'ye bağlıdır, Y'den bağımsız. Mevcut `query_biome` her voxel için 2D noise çağırıyor → **32³ = 32768 çağrı** yerine sadece **32×32 = 1024 column** hesaplanmalı.

Voxel Play 4 ve Minecraft (`cache_2d` / `flat_cache`) aynı prensibi kullanır: **~10× hızlanma**.

```rust
/// Bir sektör için sadece X/Z köşelerinde biome hesapla (32×32 = 1024).
/// Y ekseni boyunca tekrar YOK.
pub fn cache_biome_columns(
    origin: IVec3,
    ctx: &WorldGenContext,
) -> [BiomeSample; 1024] {
    let mut cols = [BiomeSample::default(); 1024];

    // 2D noise batch — GenUniformGrid2D ile tek SIMD çağrı
    let mut temps = [0f32; 32 * 32];
    let mut moist = [0f32; 32 * 32];
    ctx.noise.temperature.gen_uniform_grid_2d(
        &mut temps, origin.x as f32, origin.z as f32, 32, 32, 1.0, 1.0, ctx.seed.0,
    );
    ctx.noise.moisture.gen_uniform_grid_2d(
        &mut moist, origin.x as f32, origin.z as f32, 32, 32, 1.0, 1.0, ctx.seed.0,
    );

    for lz in 0..32 {
        for lx in 0..32 {
            let t = temps[(lz * 32 + lx) as usize] * 0.5 + 0.5;
            let m = moist[(lz * 32 + lx) as usize] * 0.5 + 0.5;
            let biome = ctx.diagram.lookup(t, m);
            cols[(lz * 32 + lx) as usize] = BiomeSample { biome, temp: t, moist: m };
        }
    }
    cols
}
```

**Kullanım:** `generate_sector_voxels` (§5.3) içinde, inner-loop başlamadan önce `cache_biome_columns` çağrılır. Her voxel sadece `cols[(lz * 32 + lx)]` index'inden okur.

**Kazanç:** Biome sorgusu 32768 → 1024 (**32× azalma**). 2D noise SIMD batch ile birleştirilirse ek hızlanma.
}
```

### 4.3 BiomeDefinition

```rust
/// Biome tanımı — density function parametrelerini kontrol eder.
/// TOML'den yüklenir (`05-block-registry.md` ile aynı pattern).
#[derive(Clone, Debug)]
pub struct BiomeDefinition {
    pub id: BiomeId,
    pub name: StringId,

    // --- Density function parametreleri ---
    /// Temel arazi yüksekliği (deniz seviyesine göre, blok).
    pub base_height: f32,
    /// Yükseklik genliği (noise çarpanı).
    pub height_amplitude: f32,
    /// Detay genliği (yüzey mikro-varyasyon).
    pub detail_amplitude: f32,
    /// Yüzey eğrisi — Y'ye göre density falloff.
    pub height_curve: HeightCurve,

    // --- Blok tipleri ---
    pub surface_block: u16,
    pub subsurface_block: u16,
    pub base_block: u16,

    // --- Özellikler ---
    pub water_level: i32,
    pub tree_type: Option<TreeType>,
    pub tree_density: f32,         // 0.0-1.0
    pub vegetation: Vec<VegetationEntry>,
    pub ore_modifiers: Vec<OreModifier>,
}

/// Yükseklik eğrisi — Y'ye göre density nasıl düşer.
#[derive(Clone, Debug)]
pub enum HeightCurve {
    /// Düz arazi: base_height etrafında keskin düşüş.
    Flat { falloff: f32 },
    /// Dağlık: geniş genlik, yavaş düşüş.
    Mountain { falloff: f32, peak_boost: f32 },
    /// Plato: belirli yükseklikte düzleşme.
    Plateau { plateau_y: i32, plateau_width: f32 },
    /// Hills: orta genlik, dalgalı.
    Hills { wave_freq: f32, wave_amp: f32 },
}

/// TOML örneği (biomes/plains.toml):
/// [biome]
/// id = 1
/// name = "plains"
/// base_height = 64.0
/// height_amplitude = 8.0
/// detail_amplitude = 2.0
/// height_curve = { type = "flat", falloff = 0.02 }
/// surface_block = "grass_block"
/// subsurface_block = "dirt"
/// base_block = "stone"
/// water_level = 62
/// tree_type = "oak"
/// tree_density = 0.02
```

---

## 5. Density Function Terrain

### 5.1 Per-Voxel Density Hesaplama (Batch Optimized)

Sütun iterasyonu (y: 0..128) yerine, her voxel için yoğunluk fonksiyonu hesaplanır. Bu, kübik sektörlerle (`06`) doğal uyumludur.

**ÖNEMLİ:** Density hesaplama inner-loop'ta tekil noise çağrısı YAPMAZ. Tüm 3D noise kanalları §3.1 batch API ile önceden doldurulmuş `Vec<f32>` buffer'larından okunur. Bu, SIMD util'i korur ve ~5-8× hızlanma sağlar.

```rust
/// Bir sektörün tüm density alanını batch hesapla.
/// Ön koşul: terrain/detail/cave/erosion buffer'ları §3.1 ile doldurulmuş.
pub fn compose_sector_density(
    out: &mut [f32; 32 * 32 * 32],
    terrain: &[f32; 32 * 32 * 32],
    detail: &[f32; 32 * 32 * 32],
    cave: &[f32; 32 * 32 * 32],
    erosion: &[f32; 32 * 32 * 32],
    biome_cols: &[BiomeSample; 1024],
    origin: IVec3,
) {
    for i in 0..(32 * 32 * 32) {
        let lx = (i % 32) as i32;
        let ly = ((i / 32) % 32) as i32;
        let lz = (i / (32 * 32)) as i32;
        let wy = origin.y + ly;

        let biome = &biome_cols[(lz * 32 + lx) as usize]; // column cache (§4.2.1)

        // 1. Temel 3D noise (batch buffer'dan)
        let base_noise = terrain[i];

        // 2. Yükseklik eğrisi — biome'a özgü Y-bağımlı falloff
        let height_mod = height_curve_value(wy, biome);

        // 3. Detay noise (batch buffer'dan)
        let detail_val = detail[i] * biome.detail_amplitude;

        // 4. Mağara oyma (batch buffer'dan)
        let cave_val = cave[i];

        // 5. Termal erozyon (batch buffer'dan)
        let erosion_val = erosion[i] * biome.erosion_factor();

        // Birleşik density
        out[i] = base_noise * biome.height_amplitude + height_mod + detail_val - erosion_val - cave_val;
    }
}
```

**Not:** `density_at` (tekil voxel) hâlâ modding hook'ları (`32`) ve debug için tutulur, ancak hot-path'te `compose_sector_density` kullanılır.

```rust
/// Yükseklik eğrisi — Y'ye göre base density offset.
fn height_curve_value(y: i32, biome: &BiomeDefinition) -> f32 {
    let base_y = biome.base_height;
    match biome.height_curve {
        HeightCurve::Flat { falloff } => {
            // base_height'tan uzaklaştıkça density düşer
            -((y as f32 - base_y).abs() * falloff)
        }
        HeightCurve::Mountain { falloff, peak_boost } => {
            let dist = y as f32 - base_y;
            if dist > 0.0 {
                // Yukarı: geniş genlik, peak_boost ile zirve güçlendirme
                -dist * falloff + peak_boost * (dist * 0.01).sin()
            } else {
                // Aşağı: daha keskin
                dist.abs() * falloff * 1.5
            }
        }
        HeightCurve::Plateau { plateau_y, plateau_width } => {
            let dist = (y as f32 - plateau_y as f32).abs();
            if dist < plateau_width {
                // Plato içi: neredeyse düz (yüksek density)
                -(dist / plateau_width) * 2.0
            } else {
                // Plato dışı: normal falloff
                -(dist - plateau_width) * 0.05
            }
        }
        HeightCurve::Hills { wave_freq, wave_amp } => {
            let base = -((y as f32 - base_y).abs() * 0.02);
            base + (y as f32 * wave_freq).sin() * wave_amp
        }
    }
}
```

### 5.1.1 Data-Driven Density Node Graph (Compile-Time Flatten)

Minecraft'ın `noise_router` modeli örnek alınır: density fonksiyonları **composable node'lar** olarak tanımlanır. Ancak Strata'da bu **runtime interpreter DEĞİL**, **compile-time flattened op-list** olarak uygulanır (Rust enum + monomorfik `eval_sector`).

**Neden runtime interpreter değil?** Araştırma (Voxel Tools docs): scripted node graph'lar hardcoded C++'tan **20-30× yavaş** (graph traversal + state switching overhead). Compile-time flatten ile bu overhead neredeyse sıfırdır — Rust enum matching CPU-friendly, virtual call yok.

```rust
/// Composable density node — TOML'dan parse edilir, compile-time monomorfik.
/// eval_sector: tüm sektör için f32 buffer doldurur (batch, SIMD-friendly).
pub enum DensityNode {
    Constant(f32),
    Noise(NoiseChannel),                    // §3.1 batch API
    Add(Box<Self>, Box<Self>),
    Mul(Box<Self>, Box<Self>),
    Min(Box<Self>, Box<Self>),
    Max(Box<Self>, Box<Self>),
    YGradient { base: f32, falloff: f32 },  // height_curve_value karşılığı
    Spline { points: Vec<(f32, f32)> },     // biome-transition smoothing
    Cache2D(Box<Self>),                     // column cache wrapper (§4.2.1)
}

impl DensityNode {
    /// Tüm sektör için density hesapla — recursive, cache-aware.
    /// Cache2D node'u otomatik column caching yapar (1024 çağrı).
    pub fn eval_sector(&self, out: &mut [f32], coord: SectorCoord, ctx: &WorldGenContext) {
        match self {
            DensityNode::Constant(v) => out.iter_mut().for_each(|o| *o = *v),
            DensityNode::Noise(ch) => ctx.noise.batch_3d(*ch, out, coord, ctx.seed),
            DensityNode::Add(a, b) => {
                let mut tmp = vec![0f32; out.len()];
                a.eval_sector(out, coord, ctx);
                b.eval_sector(&mut tmp, coord, ctx);
                out.iter_mut().zip(tmp).for_each(|(o, t)| *o += t);
            }
            // ... Mul, Min, Max benzer
            DensityNode::Cache2D(inner) => {
                // Sadece X/Z'ye bağlı → 32×32 hesapla, Y boyunca broadcast
                let mut cols = [0f32; 1024];
                inner.eval_columns(&mut cols, coord, ctx);
                for ly in 0..32 {
                    for i in 0..1024 {
                        out[ly * 1024 + i] = cols[i];
                    }
                }
            }
            _ => { /* diğer node'lar */ }
        }
    }
}
```

**Avantajlar:**
- **Modding (`32`):** Yeni density node WIT ile eklenebilir, kod değişikliği gerekmez
- **Balancing:** Sadece TOML değişikliği (yeni node graph)
- **Performans:** Compile-time flatten → hardcoded ile aynı hız (Rust enum)
- **Cache2D:** Otomatik column caching (Voxel Tools `use_optimized_execution_map` ile aynı)

**Entegrasyon:** `BiomeDefinition` içindeki `height_curve` ve `density_at` mantığı, TOML'dan yüklenen `DensityNode` ağacı ile değiştirilir. Hot path: `DensityNode::eval_sector` → batch buffer.

### 5.2 Block Type Belirleme

Density > 0 olan voxel'ler için blok tipi, yüzeye olan derinliğe göre belirlenir:

```rust
/// Katı voxel için blok tipi belirle.
/// Yüzeyden derinliğe göre: surface → subsurface → base.
fn block_type_for_density(
    density: f32,
    depth_from_surface: f32,
    biome: &BiomeDefinition,
    water_level: i32,
    world_y: i32,
) -> Option<u16> {
    if density <= 0.0 {
        // Hava veya su
        if world_y <= water_level {
            Some(WATER_BLOCK_ID)
        } else {
            None // hava
        }
    } else if depth_from_surface < 1.0 {
        Some(biome.surface_block)
    } else if depth_from_surface < 4.0 {
        Some(biome.subsurface_block)
    } else {
        Some(biome.base_block)
    }
}

/// Yüzeyden derinlik tahmini — surface voxel tespiti ile.
/// density > 0 VE yukarıdaki voxel hava (density <= 0) ise surface block.
/// Aşağı doğru sayım ile depth hesaplanır.
///
/// NOT: Eski `estimate_depth(density)` YANLIŞTI — density değeri surface
/// depth'i değildir (density sadece katılık gösterir, yüzeye uzaklığı değil).
/// Doğru yaklaşım: komşu voxel'lerden surface tespit et.
fn estimate_depth_surf(
    density_volume: &[f32; 32 * 32 * 32],
    lx: usize, ly: usize, lz: usize,
) -> f32 {
    // Surface block mu? (kendisi dolu, üstü hava)
    if ly + 1 < 32 {
        let above = density_volume[((ly + 1) * 32 + lz) * 32 + lx];
        if above <= 0.0 {
            return 0.0; // surface
        }
    }
    // Değilse: aşağı doğru hava bulana kadar say (max 4)
    let mut depth = 1.0;
    for dy in 1..4 {
        if ly as i32 - dy < 0 { break; }
        let below = density_volume[((ly - dy) * 32 + lz) * 32 + lx];
        if below <= 0.0 { break; }
        depth += 1.0;
    }
    depth
}
```

### 5.3 Sektör Üretim Pipeline (Batch + Column Cache)

```rust
/// Tek bir 32³ sektörün üretimi — OPTİMİZE EDİLMİŞ.
/// Tüm noise kanalları batch (§3.1), biome column cache (§4.2.1).
pub fn generate_sector_voxels(
    coord: SectorCoord,
    ctx: &WorldGenContext,
) -> SectorVoxelData {
    let origin = coord.world_origin_voxel();

    // 1. Batch noise — tüm 3D kanallar tek SIMD çağrısıyla
    let mut terrain = [0f32; 32 * 32 * 32];
    let mut detail = [0f32; 32 * 32 * 32];
    let mut cave   = [0f32; 32 * 32 * 32];
    let mut erosion= [0f32; 32 * 32 * 32];
    fill_sector_3d(&ctx.noise.terrain_density, &mut terrain, origin, ctx.seed.0);
    fill_sector_3d(&ctx.noise.detail, &mut detail, origin, ctx.seed.0);
    fill_sector_3d(&ctx.noise.cave_noise, &mut cave, origin, ctx.seed.0);
    fill_sector_3d(&ctx.noise.erosion, &mut erosion, origin, ctx.seed.0);

    // 2. Column cache — sadece 1024 biome sorgusu (32× tekrar yok)
    let biome_cols = cache_biome_columns(origin, ctx);

    // 3. Density kompozisyonu — batch buffer'dan, heap-free
    let mut density = [0f32; 32 * 32 * 32];
    compose_sector_density(&mut density, &terrain, &detail, &cave, &erosion, &biome_cols, origin);

    // 4. Blok tipi — gradient tabanlı depth (§5.2)
    let mut blocks = vec![0u16; 32 * 32 * 32];
    for i in 0..(32 * 32 * 32) {
        let lx = (i % 32) as usize;
        let ly = ((i / 32) % 32) as usize;
        let lz = (i / (32 * 32)) as usize;
        let wy = origin.y + ly as i32;
        let biome = &ctx.biome_registry.get(biome_cols[(lz * 32 + lx) as usize].biome);

        if density[i] > 0.0 {
            let depth = estimate_depth_surf(&density, lx, ly, lz);
            let block_id = block_type_for_density(
                density[i], depth, biome, biome.water_level, wy,
            );
            blocks[i] = block_id.unwrap_or(WATER_BLOCK_ID);
        } else if wy <= biome.water_level {
            blocks[i] = WATER_BLOCK_ID;
        }
        // else: hava (0 = AIR)
    }

    SectorVoxelData { coord, blocks }
}
```

**Performans Karşılaştırması (tahmini):**

| Adım | Eski (tekil çağrı) | Yeni (batch) | Kazanç |
|------|-------------------|--------------|--------|
| Terrain 3D noise | 32768 × `get_noise_3d` | 1 × `gen_uniform_grid_3d` | **~8×** |
| Biome 2D noise | 32768 × 2 çağrı | 1024 × 2 (column) | **~32×** |
| Density compose | inner-loop hesaplama | batch buffer | **~5×** |
| **Toplam sektör gen** | ~5-10 ms | **~0.5-1 ms** | **~10×** |

---

## 6. Mağara Sistemi (Hybrid)

Araştırma sonuçları (Diva-Portal cave study, 2024): 3D noise isosurface'ler chunk-bağımsız üretim için en uygun; hücresel otomata daha doğal görünüm sağlar ancak komşu verisi gerektirir (sektör bağımsızlığını bozar). Hibrit yaklaşım her iki dünyanın en iyisini sunar.

```rust
/// Mağara density'si — terrain density'den çıkarılır.
/// İki katman: isosurface (geniş boşluklar) + worm (kıvrımlı tüneller).
pub fn cave_density(
    x: i32, y: i32, z: i32,
    noise: &NoisePipeline,
) -> f32 {
    // Katman 1: 3D noise isosurface — geniş mağara boşlukları
    // Eşik değeri: ~0.55 → %15-20 hacim (ayarlanabilir)
    let iso = noise.cave_noise.get_noise_3d(x as f32, y as f32, z as f32);
    let cave_iso = if iso > 0.55 { (iso - 0.55) * 8.0 } else { 0.0 };

    // Katman 2: Worm noise — kıvrımlı tünel modifikatörü
    // Cellular distance function ile tünel benzeri pattern
    let worm = noise.worm_noise.get_noise_3d(x as f32, y as f32, z as f32);
    let cave_worm = if worm > 0.6 { (worm - 0.6) * 5.0 } else { 0.0 };

    // Birleşik: iki sistemin birleşimi (max = daha geniş oyma)
    cave_iso.max(cave_worm)
}
```

**Derinlik modülasyonu:** Yüzeye yakın mağaralar daha az (collapse risk), derinde daha sık:

```rust
/// Y-bağımlı mağara yoğunluk çarpanı.
fn cave_depth_multiplier(world_y: i32) -> f32 {
    if world_y > 60 { 0.2 }       // Yüzey yakın: nadir
    else if world_y > 20 { 0.6 }   // Orta: ortalama
    else if world_y > -20 { 1.0 }  // Derin: tam
    else { 1.2 }                    // Çok derin: yoğun (lava mağaraları)
}
```

---

## 7. Termal Erozyon

Hidrolik erozyon (particle-based) global pass gerektirir ve chunk-bağımsızlığı bozar. Termal erozyon, noise-modulated eğim bazlı yaklaşım ile chunk-bağımsız kalır.

```rust
/// Termal erozyon — eğim bazlı noise modülasyonu.
/// Yüksek eğimli alanlarda density azaltılır (kayma efekti).
/// Vadiler ve sırtlar doğal olarak oluşur.
pub fn thermal_erosion(
    x: i32, y: i32, z: i32,
    noise: &NoisePipeline,
    biome: &BiomeDefinition,
) -> f32 {
    // Erozyon noise'u — düşük frekanslı, yumuşak
    let erosion_val = noise.erosion.get_noise_3d(
        x as f32, y as f32, z as f32
    );

    // Sıcak biyomlarda daha az erozyon (kuru toprak),
    // soğuk/nemli biyomlarda daha fazla (don-çözülme, yağmur)
    let erosion_strength = match biome.erosion_factor() {
        f if f < 0.3 => 0.0,  // Çöl: erozyon yok
        f => f * 0.5,
    };

    // Sadece belirli yükseklik aralığında etkili (yüzey yakını)
    let height_factor = 1.0 - ((y as f32 - biome.base_height).abs() / 32.0).min(1.0);

    erosion_val * erosion_strength * height_factor
}
```

---

## 8. Yapı Sistemi

### 8.1 Hash-Grid Yerleşim

Poisson disk sampling komşu araması gerektirir (sonsuz dünyada zor). Hash-grid, chunk-bağımsız ve deterministik bir alternatiftir.

```rust
/// Hash-grid yapı yerleştirici.
/// Dünyayı yapı hücrelerine böler (örn: 128×128 blok).
/// Her hücrede hash ile yapının olup olmadığı belirlenir.
pub struct StructurePlacer {
    pub definitions: Vec<StructureDefinition>,
    pub cell_size: i32,  // default 128 blok
}

impl StructurePlacer {
    /// Bir sektör için yapı adaylarını döndür.
    pub fn candidates_for_sector(
        &self,
        coord: SectorCoord,
        seed: WorldSeed,
    ) -> Vec<StructureCandidate> {
        let origin = coord.world_origin_voxel();
        let mut candidates = Vec::new();

        // Sektörün kapsadığı hücreleri bul
        let cell_min_x = origin.x / self.cell_size;
        let cell_max_x = (origin.x + 31) / self.cell_size;
        let cell_min_z = origin.z / self.cell_size;
        let cell_max_z = (origin.z + 31) / self.cell_size;

        for cx in cell_min_x..=cell_max_x {
            for cz in cell_min_z..=cell_max_z {
                let hash = wyhash_coord(seed.0, cx, 0, cz);
                let rng_val = (hash & 0xFFFF) as f32 / 65535.0;

                for def in &self.definitions {
                    if rng_val < def.spawn_chance {
                        // Yapı pozisyonu: hücre içinde hash-bazlı offset
                        let px = (hash >> 16) & 0x7F; // 0-127
                        let pz = (hash >> 24) & 0x7F;
                        let world_pos = IVec3::new(
                            cx * self.cell_size + px as i32,
                            0, // Y: surface height'tan hesaplanır
                            cz * self.cell_size + pz as i32,
                        );
                        candidates.push(StructureCandidate {
                            definition: def,
                            world_pos,
                            cell: (cx, cz),
                        });
                    }
                }
            }
        }
        candidates
    }
}
```

### 8.2 StructureDefinition

```rust
pub struct StructureDefinition {
    pub name: StringId,
    pub spawn_chance: f32,           // 0.0-1.0
    pub min_spacing_cells: i32,      // minimum hücre aralığı
    pub valid_biomes: Vec<BiomeId>,
    pub placement: StructurePlacement,
    pub template: StructureTemplate,
}

pub enum StructurePlacement {
    /// Yüzeyde (terrain surface'e snap).
    Surface,
    /// Yeraltında (min-max derinlik).
    Underground { min_depth: i32, max_depth: i32 },
    /// Su altında.
    Underwater,
    /// Herhangi (density threshold ile).
    Any,
}

/// Yapı template'i — blok dizisi + entity spawn'ları.
pub struct StructureTemplate {
    pub size: IVec3,
    /// Palette mapping (index → block_id).
    pub palette: Vec<u16>,
    /// Bloklar (flat array, palette index).
    pub blocks: Vec<u8>,
    /// Entity spawn noktaları (sandık, mob spawner vb.).
    pub entities: Vec<TemplateEntity>,
}
```

### 8.3 Cross-Sector Yapı Damgalama

Yapılar birden fazla sektöre taşabilir. `WorldGenContext`, komşu sektörlerin pool slotlarına yazım yapabilen bir context sağlar:

```rust
/// Yapı damgalama — template bloklarını sektörlere yaz.
fn stamp_structure(
    template: &StructureTemplate,
    world_origin: IVec3,
    ctx: &mut WorldGenContext,
) {
    for lz in 0..template.size.z {
        for ly in 0..template.size.y {
            for lx in 0..template.size.x {
                let world_pos = world_origin + IVec3::new(lx, ly, lz);
                let palette_idx = template.blocks[
                    (lz * template.size.y + ly) as usize * template.size.x as usize
                    + lx as usize
                ];
                if palette_idx == 0 { continue; } // 0 = air/skip

                let block_id = template.palette[palette_idx as usize];
                // WorldGenContext, pozisyonun ait olduğu sektörü bulur
                // ve o sektörün GlobalBrickPool + SectorPalette'ine yazar
                ctx.set_block_world(world_pos, block_id);
            }
        }
    }
}
```

---

## 9. Ağaç Sistemi

### 9.1 Template + Varyasyon

L-system gerçekçi ancak yavaş ve öngörülemez. Template bazlı yaklaşım hızlı, deterministik ve kolayca authoring yapılabilir.

```rust
pub enum TreeType {
    Oak,
    Spruce,
    Birch,
    Jungle,
    Acacia,
    DarkOak,
    Mangrove,
}

/// Ağaç template'i — gövde yüksekliği aralığı + yaprak pattern.
pub struct TreeTemplate {
    pub tree_type: TreeType,
    pub min_height: i32,
    pub max_height: i32,
    pub trunk_block: u16,
    pub leaf_block: u16,
    pub leaf_pattern: LeafPattern,
}

pub enum LeafPattern {
    /// Küresel yapraklar (meşe).
    Sphere { radius: i32 },
    /// Konik yapraklar (ladin).
    Cone { base_radius: i32, top_radius: i32 },
    /// Şemsiye (akasya).
    Umbrella { radius: i32, stem_height: i32 },
}

/// Ağaç üret — template + hash-bazlı varyasyon.
pub fn generate_tree(
    base_world: IVec3,
    template: &TreeTemplate,
    seed: WorldSeed,
    ctx: &mut WorldGenContext,
) {
    let hash = wyhash_coord(seed.0, base_world.x, base_world.y, base_world.z);
    let mut rng = Pcg32Rng::new(hash);

    // Varyasyon: yükseklik ±2
    let height_range = template.max_height - template.min_height;
    let height = template.min_height + (rng.next_u32() % (height_range as u32 + 1)) as i32;

    // Gövde
    for dy in 0..height {
        ctx.set_block_world(base_world + IVec3::new(0, dy, 0), template.trunk_block);
    }

    // Yapraklar (pattern'a göre)
    match template.leaf_pattern {
        LeafPattern::Sphere { radius } => {
            let center = base_world + IVec3::new(0, height - 1, 0);
            for dx in -radius..=radius {
                for dy in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx*dx + dy*dy + dz*dz <= radius*radius + radius {
                            let pos = center + IVec3::new(dx, dy, dz);
                            // Sadece hava olan yere yaprak koy
                            if ctx.get_block_world(pos).is_none() {
                                ctx.set_block_world(pos, template.leaf_block);
                            }
                        }
                    }
                }
            }
        }
        // Cone, Umbrella pattern'leri benzer şekilde...
        _ => { /* diğer pattern implementasyonları */ }
    }
}
```

### 9.2 Cross-Sector Ağaçlar

Ağaç tabanı bir sektörde, kanopisi komşu sektörde olabilir. `WorldGenContext::set_block_world` pozisyonun ait olduğu sektörü otomatik bulur:

```rust
impl WorldGenContext {
    /// Dünya koordinatına blok yaz — doğru sektörü otomatik bul.
    pub fn set_block_world(&mut self, world_pos: IVec3, block_id: u16) {
        let sector_coord = SectorCoord::from_world_voxel(world_pos);
        let local_pos = world_pos - sector_coord.world_origin_voxel();

        // Bu sektör henüz üretilmemişse, geçici buffer'a kaydet
        // Ana üretim pipeline sonunda buffer'lar ilgili sektör'lere merge edilir
        let sector_data = self.get_or_create_sector(&sector_coord);
        sector_data.set_block(local_pos, block_id, &mut self.palette_context);
    }
}
```

---

## 10. Ore (Maden) Üretimi

```rust
/// Ore damarı üretimi — density function modifikatörü.
pub struct OreModifier {
    pub block_id: u16,
    pub min_y: i32,
    pub max_y: i32,
    pub vein_size: u32,      // ortalama damar boyutu
    pub frequency: f32,       // 0.0-1.0 (ne sıklıkta)
    pub replace_target: u16,  // hangi bloğu değiştirir (genelde stone)
}

/// Ore damarı — 3D noise threshold + wyhash gate.
fn generate_ore_at(
    x: i32, y: i32, z: i32,
    ore: &OreModifier,
    seed: WorldSeed,
    noise: &NoisePipeline,
) -> Option<u16> {
    // Y aralığı kontrolü
    if y < ore.min_y || y > ore.max_y { return None; }

    // Hash gate: bu pozisyon bir ore damarının merkezine yakın mı?
    let hash = wyhash_coord(seed.0, x / 8, y / 8, z / 8);
    if (hash & 0xFF) as f32 / 255.0 > ore.frequency { return None; }

    // 3D noise ile damar şekli
    let ore_noise = noise.detail.get_noise_3d(
        x as f32 * 0.3, y as f32 * 0.3, z as f32 * 0.3
    );
    if ore_noise > 0.3 {
        Some(ore.block_id)
    } else {
        None
    }
}
```

---

## 11. Paralel Üretim (AsyncComputeTaskPool)

`tokio::spawn_blocking` yerine Bevy'nin `AsyncComputeTaskPool` (`03` §9.5) kullanılır:

```rust
use bevy::tasks::{AsyncComputeTaskPool, Task};
use futures_lite::future;

/// Paralel sektör üretim — Bevy task pool ile.
pub struct ParallelWorldGenerator {
    pub ctx: Arc<WorldGenContext>,
}

impl ParallelWorldGenerator {
    /// Birden fazla sektörü paralel üret.
    /// Her sektör bağımsız (chunk-independent ilkesi).
    pub fn spawn_generation_tasks(
        &self,
        coords: Vec<SectorCoord>,
    ) -> Vec<Task<GeneratedSector>> {
        let pool = AsyncComputeTaskPool::get();

        coords.into_iter().map(|coord| {
            let ctx = self.ctx.clone();
            pool.spawn(async move {
                // CPU-intensive: density function evaluation (32³ = 32K voxel)
                let voxel_data = generate_sector_voxels(coord, &ctx);

                // Yapı yerleştirme
                let mut with_structures = voxel_data;
                place_structures_in_sector(&mut with_structures, &ctx);

                // Ağaç yerleştirme
                place_trees_in_sector(&mut with_structures, &ctx);

                GeneratedSector {
                    coord,
                    data: with_structures,
                }
            })
        }).collect()
    }
}
```

---

## 12. ECS Entegrasyonu (WorldGenPlugin)

```rust
use bevy::prelude::*;

pub struct WorldGenPlugin;

impl Plugin for WorldGenPlugin {
    fn build(&self, app: &mut App) {
        app
            .insert_resource(WorldGenState::default())
            .add_event::<SectorGenerationRequest>()
            .configure_sets(
                Update,
                WorldSystems::Generation.after(WorldSystems::Streaming),
            )
            .add_systems(Update, (
                request_sector_generation
                    .in_set(WorldSystems::Generation),
                process_generated_sectors
                    .in_set(WorldSystems::Generation)
                    .after(request_sector_generation),
            ));
    }
}

/// World generation state — pending tasks ve tamamlanan sektörler.
#[derive(Resource, Default)]
pub struct WorldGenState {
    pub pending_tasks: Vec<(SectorCoord, Task<GeneratedSector>)>,
    pub completed: Vec<GeneratedSector>,
}

/// Streaming manager'dan gelen üretim talebi.
#[derive(Event)]
pub struct SectorGenerationRequest {
    pub coord: SectorCoord,
    pub priority: GenerationPriority,
}

/// Streaming event'lere tepki — WorldGen kaynaklı sektör taleplerini topla.
fn request_sector_generation(
    mut reader: EventReader<SectorGenerationRequest>,
    generator: Res<ParallelWorldGenerator>,
    mut state: ResMut<WorldGenState>,
) {
    let coords: Vec<SectorCoord> = reader.read()
        .map(|req| req.coord)
        .collect();

    if !coords.is_empty() {
        let tasks = generator.spawn_generation_tasks(coords);
        for (coord, task) in tasks.into_iter() {
            state.pending_tasks.push((coord, task));
        }
    }
}

/// Tamamlanan üretim task'larını ECS'e entegre et.
fn process_generated_sectors(
    mut state: ResMut<WorldGenState>,
    mut commands: Commands,
    mut pool: ResMut<GlobalBrickPool>,
    mut sector_map: ResMut<SectorMap>,
    block_registry: Res<BlockRegistry>,
) {
    // Tamamlanan task'ları topla (non-blocking poll)
    state.pending_tasks.retain(|(coord, task)| {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            state.completed.push(result);
            false // retain'dan kaldır
        } else {
            true // hâlâ bekliyor
        }
    });

    // Tamamlanan sektörleri ECS'e aktar
    for generated in state.completed.drain(..) {
        let (sector, palette) = generated.data.into_ecs_components(
            &mut pool,
            &block_registry,
        );

        let entity = commands.spawn((
            SectorEntity {
                coord: generated.coord,
                tier: Tier::Active,
            },
            sector,
            palette,
            SectorData(Arc::new(
                CompressedChunkData::snapshot_from_pool(/* ... */)
            )),
            SectorMeshState::default(),
            Transform::default(),
            Visibility::default(),
        )).id();

        // Streaming event gönder
        // (SectorLoaded { source: WorldGen } → physics, lighting, network)
    }
}
```

---

## 13. Streaming Entegrasyonu (`08-streaming.md`)

WorldGen, streaming pipeline'ın **kaynak**larından biridir (`08` §7 diyagram):

```
StreamingManager:
  1. Sektör gerekli (ChunkMap'te yok)
  2. Disk'te veri var mı?
     → Evet: disk'ten yükle (LoadSource::Disk)
     → Hayır: WorldGen talep et (LoadSource::WorldGen)
  3. WorldGen tamamlandığında:
     → SectorLoaded { source: WorldGen } event
     → Physics: collider oluştur (12)
     → Lighting: skylight propagation başlat (13)
     → Network: AOI subscription ekle (16)
```

**Frame bütçesi:** `08` §8 — `max_loads_per_frame = 2`. WorldGen task'ları bu bütçe dahilinde başlatılır.

**Spiral outward (`08` §6.4):** İlk girişte oyuncu merkezinden spiral sıra ile sektör üretimi.

---

## 14. Modding Hook'ları (`32-modding.md`)

### 14.1 WIT Interface (T1 Sandboxed WASM)

```wit
# wit/worldgen-hooks.wit
interface worldgen-hooks {
    /// Biome lookup override.
    on-biome-lookup: func(x: s32, z: s32, default-biome: u8) -> u8;

    /// Density function modifikatörü.
    on-density-compute: func(x: s32, y: s32, z: s32, base-density: f32) -> f32;

    /// Yapı yerleştirme hook'u.
    on-structure-place: func(cell-x: s32, cell-z: s32) -> option<u32>;
}
```

### 14.2 T2 Native Plugin

```rust
/// T2 native mod — özel density modifier kaydet.
pub trait DensityModifier: Send + Sync {
    fn modify(&self, x: i32, y: i32, z: i32, base: f32, biome: &BiomeDefinition) -> f32;
}

/// T2 native mod — özel yapı generator'ü kaydet.
pub trait StructureGenerator: Send + Sync {
    fn generate(&self, coord: SectorCoord, ctx: &WorldGenContext) -> Vec<StructureInstance>;
}

// Registry
app.world_mut()
    .resource_mut::<WorldGenModifiers>()
    .register_density_modifier(Box::new(MyCustomModifier));
```

---

## 15. Crate Organizasyonu

```
crates/
  world-gen/
    ├── mod.rs              ← WorldGenPlugin (Bevy Plugin)
    ├── seed.rs             ← WorldSeed, Pcg32Rng, wyhash_coord
    ├── noise.rs            ← NoisePipeline (fastnoise2 wrapper)
    ├── biome/
    │   ├── mod.rs          ← Biome sistemi entry point
    │   ├── definition.rs   ← BiomeDefinition (TOML-driven)
    │   ├── whittaker.rs    ← WhittakerDiagram lookup
    │   └── presets.rs      ← Default biome preset'leri
    ├── terrain/
    │   ├── mod.rs          ← TerrainGenerator
    │   ├── density.rs      ← density_at, height_curve_value
    │   ├── caves.rs        ← Hybrid cave: isosurface + worm
    │   ├── erosion.rs      ← Thermal erosion modifier
    │   └── ores.rs         ← Ore vein generation
    ├── structure/
    │   ├── mod.rs          ← Structure sistemi
    │   ├── definition.rs   ← StructureDefinition
    │   ├── placement.rs    ← Hash-grid StructurePlacer
    │   ├── template.rs     ← StructureTemplate + stamping
    │   └── templates/      ← RON/TOML template dosyaları
    ├── tree/
    │   ├── mod.rs          ← TreeGenerator
    │   ├── template.rs     ← TreeTemplate per type
    │   └── variation.rs    ← Hash-based random variation
    ├── context.rs          ← WorldGenContext (cross-sector write)
    └── parallel.rs         ← AsyncComputeTaskPool integration
```

---

## 16. Reddedilen Alternatifler

| Alternatif | Neden red |
|------------|-----------|
| Sütun iterasyonu (y: 0..N) | Sabit yükseklik, `06` cubic chunks ile uyumsuz |
| `BiomeMap` sabit Vec grid | Sonsuz dünya ile uyumsuz, bellek israfı |
| L-system ağaç | Yavaş, öngörülemez, chunk-independent değil |
| Hücresel otomata mağara | Komşu verisi gerektirir, sektör bağımsızlığını bozar |
| Hidrolik erozyon (particle) | Global pass, pahalı, chunk-independent değil |
| Poisson disk yapı yerleşim | Komşu araması, sonsuz dünyada zor |
| LCG RNG (Java tarzı) | Zayıf kalite, 3D'de görünür pattern |
| SDF tabanlı arazi | Pahalı per-voxel, biome kontrolü zor |
| `tokio::spawn_blocking` | Bevy ECS scheduler ile uyumsuz (`03` §9.5) |
| xoshiro256 RNG | 256-bit state, ihtiyaç fazlası |
| OpenSimplex2 (pure Rust) | 3-5× yavaş, SIMD yok |
| Voronoi biome | Determinizm zor, komşu sorgusu |
| 5+ parametre multi-noise biome | Karmaşık, debug zor |
| **Runtime node graph interpreter** | 20-30× yavaş (graph traversal + virtual calls); bunun yerine compile-time flatten (`DensityNode` enum, §5.1.1) |
| **Inner-loop tekil `get_noise_3d`** | SIMD underutilize, ~5-8× yavaş; batch API zorunlu (§3.1) |
| **`estimate_depth(density)` eski hali** | Density ≠ surface depth; gradient tabanlı `estimate_depth_surf` gerek (§5.2) |
| **Full GPU compute gen (Faz 1)** | Determinism riski (GPU float varyansı), PCIe readback stall; sadece Faz 2-3 client preview (§17.1) |
| **Hydraulic erosion (chunk-independent particle)** | Faz 1 için erken; termal erozyon yeterli. Faz 2-3'e ertelendi (§17.1) |

---

## 17. Referanslar

- Minecraft 1.18+ Noise Router — [Minecraft Wiki](https://minecraft.wiki/w/Noise_router)
- Veloren World Gen — [Veloren Docs](https://docs.veloren.net/veloren_world/gen/)
- Diva-Portal Cave Study (2024) — Simplex Noise vs 3D Cellular Automata
- Whittaker Biome Diagram — [Red Blob Games](https://www.redblobgames.com/maps/terrain-from-noise/)
- PCG Random — [pcg-random.org](https://www.pcg-random.org/)
- wyhash — Wang Yi's hash function
- FastNoise2 — [GitHub](https://github.com/Auburn/FastNoise2), [crates.io](https://crates.io/crates/fastnoise2)
- Strata anayasa: `03`, `05`, `06`, `07`, `08`, `32`

---

## 17.1 Faz 2-3: GPU Compute World-Gen (Gelecek Çalışma)

**Durum:** Şu anki CPU + `AsyncComputeTaskPool` mimarisi Strata için **EN İYİ** seçenek (determinism + `08-streaming` entegrasyonu). GPU compute gen yalnızca gelecek fazlar için değerlendirilir.

**Araştırma Bulguları (2024-2026):**
- NVIDIA GPU Gems Ch.1: GPU density evaluation ~260 blocks/s (eski GPU'da bile)
- Aokana 2025 (arXiv:2505.02017): SVDAG + GPU-driven rendering, 9× bellek azaltma
- REAC 2025 (Saber): GPU culling ~0.5ms, ancak "Requires huge refactoring", "2-frame latency"
- Async copy papers: PCIe full-duplex, buffer pool + fence sync şart

**GPU Gen'e Geçiş Şartları:**
1. **Determinism:** Server hâlâ CPU generate (validation). GPU sadece client-side LOD preview
2. **Hybrid model:** GPU density hesaplar, CPU block-type kararını verir (float varyansı block ID'yi etkilemez)
3. **Buffer pool:** `08` triple-buffer ile entegre (async copy queue)
4. **Readback minimizasyon:** GPU sadece `CompressedChunkData` snapshot üretir, XBrickMap pool'a yazmaz

**Risk:** GPU float sonuçları GPU modeline göre değişebilir (FastNoise2 FAQ: AVX2 vs SSE2). Strata server-authoritative olduğu için **client-side only** olmalı.

**Alternatif:** GPU sadece `06` §2.7 feedback loop ile görünen sektörleri generate eder, geri kalanı CPU'dan gelir. Karmaşık ama en verimli.

---

## 17.2 2024-2026 Araştırma Doğrulamaları

| Karar | Doğrulama | Kaynak |
|-------|-----------|--------|
| Batch noise (GenUniformGrid3D) | SIMD ~8× hızlanma, `GenSingle` VERY SLOW | FastNoise2 Wiki/FAQ |
| Column caching | ~10× hızlanma (2D noise tekrar) | Voxel Play 4, Minecraft `cache_2d` |
| Compile-time node flatten | Runtime interpreter 20-30× yavaş | Voxel Tools Performance docs |
| CPU async gen | GPU gen'den daha deterministik, streaming ile uyumlu | Strata `08` + `03` |
| Particle hydraulic erosion | Artık chunk-independent mümkün (2024) | VMV 2024/2025, Lirmm 2024 |
