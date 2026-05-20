# World Generation Plan — Strata

Minecraft (`1.18+`, Caves & Cliffs) referans alınarak, **en verimli ve optimize** şekilde
voxel dünya üretimi için Strata'ya özel tasarım.

---

## 1. Felsefe & Temel Prensipler

### 1.1. Neden 3D Density Function?

Minecraft **1.18 öncesi**: 2D heightmap + gürültü → overhangs ve 3D şekiller imkansız.
**1.18 sonrası**: 3D Perlin density function — her blok için `density(x,y,z) > 0` = dolu.

Bu yaklaşım:
- Overhangs,悬崖, mağara tavanları, doğal kemerler mümkün
- Tek pipeline'da hem terrain hem mağaralar
- Spline'lar ile biome-terrain bağlantısı

Strata için karar: **Faz 1'den itibaren 3D density function kullanılacak**, ancak Faz 1'de
yalnızca y-ekseninde bias eklenmiş 3D noise (overhangsiz, temel), Faz 2'de tam 3D.

### 1.2. fastnoise2 Neden En İyi Seçim?

| Kriter | fastnoise2 | pure Rust alternatifleri |
|--------|------------|--------------------------|
| SIMD (AVX2) | 624M points/s 2D Perlin | ~50-100M (rust-noise) |
| Node graph | SIMD pipeline'da fused | Ayrı ayrı generate + combine |
| FBM/Ridged | Built-in | Manuel implementasyon |
| Domain Warp | Built-in | Manuel |
| Thread safety | Full | Genelde full |

**Faz 1-2-3'te fastnoise2 devam**, Faz 6'da opsiyonel olarak Rust native noise'a geçiş
değerlendirilir (eğer wasm modding için portability gerekirse).

---

## 2. Noise Sistemi — Node Graph Tasarımı

### 2.1. Gerekli Noise Haritaları (Minecraft'ın 5 Parametresi)

Minecraft 6 parametre kullanır (depth hariç 5 horizontal noise):

| Parametre | Kullanım | Noise Tipi | Frekans | Oktav |
|-----------|----------|------------|---------|-------|
| **Continentalness** | Kara/ocean ayrımı, temel yükseklik | SuperSimplex FBM | 0.0015 | 4 |
| **Erosion** | Düz vs dağlık arazi | SuperSimplex FBM | 0.002 | 4 |
| **Weirdness (PV)** | Peak/valley, shattered terrain | Ridged FBM | 0.0025 | 3 |
| **Temperature** | Biome seçimi (sıcak/soğuk) | Value FBM | 0.003 | 3 |
| **Humidity** | Biome seçimi (kuru/yağışlı) | Value FBM | 0.003 | 3 |

**Strata için tasarruf:** İlk Faz 1'de 2 parametre (continentalness + erosion) yeterli.
Faz 2'de 5 parametreye çıkılır.

```rust
// Faz 1: Minimal noise nodes
let height = supersimplex()
    .fbm(0.5, 0.0, 4, 2.0)
    .build();
let detail = value()
    .fbm(0.5, 0.0, 3, 2.0)
    .build();

// Faz 2: Full 5-parameter node graph
// fastnoise2 node graph ile SIMD fused pipeline
fn build_full_noise_graph(seed: i32) -> NoiseGraph {
    let continental = supersimplex().fbm(0.5, 0.0, 4, 2.0).set_frequency(0.0015).build();
    let erosion = supersimplex().fbm(0.5, 0.0, 4, 2.0).set_frequency(0.002).build();
    let weirdness = super_simplex().ridged(0.5, 0.0, 3, 2.0).set_frequency(0.0025).build();
    let temperature = value().fbm(0.5, 0.0, 3, 2.0).set_frequency(0.003).build();
    let humidity = value().fbm(0.5, 0.0, 3, 2.0).set_frequency(0.003).build();
    NoiseGraph { continental, erosion, weirdness, temperature, humidity }
}
```

### 2.2. Domain Warp (Opsiyonel, Faz 3+)

