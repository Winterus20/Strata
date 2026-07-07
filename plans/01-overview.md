# 00 — Strata: Voxel Sandbox Oyunu — Sistem Mimarisi ve Doküman Haritası

> **Proje:** Strata — Rust ile yazılmış, 4-tier hibrit veri yapısı (XBrickMap + SVDAG) kullanan, sınırsız yükseklikli, multiplayer voxel sandbox oyunu.
> **Son güncelleme:** 2026-07-07 (`16-network-and-lag-compensation.md` kesinleşti → anayasa `01`–`16`; §1.1 tablo)
> **Durum:** Planlama — 38 plan dosyası; **`01`–`16` kesinleşmiş** (anayasa), **`17`+ taslak** (değişebilir). Ayrıntı: [§1.1 Plan olgunluk seviyeleri](#11-plan-olgunluk-seviyeleri).
> **Mimari:** Bevy ECS + wgpu — hızlı geliştirme, cross-platform, ekosistem desteği
> **Not:** Bu doküman, tüm `plans/` dosyalarının master indeksidir. Her sistemin hangi dosyada, hangi crate'de, hangi prensiplerle planlandığını gösterir. Yeni geliştirici onboarding için başlangıç noktasıdır.

---

## Strata Nedir?

Strata, **Minecraft benzeri bir voxel sandbox oyunudur** ancak modern grafik teknolojileri (WGSL ray tracing, SVDAG, GPU compute, HDR) ve akademik araştırma sonuçları (SIGGRAPH 2025, Pacific Graphics 2024) ile inşa edilmiştir.

**Tek cümleyle:** Rust + Bevy ECS 0.18 + wgpu 29 ile sıfırdan yazılan, 4-kademeli hibrit veri yapısı sayesinde hem yakında O(1) edit (XBrickMap) hem uzakta yüksek sıkıştırma (SVDAG) sunan, sınırsız yükseklikli, server-authoritative multiplayer bir voxel oyun motoru.

**Temel özellikler:**
- **Sınırsız yükseklik** — 32×32×32 cubic sector'ler (`06-xbrickmap.md`), dikey sınır yok
- **4-tier streaming** — Yakın: XBrickMap (O(1) edit) → Orta: XBrickMap+SVDAG → Uzak: SVDAG → Çok uzak: sıkıştırılmış disk
- **Rust + Bevy ECS 0.18 + wgpu 29** — Windows hedef, Vulkan/DX12/Metal
- **Multiplayer** — lightyear (prediction/rollback/interpolation/lag-comp, UDP/WebTransport), 600+ oyuncu hedefi (`16` kesinleşti)
- **Tiered modding** — wasmtime 45 + WIT/Component Model; data/WASM/native kademeleri, fizik dahil her katman modlanabilir (hot loop native)
- **38 plan dosyası** — Her sistem ayrıntılı planlanmış (fizik, aydınlatma, network, storage, AI, crafting, vs.)

---

## İçindekiler

1. [Dosya Serisi — Tam Liste](#1-dosya-serisi--tam-liste)
   - [1.1 Plan olgunluk seviyeleri](#11-plan-olgunluk-seviyeleri)
2. [Mimari Katmanlar](#2-mimari-katmanlar)
3. [Sistem Haritası](#3-sistem-haritası)
4. [Crate Bağımlılık Grafiği](#4-crate-bağımlılık-grafiği)
5. [Implementasyon Sırası (Faz Haritası)](#5-implementasyon-sırası-faz-haritası)
6. [Teknik Yığın](#6-teknik-yığın)

---

## 1. Dosya Serisi — Tam Liste

### 1.1 Plan olgunluk seviyeleri

| Seviye | Dosyalar | Anlamı |
|--------|----------|--------|
| **Kesinleşmiş** | `01` – `16` | İnceleme tamamlandı; mimari kararlar sabit. Kod ve diğer planlar **bunlara uymak zorunda**. Çelişki varsa önce `01`–`16` güncellenir veya `17`+ düzeltilir. |
| **Taslak** | `17` – `38` | Taslak / ön tasarım; **değişebilir**. `01`–`16` ile çelişirse **17+ esas alınmaz** — kesinleşmiş planlar önceliklidir. |

AI ve geliştiriciler: `AGENTS.md` §2 ve bu tablo, planların hangi katmanda "anayasa" sayıldığını tanımlar. Yeni bir plan `11`+ kesinleştiğinde bu bölüm ve `AGENTS.md` güncellenmelidir.

---

| # | Dosya | Konu | Crate | Durum |
|---|-------|------|-------|-------|
| 01 | `01-overview.md` | Genel bakış, dünya organizasyonu, temel prensipler (bu dosya) | — | 🔒 Kesinleşti |
| 02 | `02-implementation.md` | Crate organizasyonu, uygulama sırası, sözlük | — | 🔒 Kesinleşti |
| 03 | `03-ecs-architecture.md` | Bevy ECS mimarisi, component'lar, sistem setleri, event'ler | `ecs` | 🔒 Kesinleşti |
| 04 | `04-plugin-api.md` | Plugin trait, registry, hook sistemi, lifecycle, granülerlik×güven matrisi (L0–L4), dispatcher/strateji registry | `plugin-api` | 🔒 Kesinleşti |
| 05 | `05-block-registry.md` | Block registry, property sistemi, block ID yapısı, TOML loading | `core` | 🔒 Kesinleşti |
| 06 | `06-xbrickmap.md` | XBrickMap veri yapısı, SOA+SIMD, ray tracing | `core` | 🔒 Kesinleşti |
| 07 | `07-svdag.md` | SVDAG, Shared Node Pool, transform-aware, shallow streaming, bake/unbake, ECS | `core` | 🔒 Kesinleşti |
| 08 | `08-streaming.md` | 4-tier orchestration, hysteresis, StreamingManager, GPU feedback priority, predictive prefetch, lifecycle/AOI | `streaming` | 🔒 Kesinleşti |
| 09 | `09-meshing.md` | Binary greedy, PackedQuad, GigaBuffer, ECS incremental mesh, tier stratejisi | `meshing` | 🔒 Kesinleşti |
| 10 | `10-render-pipeline.md` | Unified visibility buffer (64-bit, Aokana layout), Hi-Z occlusion + re-execution, VRCS, tile-chunk pairs, HDR pipeline | `render` | 🔒 Kesinleşti |
| 11 | `11-world-gen.md` | Prosedürel terrain, biome sistemi, noise pipeline, yapılar | `world-gen` | 🔒 Kesinleşti |
| 12 | `12-physics.md` | Rapier Voxels, BVH, character controller, destruction, GPU physics | `physics` | 🔒 Kesinleşti |
| 13 | `13-lighting.md` | 5-kademeli hybrid aydınlatma (L0–L4) | `lighting` | 🔒 Kesinleşti |
| 14 | `14-inventory-player.md` | Player controller, envanter, block interaction, input mapping | `player` | ✅ Kesinleşti |
| 15 | `15-storage-and-persistence.md` | Hybrid tiered storage, region files, SQLite, save/load, cloud backup | `storage`, `save` | 🔒 Kesinleşti |
| 16 | `16-network-and-lag-compensation.md` | Network senkronizasyonu, delta compression, lag compensation, client prediction | `network` | 🔒 Kesinleşti |
| 17 | `17-server-and-security.md` | Headless server, tokio, input validation, anti-cheat, commands | `server`, `security` | 📝 Taslak |
| 18 | `18-multiplayer-and-social.md` | Multiplayer lobby, server browser, voice chat, text chat | `multiplayer`, `chat` | 📝 Taslak |
| 19 | `19-entities-and-ai.md` | Behavior tree, A* pathfinding, mob lifecycle, loot tables | `ai`, `entities` | 📝 Taslak |
| 20 | `20-crafting.md` | Shaped/shapeless recipes, crafting grid, furnace | `crafting` | 📝 Taslak |
| 21 | `21-building-tools.md` | Selection, copy/paste, blueprints, transform | `building` | 📝 Taslak |
| 22 | `22-fluids.md` | Fluid simulation (su/lava), cellular automata | `fluids` | 📝 Taslak |
| 23 | `23-environment-time-weather.md` | Day/night cycle, weather, seasons, calendar | `daynight`, `seasons` | 📝 Taslak |
| 24 | `24-ui-and-ux.md` | UI/HUD, glyphon, flexbox, settings, i18n, accessibility | `ui`, `config` | 📝 Taslak |
| 25 | `25-audio.md` | 3D spatial audio, ambient system, block sounds | `audio` | 📝 Taslak |
| 26 | `26-particles-vfx.md` | GPU compute particles, emitter'lar, yağmur/kar/patlama | `particles` | 📝 Taslak |
| 27 | `27-assets.md` | Asset pipeline, texture/model loading, hot-reload | `assets` | 📝 Taslak |
| 28 | `28-animation.md` | Skeletal/keyframe animation, state machine, blending | `animation` | 📝 Taslak |
| 29 | `29-map.md` | Minimap & world map, fog of war, waypoints | `map` | 📝 Taslak |
| 30 | `30-progression-and-events.md` | Achievements, tutorial, dynamic world events, quests | `events` | 📝 Taslak |
| 31 | `31-client-binary.md` | Client binary, wgpu+winit, config, workspace yapısı | `bin/client` | 📝 Taslak |
| 32 | `32-modding.md` | Tiered modding (T0 data / T1 WASM / T2 native), WIT+Component Model, fizik/world-gen batch policy hook, permission/allowlist | `modding` | 📝 Taslak |
| 33 | `33-diagnostics-and-testing.md` | Debug HUD, profiling, metrics, testing, benchmark, crash telemetry | `debug`, `tests` | 📝 Taslak |
| 34 | `34-performance.md` | Performans hedefleri, risk listesi, alternatifler | — | 📝 Taslak |
| 35 | `35-controller-gamepad.md` | Gamepad support, vibration/haptic | `input` | 📝 Taslak |
| 36 | `36-screenshot-video.md` | Screenshot & video capture, replay system | `media` | 📝 Taslak |
| 37 | `37-platform-integration.md` | Steam/Epic/GOG platform entegrasyonu | `platform` | 📝 Taslak |
| 38 | `38-update-patch.md` | Update/patch system, delta patching | `updater` | 📝 Taslak |

---

## 2. Mimari Katmanlar

Strata, 4 ana katmandan oluşan hiyerarşik bir voxel motorudur:

```
┌─────────────────────────────────────────────────────────────────────┐
│                    KATMAN 1: VERİ KATMANI (Data Layer)              │
│  Dosyalar: 05, 06, 07, 15                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐  │
│  │ XBrickMap    │  │ SVDAG        │  │ Storage (Region + SQLite) │  │
│  │ (Tier 1/2)   │  │ (Tier 3/4)   │  │ (Tier 4+ disk)           │  │
│  │ O(1) edit    │  │ dedup + LOD  │  │ dedup + compression      │  │
│  │ SOA + SIMD   │  │ transform    │  │ content-defined chunking  │  │
│  └──────────────┘  └──────────────┘  └──────────────────────────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                    KATMAN 2: OYUN KATMANI (Gameplay Layer)          │
│  Dosyalar: 11–14, 19–23, 30                                        │
│  ┌──────────┐ ┌────────┐ ┌──────────┐ ┌────────┐ ┌─────────────┐  │
│  │ WorldGen │ │ Physics│ │ Lighting │ │ Player │ │ AI/Entities  │  │
│  │ Terrain  │ │ Rapier │ │ L0-L4    │ │ Inv +  │ │ Behavior    │  │
│  │ Biome    │ │ +Custom│ │ hybrid   │ │ Ctrl   │ │ Tree + A*   │  │
│  │ Struc.   │ │ Destr. │ │ GI       │ │        │ │ Spawn/Loot  │  │
│  └──────────┘ └────────┘ └──────────┘ └────────┘ └─────────────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                    KATMAN 3: RENDER KATMANI (Render Layer)          │
│  Dosyalar: 09, 10, 24–26, 29                                       │
│  ┌────────────┐ ┌──────────┐ ┌──────────┐ ┌──────────┐ ┌───────┐  │
│  │ Meshing    │ │ Render   │ │ UI/HUD   │ │ Frustum  │ │ HDR + │  │
│  │ Greedy/GPU │ │ Pipeline │ │ glyphon  │ │ Culling  │ │ Bloom  │  │
│  │ VertexPool │ │ VisBuf   │ │ Flexbox  │ │ Hi-Z Occ │ │ Tone   │  │
│  └────────────┘ └──────────┘ └──────────┘ └──────────┘ └───────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                    KATMAN 4: ALTYAPI KATMANI (Infrastructure)       │
│  Dosyalar: 02, 03, 04, 08, 16–18, 31–33, 38                        │
│  ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐ ┌──────┐  │
│  │ ECS  │ │Plugin│ │Netw. │ │Server│ │Secur.│ │Debug │ │Mod.  │  │
│  │ Bevy │ │ API  │ │Renet2│ │Head. │ │Anti  │ │Prof. │ │Wasm  │  │
│  │  ECS │ │ Hook │ │+Repl.│ │Tokio │ │Cheat │ │Metr. │ │WIT   │  │
│  └──────┘ └──────┘ └──────┘ └──────┘ └──────┘ └──────┘ └──────┘  │
└─────────────────────────────────────────────────────────────────────┘
```

### 2.1 Veri Temsil Katmanı (4-Tier Streaming)

| Tier | Ad | Mesafe | Veri Formatı | Render | Fizik | Dosya |
|------|----|--------|-------------|--------|-------|-------|
| **1** | ACTIVE | 0-96m | XBrickMap | Ray trace / Greedy mesh | Rapier Voxels | `06`, `09`, `12` |
| **2** | WARM | 96-384m | XBrickMap + SVDAG | Brick öncelikli, SVDAG fallback | Rapier Voxels | `06`, `07`, `08` |
| **3** | DISTANT | 384m-1.5km | SVDAG only | GPU ray march | Yaklaşık collider | `07`, `10` |
| **4** | ARCHIVE | 1.5km+ | Compressed SVDAG (disk) | Render edilmez | Yok | `07`, `15` |

---

## 3. Sistem Haritası

Her sistemin hangi dosyada planlandığı, hangi crate'de implemente edileceği, temel prensipleri ve bağımlılıkları:

### 3.1 Çekirdek Sistemler

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **XBrickMap** | `06-xbrickmap.md` | `core::xbrickmap` | SOA+SIMD, 3-level palette, left-packed, branchless ray trace | Yok |
| **ECS Mimarisi** | `03-ecs-architecture.md` | `ecs` | Plugin-first, data-oriented, Bevy ECS 0.18+, bevy_replicon-compat | Yok |
| **Block Registry** | `05-block-registry.md` | `core::registry` | Data-driven (TOML), runtime genişletilebilir, bitmask flags | Yok |
| **Plugin API** | `04-plugin-api.md` | `plugin-api` | Plugin-first, lifecycle-managed, dependency-aware | Yok |
| **SVDAG** | `07-svdag.md` | `core::svdag` | Shared Node Pool, transform-aware dedup, shallow streaming, occupancy encoding | XBrickMap |
| **4-Tier Streaming** | `08-streaming.md` | `streaming` | 4-tier, predictive, ghost page table, pop-in-free | Core, SVDAG |

### 3.2 Render & Görsel Sistemler

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Meshing** | `09-meshing.md` | `meshing` | Trait-based, algorithm-agnostic, CPU+GPU, incremental | Core, BlockRegistry |
| **Render Pipeline** | `10-render-pipeline.md` | `render` | Unified visibility buffer, vertex pool, foveated, Hi-Z | Meshing, Streaming |
| **UI/HUD** | `24-ui-and-ux.md` | `ui` | ECS-based, GPU-accelerated (glyphon), responsive, themeable | Render |
| **Frustum Culling** | `10-render-pipeline.md` | `render::culling` | GPU compute, hierarchical, Hi-Z occlusion, temporal coherence | Render |
| **Particles/VFX** | `26-particles-vfx.md` | `particles` | GPU compute, ECS entegrasyonu, collidable, sorted | Render |
| **HDR Rendering** | `10-render-pipeline.md` | `render::hdr` | FP16 swapchain, tone mapping (ACES), bloom, exposure | Render |

### 3.3 Oyun Sistemleri

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **World Generation** | `11-world-gen.md` | `world-gen` | Deterministic, chunk-independent, biome-driven, structure-aware | Core, BlockRegistry |
| **Physics** | `12-physics.md` | `physics` | Rapier Voxels + custom, 3-tier collider update, tier-bazlı frekans | Core, ECS |
| **Lighting** | `13-lighting.md` | `lighting` | 5-kademeli hybrid (L0-L4), SIMD BFS, column-first sky, clustered GI | Core, SVDAG, Render |
| **Player/Inventory** | `14-inventory-player.md` | `player` | ECS-based, server-authoritative, input-agnostic | ECS, Core |
| **AI/Pathfinding** | `19-entities-and-ai.md` | `ai` | Behavior tree, voxel-aware A*, tier-bazlı, parallel | Core, ECS |
| **Entities** | `19-entities-and-ai.md` | `entities` | Spawn rules (biome/time), loot tables, lifecycle | Core, AI |
| **Crafting** | `20-crafting.md` | `crafting` | Recipe-driven, shapeless+shaped, furnace, anvil | Core, Player |
| **Fluids** | `22-fluids.md` | `fluids` | Cellular automata, su seviyesi 0-8, lava+water interaction | Core |
| **Building Tools** | `21-building-tools.md` | `building` | Selection (box/lasso), blueprint, copy/paste, transform | Core, Player |
| **Dynamic Events** | `30-progression-and-events.md` | `events`, `quests` | Event-driven, quest chains, rewards, dynamic scaling | Core, Entities |
| **Seasons** | `23-environment-time-weather.md` | `seasons` | Seasonal changes, calendar, weather integration, crop growth | DayNight, WorldGen |

### 3.4 Network & Multiplayer

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Network Sync** | `16-network-and-lag-compensation.md` | `network` | Tier-based delta sync, quantization, smallest-three quaternion, AOI | Core, SVDAG, Streaming |
| **Headless Server** | `17-server-and-security.md` | `server` | Tokio async, render-free, server-authoritative, low memory | All systems |
| **Lag Compensation** | `16-network-and-lag-compensation.md` | `network::prediction` | Client-side prediction, server reconciliation, entity interpolation | Network, Player |
| **Lobby & Browser** | `18-multiplayer-and-social.md` | `multiplayer` | Server list, favorites, LAN discovery, direct connect | Network |
| **Voice/Text Chat** | `18-multiplayer-and-social.md` | `chat` | Proximity-based voice, text chat, spatial audio, push-to-talk | Network, Audio |

### 3.5 Storage & Persistence

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Storage** | `15-storage-and-persistence.md` | `storage` | 3-tier hybrid (RAM→cache→disk), region files, SQLite, dedup | Core |
| **Save/Load** | `15-storage-and-persistence.md` | `save` | Player data + world metadata, auto-save, sessions | Storage |
| **Cloud Save** | `15-storage-and-persistence.md` | `cloud-save` | Auto-backup, sync, conflict resolution, versioning | Save |

### 3.6 Güvenlik & Kalite

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Security** | `17-server-and-security.md` | `security` | Server-authoritative, input validation, rate limiting, anti-cheat | Network, ECS |
| **Debug/Profiling** | `33-diagnostics-and-testing.md` | `debug` | Zero-cost when disabled, real-time metrics, tracing, GPU profiling | All systems |
| **Testing** | `33-diagnostics-and-testing.md` | `tests`, `benches` | 4-tier test pyramid, deterministic, CI/CD, performance regression | All systems |
| **Crash Telemetry** | `33-diagnostics-and-testing.md` | `telemetry` | Crash dump, opt-in, privacy-first, minimal perf impact | — |

### 3.7 Kullanıcı Deneyimi

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Settings** | `24-ui-and-ux.md` | `config` | Runtime config, graphics presets, keybinding, persist | — |
| **Animation** | `28-animation.md` | `animation` | ECS-based, skeletal+keyframe, blending, state machine | ECS, Entities |
| **Day/Night** | `23-environment-time-weather.md` | `daynight` | Dynamic time, gradient skybox, biome-specific weather | Lighting, Render |
| **Map** | `29-map.md` | `map` | Minimap + world map, fog of war, waypoints, biome colors | WorldGen, UI |
| **Localization** | `24-ui-and-ux.md` | `localization` | Key-based, runtime switching, fallback, mod support | UI |
| **Tutorial** | `30-progression-and-events.md` | `tutorial` | Interactive, contextual, skippable, non-intrusive | UI, Player |
| **Accessibility** | `24-ui-and-ux.md` | `accessibility` | Colorblind modes, subtitles, screen reader, UI scaling | UI, Render |

### 3.8 Platform & Dağıtım

| Sistem | Dosya | Crate | Prensipler | Bağımlılıklar |
|--------|-------|-------|------------|--------------|
| **Client Binary** | `31-client-binary.md` | `bin/client` | wgpu+winit, plugin-first, async init, graceful shutdown | All client crates |
| **Asset Pipeline** | `27-assets.md` | `assets` | Lazy loading, cache, hot-reload, PNG/KTX2/glTF | Render, Audio |
| **Modding** | `32-modding.md` | `modding` | Tiered (T0 data/T1 WASM/T2 native), WIT+Component Model, fizik/world-gen batch hook, permission+allowlist | Plugin API |
| **Update/Patch** | `38-update-patch.md` | `updater` | Delta patches, background download, rollback, integrity | — |
| **Platform Int.** | `37-platform-integration.md` | `platform` | Steamworks, Epic, GOG, cross-platform, feature flags | Achievements |
| **Media Capture** | `36-screenshot-video.md` | `media` | PNG/JPEG/WebM, GPU readback, HUD toggle, replay | Render |

---

## 4. Crate Bağımlılık Grafiği

```
                    ┌─────────────┐
                    │  plugin-api │
                    └──────┬──────┘
                           │
              ┌────────────┼────────────┐
              │            │            │
         ┌────┴───┐  ┌────┴───┐  ┌────┴───┐
         │  core  │  │   ecs  │  │  config │
         └───┬────┘  └────┬───┘  └────────┘
             │            │
    ┌────────┼─────┬──────┼──────┬──────┬──────────┐
    │        │     │      │      │      │          │
┌───┴───┐┌──┴──┐┌──┴──┐┌──┴──┐┌──┴──┐┌──┴──┐┌─────┴────┐
│world- ││mesh-││light││phys-││play-││ai   ││streaming │
│gen    ││ing  ││ing  ││ics  ││er   ││     ││          │
└───────┘└──┬──┘└──┬──┘└──┬──┘└─────┘└─────┘└─────┬────┘
            │      │      │                       │
       ┌────┴──────┴──────┴───────────┬────────────┘
       │                              │
   ┌───┴────────┐            ┌────────┴────────┐
   │   render   │            │    storage      │
   └───┬────────┘            └────────┬────────┘
       │                             │
   ┌───┼───┬──────┬──────┐      ┌────┴────┐
   │   │   │      │      │      │   save  │
┌──┴┐ ┌┴┐┌─┴──┐┌──┴───┐   └─────────┘
│ui │ │hdr││par-││media │
└───┘ └──┘│ti- │└──────┘
          │cles│
          └────┘

    ┌──────────────┐
    │   network    │
    └──┬───────┬───┘
       │       │
   ┌───┴───┐┌──┴──────┐
   │server ││security │
   └───────┘└─────────┘

    ┌──────────────┐
    │   modding    │
    └──────────────┘
```

### 4.1 Yardımcı Crate'ler (Bağımlılıksız veya az bağımlı)

| Crate | Bağımlılık | Açıklama |
|-------|-----------|----------|
| `audio` | ECS | 3D spatial ses |
| `animation` | ECS, Entities | Skeletal/keyframe animasyon |
| `crafting` | Core, Player | Recipe sistemi |
| `daynight` | Lighting, Render | Gün/gece döngüsü |
| `fluids` | Core | Su/lava simulasyonu |
| `commands` | ECS | Komut/console sistemi |
| `map` | WorldGen, UI | Minimap/world map |
| `localization` | UI | i18n |
| `achievements` | — | Achievement/statistics |
| `tutorial` | UI, Player | Tutorial/onboarding |
| `cloud-save` | Save | Cloud backup |
| `multiplayer` | Network | Lobby/browser |
| `chat` | Network, Audio | Voice/text chat |
| `accessibility` | UI, Render | Erişilebilirlik |
| `seasons` | DayNight, WorldGen | Mevsimler |
| `building` | Core, Player | Yapı araçları |
| `events` (quest) | Core, Entities | Dinamik event/quest |
| `updater` | — | Update/patch |
| `telemetry` | — | Crash/telemetry |
| `platform` | Achievements | Platform entegrasyonu |
| `assets` | Render, Audio | Asset pipeline |
| `debug` | Tümü | Debug/profiling |

---

## 5. Implementasyon Sırası (Faz Haritası)

Her fazın hangi plan dosyalarını kapsadığı:

### Faz 1 (Hafta 1-4): Temel Altyapı

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 1 | `06-xbrickmap.md`, `03-ecs-architecture.md`, `05-block-registry.md` | SectorCoord, XBrickMap, ECS skeleton, BlockRegistry |
| 2 | `06-xbrickmap.md` devam | get_block/set_block, bitmask ops, GlobalBrickPool |
| 3 | `09-meshing.md` (CPU kısmı) | GreedyMesher, basit render |
| 4 | `12-physics.md` (Tier 1/2) | Rapier Voxels collider, character controller, boundary sync |
| 4 | `13-lighting.md` (L0) | Direct light (sun, point lights), 16-bit packed LightData |

### Faz 2 (Hafta 5-8): Render + Streaming + Block/Sky Light

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 5 | `10-render-pipeline.md`, `10-render-pipeline.md` | wgpu pipeline, visibility buffer, frustum culling |
| 6 | `10-render-pipeline.md` devam | XBrickMap ray trace pass (WGSL), vertex pool |
| 7 | `08-streaming.md` | 4-tier sistemi, sector yükleme/boşaltma |
| 8 | `15-storage-and-persistence.md` | Region file format, SQLite metadata, rkyv+zstd |
| 7-8 | `13-lighting.md` (L1+L2) | L1 block light (CPU BFS), L2 sky light (column-first+heightmap) |

### Faz 3 (Hafta 9-12): SVDAG + Indirect GI

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 9 | `07-svdag.md` (CPU) | SVDAG builder (CPU), Shared Node Pool |
| 10 | `07-svdag.md` (GPU) | GPU SVDAG bake (compute shader) |
| 11 | `07-svdag.md`, `10-render-pipeline.md` | SVDAG ray march pass (WGSL) |
| 12 | `10-render-pipeline.md`, `10-render-pipeline.md` | Hi-Z occlusion, unified pipeline |
| 10-11 | `13-lighting.md` (L3+L4) | L3 clustered GI, L4 SVDAG cone trace, temporal accumulation |
| 12 | `13-lighting.md` devam | SIMD acceleration (wide), two-phase removal, mesh bake |

### Faz 4 (Hafta 13-18): Network + Lighting Optimizasyon

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 13 | `16-network-and-lag-compensation.md` | Brick delta sync |
| 14 | `16-network-and-lag-compensation.md` devam | SVDAG snapshot sync, quantization |
| 15 | `08-streaming.md` devam | Predictive preload |
| 16 | `13-lighting.md` (optimizasyon) | Hierarchical light culling, Morton Z-order, day/night cycle |
| 17 | `33-diagnostics-and-testing.md`, `33-diagnostics-and-testing.md` | Profiling, benchmark, GPU memory, cache |
| 18 | Tümü (optimizasyon) | Network, TPS, colored light mixing |

### Faz 5 (Hafta 19-24): Wasm Modding + Plugin API

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 19-24 | `32-modding.md`, `04-plugin-api.md` | Wasm runtime (wasmtime 45), WIT bindings, T2 native loader; `04` kesinleşti — entegrasyon |

### Faz 6 (Hafta 25-30): Storage + Gameplay Sistemleri

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 25-27 | `15-storage-and-persistence.md` (optimizasyon) | Dedup optimization (GearHash), GC/compaction, Merkle tree |
| 28-29 | `15-storage-and-persistence.md`, `14-inventory-player.md` | Save/load sistemi, player controller, inventory |
| 30 | `11-world-gen.md` | Terrain generation, biomes, structures |

### Faz 7 (Hafta 31-36): Gameplay + UX Sistemleri

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 31-32 | `19-entities-and-ai.md`, `19-entities-and-ai.md` | Behavior tree, A*, mob AI, loot tables |
| 33 | `20-crafting.md`, `22-fluids.md` | Crafting, fluid simulation |
| 34 | `24-ui-and-ux.md`, `24-ui-and-ux.md` | HUD, ayarlar sistemi |
| 35 | `25-audio.md`, `26-particles-vfx.md` | Spatial audio, particle system |
| 36 | `23-environment-time-weather.md`, `23-environment-time-weather.md` | Day/night, weather, seasons |

### Faz 8 (Hafta 37-42): Multiplayer + Security

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 37-38 | `17-server-and-security.md`, `16-network-and-lag-compensation.md` | Headless server, client prediction |
| 39 | `17-server-and-security.md` | Input validation, anti-cheat |
| 40 | `18-multiplayer-and-social.md`, `18-multiplayer-and-social.md` | Chat, lobby/browser |
| 41 | `37-platform-integration.md` | Steam/Epic integration |
| 42 | `17-server-and-security.md` | Command system |

### Faz 9 (Hafta 43-48): UX + Polish

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 43 | `29-map.md`, `24-ui-and-ux.md` | Minimap, i18n |
| 44 | `30-progression-and-events.md`, `30-progression-and-events.md` | Achievements, tutorial |
| 45 | `21-building-tools.md`, `30-progression-and-events.md` | Building tools, quests |
| 46 | `27-assets.md`, `28-animation.md` | Asset pipeline, animation |
| 47 | `10-render-pipeline.md`, `35-controller-gamepad.md` | HDR, gamepad |
| 48 | `36-screenshot-video.md`, `24-ui-and-ux.md` | Media capture, accessibility |

### Faz 10 (Hafta 49-52): Dağıtım + Bakım

| Hafta | Kapsanan Dosyalar | Çıktı |
|-------|------------------|-------|
| 49-50 | `38-update-patch.md`, `33-diagnostics-and-testing.md` | Update system, telemetry |
| 51 | `15-storage-and-persistence.md` | Cloud save |
| 52 | `31-client-binary.md`, `17-server-and-security.md` (final) | Release, profiling, benchmarks |

---

## 6. Teknik Yığın

### 6.1 Programlama Dili & Runtime

| Bileşen | Teknoloji | Versiyon |
|---------|-----------|----------|
| Dil | Rust | 2024 edition |
| ECS | Bevy ECS | 0.18+ |
| Render | wgpu | 29 (Vulkan/DX12/Metal) |
| Window | winit | 0.30 |
| UI Text | glyphon | GPU-accelerated text |
| Shader | WGSL | Native wgpu shader |
| Async | tokio | 1.x |
| Network | renet2 + bevy_replicon | 0.13 / 0.40 |
| Physics | bevy_rapier3d | 0.22+ (enhanced-determinism) |
| Compression | zstd | 0.13 |
| Database | rusqlite (SQLite WAL) | 0.32 |
| Hash | blake3 + xxhash-rust | 1.5 / 0.8 |
| Noise | fastnoise2 | 0.4 |
| SIMD | wide | 0.7 |
| Serialization | rkyv + postcard | 0.8 / 1.1 |
| Modding | wasmtime | 45.0 |
| SlotMap | slotmap | 1.0 |
| Hashing | ahash | 0.8 |
| Audio | bevy_audio | 3D spatial audio |

### 6.2 Donanım Gereksinimleri (Hedef)

| Bileşen | Minimum | Önerilen |
|---------|---------|----------|
| GPU | GTX 1060 (DX12/Vulkan) | RTX 3060+ (DX12/Vulkan) |
| VRAM | 2GB | 4GB+ |
| RAM | 8GB | 16GB |
| CPU | 4-core | 8-core (AMD Zen 3 / Intel 12th gen) |
| Storage | SSD | NVMe SSD |
| OS | Windows 10 22H2 | Windows 11 |

### 6.3 Geliştirme Araçları

| Araç | Kullanım |
|------|----------|
| cargo fmt | Code formatting |
| cargo clippy | Linting |
| cargo test | Unit/integration tests |
| cargo bench | Benchmark tests |
| tracing | Structured logging |
| GPU Timestamp Query | GPU profiling (wgpu) |
| GitHub Actions | CI/CD pipeline |

---

## Ek: Dosya İçerik Özetleri

Her plan dosyasının içerdiği temel başlıklar:

| Dosya | İçerdiği Alt Başlıklar |
|-------|----------------------|
| `01-overview.md` | Bu dosya — master indeks |
| `02-implementation.md` | Full crate organization (core/meshing/render/physics/lighting/network/storage/streaming), 6-phase implementation plan, glossary |
| `03-ecs-architecture.md` | Bevy ECS architecture, Plugin trait, World/Player/Entity/Render/Network components, SystemSets, system ordering, example systems, resources, events, messages |
| `04-plugin-api.md` | Plugin trait, SubApp (`take_extract`, write-back kanalı), `StrataCorePlugins` vs `StrataPlugin` (03), `On<E>` Observer, BlockRegistry entegrasyonu (05), wasmtime 45 özet (§7), L0–L4 modding (§10) |
| `05-block-registry.md` | Block ID structure, BlockRegistry, BlockDefinition (appearance/physics/lighting/gameplay), connectivity rules, state system, bitmask flags, TOML loading |
| `06-xbrickmap.md` | 3-level hierarchy (VoxelRT eXtendedBrickMap ref), `CompressedChunkData`, memory calc, GlobalBrickPool/SlotMap, branchless WGSL ray trace, GPU feedback, LOD traversal |
| `07-svdag.md` | Generational Node Pool, GpuHashTable, GPU allocator, variable-length node encoding, bake pipeline, transform-aware dedup, ghost page table, shallow SVDAG streaming, incremental unbake |
| `08-streaming.md` | 4-tier policy, SectorCoord (32³), determine_tier+hysteresis, LODError complement, dual-rep transitions, StreamingManager/resident set, hybrid priority (GPU feedback+predictor), pulse filter, frame budget, sector lifecycle, ECS system order, AOI/rate limits, test criteria |
| `09-meshing.md` | Mesher trait, MeshData/Vertex structs, GreedyMesher (CPU) with face mask + greedy merge + AO + vertex color, GpuMesher (compute), MesherRegistry, IncrementalMesher |
| `10-render-pipeline.md` | 6-pass unified pipeline, 64-bit visibility buffer layout, WGSL 64-bit atomic strategy, Hi-Z occlusion, GPU frustum culling, HDR Pipeline (FP16), ToneMapping, BloomPass |
| `11-world-gen.md` | Density-function terrain (per-voxel f(x,y,z)), Whittaker biome diagram, hybrid cave (3D noise isosurface + worm), hash-grid structure placement, template trees, thermal erosion, PCG32+wyhash RNG, WorldGenPlugin (AsyncComputeTaskPool), modding hooks (WIT) |
| `12-physics.md` | Rapier Voxels API (VoxelType/VoxelState), tier-based broad-phase (BVH + spatial hash), 3-tier incremental collider update, sector boundary sync, character controller + XBrickMap-optimized ground check, custom physics (falling sand, spatial hash), destruction (Voronoi fracture, fragment→rigid-body spawn), GPU physics vision (wgrapier), performance targets |
| `13-lighting.md` | 5-kademeli hybrid aydınlatma (L0–L4), 16-bit packed LightData (storage) + 8-bit/r16f shading, one-channel-per-lane SIMD + WLP oracle, BlockLightEngine (Dial 16-bucket BFS + Starlight dual-queue + two-phase removal), SkyLightEngine (column-continuity + GPU column DDA, JFA retracted), DDGI probe grid (canonical octahedral + Chebyshev; SH L2 retracted), ReSTIR GI + two-level radiance cache + LOD-anchored SVDAG march, hierarchical light culling (Morton 32-ary BVH), voxel-keyed temporal accumulation (SVGF/NRD), mesh light bake, NIV (Faz 6 distant-only experimental), Hillaire 2020 day/night, GPU pipeline |
| `14-inventory-player.md` | PlayerController (movement/jump/sprint/sneak), PlayerState (grounded/flying/game mode), Inventory (27 main + 4 armor + 9 hotbar + offhand), ItemStack (NBT + enchantments), block interaction (raycast + place/break), InputMapper + InputAction enum |
| `15-storage-and-persistence.md` | 3-tier hybrid storage (in-memory → LRU cache → disk), region file format (.strata), content-addressable dedup (xxHash64), async I/O strategy, SQLite schema, write-back pipeline, tier-based compression, GC + compaction, PlayerSaveData, WorldMetadata, SaveManager, CloudSaveManager (upload/download/sync) |
| `16-network-and-lag-compensation.md` | Tier-based delta sync, BrickDelta format, SVDAG snapshot sync, position quantization (i16), smallest-three quaternion compression, delta encoding, AOI/InterestManager, PredictionState, InterpolatedEntity, InputBuffer, server reconciliation flow |
| `17-server-and-security.md` | Server binary (tokio), ServerConfig, Server runtime, InputValidator, rate limiting, AntiCheat (suspicion scoring + auto-ban), server-side world update, CommandRegistry, Console (history/input/messages), built-in commands |
| `18-multiplayer-and-social.md` | ServerBrowser (filters + sort), ServerInfo (ping/players/map/mods), LobbyManager, LanDiscovery (UDP broadcast), VoiceChatManager (proximity/team/global), ChatManager (channels + messages) |
| `19-entities-and-ai.md` | Behavior tree (BtNode trait, Sequence/Selector/Parallel, Inverter/Repeater/Cooldown, Condition/Action), VoxelPathfinder (A* with octile heuristic), zombie/passive mob AI, Mob component, MobState, LootTable, SpawnRule |
| `20-crafting.md` | RecipeRegistry, Recipe (Shaped/Shapeless/Smelting), CraftingGrid, Furnace (input/fuel/output/progress) |
| `21-building-tools.md` | SelectionTool (Box/Lasso/Wand), Blueprint (blocks + metadata + transform), BlueprintTransform (Rotate/Mirror/Flip), BlueprintPreview (holographic) |
| `22-fluids.md` | FluidBlock (water/lava, level 0-8, is_source, flow_direction), cellular automata update |
| `23-environment-time-weather.md` | DayNightCycle (day_length/current_time/sun_position), WeatherState (current/target/transition/rain/snow/fog), SeasonManager, SeasonColors, Calendar, SeasonalEffects |
| `24-ui-and-ux.md` | UiNode (flexbox layout), HUD layout, GlyphonRenderer, UiInputHandler, GraphicsSettings, AudioSettings, ControlSettings, Localization (key-based, fallback, runtime switching), ColorblindFilter, SubtitleSystem, AccessibilitySettings |
| `25-audio.md` | AudioEngine (bevy_audio-based), SpatialListener, 3D spatial audio (inverse square attenuation + stereo pan + occlusion), SoundRegistry, BlockSounds, AmbientController |
| `26-particles-vfx.md` | GpuParticle struct, ParticleEmitter (Point/Area/Sphere/Box/Surface/Line), compute shader simulation (gravity/drag/collision/lifetime fade), presets, particle render shader |
| `27-assets.md` | AssetManager, TextureLoader (PNG/KTX2), hot-reload (notify watcher) |
| `28-animation.md` | Animator (state machine + bones + blending), AnimationClip (keyframes + duration), AnimationStateMachine (states + transitions) |
| `29-map.md` | Minimap (position/size/zoom/rotation), MapData (explored chunks + waypoints), ChunkMapData (colors), Waypoint system |
| `30-progression-and-events.md` | Achievement (condition-based unlock), PlayerStatistics, TutorialManager (steps + triggers), HintSystem, WorldEventManager (active_events + cooldowns), Quest (objectives + rewards) |
| `31-client-binary.md` | Client entry point, config (TOML), Client struct, wgpu+winit init, Bevy ECS plugins, init sequence, workspace structure, Cargo.toml dependencies |
| `32-modding.md` | Tiered model (T0 data / T1 WASM / T2 native), WIT+Component Model interface'leri (block-registry/world-read/world-write/**physics-hooks**/**worldgen-hooks**/entities/network/ui/events/timers/logging), WasmRuntime (wasmtime 45 + cranelift), opsiyonel/bütçeli on-tick, fizik batch policy hook bus, capability→kademe eşlemesi + sunucu allowlist, determinizm/server-authoritative, compile-time native (T2) |
| `33-diagnostics-and-testing.md` | DebugHUD panels, MetricsCollector, GpuProfiler, BenchmarkRunner, Unit tests, integration tests, CI/CD pipeline, CrashReporter, TelemetryCollector |
| `34-performance.md` | Render/physics/network/streaming/storage performance targets, 30+ risk/mitigation entries, 20+ rejected alternatives |
| `35-controller-gamepad.md` | GamepadManager (XInput/DirectInput), GamepadConfig (deadzone/vibration/button_map), VibrationController (rumble/trigger_pulse) |
| `36-screenshot-video.md` | ScreenshotManager (PNG/JPEG/BMP), VideoCapture (WebM/MP4), ReplaySystem (events + player snapshots) |
| `37-platform-integration.md` | SteamIntegration (overlay/achievements/friends/rich presence), PlatformProvider trait, Platform enum (Steam/Epic/GOG/Standalone) |
| `38-update-patch.md` | UpdateManager (check/download/apply/rollback), Version (semver), DeltaPatch (New/Modified/Deleted), IntegrityChecker (FileManifest + verify + repair) |

---

*Bu doküman, `plans/` dizinindeki tüm 38 plan dosyasının kapsamlı bir indeksi, mimari haritası ve implementasyon yol haritasıdır. Herhangi bir sistem hakkında detaylı bilgi için ilgili dosyaya başvurun.*

---

## 7. Konsolide Araştırma Bulguları (2026-06)

> **Kaynak:** 5 ayrı araştırma worker'ı (toplam 40+ WebSearch sorgusu, SIGGRAPH/akademik paper'lar, Rust/Bevy ekosistem kaynakları, karşılaştırmalı voxel motor analizi).

### 7.1 Genel Değerlendirme

Strata'nın kesinleşmiş planları (01-16) 2024-2026 SOTA araştırmalarıyla **yüksek uyumlu**. Temel mimari değişiklik gerekmiyor — tüm öneriler **eklemeli**.

### 7.2 Doğrulanan Kararlar (Değişiklik Gerekmez)

| Karar | Kaynak | Doğrulama |
|-------|--------|-----------|
| Flat `crates/` layout | Plan 02 | matklad/rust-analyzer validated |
| Archetype-based ECS | Plan 03 | SAC 2026 benchmark: ~10× faster than OOP |
| XBrickMap 3-level bitmask + palette | Plan 06 | SOTA aligned |
| SVDAG + ghost page starvation-free | Plan 07 | GigaVoxels DP comparable, wgpu uyumlu |
| Aokana visibility buffer | Plan 10 | SOTA, no voxel-specific alternative found |
| BlockEntity hybrid pattern | Plan 03/05 | Unity community validated |
| T0/T1/T2 modding tiers | Plan 04 | cyubeVR validated |
| Binary greedy meshing + PackedQuad | Plan 09 | SOTA aligned |
| 4-tier streaming architecture | Plan 08 | Aokana + GigaVoxels validated |

### 7.3 P0 — Kesin Öneriler (Phase 1)

| # | Öneri | Plan | Etki |
|---|-------|------|------|
| 1 | **Bevy 0.17+ API terminology** | 03/04 | `EventWriter→MessageWriter`, lifecycle isimleri |
| 2 | **`bitflags` crate** | 05 | Type-safe BlockFlags, zero overhead |
| 3 | **`cargo-hakari`** | 02 | ~%50 build time kazancı |
| 4 | **Change detection optimization** | 03 | `set_if_neq()`, `bypass_change_detection()` |

### 7.4 P1 — Önerilen Öneriler (Phase 1-2)

| # | Öneri | Plan | Etki |
|---|-------|------|------|
| 5 | **`lld` linker** | 02 | Windows'ta 5-30s linking kazancı |
| 6 | **`strata-types` crate** | 02 | Ortak tipler ayrı crate |
| 7 | **LODError zorunlu** | 08 | "Opsiyonel" → "zorunlu" |
| 8 | **SSE bazlı LOD** | 08 | FOV/DPI adaptif |
| 9 | **TOML+RON hibrit** | 05 | Enum/state tanımları RON |

### 7.5 P2 — Değerlendir (Phase 2-3)

| # | Öneri | Plan | Etki |
|---|-------|------|------|
| 10 | **AADF cache** | 06 | XBrickMap RT ~10× hızlanma |
| 11 | **Full occupancy encoding** | 07 | SVDAG traversal %10-15 hız |
| 12 | **Geometry/color separation** | 07 | LOD 1+ SVDAG %5-15 VRAM |
| 13 | **`soa-rs` / AoSoA** | 05 | GPU-critical hot path (benchmark gerekli) |
| 14 | **R64Uint visbuffer** | 10 | Cache locality %15-30 |
| 15 | **SPD Hi-Z build** | 10 | ~0.15ms tasarruf |

### 7.6 P3 — Gelecek (Phase 5+)

| # | Öneri | Plan | Etki |
|---|-------|------|------|
| 16 | **WIT Component Model** | 04 | Raw ABI → typed WIT interface |
| 17 | **WASM production hardening** | 04 | Fuel/epoch/memory cap |
| 18 | **AgX tonemapper** | 10 | HDR hue-preserving |
| 19 | **VRCS deblocking filter** | 10 | Foveated shading smoothing |
| 20 | **Tagged palette entry** | 05 | 2/4 byte packing |

### 7.7 Bevy 0.17+ API Değişiklikleri (Kritik)

| Eski (≤0.16) | Yeni (0.17+) | Etkilenen Plan |
|---|---|---|
| `EventWriter` | **`MessageWriter`** | 03, 04 |
| `EventReader` | **`MessageReader`** | 03, 04 |
| `OnAdd` / `OnRemove` | **`Add`** / **`Remove`** / **`Insert`** / **`Replace`** / **`Despawn`** | 03 |
| `Trigger<E>` | **`On<E>`** | 04 |

**Not:** `Event` trait ve `app.add_event::<T>()` aynı kalır; sadece reader/writer isimleri değişti.

---

*Son güncelleme: 2026-07-06 (Konsolide araştırma raporu eklendi)*
