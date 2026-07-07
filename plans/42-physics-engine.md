# Physics Engine Kararı

**Tarih:** 2026-05-22
**Karar:** Rapier (rapier3d) + bevy_rapier3d + custom voxel collision
**Önceki durum:** "Rapier varsayılmış" — kesinleşmemişti
**Otorite:** Kesinleşmiş fizik planı `plans/12-physics.md` (anayasa, §01–16 bandı). Bu dosya (42) TASLAKTIR; çelişkide Plan 12 esas alınır, bu dosya revize edilir.

---

## 2026-07 Denetim Güncellemesi

Bu dosya, 2026-07 tarihli kapsamlı teknik denetim (web araştırması + 5 alt-ajan) sonrası aşağıdaki noktalarda düzeltilmiştir. Denetim tam raporu ayrıdır; özet değişiklikler:

1. **Avian red gerekçeleri bayatladı.** Avian 0.6.1 (Bevy 0.18) hem çalışan voxel collider'ı (`Collider::voxels`) hem de KCC yollarını (`move_and_slide`, `bevy_tnua-avian3d 0.11.1`, `bevy_ahoy`) içerir. Plan 12 §1.11 zaten "yeniden değerlendir" der — bu dosya ona hizalanmıştır.
2. **Sürüm notları düzeltildi.** Gerçek (crates.io, 2026-07): `bevy_rapier3d 0.34` → `bevy ^0.18.1`; `rapier3d 0.32`; `parry3d 0.28`. `AGENTS.md §7`'deki "rapier3d 0.34" / "parry3d 0.29" ifadeleri hatalıdır.
3. **Kod taslağı güncellendi.** `voxels_from_points` "eski API" değildir (construct API); `as_voxels_mut()` diye metot yok → `shape_mut().as_mut_any().downcast_mut::<Voxels>()`.
4. **Plan 12'e referans eklendi.** Broad-phase, collision groups, tier frekansı, determinizm profilleri, substep ve KCC+XBrickMap bu taslağın kapsamı dışındadır ve Plan 12 tarafından tanımlanır; burada özetlenir.

---

## Değerlendirilen Alternatifler

