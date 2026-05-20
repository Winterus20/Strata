# 32 — Crafting & Recipes

## 1. Genel Bakış

Strata'nın crafting sistemi **recipe-based** crafting destekler. Envanter sistemi ile entegre çalışır.

### Temel Prensipler

- **Recipe-driven:** Tarifler JSON/data-driven
- **Shapeless & Shaped:** Şekilli ve şekilsiz tarifler
- **Furnace:** Pişirme sistemi
- **Anvil:** Onarma ve enchant birleştirme

---

## 2. Recipe Sistemi

```rust
pub struct RecipeRegistry {
    pub recipes: HashMap<RecipeId, Recipe>,
}

pub enum Recipe {
    Shaped {
        pattern: Vec<String>,
        ingredients: HashMap<char, ItemStack>,
        result: ItemStack,
    },
    Shapeless {
        ingredients: Vec<ItemStack>,
        result: ItemStack,
    },
    Smelting {
        input: ItemStack,
        result: ItemStack,
        xp: f32,
        time: f32,
    },
}
```

---

## 3. Crafting Grid

```rust
#[derive(Component)]
pub struct CraftingGrid {
    /// Grid boyutu (2x2 veya 3x3).
    pub size: u8,

    /// Slot'lar.
    pub slots: [Option<ItemStack>; 9],

    /// Sonuç.
    pub result: Option<ItemStack>,
}

impl CraftingGrid {
    /// Tarif eşleşmesi kontrol et.
    pub fn match_recipe(&self, registry: &RecipeRegistry) -> Option<RecipeId> {
        // Grid'i recipe'lerle eşleştir
    }
}
```

---

## 4. Furnace

```rust
#[derive(Component)]
pub struct Furnace {
    /// Input slot.
    pub input: Option<ItemStack>,

    /// Yakıt slot.
    pub fuel: Option<ItemStack>,

    /// Sonuç slot.
    pub output: Option<ItemStack>,

    /// Pişirme progress (0-1).
    pub progress: f32,

    /// Kalan yakıt süresi.
    pub fuel_time: f32,
}
```

---

## 5. Crate Organizasyonu

```
crates/
  crafting/
    ├── mod.rs
    ├── recipe.rs
    ├── registry.rs
    ├── crafting_grid.rs
    ├── furnace.rs
    └── anvil.rs
```
