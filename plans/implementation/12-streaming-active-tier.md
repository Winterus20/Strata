# 12 — Streaming: ACTIVE Tier Only (M9)

**Kaynak:** `08-streaming.md`
**Hedef:** 4-tier yerine tek ACTIVE tier; sector load/unload çevresel radius. SVDAG (07) YOK.

## 1. Tier Karar (08 §)
- Prototipte `Tier::Active` sabit. `determine_tier` hysteresis (08) sonraki faz.
- `SectorTransform.tier` = authoritative; `SectorEntity.tier` spawn-only.

## 2. Resident Set (08 §StreamingManager)
- Player coord → radius R (örn. 3 sector ≈ 96 m) içindeki sector listesi.
- Load: eksik sector → `WorldGenPlugin` (08) → XBrickMap + mesh + collider.
- Unload: radius dışı → `ChunkDirty` clear, mesh drop, collider evict (LRU).
- Hysteresis: radius ±1 buffer (pop-in azaltma).

## 3. AOI (08 §)
- Prototipte tek oyuncu → AOI = player radius. Network (16) sonra genel AOI.

## 4. GPU Feedback (08 §5) — prototipte YOK
- CPU frustum cull (07) yeterli; GPU feedback SSBO sonraki faz.

## 5. Predictive Prefetch (08) — minimal
- Hareket yönüne göre 1 sector önceden generate (async queue).

## 6. Adımlar
1. `StreamingManager` resource (resident set + radius).
2. Load sistemi: `WorldGen` (08) → apply → mesh (06) → collider (09) → light (10).
3. Unload sistemi: evict (LRU, memory guard).
4. Hysteresis buffer (±1) + predictive prefetch (yön).
5. ECS order: `Streaming -> WorldGen -> Meshing -> Physics -> Lighting`.

## 7. Doğrulama
- M9 sonu: sonsuz yürüme; sector'lar yüklanır/boşaltılır, pop-in minimal.
- `cargo test`: radius dışı sector unload; içeri giren load.
- Perf: load < frame budget (async); memory steady-state (LRU evict).

## 8. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Load spike (hızlı hareket) | Predictive prefetch + frame budget throttle |
| Memory leak | LRU evict + SlotMap free (05 pool) |
