# 10 — Lighting L0/L1 (M7)

**Kayant:** `13-lighting.md`
**Hedef:** L0 (sun + point) + L1 (block light BFS), 16-bit packed LightData. GI (L2-L4) YOK.

## 1. LightData (13)
- `packed 16-bit` storage: `r,g,b,s` 4×4 bit (14-bit kullanım). Shading 8-bit/r16f.
- `LightData { sun: u8, block: [u8;3] }` — prototipte tek kanal yeterli (sky+block).

## 2. L0 — Direct Light
- Sun: directional, uniform `sun_dir`. Point lights: `light_emission` (04) bloklardan.
- Hop: L0 sadece emit + uniform; per-voxel yayılım L1'de.

## 3. L1 — Block Light BFS (13 §BlockLightEngine)
- Dial 16-bucket BFS + Starlight dual-queue (13).
- `light_emission > 0` blok → source; BFS ile komşuya azalan yayılım.
- Two-phase removal: blok kırılınca ışık geri çekme (BFS reverse).
- SIMD (13 §SIMD acceleration) prototipte scalar; P2 sonra.

## 4. Sky Light (L2 partial — 13 §SkyLightEngine)
- Prototipte basit: `y > terrain_height` → full sky; column-first (13 column-continuity).

## 5. Adımlar
1. `LightData` packed struct + read/write helpers.
2. L0 emit (sun uniform + block emission).
3. L1 BFS (dual-queue) block light.
4. Sky light (column-first, heightmap).
5. Lighting sistemi `Lighting` set (03) → mesh vertex color / visbuf uniform.

## 6. Doğrulama
- `cargo test`: glowstone → çevre 15→0 gradyan (BFS doğru).
- `cargo test`: block break → ışık geri çekilir (two-phase).
- Boundary: tam karanlık kapalı oda; tam açık gökyüzü.

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| BFS her frame | Sadece `ChunkDirty`/`NeedsRemesh` tetikler; incremental |
| SIMD olmadan yavaş | ACTIVE radius küçük; scalar yeterli |
