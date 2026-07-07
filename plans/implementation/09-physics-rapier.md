# 09 — Physics: Rapier Voxels (M6)

**Kaynak:** `12-physics.md`, `42-physics-engine.md`
**Hedef:** Rapier `Voxels` collider + `KinematicCharacterController`, XBrickMap sync, O(1) edit.

## 1. Karar (42)
- `bevy_rapier3d` 0.34, feature `enhanced-determinism`.
- Voxel shape: `ColliderBuilder::voxels_from_points(voxel_size, &samples)`.
- Runtime edit: `voxels.set_voxel(key, occupied)` (O(1), partial rebuild).

## 2. Static Collision (dünya blokları)
- Her **sector** bir `SharedShape::voxels` collider (42 §Detay 1).
- Sector dirty (`ChunkDirty`) → XBrickMap'ten occupied list → `set_voxel`.
- Sadece ACTIVE radius'taki sector'lar collider (memory: 42 risk Orta).
- Chunk boundary sync: komşu sector collider'ları overlap (32³ grid align).

## 3. Character Controller (42 §3)
- `KinematicCharacterController` (Rapier) → player movement (M8).
- Ground check: XBrickMap-optimized (raycast down 1 voxel, branchless).
- `RigidBody::Kinematic` player; `Dynamic` için sonra (item/mob).

## 4. Tier-based Frequency (12)
- Prototipte tek tier → her frame snap sync (ACTIVE). 08 WARM sonra frequency düşer.

## 5. Adımlar
1. `bevy_rapier3d` plugin (headless-safe: sadece client).
2. Sector → voxel collider build (voxels_from_points).
3. `ChunkDirty` → `set_voxel` sync sistemi (03 `Physics` set, sonra `Meshing`).
4. `KinematicCharacterController` resource + ground check (XBrickMap ray).
5. Collision query test (blok içine girme engeli).

## 6. Doğrulama
- `cargo test`: block place → collider güncellenir; ray alır.
- Perf: 1 block break < 0.1 ms collider update (set_voxel O(1)).
- Boundary: sector edge collision continuity (komşu overlap).

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Büyük dünya memory | Sadece ACTIVE radius collider; LRU evict |
| bevy_rapier API drift | Fixed version pin |
