# 08 — World Generation (M5)

**Kaynak:** `11-world-gen.md`
**Hedef:** Deterministik density-function terrain, 3-4 biome, basit cave, chunk-independent, async.

## 1. Deterministik RNG
- `PCG32 + wyhash` (11). Seed global; sector coord → hash → deterministik.
- **Chunk-independent:** her sector kendi verisini üretir (komşu gerekmeden, cave hariç ±1).

## 2. Density Function
```rust
fn density(x: f32, y: f32, z: f32, biome: Biome) -> f32; // f(x,y,z) > 0 => solid
```
- Terrain: `y - heightfield(x,z)` + 3D noise (fastnoise2, `wide` SIMD 0.7).
- Cave: 3D noise isosurface (worm cave sonraki faz).
- Biome: Whittaker diagram (cached per-column) — prototipte 3-4 biome (plains, hills, desert, snow).

## 3. Structures (11)
- Prototip: ağaç template (trunk + leaf blob) — basit hash-grid placement.

## 4. Async (03 ordering: WorldGen set)
- `WorldGenPlugin` → `AsyncComputeTaskPool` ile sector generate.
- Sonuç: `Arc<CompressedChunkData>` (06 §1.4) → XBrickMap apply (main thread, `set_if_neq`).
- Column caching: aynı column tekrar üretilmez.

## 5. Adımlar
1. `PCG32+wyhash` RNG wrapper (deterministik).
2. `density()` + heightfield (fastnoise2).
3. Biome selector (Whittaker, 3-4 biome).
4. Cave noise + tree template.
5. `WorldGenPlugin` async → `XBrickMap` apply.

## 6. Doğrulama
- `cargo test`: aynı seed+coord → aynı sector (determinism).
- `cargo test`: round-trip — generate → pack → unpack → eşit (05).
- Boundary: y=0 (bedrock) full solid; y=max sky.

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Async race apply | Main-thread apply queue; single consumer |
| Noise alloc | Scratch buffer reuse; SIMD `wide` |
