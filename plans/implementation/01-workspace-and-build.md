# 01 — Workspace & Build Strategy (M0)

**Kaynak:** `02-implementation.md`, `44-build-strategy.md`
**Hedef:** Hybrid cargo workspace, hızlı iterasyon, headless-safe ayrım.

## 1. Workspace Manifest
`Cargo.toml` (root) — `[workspace]` + `resolver = "3"`. Üye:
```
crates/strata_core      # ECS componentler, math, types (ortak)
crates/strata_world     # XBrickMap, world gen
crates/strata_physics   # Rapier wrapper
crates/strata_render    # wgpu pipeline, meshing
crates/strata_player    # player, input, interaction
bin/client              # wgpu+winit, grafikli
```
> `strata_network` prototipte YOK (declared later). `bin/server` prototipte YOK.

## 2. Crate Sorumlulukları (DDD, agonsuz)
- `strata_core`: `SectorCoord`, `BlockId`, `SectorTransform`, ZST marker'lar (`ChunkDirty`, `NeedsRemesh`), `GlobalBrickPool` (06). **Hiçbir GPU bağımlılığı yok** → server sonra paylaşır.
- `strata_world`: `XBrickMap`, `WorldGenPlugin`. Bağımlı: core.
- `strata_physics`: `bevy_rapier3d` wrapper, voxel collider sync. Bağımlı: core, world.
- `strata_render`: wgpu, meshing, visbuf. Bağımlı: core, world.
- `strata_player`: controller + interaction. Bağımlı: core, world, physics.
- `bin/client`: `bevy` + `wgpu` + `winit` + `bevy_rapier3d`. **render/audio GPU özellikleri sadece burada.**

## 3. Build Optimizasyonu (`44` §3)
- `.cargo/config.toml`:
  - Windows: `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "link-arg=/LINKER:rust-lld"]` (veya `-Clinker-flavor=lld-link`).
  - Linux: `mold` (`.cargo/config.toml` `linker = "clang"` + `-fuse-ld=mold`).
- `Cargo.toml` `[profile.dev]`: `opt-level = 1`, `debug = true`, `split-debuginfo = "..."` ; bağımlılıklar için `[profile.dev.package."*"] opt-level = 2`.
- `bevy` `dynamic_linking` feature **dev**'de açık, **release**'te kapalı.
- `cargo-hakari` workspace için (P0-3): `~%50 build time`.

## 4. Adımlar
1. Root `Cargo.toml` + `.cargo/config.toml` (linker).
2. Boş crate iskeletleri + `lib.rs` (her birine minimal `pub mod`).
3. `bevy` 0.18 + `bevy_rapier3d` 0.34 (enhanced-determinism) pin'le.
4. `bin/client/src/main.rs`: boş Bevy `App` + winit pencere (siyah ekran doğrulaması).
5. `cargo build -p client` → < 3 s incremental (dynamic linking) doğrula.

## 5. Doğrulama
- `cargo build` temiz; `cargo clippy --all-targets` temiz.
- `bin/client` pencere açar, boş render.

## 6. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| bevy_rapier3d 0.34 ↔ Bevy 0.18 uyumu | Fixed version; feature `enhanced-determinism` |
| rust-lld Windows path | `cargo +nightly` gerekmez; stable rust-lld bundled |
| hakari karmaşıklık | Sonraki faz; M0'da bypass |
