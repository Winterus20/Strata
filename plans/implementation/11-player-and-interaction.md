# 11 — Player & Interaction (M8)

**Kaynak:** `14-inventory-player.md`
**Hedef:** Hareket (yürü/düş/zıpla), break/place (raycast), hotbar (tek slot yeterli), input mapping.

## 1. Player Controller (14)
- `PlayerController`: movement / jump / sprint / sneak (state machine).
- `PlayerState`: grounded / flying / gamemode (prototipte survival walking).
- `KinematicCharacterController` (09) ile hareket; `set_if_neq` ile state.

## 2. Input Mapping (14 §InputMapper)
- `InputAction` enum: MoveX, MoveZ, Jump, Break, Place, HotbarNext.
- `InputMapper` → Bevy `Input`/`ActionState` (winit). Keyboard + mouse.

## 3. Block Interaction (14 §block interaction)
- Raycast: XBrickMap branchless ray (06 §B.4) → hedef voxel + face normal.
- **Break:** `set_block(AIR)` → `ChunkDirty` + `NeedsRemesh` + physics sync (09).
- **Place:** hedef + normal → `set_block(selected)` ( AIR kontrolü + player overlap).
- Sonuç: `Arc<CompressedChunkData>` snapshot (06) → mesh + light + collider update.

## 4. Inventory (14 — minimal)
- `Inventory { hotbar: [ItemStack; 9] }`; prototipte 1 slot + scroll.
- `ItemStack` (NBT + enchant sonra); prototipte `block_id` only.

## 5. Adımlar
1. `PlayerController` + `PlayerState` component.
2. `InputMapper` + `InputAction` (winit bindings).
3. XBrickMap raycast (branchless) → hit voxel.
4. Break/Place sistemi (ChunkDirty/NeedsRemesh trigger).
5. Hotbar HUD (basit, 24-ui sonra; prototipte debug text).

## 6. Doğrulama
- M8 sonu: yürü + blok kır + blok koy çalışır.
- `cargo test`: raycast → doğru voxel+normal; place yer değiştirmez.
- Boundary: dünya sınırı (sky) place engeli; kendine place engeli.

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Ray branchy | `firstTrailingBit`/`select` branchless (06) |
| Place/break race | Tek thread apply; event queue |
