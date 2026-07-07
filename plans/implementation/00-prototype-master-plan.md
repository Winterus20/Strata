# Prototype Master Plan — Strata Playable Prototype

> **Amaç:** Tek oyunculu, yürünebilir, terrain görülen ve blok kırılıp koyulabilen bir
> **oynanabilir prototip** için uygulama yol haritası. Tam oyun degil; çekirdek oyun döngesinin
> kanıtı (proof-of-loop).
> **Tarih:** 2026-07-07
> **Dayanak:** `01`–`16` anayasa (kesinleşti), `39`–`44` karar dokümanları.
> **Dil:** Kod/tanımlayıcı İngilizce, plan Türkçe (AGENTS.md §7.B).

---

## 1. Prototip Kapsamı (IN / OUT)

### IN — Prototipte zorunlu
| Sistem | Kaynak Plan | Kapsam |
|--------|-------------|--------|
| Workspace + build | `02`, `44` | Hybrid cargo workspace, rust-lld/mold, dev profile |
| ECS foundation | `03` | Plugin-first skeleton, system sets, change detection |
| Plugin API | `04` | Minimal `Plugin` trait + lifecycle (ileride genişler) |
| Block registry | `05` | SoA registry, TOML, ~16 blok (air, stone, dirt, grass, wood, leaf, sand, water-air, log, plank, cobble, glass, glowstone, metal, ice, lava-air) |
| XBrickMap | `06` | 3-level bitmask, GlobalBrickPool, get/set, SOA |
| Meshing (CPU) | `09` | Binary greedy (CachedGreedy benzeri), PackedQuad 8B, OccupancyScratch |
| Render (minimal) | `10` | Tek-tier: XBrickMap ray-trace pass + visibility buffer (64-bit), HDR+ACES, frustum cull |
| World gen | `11` | Density-function terrain + 3-4 biome, basit cave, deterministik (PCG32+wyhash) |
| Physics | `12`, `42` | Rapier Voxels collider + KinematicCharacterController |
| Lighting | `13` | L0 (sun/point) + L1 (block light BFS), 16-bit packed LightData |
| Player + interaction | `14` | Movement, break/place via raycast, hotbar (1 slot yeterli) |
| Streaming (minimal) | `08` | Sadece ACTIVE tier; sector load/unload çevresel (radius) |
| Client binary | `31`, `44` | `bin/client` wgpu+winit bootstrap |

### OUT — Prototipten erte
- `07` SVDAG (uzak render), `16` Network, `15` Storage (dünya regenerate), `17`+ tüm gameplay/UX/modding/platform.

---

## 2. Performans Bütçeleri (Prototip Hedefleri)

| Metrik | Hedef | Not |
|--------|-------|-----|
| Frame time (ACTIVE tier, ~10 sektör) | < 8 ms CPU | 120 FPS headroom |
| Sector meshing (32³) | < 0.5 ms | Greedy, heap-free scratch |
| Block set/get | O(1) | GlobalBrickPool SlotMap |
| Memory / sektör (boş) | ~8 B | bitmask-only (06 §B.2) |
| Heap alloc / frame (hot path) | 0 | OccupancyScratch stack-allocated |
| Build (dev, dynamic linking) | < 3 s incremental | 44 §3 |
| GPU draw call | 1 batch/opaque + 1 transparent | vertex pulling |

**Yasaklar (AGENTS.md §7.G):** Canlı voxel için per-sector `Vec`; hot path'te heap fragmantasyonu; `if option.is_some()` per-entity tarama; change-detection guard'sız `mut`.

---

## 3. Build Sırası (Milestones)

```
M0  Workspace scaffold + build config           (01-workspace-and-build)
M1  ECS + Plugin API skeleton                   (02-ecs-foundation, 03-plugin-api)
M2  Block registry + XBrickMap core             (04-block-registry, 05-xbrickmap-core)
M3  CPU greedy meshing + PackedQuad             (06-meshing-cpu)
M4  Minimal render (XBrickMap RT + visbuf)      (07-render-minimal)
M5  World gen (deterministic terrain)           (08-world-generation)
M6  Physics (Rapier voxels + controller)        (09-physics-rapier)
M7  Lighting L0/L1                              (10-lighting-l0l1)
M8  Player + break/place + hotbar               (11-player-and-interaction)
M9  Streaming ACTIVE tier + client bootstrap    (12-streaming-active-tier, 13-client-bootstrap)
```

Her milestone **derlenen, test edilebilir** bir ara ürün verir. M4'te ilk görüntü (statik), M6'da yürüme, M8'de blok kırama/koyma, M9'da sonsuz yürüme.

---

## 4. Ortak Prensipler (tüm roadmap'lerde geçerli)

1. **Filter-First:** Query'ler `With<T>`/`Without<T>` + ZST (`NeedsRemesh`, `ChunkDirty`) ile archetype seviyesinde.
2. **SoA:** Hot/cold ayır; component'ları parçala.
3. **Change detection:** `set_if_neq()` / `bypass_change_detection()`; `mut` alınca guard.
4. **Heap-free hot path:** `GlobalBrickPool` (SlotMap + SecondaryMap); scratch stack'te.
5. **Branchless:** GPU'da `select`/`firstTrailingBit`; CPU'da bitmask.
6. **Async:** World gen + meshing `AsyncComputeTaskPool`; main thread sadece apply.
7. **Test:** Round-trip (encode→decode→compare) + boundary (empty/full/edge) zorunlu.

---

## 5. Dosya İndeksi (bu dizin)

| # | Dosya | Milestone |
|---|-------|-----------|
| 00 | `00-prototype-master-plan.md` | — |
| 01 | `01-workspace-and-build.md` | M0 |
| 02 | `02-ecs-foundation.md` | M1 |
| 03 | `03-plugin-api.md` | M1 |
| 04 | `04-block-registry.md` | M2 |
| 05 | `05-xbrickmap-core.md` | M2 |
| 06 | `06-meshing-cpu.md` | M3 |
| 07 | `07-render-minimal.md` | M4 |
| 08 | `08-world-generation.md` | M5 |
| 09 | `09-physics-rapier.md` | M6 |
| 10 | `10-lighting-l0l1.md` | M7 |
| 11 | `11-player-and-interaction.md` | M8 |
| 12 | `12-streaming-active-tier.md` | M9 |
| 13 | `13-client-bootstrap.md` | M9 |

*Bu roadmap'ler anayasayla (`01`–`16`) çelişirse anayasa esas alınır; burada prototip kapsamı için yapılan sadeleştirmeler (tek-tier streaming, L0/L1 lighting, TOML-only registry) bilinçli trade-off'lardır.*
