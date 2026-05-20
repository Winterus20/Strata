# 12 — Block Registry & Property Sistemi

## 1. Genel Bakış

Strata'nın block registry'si, tüm blok tiplerini, özelliklerini ve davranışlarını merkezi olarak yönetir. Her blok bir **block ID** (u16) ile tanımlanır ve registry'de metadata ile kaydedilir.

### Temel Prensipler

- **Data-driven:** Bloklar kod yerine JSON/TOML tanımlarından yüklenir
- **Runtime genişletilebilir:** Modlar yeni blok tipleri ekleyebilir
- **Bitmask-friendly:** Özellikler bitmask ile paketlenir (hızlı query)
- **Version-lu:** Dünya dosyaları ile uyumluluk için schema version

---

## 2. Block ID Yapısı

```
┌─────────────────────────────────────────┐
│ Block ID (u16 = 65.536 max)             │
├─────────────────────────────────────────┤
│ Bits 0-11:   Block type (4096 tip)      │
│ Bits 12-15:  State variant (16 variant) │
└─────────────────────────────────────────┘
```

| Aralık | Kullanım |
|---|---|
| 0 | AIR (boşluk) |
| 1-2047 | Vanilla bloklar |
| 2048-4095 | Mod blokları (slot 1) |
| 4096-6143 | Mod blokları (slot 2) |
| 6144-65535 | Rezerve / dinamik |

---

## 3. Block Registry

```rust
/// Merkezi blok kayıt defteri.
/// Thread-safe: okuma Rc/RefCell, yazma init sırasında.
pub struct BlockRegistry {
    /// Blok tanımları (index = block_type).
    definitions: Vec<BlockDefinition>,

    /// İsim → ID mapping (hızlı lookup).
    name_to_id: HashMap<String, u16>,

    /// Tag → blok listesi (hızlı tag query).
    tag_index: HashMap<String, Vec<u16>>,

    /// Registry versiyonu (schema migration için).
    version: u32,
}

impl BlockRegistry {
    /// Bir blok tanımlamasını kaydet.
    pub fn register(&mut self, def: BlockDefinition) -> Result<u16> {
        let id = self.definitions.len() as u16;
        self.name_to_id.insert(def.name.clone(), id);
        self.definitions.push(def);
        Ok(id)
    }

    /// İsme göre blok ID'si bul.
    pub fn get_id(&self, name: &str) -> Option<u16> {
        self.name_to_id.get(name).copied()
    }

    /// Tag'e sahip tüm blok ID'lerini bul.
    pub fn get_by_tag(&self, tag: &str) -> &[u16] {
        self.tag_index.get(tag).map_or(&[], |v| v.as_slice())
    }

    /// Bir blok tanımını getir.
    pub fn get(&self, id: u16) -> &BlockDefinition {
        &self.definitions[(id >> 4) as usize]
    }
}
```

---

## 4. Block Definition

```rust
/// Bir blok tipinin tam tanımı.
pub struct BlockDefinition {
    /// Benzersiz isim (örn. "stone", "oak_log").
    pub name: String,

    /// Görünüm özellikleri.
    pub appearance: Appearance,

    /// Fizik özellikleri.
    pub physics: PhysicsProperties,

    /// Aydınlatma özellikleri.
    pub lighting: LightingProperties,

    /// Gameplay özellikleri.
    pub gameplay: GameplayProperties,

    /// Bağlantı/komşuluk kuralları.
    pub connectivity: ConnectivityRules,

    /// State variant'ları (yön, açık/kapalı, güç seviyesi vb.).
    pub states: Vec<StateDefinition>,

    /// Tag'ler (örn. "mineable/pickaxe", "transparent").
    pub tags: Vec<String>,
}

/// Görünüm özellikleri.
pub struct Appearance {
    /// Doku isimleri (6 yüz: +X, -X, +Y, -Y, +Z, -Z).
    pub textures: [String; 6],

    /// Renk tint (biome için).
    pub tint_color: Option<[u8; 4]>,

    /// Şeffaflık tipi.
    pub transparency: TransparencyType,

    /// Render tipi (solid, cutout, translucent, alpha).
    pub render_type: RenderType,

    /// AO (Ambient Occlusion) desteği.
    pub supports_ao: bool,

    /// Animasyonlu doku (su, lava, ateş).
    pub animated_texture: Option<AnimationParams>,
}

#[derive(Clone, Copy)]
pub enum TransparencyType {
    Opaque,
    Transparent,
    Translucent,
}

#[derive(Clone, Copy)]
pub enum RenderType {
    Solid,
    Cutout,    // Alpha test
    Translucent, // Alpha blend
    Alpha,     // Sorted alpha blend
}

pub struct AnimationParams {
    pub frame_count: u8,
    pub frame_duration_ms: u16,
    pub tile_size: u16,
}
```