### 1. Rapier + bevy_rapier (Seçilen)
| Kriter | Durum |
|--------|-------|
| Bevy sürümü | `bevy_rapier3d 0.34` → Bevy 0.18.1 |
| Core sürüm | `rapier3d 0.32` / `parry3d 0.28` (glam migration) |
| Olgunluk | ~5.3k star, official Dimforge |
| Voxel shape | **Native:** `ColliderBuilder::voxels` / `voxels_from_points` (Parry Voxels, sparse ~1 byte/voxel) |
| Runtime voxel edit | ✅ `set_voxel(key, bool)` + `propagate_voxel_change` / `combine_voxel_states` (cross-collider) |
| Heightfield terrain | ✅ |
| Character controller | ✅ built-in `KinematicCharacterController` |
| Jointler | ✅ (6 DOF, motor, gear, pulley) |
| Determinizm | ✅ `enhanced-determinism` (server) / `simd-stable` (client) — ayrı build profili |
| CCD | ✅ |
| Performans | 0.5–2 ns/entity iteration (scalar deterministik build'te daha yavaş) |

### 2. Avian (eski bevy_xpbd) — YENİDEN DEĞERLENDİR
| Kriter | Durum |
|--------|-------|
| Bevy sürümü | `avian3d 0.6.1` → Bevy 0.18 (0.7.0 → Bevy 0.19, Strata ile şu an uyumsuz) |
| Olgunluk | ~3k star, hızlı büyüyen |
| Voxel shape | ✅ `Collider::voxels` (Parry Voxels — internal-edge tracking). Tek başına çalışır; issue #945 yalnızca `compound` içinde nesting edge-case'i (Strata'nın "sektör başına tek Voxels" desenini etkilemez) |
| Runtime voxel edit | ✅ Parry Voxels API (aynı temel) |
| Character controller | ⚠️ Built-in all-in-one `KinematicCharacterController` yok; ancak `move_and_slide` (0.6+) + resmi `kinematic_character_3d` örneği + **bevy_tnua-avian3d 0.11.1** + **bevy_ahoy** ile çalışan yollar mevcut |
| Custom collision backend | ✅ Support var |
| Determinizm | ✅ `f64` + `parry-f64` |
| Performans | İyi (XPBD, BVH broad-phase 0.6+) |

> **Değerlendirme:** Avian gerçekçi alternatiftir. Ancak (a) built-in KCC yok, (b) Rapier'e özgü `combine_voxel_states`/`propagate_voxel_change` cross-collider API'lerinin Avian'daki exact equivalence'i doğrulanmadı (Plan 12 §1.3'e ağır bağımlıyız), (c) Bevy 0.18'da 0.6.1'e sabitlenir. Rapier seçimi **korunur**; migration riski düşük ama izlenmeli (bkz. "Avian migration watchlist" notu).

### 3. Jolt Physics
| Kriter | Durum |
|--------|-------|
| Rust binding | `rolt` 0.3.1 / `jolt-rust` 0.2.0 — "early work in progress", resmi/olgun binding yok |
| Bevy entegrasyonu | ❌ Yok |
| Production durumu | AAA oyunlarda (C++) olgun; Rust'ta değil |
| Voxel support | C++ tarafında var, Rust'ta erişim belirsiz |

### 4. Parry
- Sadece **collision detection** (spatial queries, raycast, shape intersection) — Voxels shape'i sağlar.
- Rapier ve Avian zaten altında Parry kullanıyor.
- Rigid body dinamiği, joint, karakter kontrolcüsü yok → bağımsız motor değil, bağımlılık.

---

## Karar Gerekçesi

### Neden Rapier?
1. **Native voxel collision shape** — `ColliderBuilder::voxels` / `voxels_from_points` ile dolu voxel koordinatlarından doğrudan Voxels collider oluşturulabiliyor. Runtime'da `set_voxel` ile blok ekleme/çıkarma + `propagate_voxel_change`/`combine_voxel_states` ile sektör sınırı senkronu yapılabiliyor. Bu, XBrickMap ile entegrasyon için kritik.
2. **Heightfield shape** — Düz olmayan terrain'ler için ideal (ör. WARM/DISTANT query-only yedekleri).
3. **Character controller** — Hazır `KinematicCharacterController`; XBrickMap tamamlayıcı ground check ile birlikte (Plan 12 §1.4).
4. **Olgunluk** — En geniş community, en çok test edilmiş Bevy entegrasyonu.
5. **Determinizm** — `enhanced-determinism` (server-authoritative netcode ile uyumlu); `simd-stable` (client) ile ayrı build profili.
6. **Rapier docs'ta voxel worlds için özel bölüm** var (rapier.rs/docs/).
7. **Explicit cross-collider coupling API** — `combine_voxel_states` / `propagate_voxel_change` ile sektör seam'lerinde ghost-edge eliminasyonu Plan 12 §1.3'te zorunlu.

### Neden Avian DEĞİL (güncellenmiş)?
- ~~Voxel collision bug'ları (compound collider'da çalışmıyor — issue #945)~~ → **Düzeltme:** Tek başına `Collider::voxels` çalışır; #945 yalnızca `compound` içinde voxel nesting edge-case'idir ve Strata'nın sektör-başına-tek-Voxels desenini etkilemez.
- ~~Karakter kontrolcüsü yok~~ → **Düzeltme:** Built-in all-in-one `KinematicCharacterController` yok; ancak `move_and_slide` + `bevy_tnua-avian3d` + `bevy_ahoy` ile çalışan KCC yolları mevcut. Rapier sadece "daha turnkey".
- **Asıl belirleyici fark:** Rapier'e özgü cross-collider `combine_voxel_states`/`propagate_voxel_change` API'leri Plan 12'nin sektör sınırı senkron stratejisinin temelidir; Avian'da bu exact equivalence doğrulanmadı.
- Daha genç, daha az test edilmiş (yine de hızlı büyüyor).
- Bevy 0.18'da 0.6.1'e sabitlenir (0.7 → Bevy 0.19); migration ileride ek iş gerektirir.

### Neden Jolt DEĞİL?
- Rust binding'i production-ready değil.
- Bevy plugin'i yok — manuel FFI + schedule entegrasyonu çok büyük iş yükü.
- Strata'nın zaman çizelgesi için uygun değil.

### Salva (SPH) — REDDEDİLDİ (Plan 12 §1.5)
- `salva3d 0.9.0` (2024-02, >2 yıl sessiz), bağımlılıkları `rapier3d ^0.18` + `nalgebra ^0.32` + `bevy ^0.12`.
- Strata stack'i `rapier3d 0.32+` (glam) / `Bevy 0.18` → sadece semver çakışması değil, **nalgebra→glam portu** gerekir.
- SPH non-deterministik → server-authoritative ile uyumsuz.
- **Karar:** Su/akışkan = deterministik CA (Plan 12 §1.5, ayrı `strata-simulation` crate). Kum/çakıl = deterministik CA. Patlama debris = Rapier rigid cisim.

---

## Entegrasyon Stratejisi

> Bu bölüm yalnızca üst seviye akışı verir. Detaylı fizik mimarisi (broad-phase, collision groups, tier frekansı, determinizm, substep, KCC, CA, destruction, crate organizasyonu) **`plans/12-physics.md`**'de tanımlıdır ve otoritatifdir. Aşağıdaki maddeler Plan 12'ye yapılan referanslardır.

### Voxel Fizik Katmanı

```
┌──────────────────────────────────────┐
│  Strata Gameplay Systems             │
│  (block place/break, falling sand)   │
└──────────┬───────────────────────────┘
           │ XBrickMap değişikliği
           ▼
┌──────────────────────────────────────┐
│  Chunk Mesh Builder                  │
│  → render mesh (vertices)            │
│  → voxel collider update             │
└──────────┬───────────────────────────┘
           │ set_voxel / rebuild (Plan 12 §1.1 / §1.3)
           ▼
┌──────────────────────────────────────┐
│  Rapier Voxels Shape                 │
│  (ColliderBuilder::voxels → runtime edit,
│   surface-only WARM için §1.1a)      │
└──────────┬───────────────────────────┘
           │ collision queries (Plan 12 §1.2 broad-phase)
           ▼
┌──────────────────────────────────────┐
│  Rapier Physics Pipeline             │
│  (rigid bodies, joints, controller)  │
│  + Custom layer (CA, debris) §1.5    │
└──────────────────────────────────────┘
```

### Detaylar

1. **Static collision (dünya blokları):** Plan 12 §1.1
   - Her **sektör (32³)** için bir `Voxels` collider (chunk yerine sektör birimi — Strata cubic chunk).
   - Sektör dirty → `set_voxel` ile güncelle; `propagate_voxel_change`/`combine_voxel_states` ile komşu sektör seam senkronu (§1.3).
   - **Domain pre-size:** Her sektörün Voxels domain'i kurulumda 32³'e `resize_domain` ile sized edilmeli → `set_voxel` O(1) (out-of-bounds O(N) realloc önlenir).
   - **WARM tier:** INTERIOR voxel'ler collider'dan çıkarılır (surface-only, §1.1a) → bellek + broad-phase/BVH leaf + cross-sector bookkeeping kazancı. ACTIVE'da tam set.
   - **DISTANT:** solid `Cuboid` YANLIŞ (içi boş olabilir) → surface-only Voxels query-only VEYA collider tamamen kaldırılıp sorgu XBrickMap/SVDAG'e yönlendirilir (§1.7).

2. **Dynamic rigid bodies:** Plan 12 §1.2
   - Rapier standart rigid body'leri (player, item, mob, araç). `RigidBody::Dynamic` / `Kinematic` / `Static`.
   - **Collision groups:** `PhysicsLayer` bitflags (TERRAIN / DYNAMIC / SENSOR / DEBRIS). Terrain yalnızca DYNAMIC + SENSOR ile etkileşir (`InteractionGroups::new(memberships, filter, test_mode)` — 3 argüman, Plan 12 §1.2). Batch collider ekleme anti-pattern → streaming anında incremental ekle/çıkar.

3. **Character controller:** Plan 12 §1.4
   - Rapier `KinematicCharacterController` (kinematic + capsule). `snap_to_ground: 0.2`, `offset: 0.01`, autostep, `apply_impulse_to_dynamic_bodies: true`.
   - **Bilinen bug (#327):** KCC `grounded` Voxels collider ile ara sıra yanlış `false`. → **Zorunlu tamamlayıcı:** `is_grounded = kcc.grounded || xbrick_grounded` + grounding hysteresis (coyote-time latch 100–150ms).
   - *Opsiyonel alternatif:* `bevy_tnua-rapier3d` (kendi aşağı-probe'u #327'yi bypass eder, coyote-time/jump-buffer/SensorShape yerleşik) — ancak dinamik rigid body gerektirir, kinematic paradigm sapması. Kullanıcı onayı gerektirir.

4. **Falling sand / fluid (deterministik CA):** Plan 12 §1.5
   - Ayrı `strata-simulation` crate'inde deterministik CA (chunked, dirty-rect, dense GridHash cell-linked-list).
   - **Pure-integer CA** önerilir (float velocity yerine) → server-authoritative determinizm garanti.
   - Rapier ile yalnızca debris rigid-body spawn; su/kum akışı CA ile.

### Kod Taslağı (güncellenmiş)

```rust
use rapier3d::prelude::*;
use parry3d::math::{Vector, IVector};   // parry 0.26+ glam: IVector = I32Vec3
use parry3d::shape::Voxels;

// Chunk/sektör collision oluşturma — construct API (voxels VEYA voxels_from_points, ikisi de geçerli)
fn create_voxel_collider(occupied: &[IVector], voxel_size: Vector, origin: Vector) -> Collider {
    ColliderBuilder::voxels(voxel_size, occupied)   // veya ::voxels_from_points(voxel_size, &samples)
        .position(Pose::translation(origin.x, origin.y, origin.z))
        .collision_groups(terrain_groups())         // Plan 12 §1.2 PhysicsLayer
        .build()
}

// Runtime edit — as_voxels_mut() YOK; shape_mut() → downcast
fn update_voxel_collider(collider: &mut Collider, key: IVector, occupied: bool) {
    let voxels = collider
        .shape_mut()
        .as_mut_any()
        .downcast_mut::<Voxels>()
        .expect("collider shape is Voxels");
    // Domain 32³'e pre-size edildiği için set_voxel O(1) in-bounds.
    voxels.set_voxel(key, occupied);
}

// Cross-collider seam sync (Plan 12 §1.3) — zorunlu
fn sync_sector_boundaries(a: &mut Collider, b: &mut Collider, shift: IVector) {
    let va = a.shape_mut().as_mut_any().downcast_mut::<Voxels>().unwrap();
    let vb = b.shape_mut().as_mut_any().downcast_mut::<Voxels>().unwrap();
    va.combine_voxel_states(vb, shift);
}
```

---

## Karşılaştırma Tablosu

| Kriter | Rapier | Avian | Jolt (Rust) |
|--------|--------|-------|-------------|
| Bevy Plugin | ✅ Resmi (0.34 / Bevy 0.18) | ✅ Native (0.6.1 / Bevy 0.18) | ❌ Yok |
| Voxel Shape | ✅ Native (`Voxels`, sparse) | ✅ Native (`Collider::voxels`; #945 yalnızca compound) | ❓ Belirsiz |
| Runtime Edit | ✅ `set_voxel` + propagate/combine | ✅ Parry Voxels API | ❓ |
| Character Controller | ✅ built-in KCC | ⚠️ built-in yok; tnua/ahoy var | ✅ (C++) |
| Cross-collider coupling | ✅ explicit API | ⚠️ exact equivalence doğrulanmadı | ❓ |
| Joints | ✅ | ✅ (motor 0.6) | ✅ (full, C++) |
| Soft Body | ❌ | ❌ | ✅ |
| CCD | ✅ | ✅ | ✅ |
| Determinizm | ✅ enhanced-determinism (server) / simd (client) | ✅ f64 + parry-f64 | ✅ |
| Stars | ~5.3k | ~3k | 10.4k (C++) |
| Rust Binding | Native | Native | 88 star, erken |
| Community | Geniş, olgun | Büyüyen | C++ büyük, Rust küçük |

> Salva (SPH) alternative olarak değerlendirildi → **reddedildi** (bkz. yukarı, Plan 12 §1.5).

---

## Kaynaklar

- [Rapier GitHub](https://github.com/dimforge/rapier)
- [bevy_rapier GitHub](https://github.com/dimforge/bevy_rapier)
- [Rapier User Guide](https://rapier.rs/docs/)
- [Rapier Voxel Example](https://github.com/dimforge/rapier/blob/master/examples3d/voxels3.rs)
- [Rapier Heightfield Example](https://github.com/dimforge/rapier/blob/master/examples3d/heightfield3.rs)
- [Rapier Voxels docs](https://docs.rs/rapier3d/latest/rapier3d/geometry/struct.Voxels.html)
- [Parry Voxels docs](https://docs.rs/parry3d/latest/parry3d/shape/struct.Voxels.html)
- [Avian GitHub](https://github.com/avianphysics/avian)
- [Avian v0.6.0 release](https://github.com/avianphysics/avian/releases/tag/v0.6.0)
- [Avian issue #945 (compound-of-voxels)](https://github.com/avianphysics/avian/issues/945)
- [bevy_tnua-avian3d](https://crates.io/crates/bevy-tnua-avian3d)
- [bevy_ahoy (Avian kinematic KCC)](https://github.com/janhohenheim/bevy_ahoy)
- [jolt-rust GitHub](https://github.com/SecondHalfGames/jolt-rust)
- [Jolt Physics GitHub](https://github.com/jrouwe/JoltPhysics)
- [Parry GitHub](https://github.com/dimforge/parry)
- [Salva3d crates.io](https://crates.io/crates/salva3d) (reddedildi — rapier ^0.18 pin, nalgebra, 2+ yıl sessiz)
- [Rapier KCC grounded bug #327](https://github.com/dimforge/rapier.js/issues/327)
- [Rapier broad-phase determinism #910](https://github.com/dimforge/rapier/issues/910)
- [Nexus (GPU physics, prototip)](https://github.com/dimforge/nexus)
- [Dimforge Q1 2026 raporu](https://dimforge.com/blog/2026/04/05/dimforge-Q1-technical-report/)

---

## Riskler

| Risk | Olasılık | Çözüm |
|------|----------|-------|
| Rapier'ın Voxels shape'i büyük dünyalarda memory sorunu | Orta | Sektör başına ayrı voxel collider; WARM'ta surface-only (§1.1a); yalnızca yakın sektörler aktif |
| Voxel set/clear performansı (blok kırma anında) | Düşük | `set_voxel` O(1); domain 32³'e pre-size; `propagate`/`combine` ile seam sync |
| Broad-phase determinizm (issue #910) | Orta | Rapier ≥0.33'e yükselt **VEYA** server'da `BvhOptimizationStrategy::None` zorunlu (Plan 12 §1.2) |
| KCC `grounded` yanlış false (#327) | Orta | Zorunlu XBrickMap tamamlayıcı + coyote-time hysteresis (Plan 12 §1.4) |
| Rapier + bevy_rapier API değişiklikleri | Düşük | Stabil versiyon pin'le; `as_voxels_mut()` yok → `shape_mut().downcast` |
| Avian ileride Rapier'ı geçerse regret riski | Düşük | Avian 0.6.1 Bevy 0.18'e sabit; 0.7 → Bevy 0.19. "Avian migration watchlist" ile periyodik izleme |
| Nexus GPU physics üretim hazır değil | Düşük | Plan 12 §1.8 "Research / Stretch Goal" olarak etiketli; client-only, Faz 7+ |

---

## Avian Migration Watchlist (eklendi 2026-07)

Periyodik (her çeyrek) gözden geçirilecek maddeler:
- `bevy_tnua-avian3d` ve Avian `move_and_slide` olgunluğu.
- Avian 0.7'nin Bevy 0.19 desteği ve Strata'nın Bevy yükseltme planı.
- Rapier `combine_voxel_states`/`propagate_voxel_change`'e denk Avian API parity'si.
- Eğer Avian bu maddelerde Rapier'ı yakalarsa, migration maliyeti benzer API nedeniyle düşüktür; karar gözden geçirilir.

---

## Sonuç

**Rapier + bevy_rapier** kullanılacak. Voxel collision için Rapier'ın `Voxels` shape'i (sparse, surface-only WARM optimizasyonu, cross-collider seam sync) kullanılacak, XBrickMap ile entegre edilecek. Karakter controller, rigid body, joint gibi tüm dinamik fizik Rapier üzerinden yürütülecek; falling sand/fluid deterministik CA (`strata-simulation` crate) ile, Patlama debris Rapier rigid body ile. **Detaylı fizik mimarisi Plan 12'dedir ve bu taslak ona tabidir.**
