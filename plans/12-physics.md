# 12 — Fizik Entegrasyonu

## 1. Fizik Entegrasyonu (Rapier + Custom)

### 1.1 Rapier Voxels Shape — Güncel API

**Versiyon:** `bevy_rapier3d 0.34` / `rapier3d 0.32` / `parry3d 0.26` (Bevy 0.18.1 ile uyumlu, 14 May 2026).
> **Versiyon notu (2026-07):** `rapier3d 0.32` aslında `parry3d ^0.26`'ya bağlıdır — `parry3d 0.23` planın önceki halindeki yanlış referanstı (0.23 hiçbir zaman 0.32 ile eşleşmedi). Güncel stable: `rapier3d 0.34` / `parry3d 0.29` (Tem 2026). Parry 0.26+ ile: Voxels shape **sparse storage** kullanır (0.25+), `try_set_voxel` kaldırıldı (yerine `set_voxel` auto-resize), ve nalgebra→glam migration (`Point`→`Vector`, `Isometry`→`Pose`, `IVector` = glam `I32Vec3`). Voxels collider Rapier **0.25**'te eklendi.

| Avantaj | Açıklama |
|---|---|
| **Düşük bellek** | Her voxel ~1 byte (neighborhood info) |
| **Ghost collision yok** | Internal edge tracking |
| **Otomatik blok gruplama** | Bitmask-based neighbor lookup, O(1) |
| **Sparse storage** | Boş bölgeler minimum bellek kaplar |
| **Incremental edit** | `set_voxel()` + `propagate_voxel_change()` |

> **Performans notu:** Rapier docs açıkça belirtiyor — *"performance scales with the number of voxels"*. 32³ × 100+ aktif sektör = 3M+ voxel. Bu nedenle WARM tier'da INTERIOR voxel'ler collider'dan çıkarılmalı (bkz. §1.1a).

#### Temel API

```rust
use rapier3d::prelude::*;
use parry3d::math::{Vector, IVector}; // parry 0.26+ glam migration: IVector = I32Vec3

pub fn sector_to_voxels(sector: &Sector, voxel_size: Vector) -> Collider {
    // Yalnızca DOLU voxel grid koordinatları — Voxels shape 0.25+ sparse storage kullanır.
    let occupied: Vec<IVector> = sector
        .iter_occupied()
        .map(|p| IVector::new(p.x, p.y, p.z))
        .collect();

    ColliderBuilder::voxels(voxel_size, &occupied)
        .position(Pose::translation(
            sector_origin.x, sector_origin.y, sector_origin.z,
        ))
        .build()
}
```

#### VoxelState ve VoxelType

> `VoxelType` ve `VoxelState` Parry tarafından sağlanır (`parry3d::shape::{VoxelState, VoxelType, AxisMask}`). Bunlar **gerçek Rust tipleridir** (u8 encode DEĞİL): `VoxelState` private-field struct + `INTERIOR`/`EMPTY` const, `VoxelType` bir enum (`Empty, Vertex, Edge, Face, Interior`), `AxisMask` bir struct. `VoxelState::INTERIOR` (tüm komşular dolu) ve `VoxelState::EMPTY` sabitleri mevcuttur. `free_faces()` → `AxisMask`, `voxel_type()` → `VoxelType`, `is_empty()` → `bool` döndürür (hepsi `const fn`). Plan kendi `VoxelState` struct'ını tanımlamamalı, Parry'ninkini kullanmalı.

```rust
use parry3d::shape::{Voxels, VoxelState, VoxelType, AxisMask};

// Örnek: bir voxel'ın durumunu sorgula
let state = voxels.voxel_state(IVector::new(0, 0, 0)).unwrap();
assert!(!state.is_empty());
let free = state.free_faces(); // AxisMask — hangi yüzler açık
if free.contains(AxisMask::X_NEG) {
    // -X yüzü açık (hava ile temas)
}
// Interior ⟺ tüm 6 komşu dolu ⟺ free_faces().is_empty()
// voxel_type() != Interior  VE  free_faces().is_empty()  eşdeğerdir;
// voxel_type() biraz daha ucuz (AxisMask allocate etmez).
```

#### Desteklenen İşlemler

| İşlem | API | Kompleksite |
|---|---|---|
| Voxel ekle/kaldır | `set_voxel(key, is_filled) -> Option<VoxelState>` | O(1) in-bounds; O(N) out-of-bounds auto-resize |
| Neighborhood yayma (çapraz-collider) | `propagate_voxel_change(&mut other, voxel, origin_shift)` | O(1) lokal |
| Neighborhood birleştirme | `combine_voxel_states(&mut other, origin_shift)` | O(1) lokal (full-face) |
| Voxel durumu sorgula | `voxel_state(key) -> Option<VoxelState>` | O(1) hashmap |
| AABB'deki voxel'ler | `voxels_intersecting_local_aabb()` | O(K) |
| Mesh'e dönüştür | `to_trimesh()` | O(N) (optimizasyon yok, debug only) |
| Outline'a dönüştür | `to_outline()` | O(N) |
| Bölge kırp | `crop(mins, maxs)` / `split_with_box(&Aabb) -> (Opt, Opt)` | O(N) |

> **`propagate_voxel_change` imzası:** `propagate_voxel_change(&mut self, other: &mut Voxels, voxel: IVec3, origin_shift: IVec3)`. `combine_voxel_states` de `origin_shift` alır. Komşu sektör çiftleri için bu argümanlar zorunludur (bkz. §1.3).
>
> **`split_with_box` MEVCUTTUR** (parry 0.26): `(Self, Self)` döndürür. Planın önceki hali "kaldırıldı" diyordu — YANLIŞ. Ancak edit hot-path'inde O(N) allocation yarattığı için bölgesel rebuild yerine çoklu `set_voxel` döngüsü kullanılır (bkz. §1.3). `crop` alt-bölge extract için uygundur.
>
> **`as_voxels_mut()` DİYE BİR METOT YOK.** `Collider` üzerinde `shape_mut()` → `as_mut_any().downcast_mut::<Voxels>()` ile `&mut Voxels` alınır (bkz. §1.3). `shape_mut()` çağrısı collider'ı `ColliderChanges::SHAPE` ile doğru şekilde flag'ler (broad-phase/AABB refresh).
>
> **`set_voxel` out-of-bounds maliyeti:** domain dışı insert O(N) realloc yapar. 32³ sector için `resize_domain` ile domain önceden sized edilmeli (allocation-free incremental edit).