Daha organik terrain için noise koordinatlarını warp etmek:

```
Terrain → Warp(input) → FBM noise
            ↑
         DomainWarp (ikinci bir noise ile X/Z kaydırma)
```

fastnoise2'de `fractional_brownian` + `domain_warp_gradient` ile yapılır.
Faz 3'te eklenir. Performans maliyeti: ~%15-20 daha yavaş noise, ~%30 daha iyi görsellik.

---

## 3. 3D Density Function — Terrain Sistemi

### 3.1. Matematik

```
density(x, y, z) = base_noise(x, z)
                  + height_bias(y)
                  + detail_noise(x, y, z)
                  + cave_noise(x, y, z)
                  + biome_offset(x, z, y)

block = density > 0 ? SOLID : AIR
```

### 3.2. Height Bias (Ana Terrain Şekli)

```
fn height_bias(y: f32, base_height: f32) -> f32 {
    // yüksek Y'lerde negatif bias → air
    // düşük Y'lerde pozitif bias → solid
    (base_height - y as f32) * 0.1
}
```

**Continentalness + Erosion ile base_height belirleme:**

```
base_height(x, z) = lerp(
    40.0,                           // ocean
    120.0,                          // high land
    continentalness(x, z)           // 0..1
) + erosion_offset(erosion(x, z))   // ±20
```

Spline haritası (Minecraft stilinde):

```
Continentalness:
  -1.0 .. -0.4  → Deep ocean (base_height ≈ 30)
  -0.4 .. -0.2  → Shallow ocean (base_height ≈ 45)
  -0.2 .. -0.1  → Beach (base_height ≈ 55)
  -0.1 ..  0.3  → Land (base_height ≈ 65-90)
   0.3 ..  0.5  → Hills (base_height ≈ 90-110)
   0.5 ..  1.0  → Mountains (base_height ≈ 110-140)

Erosion offset:
  -1.0 .. -0.7  → High offset (+20..+30) — jagged peaks
  -0.7 .. -0.3  → Moderate offset (+5..+15)
  -0.3 ..  0.3  → Neutral (0)
   0.3 ..  0.7  → Gentle valleys (-5..-10)
   0.7 ..  1.0  → Flat plains (-15..-25)
```

### 3.3. Detay Noise (3D)

Yüksek frekanslı 3D noise terrain yüzeyine organik detay ekler.
Faz 1'de sadece 2D heightmap kullanılır (düşük kalite).
Faz 2'de 3D noise eklenir.

```
detail_noise(x, y, z) = supersimplex_3d(x * 0.05, y * 0.05, z * 0.05)
                       .fbm(0.5, 0.0, 3, 2.0)
                       .gen() * 3.0  // ±3 blok oynama
```

### 3.4. Faz 1 için Basitleştirilmiş 3D Density

```rust
// Faz 1: heightmap tabanlı (hızlı, basit)
pub fn generate_chunk_faz1(chunk: &mut Chunk, noise: &TerrainNoise) {
    for x in 0..16 {
        for z in 0..16 {
            let wx = chunk.world_x() + x;
            let wz = chunk.world_z() + z;
            let height = noise.continental_and_erosion(wx, wz);

            for y in 0..256 {
                let density = height - y as f32;
                let block = if density > 0.0 {
                    // surface rules burada uygulanır
                    surface_block(density, height, y, biome)
                } else {
                    BlockId::AIR
                };
                chunk.set_block(x, y, z, block);
            }
        }
    }
}
```

### 3.5. Faz 2+ için Tam 3D Density

