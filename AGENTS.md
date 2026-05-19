# AGENTS.md — Strata

Rust voxel engine (Windows x64). Design spec: `project_design.md`.

## Stack (verified from design doc)
- **Rust 2024 Edition**, Cargo Workspace
- **Bevy ECS 0.18+** (not full Bevy — ECS only)
- **wgpu 29+** / **winit 0.30+** / **glyphon 0.12+**
- **tokio 1.x** async runtime
- **renet2 0.13+** + **bevy_replicon 0.39+** + **bevy_replicon_renet2 0.14+**
- **glam 0.29+**, **rkyv 0.8+**, **postcard 1.1+**
- **fastnoise2 0.4+** (C++ build dependency — accepted for SIMD performance)
- **wasmtime 30+** (Wasm modding), **fjall 3.0** (Faz 2 storage)
- **bevy_rapier 0.33** (physics, `enhanced-determinism` feature)

## Workspace structure (planned)
```
crates/
  core/       — Block registry, Chunk (Vec<u16> flat array), World coords, Registry
  ecs/        — Bevy ECS components & systems, plugin base trait
  world-gen/  — Procedural terrain (fastnoise2 FBM), biomes, structures
  meshing/    — Mesher trait + MeshData, classic greedy (CPU), GPU compute (Faz 2)
  render/     — wgpu pipeline, frustum culling, chunk rendering, WGSL shaders
  network/    — renet2 + bevy_replicon, chunk sync, interest management
  storage/    — Custom binary+zstd (Faz 1), fjall KV store (Faz 2)
  modding/    — wasmtime runtime, WIT interfaces, native .dll loader
  physics/    — bevy_rapier wrapper, AABB, collision, raycast
  lighting/   — Minecraft-style light propagation (BFS, 15 levels sky+block)
  plugin-api/ — Plugin trait, registry, hooks, lifecycle
bin/
  client/     — Game client (winit + wgpu entrypoint)
  server/     — Headless server (tokio + ECS, no render)
```

## Key conventions
- **Plugin-first architecture**: Every subsystem is a plugin, not monolithic
- **Chunk**: 16×256×16, `Vec<u16>` flat array (NOT ndarray, NOT bit-packed), index = `x + z*16 + y*256`
- **Mesher trait**: Algorithm-agnostic — render crate doesn't know which mesher runs
- **Platform**: Windows x64 only (ADR-001)
- **No deterministic lockstep** in early phases — server-authoritative + client interpolation (ADR-008)

## Commands (once workspace exists)
- Build: `cargo build --workspace`
- Lint: `cargo clippy --workspace -- -D warnings`
- Format: `cargo fmt`
- Test: `cargo test --workspace`
- Single crate test: `cargo test -p <crate-name>`

## Code style
- `clippy` warnings = error
- `rustfmt` required
- `///` doc comments on all public API
- Commit format: `type(scope): description` (feat, fix, perf, refactor, docs, test, chore)

## Branch strategy
- `main` — production-ready
- `dev` — active development
- `feature/*` — new features
- `hotfix/*` — critical fixes

## Development phases
1. **Faz 1** (Weeks 1-4): Workspace, core, ecs, world-gen, meshing trait+greedy, storage binary+zstd, client window, physics
2. **Faz 2** (Weeks 5-8): Full render pipeline, lighting, frustum culling, GPU compute meshing
3. **Faz 3** (Weeks 9-12): Physics, player controller, entities, inventory
4. **Faz 4** (Weeks 13-18): Network (renet2+replicon), headless server, multiplayer
5. **Faz 5** (Weeks 19-24): Wasm modding (wasmtime+WIT), plugin-api refactor, native core-mods
6. **Faz 6** (Weeks 25-30): GPU lighting, fjall migration, profiling, benchmarks

## Performance targets
- 100+ chunks @ 60+ FPS
- CPU meshing <500µs/chunk, GPU <50µs/chunk
- Server 20 TPS stable, 1000+ players
- Client <2GB RAM, Server <512MB/100 players
