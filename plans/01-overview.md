# 00 — Chunk Architecture Overview

> **Son güncelleme:** 2026-05-20
> **Durum:** Onaylanmış — Uygulama aşamasına hazır
> **Not:** Mevcut `Vec<u16>` chunk sistemi tamamen değiştirilecek. Bu doküman serisi yeni mimarinin tek kaynağıdır.

---

## İçindekiler (Dosya Serisi)

| # | Dosya | Konu |
|---|---|---|
| 01 | `01-overview.md` | Genel bakış, dünya organizasyonu, temel prensipler |
| 02 | `02-xbrickmap.md` | XBrickMap veri yapısı, SOA+SIMD, ray tracing |
| 03 | `03-svdag.md` | SVDAG, Shared Node Pool, Transform-Aware, Shallow Streaming |
| 04 | `04-streaming.md` | 4-Tier streaming sistemi, predictive streaming |
| 05 | `05-render-pipeline.md` | Unified visibility buffer, Hi-Z, Vertex Pool, Foveated |
| 06 | `06-physics.md` | Rapier Voxels, BVH, character controller, destruction, GPU physics |
| 07 | `07-lighting.md` | 5-kademeli hybrid aydınlatma (L0-L4) |
| 08 | `08-network.md` | Network senkronizasyonu, delta compression, AOI |
| 09 | `09-storage.md` | Hybrid tiered storage, SQLite, dedup, content-defined chunking |
| 10 | `10-performance.md` | Performans hedefleri, riskler, alternatifler |
| 11 | `11-implementation.md` | Crate organizasyonu, uygulama sırası, sözlük |

---

## 1. Genel Bakış

Strata, 4-kademeli (tier) hiyerarşik bir voxel veri sistemi kullanır. Her kademe oyuncuya olan mesafeye göre farklı bir veri temsil formatı kullanır. Bu yaklaşım, **edit hızı**, **render performansı**, **bellek verimliliği** ve **streaming** arasında Pareto-optimal dengeyi sağlar.

### Temel Prensipler

- **Yakın = Brickmap (XBrickMap):** O(1) edit, 4-level ray skip, doğrudan fizik
- **Orta = Brickmap + SVDAG birlikte:** Pop-in olmadan yumuşak geçiş
- **Uzak = SVDAG:** Deduplication, LOD, GPU ray march
- **Çok uzak = Sıkıştırılmış SVDAG:** Disk, zstd + rkyv, lazy streaming

### Kanıtlanmış Referanslar

| Bileşen | Kaynak |
|---|---|
| XBrickMap | dubiousconst282/VoxelRT (2024) — en hızlı ray trace yöntemlerinden |
| SVDAG + GPU Editing | GPU-SVDAG-Editing, Pacific Graphs 2024 |
| Aokana Framework | Fang et al., ACM SIGGRAPH 2025 — 4.8x hız, 9x VRAM azalması |
| Hybrid Voxel Formats | Molenaar & Eisemann, Eurographics 2024 |
| Transform-Aware SVDAG | Molenaar & Eisemann, SIGGRAPH 2025 — %20-45 ek deduplication |
| Shallow SVDAG Streaming | Fang et al., Aokana, SIGGRAPH 2025 — %5 VRAM, 2-4× hız |
| Vertex Pooling | Nick McDonald — %40 frame time, %25 meshing time azalması |
| Foveated Rendering | SIGGRAPH 2025 — %60-80 ray/pixel azalması, %99.3 periferik animasyon |
| Rapier Voxels Shape | dimforge/rapier 0.32+ / parry3d 0.26+ — native sparse voxel collider |
| WGSL 64-bit Atomics | wgpu PR #5383 (2024) — SHADER_INT64_ATOMIC_ALL_OPS / MIN_MAX |
| GearHash Chunking | HuggingFace Xet — content-defined chunking, BLAKE3 Merkle tree |
| Delta Compression | Network quantization — smallest-three quaternion, varint delta |
| AOI / Interest Management | Spatial partitioning — %80-90 bant genişliği azalması |
| BFS Flood-Fill Lighting | Seed of Andromeda (2015), voxel-light crate (2026) |
| Starlight Propagation | PaperMC/Starlight — Vanilla'dan 28x hızlı |
| SIMD Flood-Fill | atrufulgium.net (2024) — 128 voxel/iterasyon, 15x hızlanma |
| Clustered Voxel GI | Ayerbe & Patow, CGF 2022 — 100x az visibility test |
| Hierarchical Bitmask Culling | SCITEPRESS 2024 — Morton Z-order + light culling |
| TU Wien RGI | Ott et al., 2025 — voxel-specific TAA, noise-free path tracing |
| Neural Irradiance Volume | Adobe, 2024 — 1-5MB, ~1ms inference, noise-free GI |