```rust
// Faz 2: gerçek 3D density — overhangs, cliffs mümkün
pub fn generate_chunk_faz2(chunk: &mut Chunk, graph: &NoiseGraph) {
    for x in 0..16 {
        for y in 0..256 {
            for z in 0..16 {
                let wx = (chunk.world_x() + x) as f32;
                let wy = y as f32;
                let wz = (chunk.world_z() + z) as f32;

                let continental = graph.continental_3d(wx, wy, wz);
                let erosion = graph.erosion_3d(wx, wy, wz);
                let weirdness = graph.weirdness_3d(wx, wy, wz);

                let base_h = base_height_from_continental(continental);
                let erosion_off = erosion_offset(erosion);
                let pv_off = peaks_valleys_offset(weirdness);

                let height_bias = (base_h + erosion_off + pv_off - wy) * 0.08;
                let detail = graph.detail_noise_3d(wx, wy, wz) * 2.0;
                let caves = cave_density(wx, wy, wz, &graph);

                let density = height_bias + detail + caves;

                if density > 0.0 && !cave_override(caves) {
                    chunk.set_block(x, y, z, surface_block_for_biome(y, height_bias, biome));
                }
            }
        }
    }
}
```

---

## 4. Biome Sistemi

### 4.1. Multi-Noise Biome Source (Minecraft Stili)

Minecraft'ın yaklaşımı: 6D parametre uzayı (T, H, C, E, W, D) + her biome'a
ait bir hyper-rectangle (aralık). Koordinat hangi aralığa düşüyorsa o biome.

**Strata için optimize edilmiş versiyon:**

```rust
pub struct Biome {
    pub id: u16,
    pub name: &'static str,
    // Hyper-rectangle in noise parameter space [min, max]
    pub temperature_range: (f32, f32),
    pub humidity_range: (f32, f32),
    pub continentalness_range: (f32, f32),
    pub erosion_range: (f32, f32),
    pub weirdness_range: (f32, f32),
    // Surface rules
    pub surface_top: BlockId,
    pub surface_filler: BlockId,   // top'un altındaki 2-4 blok
    pub deep_filler: BlockId,      // onun altı (genelde stone)
    pub filler_depth: u8,          // surface_filler kalınlığı
    pub ocean_block: Option<BlockId>,
    // Terrain params
    pub base_height_offset: f32,
    pub erosion_resistance: f32,
    pub tree_density: f32,         // 0.0 - 1.0
    pub tree_type: TreeType,
    pub cave_modifier: f32,        // mağara sıklığı çarpanı
}
```

### 4.2. Biome Selection (En Yakın Komşu)

```rust
pub fn select_biome(
    biomes: &[Biome],
    params: &NoiseParams,  // T, H, C, E, W
    depth: f32,
) -> &Biome {
    // Depth çok derinse → deep dark / cave biome
    if depth > 0.9 && params.continentalness > 0.8 {
        return &biomes[CAVE_BIOME_OFFSET..];
    }

    // 6D Euclidean distance — en yakın biome'u bul
    let mut best_dist = f32::MAX;
    let mut best_idx = 0;

    for (i, biome) in biomes.iter().enumerate() {
        if !biome.is_surface() { continue; }
        let dist = hypercube_distance(params, biome);
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }

    &biomes[best_idx]
}
```

**Performans optimizasyonu:** Faz 1'de tüm chunk için tek biome (merkez koordinattan).
Faz 2'de column-level biome (16×16 = 256 lookup/chunk).
Faz 3+ için block-level biome (65k lookup/chunk — pahalı, `select_biome` SIMD ile hızlandırılabilir).

### 4.3. Faz 1 Biome Seti (Minimal)

| Biome | Continentalness | Erosion | Surface | Color |
|-------|----------------|---------|---------|-------|
| DeepOcean | -1.0 .. -0.4 | any | Sand, Gravel | Koyu mavi |
| Ocean | -0.4 .. -0.2 | any | Sand | Mavi |
| Beach | -0.2 .. -0.1 | any | Sand | Açık sarı |
| Plains | -0.1 .. 0.4 | >0.3 | Grass | Açık yeşil |
| Forest | -0.1 .. 0.4 | <0.3 | Grass + Trees | Koyu yeşil |
| Desert | 0.0 .. 0.6 | >0.5 | Sand + Cactus | Açık sarı |
| Hills | 0.4 .. 0.7 | -0.3..0.3 | Grass/Stone | Gri-yeşil |
| Mountains | 0.7 .. 1.0 | <-0.3 | Stone | Gri |
| SnowyPeaks | 0.8 .. 1.0 | <-0.5 | Snow/Stone | Beyaz |

