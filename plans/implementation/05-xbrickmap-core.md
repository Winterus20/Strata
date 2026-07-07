# 05 — XBrickMap Core (M2)

**Kaynak:** `06-xbrickmap.md`, `39-memory-allocation.md`
**Hedef:** 3-level bitmask voxel store, O(1) get/set, GlobalBrickPool (SlotMap), heap-free hot path.

## 1. Hiyerarşi (06 §B)
```
Sector (32³)  -> u64 mask: 64 Brick (8³) doluluk
Brick  (8³)   -> u64 mask: 64 SubBrick (2³) doluluk
SubBrick (2³=8 voxel) -> u8 mask + palette idx
```
- Boş sektör = 8 B (sadece mask). Dolu brick pool'da.

## 2. GlobalBrickPool (39 — heap fragmentation yasak)
```rust
pub struct GlobalBrickPool {
    bricks: SlotMap<BrickHandle, Brick>,     // O(1) alloc/free
    palette: SecondaryMap<BrickHandle, u16>, // sector-local palet idx
}
```
- **Yasak:** per-sector `Vec<Brick>`. Tüm brick'ler global pool'da SlotMap.
- `Brick = { sub_mask: u64, voxel: [u8;8] }` (palette idx'leri).

## 3. API (branchless, O(1))
```rust
impl XBrickMap {
    pub fn get_block(&self, coord: VoxelCoord) -> BlockId; // bitmask select
    pub fn set_block(&mut self, coord: VoxelCoord, b: BlockId); // O(1) pool
    pub fn is_occupied(&self, coord: VoxelCoord) -> bool; // firstTrailingBit
}
```
- Coord → sector/brick/sub index math: `>> 5`, `>> 3`, `& 7` (shift, no div).
- get: `select(brick_full, pool_val, AIR)` — branchless.

## 4. CompressedChunkData (06 §1.4) — bake snapshot kaynağı
- `Arc<CompressedChunkData>` = serialize edilmiş sector (SVDAG bake için sonra; prototipte mesh için).
- Prototipte: `pack()`/`unpack()` round-trip test zorunlu.

## 5. Adımlar
1. `SectorCoord`/`VoxelCoord` + coord math (shift-based).
2. `GlobalBrickPool` (slotmap + secondarymap).
3. `XBrickMap::get/set/is_occupied` (bitmask, branchless).
4. `SectorPalette` bağla (04).
5. `CompressedChunkData` pack/unpack (rkyv/postcard — `40/41`).

## 6. Doğrulama
- `cargo test`: round-trip — 32³ sektör random dolu → pack → unpack → eşit.
- Boundary: tam boş sektör = 8 B; tam dolu = pool alloc; edge coord (31,31,31) doğru.
- `cargo bench`: 1M get/set < X ns (heap-free kanıtı).

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| SlotMap handle reuse | Generational index; stale handle guard |
| Palette collision | `get_or_insert` unique; sector-local scope |
| SIMD (06 SOA+SIMD) | Prototipte scalar; P2-13 `soa-rs` sonra |
