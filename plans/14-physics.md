# 06 — Fizik Entegrasyonu

## 1. Fizik Entegrasyonu (Rapier + Custom)

### 1.1 Rapier Voxels Shape — Güncel API

**Versiyon:** `rapier3d 0.32+` / `parry3d 0.26+` / `bevy_rapier3d 0.30+`

| Avantaj | Açıklama |
|---|---|
| **Düşük bellek** | Her voxel ~1 byte (neighborhood info) |
| **Ghost collision yok** | Internal edge tracking |
| **Otomatik blok gruplama** | Bitmask-based neighbor lookup, O(1) |
| **Sparse storage** | Boş bölgeler minimum bellek kaplar |
| **Incremental edit** | `set_voxel()` + `propagate_voxel_change()` |

#### Temel API

```rust
use bevy_rapier3d::prelude::*;
use glam::{IVec3, Vec3};

pub fn sector_to_voxels(sector: &Sector, voxel_size: Vec3) -> Collider {
    let occupied: Vec<IVec3> = sector
        .iter_occupied()
        .map(|p| IVec3::new(p.x, p.y, p.z))
        .collect();

    ColliderBuilder::voxels(voxel_size, &occupied).build()
}
```

#### VoxelState ve VoxelType (Parry 0.26)

```rust
pub enum VoxelType {
    Internal,   // Tüm 6 komşu dolu — collision check atlanır
    Surface,    // 1-5 komşu dolu — collision check gerekli
    Feature,    // Köşe veya kenar
}

pub struct VoxelState {
    pub filled: bool,
    pub free_faces: u8,
    pub voxel_type: VoxelType,
}
```

#### Desteklenen İşlemler

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

#### Gerçek Sınırlamalar (2026 Ocak)

| Özellik | Durum | Not |
|---|---|---|
| Static kinematic collider | ✅ Tam destek | Terrain için ideal |
| Dynamic rigid-body | ⚠️ Kısmi | Mass/inertia manuel hesaplanmalı |
| Voxels vs Capsule/Ball/Cuboid | ✅ Tam destek | Oyuncu vs terrain |
| Voxels vs Voxels | ⚠️ Parry 0.26'da düzeltildi | Force calculation arası çalışıyor |
| Voxels vs TriMesh | ✅ Çalışıyor | |
| Shape-casting (CCD) | ❌ Desteklenmiyor | |
| `set_voxel()` incremental edit | ✅ Çalışıyor | |
| `propagate_voxel_change()` | ✅ Çalışıyor | |
| `combine_voxel_states()` | ✅ Çalışıyor | Sector boundary merge |

**Strateji:** Aktif alan (Tier 1/2) için Rapier Voxels kullan. Voxel vs Voxel durumlar için **custom physics layer** kullan.

---

### 1.2 Broad-Phase Acceleration

Rapier 0.27.0'dan itibaren broad-phase **Dynamic BVH** tabanlı.

```rust
pub struct BvhBroadPhase {
    tree: Qbvh<ColliderHandle>,
    query_pipeline: QueryPipeline,
}
```

**Avantajlar:**
- **SIMD-accelerated traversal** — `wide` crate ile vectorized
- **Otomatik rebalancing** — collider hareket ettiğinde tree kendini dengeler
- **Tek acceleration structure** — broad-phase + scene queries aynı BVH'yi kullanır
- **Persistent islands** — simulation islands frame'ler arası persist olur

#### Sector-Level Spatial Hashing

```rust
pub struct PhysicsSpatialHash {
    cell_size: IVec3,
    cells: HashMap<SectorCoord, Vec<Entity>>,
    active_pairs: HashSet<(Entity, Entity)>,
}
```

#### Tier-Bazlı Broad-Phase Frekansı

| Tier | Broad-Phase | Frekans | Not |
|---|---|---|---|
| **ACTIVE** | Tam BVH traversal | Her frame (60Hz) | Tüm collider'lar |
| **WARM** | BVH + spatial hash prune | Her 3 frame (20Hz) | Sadece dinamik entity'ler |
| **DISTANT** | Sadece oyuncu query | Her 10 frame (6Hz) | Oyuncu vs terrain AABB |
| **ARCHIVE** | Yok | — | |

