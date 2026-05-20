# 13 — World Generation Sistemi

## 1. Genel Bakış

Strata'nın prosedürel dünya üretimi **fastnoise2 FBM** tabanlı, **biome-driven** ve **structure-aware** bir sistemdir. Dünya üretimi **deterministik** olmalıdır (aynı seed = aynı dünya).

### Temel Prensipler

- **Deterministik:** Aynı seed + aynı koordinat = aynı sonuç
- **Chunk-bağımsız:** Her sector bağımsız üretilebilir (parallel)
- **Biome-driven:** Biome haritası terrain parametrelerini belirler
- **Structure-aware:** Yapılar (mağara, köy, dungeon) önceden tanımlanır
- **Lazy evaluation:** Sadece ihtiyaç duyulan sector'lar üretilir

---

## 2. World Seed & Determinism

```rust
/// Dünya seed'i ve üretim parametreleri.
pub struct WorldSeed {
    /// Ana seed (64-bit).
    pub seed: u64,

    /// Dünya boyutu (sınırsız = None).
    pub world_size: Option<WorldBounds>,

    /// Üretim versiyonu (generator migration için).
    pub generator_version: u32,
}

/// Deterministik random number generator.
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Split — alt-thread'ler için bağımsız RNG oluştur.
    pub fn split(&mut self, offset: u64) -> Self {
        Self {
            state: self.state ^ offset.wrapping_mul(0x5DEECE66D).wrapping_add(0xB),
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(0x5DEECE66D).wrapping_add(0xB);
        self.state
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
```

---

## 3. Noise Pipeline

```rust
/// Noise pipeline — birden fazla noise fonksiyonunu birleştirir.
pub struct NoisePipeline {
    /// Terrain yükseklik haritası (FBM).
    pub terrain_height: FastNoise2,

    /// Nem haritası (biome için).
    pub moisture: FastNoise2,

    /// Sıcaklık haritası (biome için).
    pub temperature: FastNoise2,

    /// Mağara noise'u (3D).
    pub cave: FastNoise2,

    /// Detay noise'u (yüzey varyasyonu).
    pub detail: FastNoise2,

    /// Biome haritası (2D).
    pub biome: FastNoise2,

    /// Yapı yerleşim noise'u.
    pub structure: FastNoise2,
}

impl NoisePipeline {
    /// Seed'den noise pipeline oluştur.
    pub fn from_seed(seed: u64) -> Self {
        let mut rng = SeededRng::new(seed);

        Self {
            terrain_height: FastNoise2::new(rng.next_u64())
                .fractal_type(FractalType::FBM)
                .frequency(0.005)
                .octaves(6)
                .build(),

            moisture: FastNoise2::new(rng.next_u64())
                .fractal_type(FractalType::FBM)
                .frequency(0.002)
                .octaves(4)
                .build(),

            temperature: FastNoise2::new(rng.next_u64())
                .fractal_type(FractalType::FBM)
                .frequency(0.002)
                .octaves(4)
                .build(),

            cave: FastNoise2::new(rng.next_u64())
                .noise_type(NoiseType::Simplex)
                .frequency(0.02)
                .octaves(3)
                .build(),

            detail: FastNoise2::new(rng.next_u64())
                .noise_type(NoiseType::Cellular)
                .frequency(0.05)
                .build(),

            biome: FastNoise2::new(rng.next_u64())
                .fractal_type(FractalType::FBM)
                .frequency(0.001)
                .octaves(3)
                .build(),

            structure: FastNoise2::new(rng.next_u64())
                .noise_type(NoiseType::Cellular)
                .frequency(0.0005)
                .build(),
        }
    }
}
```

---

## 4. Biome Sistemi

```rust
/// Biome tanımı.
pub struct BiomeDefinition {
    /// Benzersiz biome ID.
    pub id: BiomeId,

    /// Biome ismi.
    pub name: String,

    /// Sıcaklık aralığı.
    pub temperature_range: (f32, f32),

    /// Nem aralığı.
    pub moisture_range: (f32, f32),

    /// Yükseklik dağılımı (ortalama, varyans).
    pub height_distribution: (f32, f32),

    /// Yüzey blok tipi.
    pub surface_block: u16,

    /// Alt yüzey blok tipi.
    pub subsurface_block: u16,

    /// Temel blok tipi (derin).
    pub base_block: u16,

    /// Ağaç tipi (varsa).
    pub tree_type: Option<TreeType>,

    /// Bitki örtüsü.
    pub vegetation: Vec<VegetationEntry>,

    /// Su seviyesi.
    pub water_level: i32,

    /// Yağmur/kar yağışı.
    pub precipitation: PrecipitationType,
}

/// Biome ID (u8 = 256 max biome).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct BiomeId(pub u8);

/// Biome haritası — 2D grid.
pub struct BiomeMap {
    /// Biome grid (her hücre 16×16 blok).
    grid: Vec<BiomeId>,

    /// Grid boyutu (hücre cinsinden).
    grid_size: IVec2,

    /// Hücre boyutu (blok cinsinden).
    cell_size: i32,
}

impl BiomeMap {
    /// Koordinattaki biome'i bul (bilinear interpolation).
    pub fn get_biome(&self, x: i32, z: i32) -> BiomeId {
        let cell_x = x / self.cell_size;
        let cell_z = z / self.cell_size;

        // Bilinear interpolation ile komşu biome'ları karıştır
        let bx = cell_x.rem_euclid(self.grid_size.x as i32);
        let bz = cell_z.rem_euclid(self.grid_size.y as i32);

        self.grid[(bx + bz * self.grid_size.x) as usize]
    }
}
```

