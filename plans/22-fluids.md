# 31 — Fluid Simulation

## 1. Genel Bakış

Strata'nın fluid sistemi **su ve lav** akışını simüle eder. Basit **cellular automata** tabanlı akış modeli kullanılır.

### Temel Prensipler

- **Cellular automata:** Basit komşuluk kuralları
- **Su seviyesi:** Her blok 0-8 seviye arası
- **Yerçekimi:** Su aşağı akar
- **Yayılma:** Yatay olarak yayılır
- **Lav:** Su ile etkileşim (obsidian, cobblestone)

---

## 2. Fluid Component

```rust
#[derive(Component)]
pub struct FluidBlock {
    /// Fluid tipi.
    pub fluid_type: FluidType,

    /// Seviye (0-8, 0 = boş).
    pub level: u8,

    /// Akış yönü.
    pub flow_direction: Option<u8>,

    /// Kaynak mı (sonsuz)?
    pub is_source: bool,
}

#[derive(Clone, Copy)]
pub enum FluidType {
    Water,
    Lava,
}
```

---

## 3. Fluid Update Sistemi

```rust
pub fn fluid_update_system(
    mut fluids: Query<&mut FluidBlock>,
    world: Res<World>,
) {
    // Her fluid için:
    // 1. Aşağı akış kontrolü
    // 2. Yatay yayılma
    // 3. Seviye güncelleme
    // 4. Lava + Water = obsidian/cobblestone
}
```

---

## 4. Crate Organizasyonu

```
crates/
  fluids/
    ├── mod.rs
    ├── fluid.rs
    ├── simulation.rs
    └── interactions.rs
```