#### Gerçek Sınırlamalar (2026 Temmuz)

| Özellik | Durum | Not |
|---|---|---|
| Static kinematic collider | ✅ Tam destek | Terrain için ideal |
| Dynamic rigid-body | ⚠️ Kısmi | Mass/inertia manuel hesaplanmalı (auto mass yok) |
| Voxels vs Capsule/Ball/Cuboid | ✅ Tam destek | Oyuncu vs terrain |
| Voxels vs Voxels | ⚠️ Kısmi | Dynamic-dynamic zayıf → custom layer (§1.5) |
| KCC `grounded` + Voxels | ⚠️ Bilinen bug | `grounded` ara sıra false dönebilir (rapier.js #327 / Rust shape-cast sınırı); XBrickMap OR workaround (§1.4) |
| Voxels vs TriMesh | ✅ Çalışıyor | |
| Shape-casting (CCD) | ✅ Destekleniyor | Rapier 0.32+ Voxels `RayCast` + `ccd_thickness` implement eder |
| `set_voxel()` incremental edit | ✅ Çalışıyor | |
| `propagate_voxel_change()` | ✅ Çalışıyor | |
| `combine_voxel_states()` | ✅ Çalışıyor | Sector boundary merge |

**Strateji:** Aktif alan (Tier 1/2) için Rapier Voxels kullan. Voxel vs Voxel durumlar için **custom physics layer** kullan.

#### 1.1a WARM Tier: Surface-Only Voxels (Optimizasyon)

WARM tier collider'ında INTERIOR voxel'ler (tüm 6 komşusu dolu) collision açısından gereksizdir — hiçbir dinamik cisim onlara temas etmez. `VoxelState::free_faces()` ile yalnızca en az bir açık yüzü olan voxel'ler collider'a basılır. Bu, WARM collider boyutunu 5-10× küçültebilir.

> **Nüans (2026 doğrulaması):** Parry zaten internal-edge problemini `VoxelType`/normal-cone mekanizmasıyla çözer; bir INTERIOR voxel, aynı sector içinde yüzeyde kayan bir cisim için zaten "ücretsizdir". INTERIOR'ı DROPMAK **narrow-phase temas maliyetini düşürmez** (surface temas maliyeti değişmez) — sadece **bellek**, **broad-phase/BVH leaf sayısı** ve **cross-sector edge bookkeeping** (daha az `propagate_voxel_change`) kazandırır. Planın "performance scales with number of voxels" ifadesi narrow-phase için yanıltıcı; scaling aslında *face/touched voxel* sayısıyla olur. Yine de WARM için surface-only doğru bir bellek/broad-phase optimizasyonudur.

```rust
use parry3d::math::{Vector, IVector};
use parry3d::shape::{Voxels, VoxelType};

pub fn sector_to_surface_voxels(voxels: &Voxels, voxel_size: Vector) -> Collider {
    let occupied: Vec<IVector> = voxels
        .voxels() // yalnızca dolu voxel'ler
        .filter(|v| v.state.voxel_type() != VoxelType::Interior)
        .map(|v| v.grid_coords)
        .collect();
    // En ucuz yol: interior-ness'i XBrickMap neighbor mask'inden derive edip
    // INTERIOR voxel'leri build anında hiç eklemeyebiliriz (Voxels lookup gerektirmez).
    ColliderBuilder::voxels(voxel_size, &occupied).build()
}
```

> ACTIVE tier'da TAM voxel seti kullanılmalı (hassas collision + boundary coupling için). Yalnızca WARM (ve ötesi) için surface-only uygundur.

---

### 1.2 Broad-Phase Acceleration

Rapier **0.27.0** (Tem 2025) ile eski QBVH / hierarchical-SAP broad-phase kaldırıldı; yerine Parry'nin yeni **BVH** implementasyonu geldi. `Qbvh<ColliderHandle>` artık yok — `DefaultBroadPhase` (iç `broad_phase_bvh.rs`) kullanılır. `QueryPipeline` ayrı güncellenmez; `broad_phase.as_query_pipeline()` ile ephemeral alınır.

**Avantajlar (Parry BVH):**
- **Incremental insert/remove** — tam rebuild yerine lokal güncelleme
- **Tek acceleration structure** — broad-phase + scene queries aynı BVH'yi kullanır
- **Persistent islands** — simulation islands frame'ler arası persist olur
- **Ray-cast performansı** — QBVH'ye göre belirgin şekilde daha hızlı, SIMD-accelerated tree traversal (Parry PR #361; resmi 4× rakamı bir benchmark'a bağlı, "significantly faster" olarak ifade edileli)

> Rapier'in `parallel` feature'ı broad-phase'i paralelleştirmez (yalnızca island solver). Strata determinizm gereği `parallel` kullanmaz (bkz. §1.12).

#### Collision Groups — Statik / Dinamik Ayrımı (Jolt dersi)

Jolt'un `NON_MOVING` / `MOVING` broad-phase katman felsefesi, Rapier'de `CollisionGroups` + `InteractionGroups` ile taklit edilir. Statik terrain collider'ları dinamik entity'lerle çarpışır; terrain-terrain çarpışması yapılmaz.

```rust
bitflags! {
    pub struct PhysicsLayer: u32 {
        const TERRAIN  = 0b0001; // Statik Voxels collider (sektör)
        const DYNAMIC  = 0b0010; // Oyuncu, araç, fragment
        const SENSOR   = 0b0100; // Trigger, AOI
        const DEBRIS   = 0b1000; // Kısa ömürlü fragment (DEBRIS ↔ DEBRIS kapalı)
    }
}

use rapier3d::geometry::InteractionGroups;
use parry3d::math::Group;

// InteractionGroups::new 3 ARGÜMAN alır (rapier 0.32): (memberships, filter, test_mode)
let terrain_groups = InteractionGroups::new(
    Group::from_bits_retain(PhysicsLayer::TERRAIN.bits()),
    Group::from_bits_retain(PhysicsLayer::DYNAMIC.bits() | PhysicsLayer::SENSOR.bits()),
    parry3d::geometry::InteractionTestMode::And, // veya Default::default()
);

// Terrain: yalnızca dinamik + sensör ile etkileş
ColliderBuilder::voxels(voxel_size, &occupied)
    .collision_groups(terrain_groups)
    .build();
```

