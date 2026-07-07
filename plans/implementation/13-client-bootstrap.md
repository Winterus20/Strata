# 13 — Client Bootstrap (M9)

**Kaynak:** `31-client-binary.md`, `44-build-strategy.md`
**Hedef:** `bin/client` wgpu+winit başlatıcı; tüm plugin'leri sırayla bağla; graceful shutdown.

## 1. Binary Yapısı (`44` §2)
```
bin/client/
├── Cargo.toml   # bevy + wgpu + winit + bevy_rapier3d (render/audio GPU feat)
└── src/main.rs  # pencereli, grafikli başlatıcı
```
- **Headless-safe ayrım:** `bin/client` ONLY GPU feat; `bin/server` (sonra) GPU'suz.

## 2. Init Sequence (31)
1. `App::new()` + `DefaultPlugins` (winit window, wgpu, input).
2. `bevy_rapier3d` plugin (physics).
3. `StrataCorePlugins` (02): core, world, meshing, physics, lighting, player, render.
4. `StreamingManager` init (12) → ilk sector batch generate.
5. `RenderPipeline` (07) wgpu device/swapchain.
6. Graceful shutdown: `AppExit` → pool free + SSBO drop.

## 3. Config (31 §config TOML)
- `client.toml`: window size, vsync, view distance (radius, 12), quality preset.
- Runtime reload (sonraki faz); prototipte init-time read.

## 4. Workspace (44)
- `bin/client` workspace üyesi; `cargo run -p client`.
- dynamic_linking dev'de açık → < 3 s incremental (01).

## 5. Adımlar
1. `bin/client/Cargo.toml` (GPU feat only).
2. `main.rs`: plugin zinciri + config load.
3. `StreamingManager` ilk batch (player çevresi R sector).
4. wgpu device + swapchain (07).
5. Shutdown hook (pool free).

## 6. Doğrulama
- `cargo run -p client` → pencere + terrain + yürüme + break/place.
- `cargo build --release` → tek .exe (dynamic_linking kapalı).
- Perf: steady 60+ FPS (ACTIVE M0-M9 bütçe içinde).

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Plugin init sırası | Açık `depends_on` (03); boot log doğrula |
| GPU feat sızıntısı server'a | `bin/client` isolate; `bin/server` sonra ayrı |
