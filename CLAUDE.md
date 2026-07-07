# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Strata** is a voxel sandbox game engine written in Rust, targeting Windows. It uses a 4-tier hybrid data structure (XBrickMap + SVDAG) for unlimited height, efficient editing, and high compression. The project is currently in the **planning phase** — all systems are documented in `plans/` but no source code exists yet.

**Tech stack:** Rust (2024 edition), Bevy ECS 0.18, wgpu 29, winit 0.30, renet2 + bevy_replicon, wasmtime 45.0, bevy_rapier3d, rusqlite, zstd, rkyv, fastnoise2, wide (SIMD).

## Critical: Read the Overview First

**Before doing any work in this repository, you MUST read `plans/01-overview.md`.** This file is the master index for all 38 plan files and contains:
- Architecture overview and 4-layer system design
- System map (which crate implements which system, dependencies)
- Crate dependency graph
- Implementation phase map (10 phases, week-by-week)
- Full technical stack with versions
- Hardware target requirements

Without reading this file first, you will lack the context needed to make correct decisions.

## Repository Structure

```
Strata/
├── plans/                          # All design documents (38 files)
│   ├── 01-overview.md              # Master index — START HERE
│   ├── 02-implementation.md        # Crate organization, glossary
│   ├── 03-ecs-architecture.md      # Bevy ECS design
│   ├── 04-plugin-api.md            # Plugin trait, lifecycle
│   ├── 05-block-registry.md        # Block definitions (TOML)
│   ├── 06-xbrickmap.md             # Core data structure
│   ├── 07-svdag.md                 # SVDAG compression
│   ├── 08-streaming.md             # 4-tier streaming
│   ├── 09-meshing.md               # CPU/GPU meshing
│   ├── 10-render-pipeline.md       # Visibility buffer, HDR
│   ├── 11-world-gen.md             # Terrain, biomes, structures
│   ├── 12-physics.md               # Rapier Voxels, destruction
│   ├── 13-lighting.md              # 5-tier hybrid lighting (L0-L4)
│   ├── 14-inventory-player.md      # Player controller, inventory
│   ├── 15-storage-and-persistence.md # Region files, SQLite, cloud save
│   ├── 16-network-and-lag-compensation.md # Delta sync, prediction
│   ├── 17-server-and-security.md   # Headless server, anti-cheat
│   ├── 18-multiplayer-and-social.md # Lobby, voice/text chat
│   ├── 19-entities-and-ai.md       # Behavior tree, A* pathfinding
│   ├── 20-crafting.md              # Recipes, crafting grid
│   ├── 21-building-tools.md        # Selection, blueprints
│   ├── 22-fluids.md                # Cellular automata fluids
│   ├── 23-environment-time-weather.md # Day/night, seasons
│   ├── 24-ui-and-ux.md             # HUD, settings, i18n
│   ├── 25-audio.md                 # 3D spatial audio
│   ├── 26-particles-vfx.md         # GPU compute particles
│   ├── 27-assets.md                # Asset pipeline, hot-reload
│   ├── 28-animation.md             # Skeletal/keyframe animation
│   ├── 29-map.md                   # Minimap, world map
│   ├── 30-progression-and-events.md # Achievements, quests
│   ├── 31-client-binary.md         # Client entry point
│   ├── 32-modding.md               # Wasm mods (wasmtime + WIT)
│   ├── 33-diagnostics-and-testing.md # Debug HUD, profiling, CI/CD
│   ├── 34-performance.md           # Targets, risks, alternatives
│   ├── 35-controller-gamepad.md    # Gamepad support
│   ├── 36-screenshot-video.md      # Media capture, replay
│   ├── 37-platform-integration.md  # Steam/Epic/GOG
│   ├── 38-update-patch.md          # Delta patching, updater
│   └── implementation/
│       └── 02-xbrickmap-implementation-plan.md
└── AGENTS.md                       # (empty)
```

## Architecture (4 Layers)

1. **Data Layer** — XBrickMap (O(1) edit, SOA+SIMD), SVDAG (compression, LOD), Storage (region files, SQLite)
2. **Gameplay Layer** — WorldGen, Physics (Rapier Voxels), Lighting (L0-L4 hybrid), Player, AI/Entities
3. **Render Layer** — Meshing (greedy/GPU), Visibility Buffer pipeline, UI/HUD, Frustum Culling, HDR+Bloom
4. **Infrastructure Layer** — ECS (Bevy), Plugin API, Network (renet2), Server, Security, Debug, Modding (Wasm)

## 4-Tier Streaming Model

| Tier | Name | Distance | Data Format | Render Method |
|------|------|----------|-------------|---------------|
| 1 | ACTIVE | 0-96m | XBrickMap | Ray trace / Greedy mesh |
| 2 | WARM | 96-384m | XBrickMap + SVDAG | Brick priority, SVDAG fallback |
| 3 | DISTANT | 384m-1.5km | SVDAG only | GPU ray march |
| 4 | ARCHIVE | 1.5km+ | Compressed SVDAG (disk) | Not rendered |

## Development Commands

Since the project is in planning phase, no build/test commands exist yet. When implementation begins:

```bash
# Build
cargo build

# Run client
cargo run -p client

# Run headless server
cargo run -p server

# Tests
cargo test
cargo test -p <crate-name>      # Single crate
cargo test <test_name>           # Single test

# Lint & Format
cargo fmt
cargo clippy

# Benchmarks
cargo bench
```

## Key Design Decisions

- **Server-authoritative**: All game state validated server-side; clients are thin renderers
- **Plugin-first architecture**: All systems are Bevy plugins with defined lifecycle hooks
- **Tier-based processing**: Physics, lighting, and rendering frequency varies by distance tier
- **Data-driven blocks**: Block definitions loaded from TOML, runtime-extensible via modding
- **Wasm modding**: Sandbox via wasmtime + WIT interface, permission-based API access
- **Deterministic world gen**: Chunk-independent, seed-driven, reproducible across clients

## Implementation Phases

The project follows a 10-phase, 52-week plan (see `plans/01-overview.md` §5):
1. **Phase 1 (Weeks 1-4):** Core infrastructure — XBrickMap, ECS, BlockRegistry, basic meshing, physics, lighting L0
2. **Phase 2 (Weeks 5-8):** Render pipeline, streaming, storage, block/sky light
3. **Phase 3 (Weeks 9-12):** SVDAG, indirect GI, Hi-Z occlusion
4. **Phase 4 (Weeks 13-18):** Network sync, lighting optimization, profiling
5. **Phase 5 (Weeks 19-24):** Wasm modding, Plugin API
6. **Phase 6 (Weeks 25-30):** Storage optimization, save/load, world gen
7. **Phase 7 (Weeks 31-36):** AI, crafting, fluids, UI, audio, particles, day/night
8. **Phase 8 (Weeks 37-42):** Multiplayer, security, server, chat, platform integration
9. **Phase 9 (Weeks 43-48):** Map, achievements, tutorial, animation, HDR, accessibility
10. **Phase 10 (Weeks 49-52):** Update system, telemetry, cloud save, release

## Plan File Conventions

- All plan files are in Turkish
- Each file covers one system with: data structures, algorithms, Rust structs, shader code, performance targets
- Cross-references between files use file numbers (e.g., `06-xbrickmap.md`)
- When implementing, always read the relevant plan file(s) first
- **Plan maturity** (`plans/01-overview.md` §1.1): **`01`–`15` finalized** (constitution — do not contradict in code or drafts); **`16`–`38` draft** (may change; on conflict, `01`–`15` win). See also `AGENTS.md` §2.
