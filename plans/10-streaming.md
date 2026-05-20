# 04 — 4-Tier Streaming Sistemi

## 1. Kademe Tanımları

| Tier | Ad | Mesafe | Veri Formatı | Render | Fizik |
|---|---|---|---|---|---|
| **1** | ACTIVE | 0-96m (~3 sector) | XBrickMap | Ray trace / Greedy mesh | Rapier Voxels collider |
| **2** | WARM | 96-384m (~3-12 sector) | XBrickMap + SVDAG | Brick öncelikli, SVDAG fallback | Rapier Voxels collider |
| **3** | DISTANT | 384m-1.5km | SVDAG only | GPU ray march | Yaklaşık collider |
| **4** | ARCHIVE | 1.5km+ | Compressed SVDAG (disk) | Render edilmez | Yok |

**Mesafe bazları:** Sector köşegeni ~132m. Tier 1 = 3×3×3 sector (yakın, düzenlenebilir). Tier 2 = yumuşak geçiş bölgesi.

## 2. Tier Geçiş Kuralları

```rust
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

## 3. Yumuşak Geçiş (Tier 2)

Tier 2'de **her iki representation birlikte** bulunur. Bu, pop-in'ı tamamen ortadan kaldırır:

```
Oyuncu uzaklaşıyor:
  Tier 1 → Tier 2:
    1. Brickmap hâlâ aktif (render + fizik)
    2. Arka planda GPU bake başlat (Brick → SVDAG)
    3. Bake bitti → sector.svdag_root = Some(root_index)
    4. Sector artık Tier 2'ye geçti

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

## 4. Predictive Streaming

```rust
pub struct StreamingPredictor {
    velocity: Vec3,
    acceleration: Vec3,
    look_direction: Vec3,
}

impl StreamingPredictor {
    pub fn predict_position(&self, current: Vec3) -> Vec3 {
        current + self.velocity * 2.0 + self.acceleration * 1.0
    }

    pub fn priority_sectors(&self, current: IVec3) -> Vec<(SectorCoord, f32)> {
        let predicted = self.predict_position(current.as_vec3());
        let predicted_sector = SectorCoord::from_world(predicted.as_ivec3());

        let mut sectors = Vec::new();
        for offset in SECTOR_RADIUS.iter() {
            let candidate = predicted_sector.0 + offset;
            let to_candidate = (candidate - predicted_sector.0).as_vec3().normalize();
            let alignment = to_candidate.dot(self.look_direction);

            let dist = (candidate - predicted_sector.0).length();
            let score = alignment * 0.6 + (1.0 - dist / MAX_RADIUS) * 0.4;

            sectors.push((SectorCoord(candidate), score));
        }

        sectors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        sectors
    }
}
```

## 5. Dünya Organizasyonu

```
World
  ├── HashMap<IVec3, Sector>     ← spatial hash, O(1) erişim
  │   └── Sector (32×128×32 = 131.072 voxel)
  │       ├── XBrickMap          ← 4-level hiyerarşik brick yapısı
  │       ├── SVDAG              ← uzak LOD için (Tier 2'den itibaren)
  │       ├── LightData[131072]  ← 16-bit packed light
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
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
pub struct SectorCoord(pub IVec3);

impl SectorCoord {
    pub fn from_world(pos: IVec3) -> Self {
        Self(IVec3::new(
            pos.x.div_euclid(32),
            pos.y.div_euclid(128),
            pos.z.div_euclid(32),
        ))
    }

    pub fn world_origin(&self) -> IVec3 {
        IVec3::new(
            self.0.x * 32,
            self.0.y * 128,
            self.0.z * 32,
        )
    }
}
```