---

## 5. Terrain Generation

```rust
/// Terrain üreteci — biome + noise pipeline ile sector üretir.
pub struct TerrainGenerator {
    noise: NoisePipeline,
    biome_map: BiomeMap,
    block_registry: Arc<BlockRegistry>,
}

impl TerrainGenerator {
    /// Bir sector'ü üret.
    pub fn generate_sector(&self, coord: SectorCoord) -> Sector {
        let origin = coord.world_origin();
        let mut sector = Sector::empty();

        for x in 0..32 {
            for z in 0..32 {
                let world_x = origin.x + x;
                let world_z = origin.z + z;

                // Biome belirle
                let biome = self.biome_map.get_biome(world_x, world_z);
                let biome_def = self.biome_map.get_definition(biome);

                // Yükseklik hesapla
                let height = self.compute_height(world_x, world_z, biome_def);

                // Sütun üret
                self.generate_column(&mut sector, x, z, height, biome_def);
            }
        }

        // Mağara oyma
        self.carve_caves(&mut sector, origin);

        // Yapı yerleştirme
        self.place_structures(&mut sector, coord);

        sector
    }

    /// Yükseklik hesapla (biome + noise).
    fn compute_height(&self, x: i32, z: i32, biome: &BiomeDefinition) -> i32 {
        let base_height = self.noise.terrain_height.get_noise_2d(x as f32, z as f32);
        let (mean, variance) = biome.height_distribution;

        (mean + base_height * variance).round() as i32
    }

    /// Dikey sütun üret.
    fn generate_column(
        &self,
        sector: &mut Sector,
        x: i32,
        z: i32,
        height: i32,
        biome: &BiomeDefinition,
    ) {
        let water_level = biome.water_level;

        for y in 0..128 {
            let block_id = if y == 0 {
                biome.base_block // Bedrock
            } else if y < height - 4 {
                biome.base_block // Derin taş
            } else if y < height {
                biome.subsurface_block // Alt yüzey
            } else if y == height {
                biome.surface_block // Yüzey
            } else if y <= water_level {
                WATER_BLOCK // Su
            } else {
                AIR_BLOCK // Hava
            };

            if block_id != AIR_BLOCK {
                sector.set_block(IVec3::new(x, y, z), Some(block_id));
            }
        }
    }

    /// Mağara oyma (3D noise threshold).
    fn carve_caves(&self, sector: &mut Sector, origin: IVec3) {
        for x in 0..32 {
            for y in 0..128 {
                for z in 0..32 {
                    let world_x = origin.x + x;
                    let world_y = y;
                    let world_z = origin.z + z;

                    let cave_value = self.noise.cave.get_noise_3d(
                        world_x as f32,
                        world_y as f32,
                        world_z as f32,
                    );

                    // Threshold üstündeyse oyma
                    if cave_value > 0.4 {
                        sector.set_block(IVec3::new(x, y, z), None);
                    }
                }
            }
        }
    }
}
```

---

## 6. Structure Sistemi

```rust
/// Yapı tanımı.
pub struct StructureDefinition {
    /// Yapı ismi.
    pub name: String,

    /// Minimum mesafe (yapılar arası).
    pub min_spacing: i32,

    /// Yerleşim olasılığı (0-1).
    pub spawn_chance: f32,

    /// Uygun biome'lar.
    pub valid_biomes: Vec<BiomeId>,

    /// Yerleşim yüksekliği.
    pub placement: StructurePlacement,

    /// Yapı template'i.
    pub template: StructureTemplate,
}

pub enum StructurePlacement {
    /// Yüzeyde.
    Surface,

    /// Yeraltında (min-max derinlik).
    Underground { min_depth: i32, max_depth: i32 },

    /// Su altında.
    Underwater,

    /// Yüzeyde veya yeraltında.
    Any,
}

/// Yapı template'i — blok dizisi.
pub struct StructureTemplate {
    /// Template boyutu.
    pub size: IVec3,

    /// Bloklar (flat array).
    pub blocks: Vec<u16>,

    /// Entity'ler (sandık, mob spawner vb.).
    pub entities: Vec<TemplateEntity>,

    /// Palette mapping (index → block_id).
    pub palette: Vec<u16>,
}

/// Structure placer — yapıları dünya'ya yerleştirir.
pub struct StructurePlacer {
    definitions: Vec<StructureDefinition>,
    noise: FastNoise2,
}

impl StructurePlacer {
    /// Bir sector'de yapı yerleştir.
    pub fn place_in_sector(
        &self,
        sector: &mut Sector,
        coord: SectorCoord,
        biome_map: &BiomeMap,
    ) {
        for def in &self.definitions {
            // Yerleşim uygun mu?
            if !self.should_place(def, coord, biome_map) {
                continue;
            }

            // Yapıyı yerleştir
            self.place_structure(sector, coord, def);
        }
    }

    fn should_place(
        &self,
        def: &StructureDefinition,
        coord: SectorCoord,
        biome_map: &BiomeMap,
    ) -> bool {
        // Noise ile deterministik olasılık
        let noise_val = self.noise.get_noise_2d(
            coord.0.x as f32,
            coord.0.z as f32,
        );

        // Spacing kontrolü
        let spacing_ok = (noise_val * 1000.0).abs() > def.min_spacing as f32;

        // Chance kontrolü
        let chance_ok = noise_val < def.spawn_chance;

        // Biome kontrolü
        let center_biome = biome_map.get_biome(coord.0.x * 32, coord.0.z * 32);
        let biome_ok = def.valid_biomes.contains(&center_biome);

        spacing_ok && chance_ok && biome_ok
    }
}
```