---

### 1.3 Incremental Collider Güncelleme

#### 3-Kademeli Güncelleme Stratejisi

```rust
impl Sector {
    pub fn update_collider(
        &mut self,
        collider: &mut Collider,
        changes: &[VoxelChange],
    ) {
        match changes.len() {
            0 => {}
            1..=8 => {
                if let Some(voxels) = collider.as_voxels_mut() {
                    for change in changes {
                        voxels.set_voxel(change.grid_pos, change.is_filled);
                        voxels.propagate_voxel_change(change.grid_pos);
                    }
                }
            }
            9..=64 => {
                self.rebuild_region(collider, changes);
            }
            _ => {
                *collider = Self::build_full_collider(self);
            }
        }
    }

    fn rebuild_region(&self, collider: &mut Collider, changes: &[VoxelChange]) {
        let aabb = Self::compute_changes_aabb(changes);

        if let Some(voxels) = collider.as_voxels_mut() {
            let (inside, outside) = voxels.split_with_box(&aabb);
            let new_inside = Self::build_region_voxels(self, &aabb);
        }
    }
}
```

#### Sector Boundary Sync

```rust
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
- **Tier 1 ↔ Tier 1:** Her frame sync
- **Tier 1 ↔ Tier 2:** Her 5 frame sync
- **Tier 2 ↔ Tier 2:** Her 15 frame sync
- **Tier 3+:** Sync yok

---

### 1.4 Character Controller Entegrasyonu

```rust
use bevy_rapier3d::prelude::*;