> **Batch collider ekleme anti-pattern (2026 düzeltmesi):** `PhysicsColliderBatch` resource'u ile frame sonunda toplu ekleme **önerilmez**. Parry BVH broad-phase'i **incremental**'dir: `BroadPhaseBvh::update` yalnızca `modified_colliders`/`removed_colliders` üzerinde lokal edit yapar, asla tam rebuild yapmaz. Collider'ları streaming (§08) anında **hemen** ekle/çıkar; BVH zaten sadece değişenleri işler. Batch geciktirmek collider'ların bir frame geç görünmesine yol açar ve maliyeti düşürmez. Çok sayıda sector aynı anda yükleniyorsa jitter'ı streaming katmanında (§08 hybrid tiering) yayarak çöz — deferred batch ile değil.

#### Sector-Level Spatial Hash

Strata'nın kendi `PhysicsSpatialHash`'i Rapier broad-phase'e **ek** olarak dinamik entity'ler için kullanılır (custom layer, falling sand, debris proximity):

```rust
pub struct PhysicsSpatialHash {
    cell_size: IVec3,
    /// Power-of-two grid; per-frame allocation yok (§1.5 GridHash ile uyumlu)
    cells: GridHash<Entity>,
    active_pairs: HashSet<(Entity, Entity)>,
}
```

#### Tier-Bazlı Fizik Frekansı (Broad-phase değil, narrow-phase/solver throttle)

> **Önemli (2026 düzeltmesi):** Parry BVH broad-phase'i **zaten incremental** — hareket etmeyen (statik terrain) collider'lar için insert sonrası **sıfır** broad-phase maliyeti oluşur. Bu yüzden "broad-phase'i her 3/10 frame'de bir çalıştır" demek anlamsız; BVH yalnızca değişen collider'ları dokunur. Tier frekansı aslında **narrow-phase + constraint solver** (ve island extraction) seviyesinde uygulanmalı: uzak tier'lar için solver/contact-generation'ı atla veya düşür, broad-phase'i değil.

| Tier | Collider | Frekans | Not |
|---|---|---|---|
| **ACTIVE** | Tam Voxels | Her frame (60Hz) tam step | Tüm dinamik + statik |
| **WARM** | Surface-only Voxels (statik) | Statik collider BVH'de kalır; dinamik body'ler için solver'ı düşük frek/iterasyon | Terrain free; sadece o tier'daki dynamic body'ler |
| **DISTANT** | Surface-only Voxels (query-only) | Solver işi YOK; sadece on-demand scene query (KCC raycast, `as_query_pipeline`) | Oyuncu asla orada traverse etmez |
| **ARCHIVE** | Collider yok | — | Removal lokal BVH edit |

> **BVH optimizasyon:** `BvhOptimizationStrategy::SubtreeOptimizer` (default) sürekli mutating world için önerilir; `BroadPhaseBvh::with_optimization_strategy` ile set edilir (bevy_rapier plugin config üzerinden). `None` yalnızca debug.

#### Broad-Phase Profiling

Rapier `Counters` / debug profiler ile tier eşiklerini (96m / 384m / 1.5km) veriyle kalibre et. Jolt'un `JPH_TRACK_BROADPHASE_STATS` karşılığı: hangi tier kombinasyonunun en çok AABB testi ürettiğini ölç, §08 hysteresis eşiklerini buna göre ayarla. `pipeline.counters.collision_detection.broad_phase_time` / `narrow_phase_time` kullan.

---

### 1.3 Incremental Collider Güncelleme

#### 2-Kademeli Güncelleme Stratejisi

`split_with_box` MEVCUTTUR ama edit hot-path'inde O(N) allocation yarattığı için kullanılmaz; yerine `set_voxel` döngüsü. Threshold **keyfi 256 değil**, sektör doluluk fraction'ıdır (aşağıda).

> **`as_voxels_mut()` DİYE METOT YOK.** `Collider` → `shape_mut()` → `as_mut_any().downcast_mut::<Voxels>()`. `shape_mut()` collider'ı `ColliderChanges::SHAPE` ile flag'ler (broad-phase/AABB refresh doğru tetiklenir).

```rust
use parry3d::shape::Voxels;
use rapier3d::geometry::Collider;

const SECTOR_VOXELS: usize = 32 * 32 * 32;       // 32768
const FULL_REBUILD_FRACTION: f32 = 0.05;          // >%5 değişirse full rebuild
const FACE_COMBINE_THRESHOLD: usize = 64;         // bir yüzde yakın değişiklikse combine

/// Voxels'a &mut erişim (as_voxels_mut YOK).
fn voxels_mut(c: &mut Collider) -> &mut Voxels {
    c.shape_mut()
        .as_mut_any()
        .downcast_mut::<Voxels>()
        .expect("collider shape is Voxels")
}

pub fn apply_voxel_edits(
    colliders: &mut ColliderSet,
    sector_a: ColliderHandle,
    neighbor: Option<(ColliderHandle, IVector)>, // (handle, origin_shift a->b)
    edits: &[(IVector, bool)],                   // (grid_pos_in_A, filled?)
) {
    if edits.is_empty() { return; }

    // 1) TAMAMINI SET ET (propagate ile interleave ETME — ara durum maskeleri bozulur)
    let mut a = colliders.get_mut(sector_a).unwrap();
    let va = voxels_mut(&mut a);
    let mut boundary: Vec<IVector> = Vec::new();
    for &(k, filled) in edits {
        let prev = va.set_voxel(k, filled);
        // Sadece state gerçekten değiştiyse propagate gerekebilir
        if prev.is_empty() != !filled {
            if is_on_sector_boundary(k) { boundary.push(k); }
        }
    }
    drop(a);

    // 2) PROPAGATE (boundary voxel başına, VEYA tüm yüz için tek combine)
    if let Some((bh, shift)) = neighbor {
        if !boundary.is_empty() {
            let (mut a2, mut b2) = colliders.get_two_mut(sector_a, bh);
            let va = voxels_mut(&mut a2);
            let vb = voxels_mut(&mut b2);
            if boundary.len() > FACE_COMBINE_THRESHOLD {
                va.combine_voxel_states(vb, shift); // tüm yüz, bidirectional
            } else {
                for &k in &boundary {
                    va.propagate_voxel_change(vb, k, shift);
                }
            }
        }
    }

    // 3) Fraction threshold aşıldıysa → full rebuild (Voxels::new)
    if (edits.len() as f32) > FULL_REBUILD_FRACTION * (SECTOR_VOXELS as f32) {
        rebuild_full_collider(colliders, sector_a);
    }
}
```

