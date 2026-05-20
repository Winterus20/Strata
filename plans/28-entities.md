# 33 — Entity/Mob Lifecycle

## 1. Genel Bakış

Strata'nın entity sistemi **mob spawn, loot, drop, ve lifecycle** yönetimini sağlar.

### Temel Prensipler

- **Spawn rules:** Biome, zaman, yükseklik bazlı spawn
- **Loot tables:** Deterministik drop tabloları
- **Lifecycle:** Spawn → Active → Idle → Death → Despawn
- **Entity tracking:** Server-client entity sync

---

## 2. Entity Component

```rust
#[derive(Component)]
pub struct Mob {
    /// Mob tipi.
    pub mob_type: MobTypeId,

    /// Sağlık.
    pub health: f32,

    /// Maksimum sağlık.
    pub max_health: f32,

    /// Hasar.
    pub damage: f32,

    /// Hız.
    pub speed: f32,

    /// Lifecycle state.
    pub state: MobState,
}

#[derive(Clone, Copy)]
pub enum MobState {
    Idle,
    Wandering,
    Chasing,
    Attacking,
    Fleeing,
    Dead,
}
```

---

## 3. Loot Tables

```rust
pub struct LootTable {
    pub entries: Vec<LootEntry>,
}

pub struct LootEntry {
    pub item_id: u16,
    pub min_count: u8,
    pub max_count: u8,
    pub chance: f32,
    pub enchantment_chance: f32,
}
```

---

## 4. Spawn System

```rust
pub struct SpawnRule {
    pub mob_type: MobTypeId,
    pub valid_biomes: Vec<BiomeId>,
    pub time_range: (f32, f32),
    pub height_range: (i32, i32),
    pub max_per_chunk: u8,
    pub spawn_weight: u32,
}
```

---

## 5. Crate Organizasyonu

```
crates/
  entities/
    ├── mod.rs
    ├── mob.rs
    ├── lifecycle.rs
    ├── loot.rs
    ├── spawn.rs
    └── tracking.rs
```
