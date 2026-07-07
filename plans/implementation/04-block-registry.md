# 04 — Block Registry (M2)

**Kaynak:** `05-block-registry.md`, `39-memory-allocation.md`, `41/40-serialization`
**Hedef:** SoA, data-driven (TOML), runtime genişletilebilir, bitmask flags, heap-free hot path.

## 1. BlockId
- `pub struct BlockId(pub u16);` — 0 = AIR (sentinel). ~16 blok prototip.
- Palet: `SectorPalette` (06) runtime `get_or_insert` ile blok ekler.

## 2. SoA Registry (`strata_core::registry`)
```rust
pub struct BlockRegistry {
    pub id: Vec<BlockId>,
    pub name: Vec<&'static str>,
    pub flags: Vec<BlockFlags>,        // bitflags crate (P0-2)
    pub solid: Vec<bool>,
    pub transparent: Vec<bool>,
    pub light_emission: Vec<u8>,       // L0/L1 için
    pub base_color: Vec<[u8;3]>,       // vertex color (meshing)
}
```
- **Yasak:** tek `BlockDefinition` AoS struct'ı hot query'de gezdirmek. Hot alanlar ayrı Vec.

## 3. Flags (`bitflags`)
```rust
bitflags::bitflags! {
    pub struct BlockFlags: u16 {
        const SOLID       = 1<<0;
        const TRANSPARENT = 1<<1;
        const AIR         = 1<<2;
        const LIGHT_SRC   = 1<<3;
        const LIQUID      = 1<<4;
    }
}
```

## 4. TOML Loading (05 §)
`blocks.toml`:
```toml
[[block]]
id = "stone"
flags = ["SOLID"]
base_color = [128,128,128]
light_emission = 0
```
- Load → `BlockRegistry` SoA doldur. Deterministik (sıralı parse).
- **Enum/state tanımları:** prototipte TOML yeterli (RON hibrit P1-9 sonraki faz).

## 5. Adımlar
1. `BlockId` + `BlockFlags` + `BlockRegistry` (SoA).
2. TOML parser (`toml` crate) → registry fill; ~16 blok.
3. `global_registry()` resource (OnceLock / Bevy `Resource`).
4. `SectorPalette::get_or_insert(block_id)` stub (06 ile bağlanır).
5. `set_if_neq` uyumlu registry read-only query.

## 6. Doğrulama
- `cargo test`: TOML round-trip (parse → serialize → parse, eşit).
- `cargo test`: `get_block("stone")` → doğru flags/solid.
- Boundary: AIR (id 0) her zaman transparent + !solid.

## 7. Risk / Mitigasyon
| Risk | Çözüm |
|------|-------|
| TOML parse alloc | Sadece init-time; hot path alloc yok |
| Flag bit tükenmesi | u16 = 16 bit; yetmezse u32 (sonraki faz) |