---

## 5. Fizik Özellikleri

```rust
pub struct PhysicsProperties {
    /// Blok sertliği (kırma süresi).
    pub hardness: f32,

    /// Patlama direnci.
    pub blast_resistance: f32,

    /// Sürtünme katsayısı.
    pub friction: f32,

    /// Sıçrama katsayısı.
    pub bounciness: f32,

    /// Geçilebilir mi?
    pub passable: bool,

    /// Yerçekimi etkisi (kum, çakıl).
    pub gravity_affected: bool,

    /// Tırmanılabilir mi? (merdiven, asma).
    pub climbable: bool,

    /// Slime bloğu gibi zıplatma.
    pub bouncy: bool,

    /// Honey bloğu gibi yavaşlatma.
    pub slow_factor: f32, // 1.0 = normal, 0.4 = yavaş
}
```

---

## 6. Aydınlatma Özellikleri

```rust
pub struct LightingProperties {
    /// Işık geçirgenliği (0-15).
    pub light_permeability: u8,

    /// Işık emisyonu (0-15).
    pub light_emission: u8,

    /// Emisyon rengi (RGB, 4-bit per channel).
    pub emission_color: [u8; 3],

    /// Sky light geçirgenliği.
    pub sky_light_permeability: u8,

    /// Işık kırılması (su, cam).
    pub light_refraction: Option<f32>,
}
```

---

## 7. Gameplay Özellikleri

```rust
pub struct GameplayProperties {
    /// Blok kırıldığında düşen item.
    pub drop: Option<DropDefinition>,

    /// Hangi alet ile kırılabilir.
    pub tool_requirement: Option<ToolRequirement>,

    /// Blok yerleştirme sesi.
    pub place_sound: String,

    /// Blok kırma sesi.
    pub break_sound: String,

    /// Üzerinde yürüme sesi.
    pub step_sound: String,

    /// Redstone benzeri mekanik destek.
    pub supports_redstone: bool,

    /// Envanter slotu var mı? (sandık, fırın).
    pub has_inventory: bool,

    /// Yakıt olarak kullanılabilir mi?
    pub fuel_value: u16, // saniye cinsinden
}

pub struct DropDefinition {
    pub item_id: u16,
    pub min_count: u8,
    pub max_count: u8,
    pub fortune_multiplier: f32,
}

pub struct ToolRequirement {
    pub tool_type: ToolType,
    pub min_tier: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolType {
    None,
    Pickaxe,
    Axe,
    Shovel,
    Hoe,
}
```

---

## 8. Bağlantı/Komşuluk Kuralları

```rust
pub struct ConnectivityRules {
    /// Komşu bloklara bağlanır mı? (çit, cam, duvar).
    pub connects_to_neighbors: bool,

    /// Bağlantı tipi.
    pub connection_type: ConnectionType,

    /// Yüz bazlı bağlantı kuralları.
    pub face_rules: [FaceRule; 6],

    /// Shape variant'ları (merdiven, kapı, yarım blok).
    pub shape_variants: Vec<ShapeVariant>,
}

#[derive(Clone, Copy)]
pub enum ConnectionType {
    None,
    Full,       // Tüm komşulara bağlanır
    SameType,   // Sadece aynı tipe bağlanır
    Tag(String), // Belirli tag'e sahip bloklara bağlanır
}

pub struct FaceRule {
    /// Bu yüzde bağlantı kurulur mu?
    pub connects: bool,

    /// Bağlantı offset'i (yarım blok, slab vb.).
    pub offset: Vec3,

    /// Bağlantı shape'i.
    pub shape: ConnectionShape,
}

pub enum ConnectionShape {
    FullFace,
    HalfFace,
    QuarterFace,
    Post,
    Cross,
}
```

---

## 9. State Sistemi

Blokların dinamik durumlarını (yön, açık/kapalı, güç seviyesi) tanımlar.