---

## 7. Tree Generation

```rust
/// Ağaç tipi.
pub enum TreeType {
    Oak,
    Spruce,
    Birch,
    Jungle,
    Acacia,
    DarkOak,
    Mangrove,
}

/// Ağaç üreteci.
pub struct TreeGenerator {
    rng: SeededRng,
}

impl TreeGenerator {
    /// Ağaç üret (L-system benzeri).
    pub fn generate(
        &self,
        sector: &mut Sector,
        pos: IVec3,
        tree_type: TreeType,
    ) {
        match tree_type {
            TreeType::Oak => self.generate_oak(sector, pos),
            TreeType::Spruce => self.generate_spruce(sector, pos),
            TreeType::Birch => self.generate_birch(sector, pos),
            // ...
        }
    }

    fn generate_oak(&self, sector: &mut Sector, base: IVec3) {
        let height = 4 + (self.rng.next_u64() % 3) as i32; // 4-6

        // Gövde
        for y in 0..height {
            sector.set_block(base + IVec3::new(0, y, 0), Some(OAK_LOG));
        }

        // Yapraklar
        let leaf_start = height - 2;
        let leaf_radius = 2;

        for dy in 0..3 {
            let radius = if dy == 0 { leaf_radius } else { leaf_radius - 1 };
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() + dz.abs() <= radius + 1 {
                        let leaf_pos = base + IVec3::new(dx, leaf_start + dy, dz);
                        if sector.get_block(leaf_pos).is_none() {
                            sector.set_block(leaf_pos, Some(OAK_LEAVES));
                        }
                    }
                }
            }
        }
    }
}
```

---

## 8. Parallel Generation

```rust
/// Paralel dünya üreteci.
pub struct ParallelWorldGenerator {
    terrain: Arc<TerrainGenerator>,
    pool: tokio::runtime::Handle,
}

impl ParallelWorldGenerator {
    /// Birden fazla sector'ü paralel üret.
    pub async fn generate_sectors(
        &self,
        coords: Vec<SectorCoord>,
    ) -> Vec<(SectorCoord, Sector)> {
        let terrain = self.terrain.clone();

        let tasks: Vec<_> = coords.into_iter().map(|coord| {
            let terrain = terrain.clone();
            tokio::task::spawn_blocking(move || {
                let sector = terrain.generate_sector(coord);
                (coord, sector)
            })
        }).collect();

        let results = futures::future::join_all(tasks).await;
        results.into_iter().filter_map(|r| r.ok()).collect()
    }
}
```

---

## 9. Crate Organizasyonu

```
crates/
  world-gen/
    ├── mod.rs              ← World generation plugin entry point
    ├── seed.rs             ← WorldSeed, SeededRng
    ├── noise.rs            ← NoisePipeline, fastnoise2 wrapper
    ├── biome/
    │   ├── mod.rs          ← Biome sistemi
    │   ├── definition.rs   ← BiomeDefinition
    │   ├── map.rs          ← BiomeMap
    │   └── presets.rs      ← Vanilla biome preset'leri
    ├── terrain/
    │   ├── mod.rs          ← TerrainGenerator
    │   ├── column.rs       ← Dikey sütun üretimi
    │   ├── caves.rs        ← Mağara oyma
    │   └── decorator.rs    ← Yüzey dekorasyonu
    ├── structure/
    │   ├── mod.rs          ← Structure sistemi
    │   ├── definition.rs   ← StructureDefinition
    │   ├── template.rs     ← StructureTemplate
    │   ├── placer.rs       ← StructurePlacer
    │   └── templates/      ← JSON template dosyaları
    ├── tree/
    │   ├── mod.rs          ← TreeGenerator
    │   └── types.rs        ← TreeType varyantları
    └── parallel.rs         ← Paralel üretim
```
