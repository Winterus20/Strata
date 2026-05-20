# 47 — Building Tools & Blueprints

## 1. Genel Bakış

Strata'nın yapı araçları sistemi oyuncuların büyük yapıları kopyalamasını, yapıştırmasını ve kaydetmesini sağlar.

### Temel Prensipler

- **Selection:** 3D bölge seçimi (box, lasso)
- **Copy/Paste:** Seçili alanı kopyala ve yapıştır
- **Blueprints:** Yapıları şablon olarak kaydet ve yükle
- **Transform:** Döndürme, aynalama, ölçekleme
- **Preview:** Yapıştırma öncesi holografik önizleme

---

## 2. Selection System

```rust
pub struct SelectionTool {
    pub mode: SelectionMode,
    pub points: [IVec3; 2],
    pub selected_blocks: Vec<BlockPosition>,
}

pub enum SelectionMode {
    Box,
    Lasso,
    Wand,
}

impl SelectionTool {
    pub fn set_corner(&mut self, index: usize, pos: IVec3);
    pub fn get_bounds(&self) -> Option<(IVec3, IVec3)>;
    pub fn collect_blocks(&self, world: &World) -> Vec<BlockPosition>;
}
```

---

## 3. Blueprint System

```rust
#[derive(Serialize, Deserialize)]
pub struct Blueprint {
    pub name: String,
    pub author: String,
    pub size: IVec3,
    pub blocks: Vec<BlueprintBlock>,
    pub metadata: BlueprintMetadata,
}

pub struct BlueprintBlock {
    pub local_pos: IVec3,
    pub block_id: u16,
    pub rotation: u8,
    pub light_data: Option<u16>,
}

pub struct BlueprintMetadata {
    pub created_at: u64,
    pub tags: Vec<String>,
    pub description: String,
    pub thumbnail: Option<Vec<u8>>,
}

impl Blueprint {
    pub fn from_selection(selection: &SelectionTool, world: &World) -> Self;
    pub fn preview(&self, anchor: IVec3, rotation: u8) -> Vec<(IVec3, u16)>;
    pub fn place(&self, world: &mut World, anchor: IVec3, rotation: u8);
}
```

---

## 4. Transform Operations

```rust
pub enum BlueprintTransform {
    Rotate90,
    Rotate180,
    Rotate270,
    MirrorX,
    MirrorZ,
    FlipY,
}

impl Blueprint {
    pub fn apply_transform(&mut self, transform: BlueprintTransform);
}
```

---

## 5. Preview Rendering

```rust
pub struct BlueprintPreview {
    pub blueprint: Blueprint,
    pub anchor: IVec3,
    pub rotation: u8,
    pub alpha: f32,
    pub valid_placement: bool,
}
```

---

## 6. Crate Organizasyonu

```
crates/
  building/
    ├── mod.rs
    ├── selection.rs
    ├── blueprint.rs
    ├── transform.rs
    ├── preview.rs
    └── clipboard.rs
```
