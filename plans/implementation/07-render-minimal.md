# 07 — Minimal Render Pipeline (M4)

**Kaynak:** `10-render-pipeline.md`
**Hedef:** Tek-tier render — XBrickMap ray-trace pass + 64-bit visibility buffer, HDR+ACES, frustum cull. SVDAG (07) YOK.

## 1. Pipeline (10 §9-pass sadeleştirilmiş → 3-pass prototip)
```
1. Depth Pre-Pass (visibility buffer 64-bit, atomicMax)
2. XBrickMap Ray-Trace Pass (WGSL, branchless)
3. HDR Color Resolve + ACES ToneMap + Bloom
```
- SVDAG RM, Hi-Z re-execution, VRCS prototipte YOK (sonraki faz).

## 2. Visibility Buffer (64-bit, Aokana Figure 7)
```
bit[0:23]  voxel_pos
bit[24:36] sector_id
bit[37:39] normal
bit[40:63] depth (reversed-Z)
```
- WGSL 64-bit atomic: `atomicMax` (u64) ile en yakın piksel (10 §WGSL atomic strategy).
- **Branchless ray trace:** `select`, `firstTrailingBit` ile boşluk atlama (06 §B.4). ASLA `if-else` wavefront bölmez.

## 3. Vertex Pulling
- Vertex buffer yerine `PackedQuad` → SSBO'dan GPU'da vertex türet (09 §vertex pulling).
- Opaque / transparent ayrı draw batch.

## 4. HDR (10 §HDR)
- FP16 swapchain; ACES tonemap; hafif bloom (prototip için tek-pass).
- Sky: gradient (day/night sonra, 23).

## 5. Frustum Cull (10)
- GPU compute frustum cull (basit); tile=8×8. Prototipte CPU frustum cull yeterli (sector AABB vs frustum).

## 6. Adımlar
1. wgpu device/queue + FP16 swapchain (render crate).
2. Visibility buffer (R64Uint) + depth pre-pass (atomicMax).
3. WGSL XBrickMap RT pass (branchless, neighbor sample).
4. Color resolve + ACES + bloom (single pass).
5. CPU frustum cull → visible sector list → indirect dispatch.

## 7. Doğrulama
- M4 sonu: statik terrain görünür (renkli bloklar, AO).
- `cargo test`: visbuf pack/unpack bit-layout doğru.
- Perf: GPU frame < 4 ms (ACTIVE ~10 sector).

## 8. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| R64Uint atomic desteği | wgpu `features`; fallback R32Uint+depth (P2-14) |
| WGSL branchy debug | `select` zorunlu; CI shader lint |