pub fn setup_character(mut commands: Commands) {
    commands
        .spawn(RigidBody::KinematicPositionBased)
        .insert(Collider::capsule_y(0.4, 0.8))
        .insert(Transform::default())
        .insert(KinematicCharacterController {
            offset: CharacterLength::Absolute(0.01),
            up: Vec3::Y,
            max_slope_climb_angle: 45_f32.to_radians(),
            min_slope_slide_angle: 30_f32.to_radians(),
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(1.0),
                min_width: CharacterLength::Absolute(0.6),
                include_dynamic_bodies: true,
            }),
            snap_to_ground: Some(CharacterLength::Absolute(0.5)),
            apply_impulse_to_dynamic_bodies: true,
            ..default()
        });
}
```

#### XBrickMap-Optimize Ground Check

```rust
impl CharacterController {
    pub fn ground_check_xbrickmap(
        &self,
        sector: &Sector,
        pos: Vec3,
        foot_radius: f32,
    ) -> GroundState {
        let grid_pos = Self::world_to_grid(pos);

        let slab_idx = (grid_pos.y >> 5) as usize;
        if sector.slabs[slab_idx].slab_mask == 0 {
            return GroundState::Air;
        }

        let brick_idx = Self::compute_brick_index(grid_pos);
        if sector.slabs[slab_idx].slab_mask & (1 << brick_idx) == 0 {
            return GroundState::Air;
        }

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

---

### 1.5 Custom Physics Layer

#### Kapsam

| Durum | Çözüm |
|---|---|
| Voxel vs Voxel collision | Custom spatial hash |
| Falling sand / gravel | Custom particle simulation |
| Explosion debris | Custom rigid-body spawn |
| Structural integrity | Custom stability check |
| Fluid simulation | Custom cellular automata |

#### Falling Sand / Gravel

```rust
pub struct FallingParticleSystem {
    particles: Vec<FallingParticle>,
    spatial_grid: SparseGrid<CellInfo>,
    sleep_manager: SleepManager,
}

pub struct FallingParticle {
    pub grid_pos: IVec3,
    pub velocity: Vec3,
    pub block_id: u16,
    pub mass: f32,
    pub settled: bool,
    pub settle_timer: f32,
}

impl FallingParticleSystem {
    pub fn simulate(&mut self, dt: f32, sector: &Sector) {
        self.sleep_manager.update(&mut self.particles, dt);

        for particle in self.particles.iter_mut() {
            if particle.settled { continue; }

            particle.velocity.y -= 9.81 * dt;
            let target_pos = particle.grid_pos + (particle.velocity * dt).as_ivec3();

            if sector.is_empty(target_pos) && self.spatial_grid.is_empty(target_pos) {
                particle.grid_pos = target_pos;
                particle.settled = false;
                particle.settle_timer = 0.0;
            } else {
                particle.velocity = Vec3::ZERO;
                particle.settled = true;
                particle.settle_timer += dt;

                if particle.settle_timer > 2.0 {
                    self.sleep_manager.sleep(particle);
                }
            }
        }

        self.spatial_grid.rebuild(&self.particles);
    }
}
```

#### Spatial Hash (Custom)

```rust
pub struct SparseSpatialHash<T> {
    cell_size: f32,
    cells: HashMap<IVec3, Vec<T>>,
}

impl<T> SparseSpatialHash<T> {
    fn hash(pos: IVec3) -> u64 {
        let x = pos.x as u64;
        let y = pos.y as u64;
        let z = pos.z as u64;
        (x * 73856093 ^ y * 19349663 ^ z * 83492791) % 0xFFFFFFFF
    }

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

### 1.6 Destruction & Fracture Sistemi

#### Hasar Birikimi

```rust
pub struct DamageSystem {
    damage_grid: SparseGrid<f32>,
    fracture_threshold: f32,
    damage_propagation: f32,
}

impl DamageSystem {
    pub fn apply_explosion(
        &mut self,
        sector: &mut Sector,
        center: Vec3,
        radius: f32,
        intensity: f32,
    ) {
        let grid_center = Self::world_to_grid(center);
        let grid_radius = (radius / VOXEL_SIZE).ceil() as i32;

        for dx in -grid_radius..=grid_radius {
            for dy in -grid_radius..=grid_radius {
                for dz in -grid_radius..=grid_radius {
                    let pos = grid_center + IVec3::new(dx, dy, dz);
                    let dist = (pos.as_vec3() - grid_center.as_vec3()).length();

                    if dist <= grid_radius as f32 {
                        let damage = intensity / (1.0 + dist * dist);
                        let current = self.damage_grid.get(pos).unwrap_or(0.0);
                        self.damage_grid.insert(pos, current + damage);

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

#### Voronoi Fracture

```rust
pub struct VoronoiFracture {
    voronoi_points: Vec<VoronoiPoint>,
    fragment_pool: ObjectPool<Fragment>,
}

pub struct Fragment {
    pub voxel_bounds: BoundingBox,
    pub voxel_count: u32,
    pub mass: f32,
    pub center_of_mass: Vec3,
    pub inertia_tensor: Mat3,
    pub collider: Option<Collider>,
}

impl VoronoiFracture {
    pub fn fracture_region(
        &mut self,
        sector: &mut Sector,
        region_aabb: BoundingBox,
        intensity: f32,
    ) -> Vec<Fragment> {
        let num_points = (intensity * 10.0) as usize;
        self.generate_voronoi_points(&region_aabb, num_points);
        let fragments = self.flood_fill_fragments(sector, &region_aabb);

        let mut result = Vec::new();
        for fragment in fragments {
            if fragment.voxel_count < 8 {
                self.spawn_debris_particles(&fragment);
                continue;
            }

            let physics_fragment = self.compute_physics(&fragment);
            result.push(physics_fragment);
        }

        self.remove_fractured_voxels(sector, &result);
        result
    }

    fn compute_physics(&self, fragment: &RawFragment) -> Fragment {
        let voxel_mass = VOXEL_SIZE.powi(3) * MATERIAL_DENSITY;
        let total_mass = fragment.voxel_count as f32 * voxel_mass;

        let com = fragment.voxel_positions.iter().sum::<Vec3>()
            / fragment.voxel_count as f32;

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

#### Fragment → Rigid-Body Spawn

```rust
pub fn spawn_fragments_as_rigidbodies(
    mut commands: Commands,
    fragments: Vec<Fragment>,
) {
    for fragment in fragments {
        let occupied = fragment.voxel_positions
            .iter()
            .map(|p| {
                IVec3::new(
                    ((p.x - fragment.voxel_bounds.min.x) / VOXEL_SIZE) as i32,
                    ((p.y - fragment.voxel_bounds.min.y) / VOXEL_SIZE) as i32,
                    ((p.z - fragment.voxel_bounds.min.z) / VOXEL_SIZE) as i32,
                )
            })
            .collect::<Vec<_>>();

        let collider = ColliderBuilder::voxels(
            Vec3::splat(VOXEL_SIZE),
            &occupied,
        ).build();

        commands
            .spawn(RigidBody::Dynamic)
            .insert(collider)
            .insert(Transform::from_translation(fragment.center_of_mass))
            .insert(Velocity::default())
            .insert(FragmentMetadata {
                mass: fragment.mass,
                voxel_count: fragment.voxel_count,
                lifetime: 30.0,
            });
    }
}
```

---

### 1.7 Physics Tier Management

| Tier | Fizik Detayı | Güncelleme Frekansı | Collider Tipi |
|---|---|---|---|
| **ACTIVE** (0-96m) | Tam Voxels + custom physics | Her frame (60Hz) | Rapier Voxels (full) |
| **WARM** (96-384m) | Voxels (static only) | Her 3 frame (20Hz) | Rapier Voxels (static) |
| **DISTANT** (384m-1.5km) | Yaklaşık AABB | Her 10 frame (6Hz) | Rapier Cuboid (AABB) |
| **ARCHIVE** (1.5km+) | Fizik yok | — | Collider yok |

#### Tier Geçişi Sırasında Fizik

```rust
impl Sector {
    pub fn update_physics_for_tier(
        &mut self,
        old_tier: Tier,
        new_tier: Tier,
        physics_world: &mut PhysicsWorld,
    ) {
        match (old_tier, new_tier) {
            (Tier::Active, Tier::Warm) => {
                self.freeze_dynamic_colliders(physics_world);
            }
            (Tier::Warm, Tier::Distant) => {
                self.simplify_to_aabb(physics_world);
            }
            (Tier::Distant, Tier::Archive) => {
                self.remove_collider(physics_world);
            }
            (Tier::Archive, Tier::Distant) => {
                self.create_aabb_collider(physics_world);
            }
            (Tier::Distant, Tier::Warm) => {
                self.rebuild_voxels_collider(physics_world);
            }
            (Tier::Warm, Tier::Active) => {
                self.activate_dynamic_colliders(physics_world);
            }
        }
    }
}
```

---

### 1.8 GPU Physics Vizyonu

Dimforge'un 2026 hedefi: **rust-gpu ile GPU physics**.

#### Mevcut Durum (2026 Ocak)

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

#### CPU/GPU Tradeoff

| Metrik | CPU Physics | GPU Physics |
|---|---|---|
| **Determinizm** | ✅ Tam deterministik | ⚠️ Floating point non-determinizm |
| **Gecikme** | Düşük (<1ms) | Yüksek (GPU dispatch + readback) |
| **Throughput** | ~5.000 body @ 60Hz | ~100.000+ body @ 60Hz |
| **Dinamik nesne** | Az sayıda (oyuncu, araçlar) | Çok sayıda (debris, particles) |
| **Network sync** | ✅ Ideal (deterministik) | ⚠️ Zor (non-deterministik) |

**Strata stratejisi:**
- **CPU physics:** Oyuncu, araçlar, dinamik entity'ler
- **GPU physics (gelecek):** Patlama debris, falling sand, büyük yığınlar

---

### 1.9 Performans Hedefleri (Fizik)

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

### 1.10 Crate Organizasyonu (Fizik)

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
    │   └── spatial_hash.rs ← Sparse spatial hash grid
    ├── destruction/
    │   ├── mod.rs          ← Destruction system
    │   ├── damage.rs       ← Damage accumulation
    │   ├── voronoi.rs      ← Voronoi fracture
    │   └── fragment.rs     ← Fragment → rigid-body spawn
    ├── tier.rs             ← Physics tier management
    └── gpu/
        ├── mod.rs          ← GPU physics abstraction
        └── backend.rs      ← PhysicsBackend trait (gelecek)
```