### 4.4. Surface Rules Sistemi

Minecraft'ın `surface_rule` sisteminden esinlenme:

```rust
pub enum SurfaceRule {
    /// Belirli biome'da belirli blok
    Block { biome: Option<BiomeId>, block: BlockId, conditions: Vec<SurfaceCondition> },
    /// Yüksekliğe bağlı blok
    HeightMap { min_y: i32, max_y: i32, block: BlockId },
    /// Noise'a bağlı blok (ör: badlands clay band)
    Noise { node: NoiseHandle, threshold: f32, below: BlockId, above: BlockId },
    /// Koşullu
    If { condition: SurfaceCondition, then: Box<SurfaceRule>, else: Box<SurfaceRule> },
    /// Sequence (ilk match)
    Sequence(Vec<SurfaceRule>),
}

pub enum SurfaceCondition {
    Biome(BiomeId),
    AboveSeaLevel,
    BelowSeaLevel,
    Steepness(f32),  // slope threshold
    And(Box<SurfaceCondition>, Box<SurfaceCondition>),
    Or(Box<SurfaceCondition>, Box<SurfaceCondition>),
}
```

**Faz 1:** Hardcoded `if-else` (mevcut terrain.rs'deki gibi).
**Faz 2:** Rule-based system (yukarıdaki gibi), serileştirilebilir.
**Faz 3+:** Data-driven (JSON/TOML ile tanımlanabilir, modding API'in parçası).

---

## 5. Mağara Sistemleri

### 5.1. Cheese Caves (Faz 1)

En basit mağara tipi. 3D Perlin noise threshold.

```
cave_density(x, y, z) = supersimplex_3d(x * 0.01, y * 0.01, z * 0.01)
                       .fbm(0.5, 0.0, 3, 2.0)
                       .gen()

// threshold üzeri = mağara
is_cave = cave_density > 0.4 && y < sea_level
```

**Optimizasyon:** Yalnızca `y < heightmap_top` olan kolonlarda 3D noise hesapla.
Air block'larda hesaplama yapma.

### 5.2. Spaghetti Caves (Faz 2)

3D noise'un edge'ini takip ederek uzun tüneller:

```
// Noise'un zero-crossing'ini takip et
spaghetti(x, y, z) = |perlin_3d(x * 0.008, y * 0.008, z * 0.008)|

// Thin threshold band
is_spaghetti = spaghetti > 0.35 && spaghetti < 0.45 && y < sea_level - 10
```

### 5.3. Noodle Caves (Faz 3)

Spaghetti'nin daha ince versiyonu:

```
noodle(x, y, z) = |perlin_3d(x * 0.015, y * 0.015, z * 0.015)|
is_noodle = noodle > 0.4 && noodle < 0.45 && y < sea_level - 20
```

### 5.4. Mağara Density Fusion

```rust
pub fn cave_system_density(x: f32, y: f32, z: f32, params: &BiomeParams) -> f32 {
    // Cheese caves ana
    let cheese = cheese_noise.gen_3d(x, y, z);
    let cheese_mask = smoothstep(0.35, 0.5, cheese);

    // Spaghetti caves (Faz 2+)
    let spaghetti = spaghetti_noise.gen_3d(x, y, z);
    let spaghetti_mask = 1.0 - (spaghetti - 0.4).abs() * 10.0;
    let spaghetti_mask = smoothstep(0.2, 0.8, spaghetti_mask);

    // Noodle caves (Faz 3+)
    let noodle = noodle_noise.gen_3d(x, y, z);
    let noodle_mask = 1.0 - (noodle - 0.425).abs() * 20.0;
    let noodle_mask = smoothstep(0.3, 0.7, noodle_mask);

    // Birleştir — maksimum değeri al
    cheese_mask.max(spaghetti_mask * params.cave_density)
               .max(noodle_mask * params.cave_density * 0.5)
}
```

### 5.5. Aquifer Sistemi (Faz 3+)

Mağaraların suyla dolmasını kontrol eden sistem:

```rust
pub struct Aquifer {
    /// 16x40x16 hücrelerde lokal su seviyesi
    pub local_water_table: f32,
    /// Su mu lava mı (64x40x64 hücreler)
    pub fluid_type: FluidType,
    /// Bariyer yüksekliği (mağaralar arası)
    pub barrier_height: f32,
}

// Minecraft stili: lokal noise belirler su/lava/empty
pub fn aquifer_state(x: i32, y: i32, z: i32, aquifer_noise: f32, sea_level: i32) -> AquiferState {
    if y > sea_level { return AquiferState::Empty; }
    if y < -55 { return AquiferState::Lava; }  // deep lava

    match aquifer_noise {
        n if n < 0.4 => AquiferState::Empty,
        n if n > 0.8 => AquiferState::Flooded,
        _ => AquiferState::Local(local_water_level(x, y, z)),
    }
}
```

**Faz 1'de aquifer yok** — tüm mağaralar boş. Sadece deniz seviyesi altında su dolu.
**Faz 3'te basit aquifer** — local water table ile.
**Faz 4+** — tam Minecraft stili aquifer.

---

## 6. Yapılar & Feature Placement

### 6.1. Tree Placement — Poisson Disk (Faz 1+)

Mevcut `structure.rs` hash-based rastgele ağaç yerleşimi yeterli değil.
Daha iyi dağılım için:

```rust
pub struct PoissonTreePlacer {
    seed: u64,
    min_radius: f32,       // 4 blok
    max_radius: f32,       // 8 blok
    density_per_biome: HashMap<BiomeId, f32>,
}
```

Poisson disk sampling her chunk için bağımsız çalışır (seed = chunk_pos + world_seed).
Çok daha doğal ağaç dağılımı.

**Faz 1'de mevcut hash-based sistem yeterli.** Faz 2'de Poisson'a geçilir.

### 6.2. Cevher Damarları (Faz 2)

Minecraft'ın ore vein sisteminden:

```rust
pub struct OreVein {
    pub block: BlockId,
    pub filler_block: BlockId,  // granite/diorite/andesite
    pub min_y: i32,
    pub max_y: i32,
    pub density: f32,
    pub size: f32,              // vein büyüklüğü
    pub noise_frequency: f32,
}

// Her chunk'ta cevher damarlarını yerleştir
pub fn place_ores(chunk: &mut Chunk, veins: &[OreVein], noise: &NoiseGraph) {
    for x in 0..16 {
        for z in 0..16 {
            let vein_value = noise.ore_noise.gen_3d(
                (chunk.world_x() + x) as f32 * 0.01,
                0.0,
                (chunk.world_z() + z) as f32 * 0.01,
            );

            for y in 0..256 {
                let block = chunk.get_block(x, y, z);
                if block != BlockId::STONE { continue; }

                for vein in veins {
                    if y < vein.min_y || y > vein.max_y { continue; }
                    let density = noise.vein_noise.gen_3d(
                        (chunk.world_x() + x) as f32 * vein.noise_frequency,
                        y as f32 * vein.noise_frequency,
                        (chunk.world_z() + z) as f32 * vein.noise_frequency,
                    );
                    if density > vein.density {
                        chunk.set_block(x, y, z, vein.block);
                    }
                }
            }
        }
    }
}
```

### 6.3. Minecraft Stili Ore Distribution (Faz 2)

| Cevher | Min Y | Max Y | Density | Frekans | Vein Size |
|--------|-------|-------|---------|---------|-----------|
| Coal | 0 | 130 | 0.2 | 0.02 | 8 |
| Iron | -24 | 72 | 0.15 | 0.025 | 5 |
| Copper | -16 | 112 | 0.12 | 0.03 | 6 |
| Gold | -64 | 30 | 0.08 | 0.04 | 4 |
| Redstone | -64 | 15 | 0.1 | 0.035 | 3 |
| Diamond | -64 | 16 | 0.03 | 0.05 | 2 |
| Emerald | -16 | 32 | 0.01 | 0.06 | 1 |

---

## 7. Chunk Generation Pipeline — Performans

### 7.1. Pipeline Mimarisi

```
Player Position → View Distance (radius) → ChunkPos list
                                                    │
                              ┌─────────────────────┤
                              ▼                     ▼
                       Cache hit?             Cache miss?
                           │                       │
                           ▼                       ▼
                     Return cached           Queue generation
                              │
                              ▼
                    ┌─────────────────┐
                    │  Worker Thread   │  (rayon parallel)
                    │  ┌─────────────┐ │
                    │  │ Terrain     │ │  → 3D density noise
                    │  │ Caves       │ │  → cave noise
                    │  │ Surface     │ │  → surface rules
                    │  │ Features    │ │  → trees, ores
                    │  │ Light       │ │  → BFS propagation
                    │  │ Mesh        │ │  → greedy mesher
                    │  └─────────────┘ │
                    └─────────────────┘
                              │
                              ▼
                     ┌────────────────┐
                     │  Main Thread   │
                     │  Render queue  │
                     └────────────────┘
```

### 7.2. Threading Modeli

**Faz 1 - Single-threaded queue:**
- ChunkGenerator bir kuyruk, her frame'de 1-2 chunk işler.
- Yeterli: düşük yükte, henüz rendering yok.

**Faz 2 - Rayon parallel:**
```rust
pub fn generate_batch(positions: &[ChunkPos], pool: &ThreadPool) -> Vec<Chunk> {
    pool.install(|| {
        positions.par_iter()
            .map(|pos| generate_single(*pos))
            .collect()
    })
}
```

**Faz 3+ - Async worker (tokio):**
```rust
pub fn generate_chunk_async(pos: ChunkPos) -> impl Future<Output = Chunk> {
    tokio::task::spawn_blocking(move || generate_single(pos))
}
```

### 7.3. 3D Density Hesaplama Optimizasyonları

**1. Heightmap bounding box:**
```rust
// Faz 1: tüm 65k blok için 3D noise hesaplama
// ~65k noise calls / chunk

// Faz 2: heightmap bounding ile sınırla
// Her kolonda: min_y = heightmap_bottom - 5, max_y = heightmap_top + 5
// ~16*16*ortalama_30 = ~7680 noise calls / chunk (%88 daha az)
for x in 0..16 {
    for z in 0..16 {
        let col = Chunk::column_index(x, z);
        let min_y = chunk.heightmap_bottom[col].saturating_sub(5) as usize;
        let max_y = chunk.heightmap_top[col].saturating_add(5).min(255) as usize;
        for y in min_y..=max_y {
            // hesapla
        }
    }
}
```

**2. SIMD batch generation:**
fastnoise2 `gen_3d` tüm chunk için tek çağrıda batch:
```rust
let noise_values = noise_node.gen_3d(&x_coords, &y_coords, &z_coords, seed);
// 1 SIMD call = 65k density değeri
```

**3. Noise cache:**
```rust
// Aynı chunk komşularıyla örtüşen bölgeleri cache'le
// 3x3 chunk'lık bir bölgeyi tek seferde generate et
pub struct NoiseCache {
    cached_region: (i32, i32, i32),  // (chunk_x, chunk_y, chunk_z) alignment
    cache: Vec<f32>,                  // 48x48x48 noise values
}
```

### 7.4. Memory Access Pattern

**Chunk layout:** `Vec<u16>`, flat array `[x + z*16 + y*256]`

Generation loop için en verimli erişim:
```rust
// Y-external loop (cache-friendly)
// x + z*16 + y*256 → y değişince stride 256, sequential erişim
for x in 0..16 {
    for z in 0..16 {
        for y in (0..256).rev() {    // top-down, heightmap erken break
            let idx = x + z * 16 + y * 256;
            // ... hesapla, blocks[idx] = ...
        }
    }
}
```

### 7.5. Lazy Loading & Frame Throttling

```rust
// AGENTS.md'deki yapıya uygun
pub struct ChunkLoadManager {
    queue: PriorityQueue<ChunkPos>,
    chunks_loading: HashSet<ChunkPos>,
    chunks_per_tick: u8,
    max_concurrent: u8,     // rayon thread count
    view_distance: u32,
    load_distance: u32,     // view + 2 (buffer)
}

impl ChunkLoadManager {
    pub fn tick(&mut self, player_pos: IVec2, chunks: &mut World) {
        // 1. Player'a yakın chunk'ları sıraya ekle
        // 2. Uzak chunk'ları kaldır (unload)
        // 3. Batch generate (rayon)
        // 4. Mesh queue'ya ekle
    }
}
```

---

## 8. Faz 1 Implementation Detayı

### 8.1. Mevcut Kodun Güçlendirilmesi

Mevcut `noise.rs`, `terrain.rs`, `structure.rs` temel yapıyı doğru kurmuş.
Eksikler:

1. **Noise:**
   - Sadece 2 noise node (height + biome) → en az 5 node gerekli
   - Frekanslar çok yüksek (0.01) → daha düşük (0.0015-0.003) olmalı
   - FBM oktav sayısı 4 → yeterli

2. **Terrain:**
   - Sadece 2D heightmap → Faz 1 sonunda 3D density'e hazırlık
   - Surface rules hardcoded → enum tabanlı yapı
   - Hiç cave yok → basit cheese caves eklenmeli

3. **Biome:**
   - 4 biome, tek noise eşiği → parametrik multi-noise yapı

### 8.2. Faz 1 Değişiklik Önerisi

```rust
// crate: world-gen/src/noise.rs (güncellenmiş)
pub struct TerrainNoise {
    // Ana terrain noise
    continental: fastnoise2::SafeNode,    // kara/ocean
    erosion: fastnoise2::SafeNode,        // dağ/düz
    detail: fastnoise2::SafeNode,         // 3D detay

    // Biome noise
    temperature: fastnoise2::SafeNode,
    humidity: fastnoise2::SafeNode,

    // Cave noise (Faz 1: basit cheese)
    cave: fastnoise2::SafeNode,

    seed: i32,
}
```

```rust
// crate: world-gen/src/biome.rs (yeni)
pub struct BiomeRegistry {
    biomes: Vec<Biome>,
}

impl BiomeRegistry {
    pub fn default() -> Self { /* 9 biome */ }
    pub fn select(&self, continental: f32, erosion: f32, temp: f32, humidity: f32) -> &Biome;
    pub fn register(&mut self, biome: Biome);
}
```

```rust
// crate: world-gen/src/surface.rs (yeni)
// Surface rules engine
pub fn apply_surface(chunk: &mut Chunk, biome: &Biome, sea_level: i32);
```

### 8.3. Cargo.toml Güncellemesi

```toml
[dependencies]
strata-core = { path = "../core" }
fastnoise2.workspace = true
glam.workspace = true
rand.workspace = true
rayon.workspace = true      # YENİ: parallel iteration

[dev-dependencies]
divan = { workspace = true }
```

---

## 9. Performans Benchmark Planı

Her Faz sonunda hedef metrikler:

| Bileşen | Faz 1 | Faz 2 | Faz 3 | Faz 4+ |
|---------|-------|-------|-------|--------|
| Chunk gen (single) | <500µs | <200µs | <100µs | <50µs |
| Batch gen (64 chunk) | <32ms | <12ms | <6ms | <3ms |
| Noise calls/chunk | 65k | ~8k | ~8k | ~4k |
| RAM/chunk (blocks) | 128KB | 128KB | 128KB | 128KB |
| RAM/chunk (temp) | 0 | 256KB | 512KB | 512KB |
| Thread count | 1 | 4-8 | 8-16 | 8-16 |
| Cave types | 0 | 1 | 3 | 3 + aquifers |
| Biomes | 4 | 9 | 16+ | data-driven |

Benchmark aracı: `divan` (mevcut `benches/world_gen_bench.rs` geliştirilecek).

---

## 10. Kod Organizasyonu

```
crates/world-gen/src/
├── lib.rs                    # Pub exports
├── noise.rs                  # Noise graph + SafeNode yönetimi
├── biome.rs                  # Biome struct, registry, selection
├── terrain.rs                # TerrainGenerator (3D density)
├── surface.rs                # Surface rules engine (yeni)
├── cave.rs                   # Cave systems (cheese, spaghetti, noodle) (yeni)
├── structure.rs              # Tree/Ore/Feature placement
├── carver.rs                 # Geleneksel carvers (Faz 3+) (yeni)
├── aquifer.rs                # Aquifer sistemi (Faz 3+) (yeni)
├── generator.rs              # ChunkGenerator pipeline
├── load_manager.rs           # ChunkLoadManager (yeni)
└── config.rs                 # Noise parametreleri, biome tanımları (yeni)
```

---

## 11. Minecraft'dan Öğrenilen Dersler & Strata Farkları

| Minecraft | Strata (daha iyisi) |
|-----------|---------------------|
| Java, GC pressure | Rust, zero-cost abstractions, no GC |
| Block-level biome = pahalı | Column-level biome = 256x daha az lookup |
| 3D noise tüm chunk = 65k call | Heightmap bounding = ~8k call |
| Single-threaded world gen | Rayon parallel batch gen |
| Monolithic gen pipeline | Modular pipeline (her aşama ayrı trait) |
| Hardcoded biomes | Data-driven biome system (Faz 3+) |
| Aquifer her chunk = ağır | Lazy aquifer sadece cave olan chunk'larda |
| Surface rules complex | Rule engine with early-out optimization |
| No SIMD for noise | fastnoise2 AVX2 = 5-10x hızlı |
| No noise cache | 3x3 chunk region cache |

---

## 12. Riskler & Mitigasyon

| Risk | Olasılık | Çözüm |
|------|----------|-------|
| fastnoise2 C++ build sorunu | Düşük | Mevcut çalışıyor; fallback: FastNoise Lite |
| 3D density çok yavaş | Orta | Heightmap bounding, SIMD batch, column-level biome |
| Biome geçişleri keskin | Düşük | Smooth interpolation + blend zone |
| Mağara-terrain uyumsuzluğu (> 1 chunk) | Orta | 3x3 chunk noise cache, cross-chunk cave stitching |
| Memory (noise cache) | Düşük | LRU cache, max N regions |
| Determinism (multiplayer) | Orta | Seed-based her şey; floating point determinizmi için `enhanced-determinism` |

---

## 13. Özet Timeline

```
Faz 1 (Hafta 1-4):
├── 3 noise parametre (continental, erosion, detail)
├── 9 biome (hardcoded selection)
├── Heightmap-based terrain (3D density'e hazırlık)
├── Cheese caves (basit threshold)
├── Tree placement (hash-based)
├── Surface rules (hardcoded if-else)
├── ChunkGenerator queue (single-threaded)

Faz 2 (Hafta 5-8):
├── 5 noise parametre (T, H, C, E, W)
├── 16+ biome (data-driven)
├── 3D density function (overhangs, cliffs)
├── Spaghetti caves
├── Poisson disk tree placement
├── Ore veins
├── Surface rules engine (enum-based)
├── Rayon parallel batch gen
├── Heightmap bounding optimization

Faz 3 (Hafta 9-12):
├── Noodle caves
├── Domain warping
├── Carvers
├── Basit aquifer (local water table)
├── Noise cache (3x3 region)

Faz 4+ (Hafta 13+):
├── Full aquifer (Minecraft stili)
├── Data-driven biome/rule system
├── Biome-specific structures (villages, dungeons)
├── LOD system için multi-resolution noise
├── GPU compute terrain generation (opsiyonel)
```
