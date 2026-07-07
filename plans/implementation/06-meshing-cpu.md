# 06 — CPU Greedy Meshing (M3)

**Kaynak:** `09-meshing.md`, `39-memory-allocation.md`
**Hedef:** Binary greedy mesher, `PackedQuad` 8 B, `OccupancyScratch` heap-free, neighbor ±1 read.

## 1. Hot Path Prensipleri
- **Heap-free:** `OccupancyScratch` stack-allocated bitmask (32³ padded yok; komşu sector ±1 okuma, 09 §4.0).
- **Vertex packing:** her vertex 4 B (pos+normal+uv+color tek u32, 06 §B.4).
- **AO:** 0=occluded, 3=open; `AO_CURVE` uniform; Faz 1b `ao_safe` sonra.

## 2. Veri Yapıları
```rust
pub struct PackedQuad { pub data: [u8; 8] } // pos(3)+normal+uv+color sıkıştırılmış
pub struct OccupancyScratch { mask: [u64; 512] } // 32³ / 8 bit-packed, stack
pub struct MeshData { pub opaque: Vec<PackedQuad>, pub transparent: Vec<PackedQuad> }
```
- `GigaBuffer` (09 §VRAM) prototipte sadece `Vec` + single upload; offset-allocator (TLSF) sonraki faz.

## 3. Mesher (Trait-based, 09)
```rust
pub trait Mesher {
    fn mesh_sector(&self, sector: &XBrickMap, neighbors: &[Option<&XBrickMap>; 6]) -> MeshData;
}
pub struct GreedyMesher; // face mask + greedy merge + AO
```
- `CachedGreedy` (09 WARM): ACTIVE mesh GigaBuffer'da kalır, re-mesh 0 µs — prototipte basit cache (`HashMap<SectorCoord, MeshData>` + `NeedsRemesh` ZST).

## 4. Incremental (09 §4, 03 change detection)
- `NeedsRemesh` ZST component → sadece dirty sector mesh edilir.
- `AsyncComputeTaskPool` ile mesh; main thread sadece `MeshData` apply (03 ordering: `Meshing` set).
- `World::get_sector` bypass **yasak** (03) — resmi query.

## 5. Adımlar
1. `OccupancyScratch` (stack bitmask) + coord→bit index.
2. `PackedQuad` pack/unpack (4 B vertex math).
3. `GreedyMesher::mesh_sector` (face mask per axis, greedy merge, AO).
4. `NeedsRemesh` ZST + async mesh sistemi (apply to `MeshData` resource).
5. `VisibilityTable` (05 §18) hook (transparent/cutout ayrımı).

## 6. Doğrulama
- `cargo test`: round-trip — quad pack→unpack→pack eşit.
- `cargo bench`: 32³ full sector mesh < 0.5 ms (greedy vs naive quad sayısı).
- Boundary: tek blok → 6 quad; tam dolu sector → 0 quad (iç yüzler yok).

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| Neighbor read race | Streaming (12) single-threaded apply; async sadece compute |
| AO dalı (branchy) | `select` ile branchless AO curve |
