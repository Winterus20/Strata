# Strata: Next-Gen Voxel Engine Architecture (SVDAG Hybrid)

This document outlines the advanced architectural upgrade plan for Strata, migrating from a standard Minecraft-style pillar chunk system to a state-of-the-art **Hybrid SVDAG (Sparse Voxel Directed Acyclic Graph) + Visibility Buffer** pipeline.

## 1. The Vision: Hybrid Architecture
To balance the O(1) edit cost required by a multiplayer game with the massive Level of Detail (LOD) and raytracing capabilities of modern voxel engines, Strata will adopt a hybrid approach:
- **Active Area (Near):** Flat arrays (`Vec<u16>`) managed by CPU. Allows instant block edits, Bevy/Rapier physics integration, and O(1) network syncing.
- **LOD Area (Far):** GPU-driven SVDAG. Provides massive draw distances (e.g., 64K resolution equivalent) with minimal memory footprint.

## 2. The 4 Core Pillars of the Architecture

### I. GPU-Side Dirty Flag (Compute Shader Merge)
- **Concept:** CPU handles logical edits (block placements) and writes a tiny command (`x, y, z, block_id`) to a GPU Ring Buffer. The GPU periodically merges these changes into the SVDAG via Compute Shaders.
- **Benefit:** Completely eliminates the CPU bottleneck of re-baking the SVDAG. Reduces bake time from ~200ms (CPU) to ~15ms (GPU).
- **Implementation in `wgpu`:** Use `StorageBuffer` and WGSL `atomicAdd` to manage the ring buffer.

### II. Global Shared Node Pool (Inter-Chunk Deduplication)
- **Concept:** Instead of each chunk having its own SVDAG, the entire world shares a single Node Pool. Identical terrain features (e.g., flat plains, repeating trees) across different chunks will point to the exact same memory address.
- **Benefit:** ~30% extra memory savings. Network can transmit node indices instead of raw voxel data if the client already has the node cached.
- **Implementation:** Requires a **Lock-Free Slab Allocator** on the GPU.

### III. Unified 64-bit Visibility Buffer
- **Concept:** Unify the two disparate rendering pipelines (Rasterized near chunks + Raymarched far SVDAG) into a single screen-space visibility buffer.
- **Layout (64-bit):**
  - 24-bit Depth
  - 3-bit Normal
  - 13-bit Chunk ID
  - 24-bit Voxel Coord
- **Benefit:** Resolves complex depth-testing between triangles and raymarched volumes. Allows a single G-Buffer resolve pass for lighting and PBR.

### IV. Predictive Streaming + LOD Pre-warm
- **Concept:** Use Bevy's ECS (`Velocity`, `Transform`) to predict player movement vector.
- **Benefit:** Pre-loads low-resolution SVDAG nodes for chunks the player is *about to* enter, eliminating "pop-in" completely.

---

## 3. Web Verification & Implementation Feasibility (WGPU / Rust)

Based on recent (2025-2026) internet research and WebGPU specifications, here are the verified technical requirements and risks for implementing this in Strata:

### Aokana SVDAG Paper (2025)
> **Verified:** The "Aokana" paper (published May 2025) indeed proposes a breakthrough GPU-driven voxel rendering framework utilizing SVDAG for massive open worlds. The "multiple shallow SVDAGs" and sub-10ms render times are achievable on modern hardware, confirming the viability of the hybrid approach.

### Lock-Free Slab Allocator in WGSL
> **Verified / Implementation Detail:** WGSL does not have native dynamic memory allocation (`malloc`/`free`). 
> **How to build:** You must pre-allocate a massive `StorageBuffer` acting as the heap. The allocator requires a "Free List" managed by `atomic<u32>` operations (`atomicAdd`, `atomicCompareExchangeWeak`). 
> **Risk:** Reference Counting is mandatory to ensure nodes shared by multiple chunks are not prematurely freed when one chunk is unloaded. 

### 64-bit Visibility Buffer in WGPU
> **Verified / Implementation Detail:** WebGPU/WGSL does not natively support a standard `u64` scalar type for all operations. 
> **How to build:** You must pack the 64-bit data into a `vec2<u32>` inside your WGSL structs. 
> **Critical Requirement:** To do hardware depth-testing (e.g., `atomicMin` on the 64-bit depth value) from a compute shader, you **must** request specific native extensions when creating the `wgpu::Device` (such as `wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS`). If the target hardware (e.g., older GPUs or strict WebGL fallbacks) doesn't support this, you will need to fallback to a 32-bit packed visibility buffer or software atomic emulation.

## 4. Roadmap Integration for Strata
- **Faz 1-3:** Ignore SVDAG. Build the core game using flat arrays (`Vec<u16>`), greedy meshing, and basic Bevy rendering. Prove the gameplay loop.
- **Faz 4-5:** Networking and Wasm modding on the flat array system.
- **Faz 6 (Next-Gen Graphics Phase):** 
  1. Implement the WGSL GPU lock-free allocator.
  2. Build the SVDAG Compute Shader baker.
  3. Swap the Bevy rendering pipeline for the Unified Visibility Buffer.