```rust
pub struct StateDefinition {
    /// State ismi (örn. "facing", "powered", "open").
    pub name: String,

    /// State tipi.
    pub state_type: StateType,
}

pub enum StateType {
    /// Enum state (facing: north, south, east, west).
    Enum(Vec<String>),

    /// Boolean state (powered: true/false).
    Boolean,

    /// Integer state (power_level: 0-15).
    Integer { min: i32, max: i32 },
}

/// State'lerin runtime temsili.
/// State variant'ları block ID'nin üst 4 bit'inde saklanır.
pub struct BlockState {
    /// Base block type ID.
    pub type_id: u16,

    /// State variant index (0-15).
    pub variant: u8,
}

impl BlockState {
    /// Block ID'den state oluştur.
    pub fn from_id(id: u16) -> Self {
        Self {
            type_id: id & 0x0FFF,
            variant: ((id >> 12) & 0xF) as u8,
        }
    }

    /// State'ten block ID oluştur.
    pub fn to_id(&self) -> u16 {
        self.type_id | ((self.variant as u16) << 12)
    }
}
```

---

## 10. Block Property Bitmask (Hızlı Query)

Performans için sık kullanılan özellikler bitmask'te saklanır:

```rust
/// Blok özellikleri bitmask'i (64-bit).
/// XBrickMap'te her blok için hızlı query.
#[repr(transparent)]
#[derive(Clone, Copy, Default)]
pub struct BlockFlags(pub u64);

impl BlockFlags {
    pub const OPAQUE: u64 = 1 << 0;
    pub const TRANSPARENT: u64 = 1 << 1;
    pub const PASSABLE: u64 = 1 << 2;
    pub const GRAVITY_AFFECTED: u64 = 1 << 3;
    pub const CLIMBABLE: u64 = 1 << 4;
    pub const EMITS_LIGHT: u64 = 1 << 5;
    pub const HAS_INVENTORY: u64 = 1 << 6;
    pub const CONNECTS_TO_NEIGHBORS: u64 = 1 << 7;
    pub const SUPPORTS_REDSTONE: u64 = 1 << 8;
    pub const IS_FLUID: u64 = 1 << 9;
    pub const IS_LEAVES: u64 = 1 << 10;
    pub const IS_LOG: u64 = 1 << 11;
    pub const IS_ORE: u64 = 1 << 12;
    pub const EXPLOSIVE_RESISTANT: u64 = 1 << 13;
    pub const BOUNCY: u64 = 1 << 14;
    pub const SLOWING: u64 = 1 << 15;

    #[inline]
    pub fn has(&self, flag: u64) -> bool {
        self.0 & flag != 0
    }

    #[inline]
    pub fn is_opaque(&self) -> bool {
        self.has(Self::OPAQUE)
    }

    #[inline]
    pub fn is_passable(&self) -> bool {
        self.has(Self::PASSABLE)
    }

    #[inline]
    pub fn emits_light(&self) -> bool {
        self.has(Self::EMITS_LIGHT)
    }
}
```

---

## 11. Block Data Loading (TOML)

```toml
# blocks/stone.toml
[name]
id = 1
display_name = "Stone"

[appearance]
textures = ["stone", "stone", "stone", "stone", "stone", "stone"]
render_type = "solid"
supports_ao = true

[physics]
hardness = 1.5
blast_resistance = 6.0
friction = 0.6
passable = false
gravity_affected = false

[lighting]
light_permeability = 0
light_emission = 0

[gameplay]
drop = { item = "cobblestone", min = 1, max = 1 }
tool_requirement = { type = "pickaxe", min_tier = 1 }
place_sound = "stone_place"
break_sound = "stone_break"
step_sound = "stone_step"

[connectivity]
connects_to_neighbors = false

[tags]
mineable = ["pickaxe"]
natural = true
```

---

## 12. Crate Organizasyonu

```
crates/
  core/
    ├── registry/
    │   ├── mod.rs          ← BlockRegistry
    │   ├── definition.rs   ← BlockDefinition
    │   ├── properties.rs   ← Physics, Lighting, Gameplay properties
    │   ├── state.rs        ← BlockState, StateDefinition
    │   ├── flags.rs        ← BlockFlags bitmask
    │   ├── connectivity.rs ← Connection rules
    │   └── loader.rs       ← TOML/JSON block loading
    └── block_id.rs         ← Block ID encoding/decoding
```