> **Zorunlu sıra:** Önce TÜM `set_voxel`, SONRA `propagate_voxel_change`/`combine_voxel_states`. `set_voxel` komşu state'lerini `update_neighbors_state` ile günceller; interleave edilirse ara bir `propagate` henüz set edilmemiş komşuyu okuyup yanlış boundary mask üretebilir. `propagate_voxel_change` bir voxel + komşularıyla lokaldir → **boundary voxel başına** çağrılmalı; bir kez çağırmak çoklu edit'i kapsamaz. Tüm bir paylaşılan yüz değiştiyse tek `combine_voxel_states` daha ucuzdur.
>
> **`origin_shift`:** grid-koordinat offseti (voxel_size'ın tam katı), dünya mesafesi DEĞİL. `other`'daki `key` voxel'ı `self`'te `key + origin_shift` konumundadır.
>
> **Threshold gerekçesi:** Parry'nin `set_voxel` O(changed) + boundary propagate; full rebuild O(occupied). Cross-over ~ changed + propagations ≈ full re-derive. %5 (~1600 voxel) emprical eşik; `SECTOR_VOXELS` sabiti olarak tunable.

#### Sector Boundary Sync

```rust
pub fn sync_sector_boundaries(a: &mut Collider, b: &mut Collider, shift: IVector) {
    let va = voxels_mut(a);
    let vb = voxels_mut(b);
    va.combine_voxel_states(vb, shift);
}

/// Edit sonrası incremental bakım — combine_voxel_states'in lokal versiyonu
pub fn propagate_boundary_after_edit(
    edited: &mut Collider,
    neighbor: &mut Collider,
    changed_voxel: IVector,
    shift: IVector,
) {
    let va = voxels_mut(edited);
    let vb = voxels_mut(neighbor);
    va.propagate_voxel_change(vb, changed_voxel, shift);
}
```

**Sync stratejisi (proje politikası — Parry guidance değil, tunable const):**
- **Tier 1 ↔ Tier 1:** Her frame sync (veya her edit sonrası `propagate_boundary_after_edit`)
- **Tier 1 ↔ Tier 2:** Her 5 frame sync
- **Tier 2 ↔ Tier 2:** Her 15 frame sync
- **Tier 3+:** Sync yok

> Sektör geçişinde karakter takılmasını önlemek için `combine_voxel_states` unutulmamalı — bu Rapier Voxels'ın doğasından gelen zorunlu bir maliyettir.

---

### 1.4 Character Controller Entegrasyonu

```rust
use rapier3d::prelude::*;
use bevy::prelude::*;

pub fn setup_character(mut commands: Commands) {
    commands.spawn((
        RigidBody::KinematicPositionBased,
        Collider::capsule_y(0.4, 0.8),
        Transform::default(),
        KinematicCharacterController {
            offset: CharacterLength::Absolute(0.01),
            up: Vec3::Y,
            max_slope_climb_angle: 45_f32.to_radians(),
            min_slope_slide_angle: 30_f32.to_radians(),
            autostep: Some(CharacterAutostep {
                max_height: CharacterLength::Absolute(1.0),
                min_width: CharacterLength::Absolute(0.6),
                include_dynamic_bodies: true,
            }),
            snap_to_ground: Some(CharacterLength::Absolute(0.2)), // 0.5 yerine 0.2 — yarım metre pop önler
            apply_impulse_to_dynamic_bodies: true,
            ..default()
        },
    ));
}
```

#### XBrickMap-Optimize Ground Check (KCC tamamlayıcı)

> **Bilinen sorun:** Rapier KCC `grounded` (Rust: `KinematicCharacterControllerOutput.grounded`; JS adı `computedGrounded`, rapier.js #327) Voxels collider ile ara sıra yanlış `false` dönebilir — bu bir shape-cast sınırıdır (voxel collider çoklu collider + internal edge). KCC'nin move-and-slide, autostep ve snap-to-ground mantığı **devre dışı bırakılmamalı** — XBrickMap ground check bunun **tamamlayıcısı**dır (bkz. hysteresis notu aşağıda).

```rust
pub struct PlayerGroundState {
    pub kcc_grounded: bool,       // KinematicCharacterControllerOutput.grounded (NOT computedGrounded — o JS binding)
    pub xbrick_grounded: bool,    // XBrickMap bitmask skip
    pub slope_angle: f32,
}

impl PlayerGroundState {
    /// Oyuncu "yerde" sayılır: KCC VEYA XBrickMap (ikisi de false ise havadadır)
    pub fn is_grounded(&self) -> bool {
        self.kcc_grounded || self.xbrick_grounded
    }
}

pub fn ground_check_xbrickmap(
    sector: &Sector,
    pos: Vec3,
    foot_radius: f32,
) -> GroundState {
    let grid_pos = world_to_sector_grid(pos);

    let slab_idx = (grid_pos.y >> 5) as usize;
    if sector.slabs[slab_idx].slab_mask == 0 {
        return GroundState::Air;
    }

    let brick_idx = compute_brick_index(grid_pos);
    if sector.slabs[slab_idx].slab_mask & (1 << brick_idx) == 0 {
        return GroundState::Air;
    }

    let grounded = check_foot_contact(sector, grid_pos, foot_radius);

    if grounded {
        let slope = compute_slope_angle(sector, grid_pos);
        GroundState::Grounded { slope_angle: slope }
    } else {
        GroundState::Air
    }
}
```

**KCC ayarları (Voxels terrain için):**
- `offset`: `CharacterLength::Absolute(0.01)` — küçük ama sıfır değil (numerical stability)
- `snap_to_ground`: `Some(CharacterLength::Absolute(0.2))` — merdiven / yokuş aşağı (0.5 yerine 0.2: yarım metre pop önler)
- `normal_nudge_factor`: duvara takılmayı azaltır (Rapier 0.25+, default 1e-4)
- **Grounding hysteresis (ZORUNLU):** `KinematicCharacterControllerOutput.grounded` voxel shape-cast jitter'ında tek-frame `false` dönebilir (official `character_controller3` timer workaround). `kcc_grounded || xbrick_grounded` sonucunu ~100-150ms **coyote-time latch** ile tut; jump input'ı tek-frame false'dan düşürme. Slope açısını XBrickMap'den al (KCC'den re-cast etme). bevy_tnua dersi: coyote-time + jump-buffer + volume-probe (XBrickMap foot-AABB = Tnua `SensorShape` karşılığı).

---

### 1.5 Custom Physics Layer

#### Kapsam

| Durum | Çözüm | Not |
|---|---|---|
| Voxel vs Voxel collision | Custom spatial hash (`GridHash`) | Rapier Voxels dynamic-dynamic zayıf |
| Falling sand / gravel | Deterministik CA (velocity-based, chunked) | §22 ile hizalı; Salva **kullanılmaz** |
| Explosion debris | Rapier Dynamic + Voxels collider | §1.6 |
| Structural integrity | Custom stability check | |
| Fluid simulation (su/lav) | Deterministik CA (§22) | Server-authoritative; Salva client-only değil |

#### Salva Entegrasyonu — Reddedildi (2026-07)

| Kriter | Salva | Strata kararı |
|---|---|---|
| Sürüm uyumu | `rapier3d ^0.18` (2024, güncellenmedi) | ❌ Strata `0.32` — semver çakışması |
| Determinizm | SPH non-deterministik | ❌ Server-authoritative ile uyumsuz |
| Su simülasyonu | Gerçekçi SPH | ❌ §22 CA daha uygun (Minecraft tarzı öngörülebilirlik) |
| Falling sand | Granular SPH | ⚠️ CA yeterli; Faz 7+ opsiyonel client-only vizyon |

**Karar:** Salva entegre edilmez (2024'ten beri `rapier3d ^0.18`'e pinli, 2026'da hâlâ terk edilmiş — 0.3x uyumlu release yok). Su/akışkan = §22 deterministik CA. Kum/çakıl = custom CA (chunked + dirty-rectangle, `bevy_falling_sand` mimarisi). Patlama debris = Rapier rigid cisim.

> **Crate organizasyonu (2026):** Falling-sand/fluid CA, `physics` crate'i İÇİNDE değil, ayrı **`strata-simulation`** (veya `strata-cellular`) crate'inde olmalı. Gerekçe: CA deterministik integer sim (float yok — pozisyonlar integer cell, gravity = "1 cell/tick"), Rapier float rigid-body; farklı determinizm domainleri. CA hem rendering (dirty-rect texel) hem voxel world state besler; AGENTS.md "Plugin-First / separate crate" kuralıyla uyumlu. `physics` crate'i yalnızca entity collision için.

#### Falling Sand / Gravel (Deterministik CA)

```rust
pub struct FallingParticleSystem {
    particles: Vec<FallingParticle>,
    spatial_grid: GridHash<CellInfo>, // per-frame allocation yok
    sleep_manager: SleepManager,
    dirty_chunks: HashSet<ChunkCoord>, // yalnızca kirli chunk'lar simüle edilir
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

        for chunk in &self.dirty_chunks {
            for particle in self.particles_in_chunk(*chunk) {
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
        }

        self.spatial_grid.rebuild_incremental(&self.particles, &self.dirty_chunks);
        self.dirty_chunks.clear();
    }
}
```

#### GridHash (Custom — HashMap yerine)

> **2026 iyileştirmesi:** `Vec<Vec<T>>` (per-bucket heap allocation + pointer indirection) cache-locality ve allocator churn yaratır. 32³ sector gibi **bounded** bir dünya için **dense flat cell-linked-list** daha iyidir: `cells: Vec<CellInfo>` (boyut `32³`, init sonrası sıfır alloc), `particles: Vec<FallingParticle>` içinde `next: i32` ile intrusive singly-linked list. `clear()` = len 0 (capacity reuse). Hashing yalnızca unbounded sparse world için; dense grid'de `x + y*32 + z*1024` ile O(1).

```rust
pub struct CellInfo { pub head: i32, pub count: u16 }
pub struct FallingParticle { pub grid_pos: IVec3, pub next: i32, /* ... */ }

pub struct GridHash {
    cells: Vec<CellInfo>,        // 32*32*32, allocation-free after init
    particles: Vec<FallingParticle>,
}
impl GridHash {
    pub fn clear(&mut self) {
        for c in &mut self.cells { c.head = -1; c.count = 0; }
        // particles len=0, capacity reuse
    }
    #[inline] pub fn cell_index(p: IVec3) -> usize {
        (p.x + p.y * 32 + p.z * 1024) as usize
    }
}
```

> `slotmap` grid'in kendisi için **aşırı** — yalnızca ECS stable particle handle gerekirse kullan. `bevy::utils::HashMap`/`hashbrown` reuse yalnızca sparse fallback için.

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
        rng: &mut Pcg32, // §11 PCG32/wyhash — deterministik
    ) -> Vec<Fragment> {
        // Gerçek Voronoi: flood-fill DEĞİL. Her fractured voxel en yakın seed'e atanır.
        // Seed sayısı: intensity*10 (1000) overkill; chunky+ucuz için clamp(round(intensity*k),4,48).
        let num_points = (intensity * 10.0).clamp(4.0, 48.0) as usize;
        // Seed placement: impact-biased + jittered (Müller 2013), RNG ile.
        self.generate_voronoi_points(&region_aabb, num_points, rng);
        // meshless_voronoi / voro_rs ile per-voxel nearest-seed (O(voxels));
        // voronator 2D-only → kullanma.
        let fragments = self.assign_voronoi_cells(sector, &region_aabb);

        let mut result = Vec::new();
        for fragment in fragments {
            if fragment.voxel_count < 8 {
                self.spawn_debris_particles(&fragment); // fizik yok, sadece görsel
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
    world: &mut World,
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

        // Çok sayıda dinamik Voxels collider yavaş → convex_hull kullan.
        // Yüzey voxel'larından convex hull vertex'leri çıkar (to_trimesh surface → convex_hull).
        let surface_verts = extract_surface_vertices(&occupied, VOXEL_SIZE);
        let collider = Collider::convex_hull(&surface_verts)
            .unwrap_or_else(|| Collider::cuboid(VOXEL_SIZE, VOXEL_SIZE, VOXEL_SIZE));

        // bevy_rapier 0.34: Velocity alanları linear/angular (PR #690).
        // Patlama impulsu: radial dışa + offset×force → angular.
        let blast = (fragment.center_of_mass - explosion_center).normalize();
        let lin = blast * (BLAST_K * intensity) / fragment.mass;
        let ang = (fragment.center_of_mass - explosion_center)
            .cross(blast * intensity) / fragment.inertia_scalar;
        commands.spawn((
            RigidBody::Dynamic,
            collider,
            Ccd::enabled(), // hızlı shard tunneling önler
            Transform::from_translation(fragment.center_of_mass),
            Velocity { linear: lin, angular: ang },
            FragmentMetadata {
                mass: fragment.mass,
                voxel_count: fragment.voxel_count,
                lifetime: 30.0,
            },
        ));
    }
}
```

> **Tier-gated destruction (2026):** ACTIVE sektör → gerçek Voronoi + rigid fragment. WARM/DISTANT → impulse-only + crack decal (GPU feedback zaten neyin göründüğünü söyler, §06/§08). Yaygın bloklar (taş, cam) için runtime Voronoi yerine **pre-fractured LOD varyantları** (SectorPalette/block-registry §05 ile) — runtime maliyeti sıfır, deterministik. Joint/constraint-based shatter yalnızca yapılar için (ImpulseJoint).

---

### 1.7 Physics Tier Management

| Tier | Fizik Detayı | Güncelleme Frekansı | Collider Tipi |
|---|---|---|---|
| **ACTIVE** (0-96m) | Tam Voxels + custom physics | Her frame (60Hz) tam step | Rapier Voxels (full) |
| **WARM** (96-384m) | Voxels (static only) | Statik collider BVH'de; dynamic body solver düşük frek | Rapier Voxels **surface-only** (§1.1a) |
| **DISTANT** (384m-1.5km) | Surface-only Voxels (query-only) | Solver işi YOK; on-demand scene query | Surface-only Voxels (sensor/query-only) — **solid Cuboid YOK** |
| **ARCHIVE** (1.5km+) | Fizik yok | — | Collider yok |

> **DISTANT = solid Cuboid YANLIŞ (2026):** Tek bir `Collider::cuboid(sector_aabb)` sector'ün içi boş (mağara, overhang, air pocket) olsa bile oyuncuyu katı kutuyla bloke eder. DISTANT yalnızca uzaktan player-vs-terrain query (6Hz) içindir, oyuncu orada traverse etmez. Doğrusu: ya surface-only Voxels collider'ı **query-only** (sensor, no contact response) tut, ya da collider'ı tamamen kaldırıp yalnızca broad-phase reject için AABB kullan. Surface set WARM'dan zaten hesaplı — ucuzca persist eder.

#### Tier Geçişi Sırasında Fizik (atomic swap — fall-through önleme)

> **Fall-through riski (2026):** Collider swap mid-step oyuncunun tunnel etmesine yol açar (rapier #466/#558). Geçişte: (1) eski collider, yenisi tamamen build edilip komşularla `combine_voxel_states` yapılana kadar **geçerli** kalmalı; (2) swap tek bir world mutation'da; (3) 1 frame eski+yeni overlap (terrain vs DYNAMIC only) penceresi. INTERIOR surface'de asla drop edilmemeli — yalnızca solid-fill düşürülür.

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
                self.rebuild_surface_only_collider(physics_world); // §1.1a
            }
            (Tier::Warm, Tier::Distant) => {
                self.make_query_only(physics_world); // surface-only'ı contact'sız query'ye çevir
            }
            (Tier::Distant, Tier::Archive) => {
                self.remove_collider(physics_world);
            }
            (Tier::Archive, Tier::Distant) => {
                self.create_surface_query_collider(physics_world); // query-only, solid cuboid DEĞİL
            }
            (Tier::Distant, Tier::Warm) => {
                self.rebuild_surface_only_collider(physics_world); // §1.1a
            }
            (Tier::Warm, Tier::Active) => {
                self.activate_dynamic_colliders(physics_world);
            }
        }
        // Atomic: eski collider'ı yalnızca yeni build+boundary-coupled olduktan sonra bırak.
    }
}
```

---

### 1.8 GPU Physics Vizyonu

Dimforge'un 2026 hedefi: **rust-gpu ile GPU physics**.

#### Mevcut Durum (2026 Temmuz)

| Proje | Açıklama | Durum |
|---|---|---|
| **wgmath** | WGSL matematik kütüphanesi (Dimforge) | ✅ Tamamlandı |
| **wgrapier** | WGSL tabanlı Rapier subset (GPU, Dimforge) | ✅ Demo çalışıyor |
| **wgsparkl** | WGSL MPM simulation (Dimforge) | ✅ Demo çalışıyor |
| **Slosh** | Slang port (wgsparkl) | 🔄 Devam ediyor |
| **Nexus** | Rust→GPU rigid-body engine (rust-gpu + khal + vortx, Dimforge) | ✅ **Q2 2026 ship etti** — 2× faster vs WGSL, browser (Safari hariç) |

**Nexus demo performansı (wgrapier/wgsparkl yerine geçer):**
- 93.000 body + 120.000 joint (GPU)
- 34.000 plank stack (GPU)
- BVH-based broad-phase + Soft-TGS constraint solver
- Rapier tipi yeniden kullanır (`RigidBodyBuilder`, `ColliderBuilder::cuboid`) → Strata'ya entegrasyonu kolay

**Strata stratejisi:**
- **CPU physics (Faz 1-6):** Oyuncu, araçlar, dinamik entity'ler, debris — `enhanced-determinism` ile deterministik.
- **GPU physics (Faz 7+, stretch goal):** **Nexus** hedef alınır (rust-gpu artık settlement oldu). Patlama debris yığınları, büyük particle simülasyonları — **client-side yalnızca**, server sync yok. Nexus şu an rigid-body only; MPM (falling-sand benzeri) roadmap'te.

> Nexus (Q2 2026) GPU float math ile bit-level cross-platform determinizm sağlamaz ve GPU→CPU sync by-design yoktur. Strata'da server-authoritative terrain/debris CPU Rapier'de kalır; Nexus yalnızca client-side eye-candy (debris pile, particle flood). Faz 1 scope'una alınmaz.

---

### 1.9 Performans Hedefleri (Fizik)

| Metrik | Hedef | Not |
|---|---|---|
| **Collider güncelleme (tek voxel)** | <0.1ms | `set_voxel` + komşu `propagate_voxel_change` |
| **Collider güncelleme (batch, frac <5%)** | <1ms | `apply_voxel_edits` — `set_voxel` döngüsü |
| **Collider güncelleme (tam rebuild)** | <8ms (ölçülecek) | 32³ sektör; occupied list XBrickMap'den cache'lenirse <5ms |
| **Collider güncelleme (WARM surface-only)** | <3ms | §1.1a — INTERIOR elendi ( input 5-10× küçük) |
| **Boundary sync (2 sektör)** | <0.5ms | `combine_voxel_states` |
| **Character ground check** | <0.05ms | XBrickMap 4-level skip + KCC OR (hysteresis) |
| **Broad-phase (ACTIVE)** | <3ms (scalar detr.) / <2ms (SIMD) | Parry BVH, 100+ sektör; `enhanced-determinism` SIMD'siz yavaş |
| **Falling sand (1K particle, dirty-chunk)** | <3ms | dense cell-linked-list GridHash, allocation-free |
| **Fracture (patlama)** | <10ms | Voronoi (meshless_voronoi) + rigid-body spawn |
| **Physics step (fixed 1/60s)** | <5ms (detr.) / <3ms (SIMD) | `enhanced-determinism` scalar; substeps=4 |
| **GPU physics (Faz 7+, stretch)** | <5ms | Client-only (Nexus), 100K+ body |

---

### 1.10 Crate Organizasyonu (Fizik)

```
crates/
  physics/
    ├── mod.rs                  ← PhysicsPlugin entry point (RapierPhysicsPlugin wrap)
    ├── integration.rs          ← TimestepMode/IntegrationParameters config + substep policy (§1.12)
    ├── collider.rs             ← Sector → Voxels / surface-only conversion (§1.1a)
    ├── collision_groups.rs     ← PhysicsLayer bitflags, InteractionGroups (3-arg, §1.2)
    ├── broad_phase.rs          ← Sector spatial hash + BvhOptimizationStrategy (batch flush YOK, §1.2)
    ├── incremental.rs          ← apply_voxel_edits batch API (set_all→propagate, §1.3)
    ├── boundary.rs             ← combine_voxel_states / propagate_boundary_after_edit
    ├── character/
    │   ├── mod.rs              ← KinematicCharacterController setup
    │   ├── ground_check.rs     ← XBrickMap ground + KCC OR + hysteresis (§1.4)
    │   └── movement.rs         ← Movement + slope handling
    ├── custom/
    │   ├── mod.rs              ← Custom physics layer
    │   ├── falling_sand.rs     ← Deterministik CA (chunked, dirty-rect) — NOT physics crate'te, ayrı strata-simulation crate (§1.5)
    │   └── grid_hash.rs        ← dense cell-linked-list GridHash (§1.5)
    ├── destruction/
    │   ├── mod.rs              ← Destruction system
    │   ├── damage.rs           ← Damage accumulation
    │   ├── voronoi.rs          ← Voronoi fracture (meshless_voronoi/voro_rs)
    │   └── fragment.rs         ← Fragment → rigid-body spawn (convex_hull + impulse)
    ├── tier.rs                 ← Physics tier management + surface-only geçiş (atomic swap, §1.7)
    └── gpu/                    ← Faz 7+ stretch goal (Nexus backend)
        ├── mod.rs              ← GPU physics abstraction (client-only)
        └── backend.rs          ← PhysicsBackend trait (Nexus)
```

#### Cargo.toml (physics crate)

> **KRİTİK:** `enhanced-determinism` + `simd-stable` **aynı anda derlenmez** (Rapier `compile_error!`). İki build profili:
> - **Server (deterministik):** `enhanced-determinism` + `serde-serialize` (+ `dim3`, `f32`), SIMD **yok**.
> - **Client (hızlı):** `simd-stable` (+ opsiyonel `parallel`), `enhanced-determinism` **yok**.

```toml
[dependencies]
bevy_rapier3d = "0.34"
# Server-authoritative build (deterministik):
rapier3d = { version = "0.32", default-features = false,
            features = ["enhanced-determinism", "serde-serialize", "dim3", "f32"] }
# NOT: "simd-stable" VE "parallel" KAPALI — enhanced-determinism ile çelişir (compile_error)
parry3d = "0.26"
# Client build için ayrı feature set: ["simd-stable", "serde-serialize", "dim3", "f32"]
```

> Ek önerilen feature'lar: `debug-render` (yalnızca dev profile, Ders 5 profiling), `profiler`. `parry3d` sürümü `rapier3d`'in bağımlılığıyla uyumlu olmalı (0.32 → 0.26).

---

### 1.11 Motor Seçimi — Karar ve Gerekçe (2026-07)

Strata fizik motoru olarak **Rapier Voxels + custom CA layer** seçildi. Alternatifler değerlendirildi; hiçbiri tüm kısıtları karşılamıyor.

| Motor | Voxel collider | Bevy 0.18 | Determinizm | Strata uyumu |
|---|---|---|---|---|
| **Rapier 0.32** | ✅ native Voxels | ✅ bevy_rapier 0.34 | ✅ enhanced-determinism | **Seçildi** |
| Jolt | ❌ manuel chunking | ❌ gayriresmi binding | ✅ | Reddedildi — voxel API yok |
| Bevy XPBD (Avian) | ✅ `Collider::voxels` (Avian 0.5+, commit #761) | ✅ native (Bevy 0.18) | ✅ f64+enhanced-determinism | Yeniden değerlendir — gerçekçi alternatif, ama Rapier Voxels ekosistemi tercih |
| Salva (SPH) | ⚠️ coupling only | ❌ rapier 0.18 pin | ❌ non-deterministik | Reddedildi — §1.5 |
| PhysX | ❌ | ❌ `physx-rs` 2026-05 archived, `bevy_mod_physx` yalnızca Bevy 0.17 | ⚠️ | Reddedildi |

**Neden Rapier en iyi pragmatic tercih:**
1. Genel amaçlı motorlarda **ilk explicit Voxels collider** desteği (~1 byte/voxel, internal-edge tracking).
2. `set_voxel` + `combine_voxel_states` — incremental edit ve sektör sınırı senkronu XBrickMap ile uyumlu.
3. `enhanced-determinism` — server-authoritative network sync ile uyumlu.
4. Dimforge ekosistemi (Parry, bevy_rapier) Strata stack'iyle (Bevy 0.18, wgpu) hizalı.

**Bilinen zayıf noktalar (kabul edildi, mitigasyon planlandı):**
- Voxel sayısı *touched/face* ile ölçeklenir (INTERIOR drop narrow-phase'ı düşürmez, §1.1a) → WARM surface-only bellek/broad-phase için, tier frekansı solver için (§1.7).
- Cross-collider coupling karmaşıklığı → `propagate_boundary_after_edit` zorunlu (§1.3).
- `parallel` feature determinizm ile çelişir → üst katman paralelliği (Bevy schedule + tier izolasyonu, §1.12).
- `enhanced-determinism` SIMD'siz scalar çalışır → solver daha yavaş; perf budget'ı buna göre (§1.9).

---

### 1.12 Alternatif Motorlardan Alınan Dersler

Rapier seçimi değiştirilmeden, Jolt / XPBD / Rapier iç dinamiklerinden alınan uygulanabilir dersler.

#### Ders 1 — Jolt: Statik / Dinamik Broad-Phase Ayrımı

Jolt `NON_MOVING` / `MOVING` ayrı BVH ağaçları kullanır. Rapier'de karşılığı: `CollisionGroups` (§1.2). Terrain-terrain çarpışmasını kapat. (Batch collider anti-pattern — §1.2 güncel.)

#### Ders 2 — XPBD: Substep > Iteration

Macklin "Small Steps": büyük step + çok iteration yerine **küçük substep + az iteration** = kuadratik hata azaltımı, daha stabil yığınlar (fragment/debris).

> **bevy_rapier 0.34 API notu:** `IntegrationParameters.num_substeps` / `num_solver_iterations` alanları **yok**. Substep sayısı `TimestepMode::Fixed { dt, substeps }` ile verilir (plugin resource'una `insert_resource` edilir). Aşağıdaki doğru kullanım:

```rust
use bevy_rapier3d::prelude::*;

// Plugin kurulumu — sabit timestep + substep
app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
   .insert_resource(TimestepMode::Fixed {
       dt: 1.0 / 60.0,   // sabit, asla değişmez
       substeps: 4,      // XPBD "small steps" — fragment/debris stabilitesi
   });
```

Substep sayısı **sabit** kalmalı (server/client aynı simüle etsin). Fizik sistemleri Bevy'nin `FixedUpdate` schedule'ında, `PhysicsSet::SyncBackend` öncesinde koşmalı.

#### Ders 3 — Rapier: Determinism ⊥ Parallel (⊃ SIMD)

> **KRİTİK:** `enhanced-determinism` ile `simd-stable`/`simd-nightly`/`parallel` **aynı anda açılamaz** (Rapier `lib.rs` `compile_error!`). İki ayrı build profili kullan (§1.10 Cargo.toml).

| Feature | Server build | Client build | Gerekçe |
|---|---|---|---|
| `enhanced-determinism` | ✅ Zorunlu | ❌ | Server-authoritative, bit-level determinizm |
| `simd-stable` | ❌ Kapalı | ✅ | Performans; determinizmle çelişir |
| `parallel` | ❌ Kapalı | ⚠️ Opsiyonel | enhanced-determinism ile açılamaz; client'ta solver paralelliği |
| `serde-serialize` | ✅ | ✅ | Network snapshot |

Paralellik Rapier içinden değil, **Bevy schedule** ve **sektör-tier izolasyonu**ndan alınır.

#### Ders 4 — Fixed Timestep (Bevy `FixedUpdate` + `TimestepMode`)

Render loop'tan bağımsız sabit fizik adımı — değişken delta tunneling ve jitter yaratır.

> **bevy_rapier 0.34 API notu:** `RapierContext::step()` diye bir metot **yok**; manuel accumulator + `physics.step()` deseni derlenmez. bevy_rapier zaten `TimestepMode::Fixed` + Bevy `FixedUpdate` schedule'ını destekler — spiral-of-death koruması dahil. Doğru kullanım:

```rust
use bevy::prelude::*;
use bevy_rapier3d::prelude::*;

fn setup_physics(app: &mut App) {
    app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
        // Sabit adım: 1/60s, substep=4 (Ders 2 ile hizalı).
        // Bevy FixedUpdate schedule'ı accumulator'ı içeriden yönetir.
        .insert_resource(TimestepMode::Fixed {
            dt: 1.0 / 60.0,
            substeps: 4,
        });

    // Fizik-etkileyen kullanıcı sistemleri FixedUpdate'a, SyncBackend öncesine:
    app.add_systems(
        FixedUpdate,
        player_movement_system.before(PhysicsSet::SyncBackend),
    );
}
```

`TimestepMode::Fixed`, `dt` sabit kaldığı sürece server/client determinizmini korur (Ders 3 ile uyumlu).

#### Ders 5 — Broad-Phase Profiling

Rapier `Counters` ile tier eşiklerini (96m / 384m / 1.5km) veriyle kalibre et. Jolt'un `JPH_TRACK_BROADPHASE_STATS` karşılığı — §1.2.

#### Özet Tablo

| Kaynak | Ders | Strata uygulaması |
|---|---|---|
| Jolt | Broad-phase katman ayrımı | `CollisionGroups` + incremental BVH (batch flush YOK, §1.2) |
| XPBD | Substep > iteration | `TimestepMode::Fixed { dt: 1/60, substeps: 4 }` |
| Rapier | Determinism ⊥ Parallel | `enhanced-determinism` (server) / `simd-stable` (client); `parallel` yalnızca client |
| Rapier FAQ | Fixed timestep | `TimestepMode::Fixed` + Bevy `FixedUpdate` (manuel accumulator YOK, §1.12 Ders 4) |
| Jolt | Broad-phase profiling | Rapier `Counters` → tier kalibrasyonu |
