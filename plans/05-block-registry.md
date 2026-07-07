# 05 — Block Registry & Property Sistemi

## 1. Genel Bakış

Strata'nın block registry'si, tüm blok tiplerini, özelliklerini ve davranışlarını merkezi olarak yönetir. Her blok bir **block type ID** (u16) ile tanımlanır. State bilgisi block ID'ye encode **edilmez** — XBrickMap'in 3-seviyeli palet sistemi üzerinden yönetilir (Bkz. §10).

### Temel Prensipler

- **Data-driven:** Bloklar kod yerine TOML tanımlarından yüklenir
- **Runtime genişletilebilir:** Modlar yeni blok tipleri ekleyebilir (init phase)
- **SoA (Structure of Arrays):** Hot/cold veriler ayrı array'lerde — cache-friendly
- **Immutable + Arc:** Init sonrası salt-okunur, `Arc` ile Bevy sistemlerinde lock-free paylaşım
- **Bitmask-friendly:** Özellikler u16 bitmask ile paketlenir (hızlı query, 4x bandwidth tasarrufu)
- **XBrickMap-entegre:** 4-seviyeli palet zinciri + `GlobalPalette` (Bkz. 06-xbrickmap.md §2.1)
- **State ownership:** Meshing state → `SectorPalette.variant`; envanter/karmaşık → `ChunkBlockEntities` (Bkz. 03 §10.6.1)
- **Version-lu:** Dünya dosyaları ile uyumluluk için schema version
- **GPU-ready:** Meshing compute shader için ayrı `#[repr(C)]` property buffer

---

## 2. Block Type ID Yapısı

```
┌─────────────────────────────────────────┐
│ Block Type ID (u16 = 65.536 max)        │
├─────────────────────────────────────────┤
│ 0:           AIR (boşluk)               │
│ 1-2047:      Vanilla bloklar            │
│ 2048-63487:  Mod blokları (dinamik)     │
│ 63488-65535: Rezerve                    │
└─────────────────────────────────────────┘
```

**ÖNEMLİ:** Eski tasarımdaki "üst 4 bit = state variant" yaklaşımı **kaldırılmıştır**. State bilgisi artık block ID'ye encode edilmez. Bunun yerine XBrickMap'in palet sistemi üzerinden `SectorPalette` ile yönetilir (Bkz. §10, §14). Bu sayede:

- Block type sınırı 4096'dan 65535'e yükselir
- State variant sayısı sınırsızdır (block başına)
- XBrickMap palette indisi → SectorPalette → block_type_id + state_variant

**PALETTE DARBOĞAZI UYARISI:**
XBrickMap SubBrick'te `u8` palette indisi kullanır (max 256). Ancak block_type `u16` (65535). Bu çelişki **SectorPalette** katmanı ile çözülür (Bkz. §14):

- SubBrick: `u8` local index (SubBrick başına max 8 farklı materyal)
- BrickPalette: `u8 → u8` remap (max 16, opsiyonel)
- **SectorPalette:** `u8 → PaletteEntry { block_type, variant }` (sektör başına max 256 kombinasyon)
- Tipik bir sektörde 20-40 farklı blok tipi bulunur → 256 sınır pratikte yeterli


| Aralık      | Kullanım                    |
| ----------- | --------------------------- |
| 0           | AIR (boşluk)                |
| 1-2047      | Vanilla bloklar             |
| 2048-63487  | Mod blokları (dinamik slot) |
| 63488-65535 | Rezerve / internal          |


---

## 3. Block Registry (SoA, Immutable, Arc)

### 3.1 Mimari Kararlar

**Neden AoS yerine SoA?**
Eski tasarımda `Vec<BlockDefinition>` vardı — tek bir struct içinde hem her frame erişilen `flags`/`hardness`/`transparency` hem de nadiren erişilen `String` isimler, ses dosyaları, drop tanımları barınıyordu. Bu, AGENTS.md'deki **"Hot/Cold verileri asla aynı struct içinde tutma"** kuralını ihlal ediyordu.

Yeni tasarımda tüm veriler erişim sıklığına göre ayrı array'lere bölünür:

```
BlockRegistry
├── HOT PATH (cache-line packed, her frame erişilir):
│   ├── flags: Vec<BlockFlags>           // u16, index = block_type
│   ├── hardness: Vec<u16>               // f16 olarak paketlenmiş
│   ├── light: Vec<LightingCompact>      // 2 byte (emission + permeability)
│   ├── appearance: Vec<AppearanceCompact> // 4 byte packed
│   └── gpu_props: Vec<BlockGpuProps>    // #[repr(C)], SSBO olarak GPU'ya aktarılır
│
├── WARM PATH (oyun sırasında nadiren erişilir):
│   ├── physics: Vec<PhysicsProperties>
│   ├── gameplay: Vec<GameplayProperties>
│   └── connectivity: Vec<ConnectivityRules>
│
└── COLD PATH (init/load time, runtime'da hiç erişilmez):
    ├── names: Vec<StringId>             // String interner
    ├── textures: Vec<[StringId; 6]>
    ├── sounds: Vec<SoundSet>
    ├── drops: Vec<DropDef>
    ├── states: Vec<Vec<StateDef>>
    └── tags: Vec<Vec<StringId>>
```

### 3.2 Registry Yapısı

```rust
use bevy::prelude::*;
use std::sync::Arc;

/// Merkezi blok kayıt defteri.
/// Init phase'da oluşturulur, Arc ile sarmalanır.
/// Runtime'da Res<BlockRegistry> ile lock-free okunur.
#[derive(Resource, Clone)]
pub struct BlockRegistry(pub Arc<BlockRegistryInner>);

pub struct BlockRegistryInner {
    // ══════════════════════════════════════════════
    // HOT PATH — Cache-line packed SoA arrays
    // Her biri index = block_type_id ile O(1) erişim
    // ══════════════════════════════════════════════

    /// Blok özellikleri bitmask'i (u16 — 2 byte).
    /// Meshing'de yüz görünürlüğü, ray-tracing'de geçirgenlik sorgusu.
    /// Cache-line: 64 byte = 32 block flags tek okumada.
    pub flags: Vec<BlockFlags>,

    /// Blok sertliği (f16 olarak paketlenmiş, 2 byte).
    /// Kırma süresi hesabında kullanılır.
    pub hardness: Vec<u16>,

    /// Aydınlatma özellikleri (2 byte packed).
    pub light: Vec<LightingCompact>,

    /// Görünüm özellikleri (4 byte packed).
    pub appearance: Vec<AppearanceCompact>,

    /// GPU meshing compute shader için #[repr(C)] property buffer.
    /// bytemuck ile zero-copy SSBO aktarımı.
    pub gpu_props: Vec<BlockGpuProps>,

    // ══════════════════════════════════════════════
    // WARM PATH — Oyun mantığında nadiren erişilir
    // ══════════════════════════════════════════════

    /// Fizik özellikleri (collision, friction, gravity).
    /// NOT: hardness hot path'te `Vec<u16>` (f16) olarak ayrıca tutulur.
    /// Burada sadece ek fizik parametreleri var (duplikasyon yok).
    pub physics: Vec<PhysicsExtra>,

    /// Gameplay özellikleri (drop, tool, fuel).
    pub gameplay: Vec<GameplayProperties>,

    /// Bağlantı/komşuluk kuralları.
    pub connectivity: Vec<ConnectivityRules>,

    // ══════════════════════════════════════════════
    // COLD PATH — Init/load time, runtime'da erişilmez
    // ══════════════════════════════════════════════

    /// Blok isimleri (StringInterner ID).
    pub names: Vec<StringId>,

    /// Texture atlas index'leri (6 yüz: +X, -X, +Y, -Y, +Z, -Z).
    pub textures: Vec<[u16; 6]>,

    /// Ses setleri (place, break, step).
    pub sounds: Vec<SoundSet>,

    /// Drop tanımları.
    pub drops: Vec<Option<DropDefinition>>,

    /// State tanımları (block başına variant listesi).
    pub state_defs: Vec<Vec<StateDefinition>>,

    /// Tag'ler (StringInterner ID).
    pub tags: Vec<Vec<StringId>>,

    // ══════════════════════════════════════════════
    // LOOKUP TABLES
    // ══════════════════════════════════════════════

    /// İsim → ID mapping (hızlı lookup).
    name_to_id: HashMap<StringId, u16>,

    /// Tag → blok listesi (hızlı tag query).
    tag_index: HashMap<StringId, Vec<u16>>,

    /// Registry versiyonu (schema migration için).
    pub version: u32,
}
```

### 3.3 Builder Pattern (Init Phase)

```rust
/// Registry builder — sadece init phase'da kullanılır.
/// TOML yükleme sonrası Arc'a dönüştürülür.
pub struct BlockRegistryBuilder {
    inner: BlockRegistryInner,
    interner: StringInterner,
}

impl BlockRegistryBuilder {
    pub fn new() -> Self { /* boş array'lerle başlat */ }

    /// TOML'den blok tanımı yükle ve registry'ye ekle.
    pub fn register_from_toml(&mut self, toml_path: &str) -> Result<u16> {
        let def: BlockTomlDef = toml::from_str(&std::fs::read_to_string(toml_path)?)?;
        self.register(def)
    }

    /// Bir blok tanımlamasını SoA array'lere yerleştir.
    pub fn register(&mut self, def: BlockTomlDef) -> Result<u16> {
        let id = self.inner.flags.len() as u16;

        // HOT PATH
        self.inner.flags.push(def.to_flags());
        self.inner.hardness.push(f32_to_f16(def.physics.hardness));
        self.inner.light.push(LightingCompact::from(&def.lighting));
        self.inner.appearance.push(AppearanceCompact::from(&def.appearance));
        self.inner.gpu_props.push(BlockGpuProps::from(&def));

        // WARM PATH (hardness zaten hot path'te, PhysicsExtra'da yok)
        self.inner.physics.push(def.physics.into_extra());
        self.inner.gameplay.push(def.gameplay.into());
        self.inner.connectivity.push(def.connectivity.into());

        // COLD PATH
        let name_id = self.interner.intern(&def.name);
        self.inner.names.push(name_id);
        self.inner.textures.push(def.appearance.to_texture_ids(&mut self.interner));
        self.inner.sounds.push(def.to_sound_set(&mut self.interner));
        self.inner.drops.push(def.gameplay.drop.map(Into::into));
        self.inner.state_defs.push(def.states.into_iter().map(Into::into).collect());
        self.inner.tags.push(def.tags.iter().map(|t| self.interner.intern(t)).collect());

        // LOOKUP
        self.inner.name_to_id.insert(name_id, id);

        // Tag index güncelle
        for tag_id in self.inner.tags.last().unwrap() {
            self.inner.tag_index.entry(*tag_id).or_default().push(id);
        }

        Ok(id)
    }

    /// Registry'yi tamamla, Arc'a sarmala.
    pub fn build(self) -> BlockRegistry {
        BlockRegistry(Arc::new(self.inner))
    }
}
```

### 3.4 Runtime Query API

```rust
impl BlockRegistryInner {
    /// Blok flags'ini O(1) oku. (Hot path, meshing/ray-tracing)
    #[inline(always)]
    pub fn flags(&self, block_type: u16) -> BlockFlags {
        // SAFETY: block_type her zaman palette tarafından doğrulanır
        unsafe { *self.flags.get_unchecked(block_type as usize) }
    }

    /// Blok şeffaf mı? (Meshing'de face culling)
    #[inline(always)]
    pub fn is_transparent(&self, block_type: u16) -> bool {
        self.flags(block_type).has(BlockFlags::TRANSPARENT)
    }

    /// Blok Işık geçirir mi? (Lighting propagation)
    #[inline(always)]
    pub fn light_permeability(&self, block_type: u16) -> u8 {
        unsafe { self.light.get_unchecked(block_type as usize) }.permeability
    }

    /// Blok sertliği (kırma süresi hesabı).
    #[inline(always)]
    pub fn hardness(&self, block_type: u16) -> f32 {
        f16_to_f32(unsafe { *self.hardness.get_unchecked(block_type as usize) })
    }

    /// GPU property buffer'ı (SSBO upload).
    pub fn gpu_buffer(&self) -> &[BlockGpuProps] {
        &self.gpu_props
    }

    /// İsme göre blok ID'si bul (init/setup time).
    pub fn get_id(&self, name: &str) -> Option<u16> {
        // StringId lookup — cold path
        self.name_to_id.get(&StringId::from(name)).copied()
    }

    /// Tag'e sahip tüm blok ID'lerini bul.
    pub fn get_by_tag(&self, tag: &str) -> &[u16] {
        self.tag_index
            .get(&StringId::from(tag))
            .map_or(&[], |v| v.as_slice())
    }
}
```

---

## 4. BlockFlags (Compact, u16)

```rust
/// Blok özellikleri bitmask'i (16-bit).
/// XBrickMap'te her blok query'sinde 2 byte okunur (u64 yerine → 4x bandwidth tasarrufu).
/// Cache-line: 64 byte = 32 block flags tek okumada.
#[repr(transparent)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockFlags(pub u16);

impl BlockFlags {
    pub const OPAQUE: u16           = 1 << 0;
    pub const TRANSPARENT: u16      = 1 << 1;
    pub const PASSABLE: u16         = 1 << 2;
    pub const GRAVITY_AFFECTED: u16 = 1 << 3;
    pub const CLIMBABLE: u16        = 1 << 4;
    pub const EMITS_LIGHT: u16      = 1 << 5;
    pub const HAS_INVENTORY: u16    = 1 << 6;
    pub const CONNECTS: u16         = 1 << 7;
    pub const REDSTONE: u16         = 1 << 8;
    pub const IS_FLUID: u16         = 1 << 9;
    pub const IS_LEAVES: u16        = 1 << 10;
    pub const IS_LOG: u16           = 1 << 11;
    pub const IS_ORE: u16           = 1 << 12;
    pub const BLAST_RESISTANT: u16  = 1 << 13;
    pub const BOUNCY: u16           = 1 << 14;
    pub const SLOWING: u16          = 1 << 15;

    #[inline(always)]
    pub fn has(self, flag: u16) -> bool {
        self.0 & flag != 0
    }

    #[inline(always)]
    pub fn is_opaque(self) -> bool { self.has(Self::OPAQUE) }
    #[inline(always)]
    pub fn is_passable(self) -> bool { self.has(Self::PASSABLE) }
    #[inline(always)]
    pub fn emits_light(self) -> bool { self.has(Self::EMITS_LIGHT) }
    #[inline(always)]
    pub fn is_fluid(self) -> bool { self.has(Self::IS_FLUID) }
}
```

---

## 5. Compact Hot-Path Structs

### 5.1 LightingCompact (2 byte)

```rust
/// Aydınlatma özellikleri — 2 byte packed.
/// Hot path: lighting propagation sisteminde her voxel için okunur.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct LightingCompact {
    /// Işık emisyonu (0-15, 4-bit) + light_permeability (0-15, 4-bit)
    pub packed: u8,
    /// Sky light permeability (0-15, 4-bit) + reserved (4-bit)
    pub sky_packed: u8,
}

impl LightingCompact {
    #[inline(always)]
    pub fn emission(self) -> u8 { self.packed & 0x0F }
    #[inline(always)]
    pub fn permeability(self) -> u8 { (self.packed >> 4) & 0x0F }
    #[inline(always)]
    pub fn sky_permeability(self) -> u8 { self.sky_packed & 0x0F }
}
```

### 5.2 AppearanceCompact (4 byte)

```rust
/// Görünüm özellikleri — 4 byte packed.
/// Hot path: meshing'de face culling, render queue seçimi.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AppearanceCompact {
    /// Bit 0-1: TransparencyType (Opaque=0, Transparent=1, Translucent=2)
    /// Bit 2-3: RenderType (Solid=0, Cutout=1, Translucent=2, Alpha=3)
    /// Bit 4:   supports_ao (bool)
    /// Bit 5:   has_animated_texture (bool)
    /// Bit 6-7: reserved
    pub packed_props: u8,

    /// Tint color index (biome tint lookup table index, 0 = none).
    pub tint_index: u8,

    /// Texture atlas face offset (6 yüz için base index).
    /// Gerçek face ID'ler: atlas_base + [0..5]
    pub atlas_base: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransparencyType { Opaque = 0, Transparent = 1, Translucent = 2 }

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RenderType { Solid = 0, Cutout = 1, Translucent = 2, Alpha = 3 }

impl AppearanceCompact {
    #[inline(always)]
    pub fn transparency(self) -> TransparencyType {
        unsafe { std::mem::transmute(self.packed_props & 0x03) }
    }
    #[inline(always)]
    pub fn render_type(self) -> RenderType {
        unsafe { std::mem::transmute((self.packed_props >> 2) & 0x03) }
    }
    #[inline(always)]
    pub fn supports_ao(self) -> bool { self.packed_props & 0x10 != 0 }
}
```

### 5.3 BlockGpuProps (GPU SSBO — #[repr(C)], WGSL-aligned)

```rust
/// GPU meshing compute shader için blok özellikleri.
/// Vec<BlockGpuProps> → bytemuck::cast_slice → SSBO olarak zero-copy upload.
///
/// WGSL storage buffer alignment kuralları:
///   - struct alignment = max member alignment = 4 (u32)
///   - struct size must be multiple of alignment
/// Boyut: 8 byte/block (u32 packed + u32 packed).
/// 8192 blok = 64 KB (L1 cache'e sığar).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockGpuProps {
    /// Word 0: flags(u16) | atlas_base(u16) — packed into single u32
    pub packed_word0: u32,
    /// Word 1: tint([u8;4]) — packed into single u32
    pub packed_word1: u32,
}

impl BlockGpuProps {
    pub fn new(flags: u16, atlas_base: u16, tint: [u8; 4]) -> Self {
        Self {
            packed_word0: (flags as u32) | ((atlas_base as u32) << 16),
            packed_word1: u32::from_le_bytes(tint),
        }
    }
}
```

**WGSL Kullanımı (Rust layout ile birebir eşleşir):**

```wgsl
struct BlockGpuProps {
    packed_word0: u32,  // flags(16) | atlas_base(16)
    packed_word1: u32,  // tint RGBA
};

@group(1) @binding(0)
var<storage, read> block_props: array<BlockGpuProps>;

fn get_flags(block_type: u32) -> u32 {
    return block_props[block_type].packed_word0 & 0xFFFFu;
}

fn get_atlas_base(block_type: u32) -> u32 {
    return block_props[block_type].packed_word0 >> 16u;
}

fn is_face_visible(neighbor_type: u32) -> bool {
    // TRANSPARENT flag = bit 1
    return (get_flags(neighbor_type) & 2u) != 0u;
}
```

---

## 6. Warm Path — Fizik Özellikleri (PhysicsExtra)

```rust
/// Hot path'te `hardness: Vec<u16>` (f16) zaten tutulur.
/// PhysicsExtra sadece ek parametreleri barındırır (duplikasyon yok).
pub struct PhysicsExtra {
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
    pub slow_factor: f32,
}
```

---

## 7. Warm Path — Aydınlatma Özellikleri (Full)

```rust
/// Tam aydınlatma özellikleri (warm path — light placement sırasında kullanılır).
/// Hot path LightingCompact (§5.1) kullanılır.
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

## 8. Warm Path — Gameplay Özellikleri

```rust
pub struct GameplayProperties {
    /// Blok kırıldığında düşen item.
    pub drop: Option<DropDefinition>,
    /// Hangi alet ile kırılabilir.
    pub tool_requirement: Option<ToolRequirement>,
    /// Redstone benzeri mekanik destek.
    pub supports_redstone: bool,
    /// Envanter slotu var mı? (sandık, fırın).
    pub has_inventory: bool,
    /// Yakıt olarak kullanılabilir mi?
    pub fuel_value: u16,
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
pub enum ToolType { None, Pickaxe, Axe, Shovel, Hoe }
```

---

## 9. Warm Path — Bağlantı/Komşuluk Kuralları

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
    Full,
    SameType,
    Tag(StringId), // String yerine StringId (interner)
}

pub struct FaceRule {
    pub connects: bool,
    pub offset: Vec3,
    pub shape: ConnectionShape,
}

pub enum ConnectionShape { FullFace, HalfFace, QuarterFace, Post, Cross }
```

---

## 10. State Sistemi (Packed Palette Encoding)

### 10.1 Mimari Karar

Eski tasarımda state bilgisi block ID'nin üst 4 bit'ine encode ediliyordu (16 variant sınırı). Bu:

- Block type sayısını 4096'ya sınırlıyordu
- Karmaşık bloklar (merdiven: 24+ variant) için yetersizdi
- XBrickMap'in palet sistemiyle çakışıyordu

**Yeni yaklaşım:** State bilgisi, XBrickMap'in palette indisi içine **packed** edilir. Her voxel için ECS entity **Oluşturulmaz** (milyarlarca entity = felaket). Bunun yerine palette indisi hem block type hem state variant taşır.

```
State Encoding Akışı:
  Palette Index (u8) → SectorPalette.resolve(u8) → (block_type: u16, state_variant: u16)
```

### 10.2 PackedPaletteIndex

```rust
/// Bir voxel'in tam kimliği.
/// SectorPalette içinde bu struct'a map edilir.
/// Palette index (u8) bu struct'ın indeksidir.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PaletteEntry {
    /// Block type ID (GlobalPalette index).
    pub block_type: u16,
    /// State variant index (block'un state_defs'ine göre).
    /// 0 = default state (state yoksa her zaman 0).
    pub variant: u16,
}
```

### 10.3 State Tanımları

```rust
pub struct StateDefinition {
    /// State ismi (StringId — interner).
    pub name: StringId,
    /// State tipi.
    pub state_type: StateType,
}

pub enum StateType {
    /// Enum state (facing: north, south, east, west).
    Enum(Vec<StringId>),
    /// Boolean state (powered: true/false).
    Boolean,
    /// Integer state (power_level: 0-15).
    Integer { min: i32, max: i32 },
}
```

### 10.4 State Kayıt ve Lookup

State'ler registry içinde block type başına kaydedilir:

```rust
/// Block tipinin state kombinasyonlarını encode eder.
/// Her entry bir PaletteEntry'e karşılık gelir.
///
/// Örnek: Merdiven bloğu (10 state property: facing(4) × half(2) × shape(5) = 40 variant)
/// Variant 0: (facing=N, half=bottom, shape=straight)
/// Variant 1: (facing=S, half=bottom, shape=straight)
/// ...
pub struct BlockStatePalette {
    /// block_type → tüm variant'ların listesi.
    /// Index = variant ID.
    pub entries: Vec<PaletteEntry>,
    /// (block_type, state_values) → variant ID lookup.
    /// HashMap sadece init phase'ta doldurulur, runtime'da kullanılmaz.
    pub lookup: HashMap<(u16, Vec<StateValue>), u16>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub enum StateValue {
    Bool(bool),
    Enum(StringId),
    Int(i32),
}
```

### 10.5 State Ownership (03-ecs ile hizalı)

**Kesinleşmiş model (hibrit):**

| Kategori | Depolama | Örnekler |
|----------|----------|----------|
| Görsel / sync / meshing | `PaletteEntry.variant` | `facing`, `half`, `powered`, redstone `0–15` |
| Büyük / seyrek | `ChunkBlockEntities` | sandık, fırın, tabela, repeater `delay` |

TOML `[states]` tanımları `state_defs` → `BlockStatePalette` → sektör `variant` ID’sine map edilir. Envanter gibi alanlar registry’de değil, 03 §10.6.1’deki `BlockEntityData` ile tutulur.

### 10.6 Neden ECS Entity Değil? (Voxel state)

| Yaklaşım | 10M state'li blok | Bellek | Performans |
|---|---|---|
| **ECS Entity (eski)** | 10M entity | ~480 MB (48B/entity min) | Query iteration: yavaş |
| **Packed Palette (yeni)** | 0 ek entity | ~0 MB (zaten palette'de) | O(1) lookup |

State değişimi (örn: kapı açma) şu şekilde çalışır:

1. `SectorPalette`'de eski entry bulunur
2. Yeni (block_type, variant+1) için entry oluşturulur veya bulunur
3. SubBrick'teki u8 index güncellenir
4. `ChunkDirty` flag'i işaretlenir → remesh tetiklenir

---

## 11. String Interner

```rust
/// Tüm string'ler startup'ta intern edilir, runtime'da u32 ID ile kullanılır.
/// String karşılaştırması yerine u32 karşılaştırması (O(1), cache-friendly).
///
/// string_interner crate'i veya custom implementation kullanılabilir.
pub struct StringInterner {
    map: HashMap<String, StringId>,
    strings: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(u32);

impl StringInterner {
    pub fn intern(&mut self, s: &str) -> StringId {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = StringId(self.strings.len() as u32);
        self.strings.push(s.to_owned());
        self.map.insert(s.to_owned(), id);
        id
    }

    pub fn resolve(&self, id: StringId) -> &str {
        &self.strings[id.0 as usize]
    }
}
```

---

## 12. Sound Set (Cold Path)

```rust
/// Ses setleri — cold path, sadece event dispatch'te kullanılır.
pub struct SoundSet {
    pub place: StringId,
    pub break_: StringId,
    pub step: StringId,
}
```

---

## 13. Block Data Loading (TOML)

```toml
# blocks/stone.toml
[name]
id = 1
display_name = "Stone"

[appearance]
textures = ["stone", "stone", "stone", "stone", "stone", "stone"]
render_type = "solid"
transparency = "opaque"
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

[states]
# Stone için state yok (basit blok)

[tags]
mineable = ["pickaxe"]
natural = true
```

---

## 14. XBrickMap Palette Entegrasyonu (SectorPalette)

### 14.1 u8 → u16 Mapping Sorunu ve Çözümü

XBrickMap SubBrick `u8` palette indisi kullanır. BlockRegistry ise `u16` block type ID kullanır.
Bu çelişkiyi çözmek için **SectorPalette** katmanı eklenir:

```
4-Seviyeli Palet Zinciri:

  SubBrick.indices[voxel_bit_idx]  →  u8 local_index (max 8/sub-brick)
    ↓
  BrickPalette.materials[u8]       →  u8 brick_local_index (max 16, opsiyonel)
    ↓
  SectorPalette.entries[u8]        →  PaletteEntry { block_type: u16, variant: u16 }  // tek kaynak
    ↓
  BlockRegistry.flags[block_type]  →  BlockFlags (hot query)
```

### 14.2 SectorPalette Yapısı

```rust
/// Sektör başına palette mapping.
/// u8 local index → (block_type: u16, state_variant: u16)
/// Max 256 farklı (block_type + state) kombinasyonu per sektör.
///
/// Tipik kullanım: 20-40 farklı blok tipi × 1-2 state = 30-60 entry.
/// 256 sınırı pratikte hiçbir zaman aşılmaz.
///
/// Bellek: 256 × 4 byte = 1 KB/sector (max), tipik ~120-240 byte.
/// Sadece dolu sektörlerde oluşturulur.
pub struct SectorPalette {
    /// u8 index → PaletteEntry.
    /// Entry 0 her zaman AIR (block_type=0, variant=0).
    pub entries: heapless::Vec<PaletteEntry, 256>,

    /// Reverse lookup: (block_type, variant) → u8 index.
    /// Sadece edit-time kullanılır (blok koyma/kırma).
    /// Runtime'da kullanılmaz, serialization'da atılır.
    pub reverse: HashMap<PaletteEntry, u8>,
}

impl SectorPalette {
    /// Bir palette entry'yi resolve et.
    #[inline(always)]
    pub fn resolve(&self, local_index: u8) -> PaletteEntry {
        // SAFETY: local_index her zaman SubBrick mask tarafından doğrulanır
        unsafe { *self.entries.get_unchecked(local_index as usize) }
    }

    /// Bir (block_type, variant) çifti için local index bul veya oluştur.
    /// Returns: Ok(u8) = mevcut veya yeni index
    ///          Err(PaletteFullError) = 256 entry aşıldı
    pub fn get_or_insert(&mut self, entry: PaletteEntry) -> Result<u8, PaletteFullError> {
        if let Some(&idx) = self.reverse.get(&entry) {
            return Ok(idx);
        }
        if self.entries.len() >= 256 {
            return Err(PaletteFullError { /* sector, entry — §20 */ });
        }
        if self.entries.len() >= 200 {
            tracing::warn!("SectorPalette near full: {} entries", self.entries.len());
        }
        let idx = self.entries.len() as u8;
        self.entries.push(entry);
        self.reverse.insert(entry, idx);
        Ok(idx)
    }
}
```

Detaylı overflow politikası: **§20**.

### 14.3 GlobalPalette (Read-Only, Init Phase)

```rust
/// Global materyal palette — sadece init phase'da BlockRegistry'den oluşturulur.
/// Runtime'da salt-okunur. XBrickMap'in ray-tracing ve mip-map sistemleri kullanır.
///
/// NOT: Block type → palette index mapping artık SectorPalette üzerinden yapılır.
/// GlobalPalette sadece materyal özellikleri (color, emission, opacity) için kullanılır.
#[derive(Resource)]
pub struct GlobalPalette {
    /// index = block_type_id
    pub materials: Vec<MaterialDef>,
}

pub struct MaterialDef {
    pub name: StringId,
    pub color: [u8; 3],
    pub emission: u8,
    pub opacity: u8,
}
```

### 14.4 Mod Namespace Sistemi

```rust
/// Blok isimleri namespace ile yönetilir.
/// Format: "namespace:block_name"
/// Örnek: "strata:stone", "mymod:custom_ore"
///
/// Namespace registry'de her mod'a bir aralık tahsis edilir:
///   strata (vanilla): 1-2047
///   mod_1:            2048-4095
///   mod_2:            4096-6143
///   ... (dinamik)
///
/// StringId ile saklanır, runtime'da string karşılaştırması yapılmaz.
pub struct ModNamespace {
    pub name: StringId,
    pub id_range: std::ops::Range<u16>,
}
```

**TOML'de namespace kullanımı:**

```toml
[name]
namespace = "strata"
id = 1
display_name = "Stone"
```

---

## 15. Bevy Plugin Entegrasyonu

```rust
/// BlockRegistryPlugin — Startup phase'da TOML'leri yükler, registry'yi oluşturur.
pub struct BlockRegistryPlugin {
    /// TOML blok tanım dosyalarının dizini.
    pub blocks_dir: String,
}

impl Plugin for BlockRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, self.load_blocks);
    }
}

impl BlockRegistryPlugin {
    fn load_blocks(&self, mut commands: Commands) {
        let mut builder = BlockRegistryBuilder::new();

        // AIR (0) her zaman ilk kayıt
        builder.register(BlockTomlDef::air()).unwrap();

        // TOML dizininden tüm blok tanımlarını yükle (namespace-aware)
        for entry in glob::glob(&format!("{}/**/*.toml", self.blocks_dir)).unwrap() {
            builder.register_from_toml(entry.unwrap().to_str().unwrap()).unwrap();
        }

        let registry = builder.build();

        // GlobalPalette oluştur (XBrickMap entegrasyonu)
        let palette = GlobalPalette::from(&*registry.0);

        commands.insert_resource(registry);
        commands.insert_resource(palette);
        // SectorPalette: runtime'da her sektör spawn edildiğinde oluşturulur
    }
}
```

---

## 16. Crate Organizasyonu

*(Bkz. §21 güncel versiyonu için)*

---

## 17. Block Behavior System (Dinamik Blok Mantığı)

### 17.1 Sorun

TOML + flags sadece **statik** özellikleri ifade edebilir. Peki ya:

- Su bloğu komşuya akacak
- Ateş bloğu odunu yakacak
- Kum bloğu yerçekimiyle düşecek
- Redstone mekaniği sinyal yayacak

Bunlar TOML'de ifade edilemez. Runtime'da **kod çalıştırması** gerekir.

### 17.2 Çözüm: Function Pointer Table

```
BlockBehavior — Warm Path (sadece dinamik bloklar için)
│
├── behaviors: Vec<Option<BlockBehavior>>  // index = block_type_id
│   └── BlockBehavior {
│       tick:      Option<fn(...)>,       // per-tick mantık (su, ateş)
│       neighbor:  Option<fn(...)>,       // komşu değişince (kum düşmesi)
│       place:     Option<fn(...)>,       // yerleştirilince (yön hesaplama)
│       interact:  Option<fn(...)>,       // oyuncu etkileşimi (kapı, sandık)
│   }
```

```rust
/// Dinamik blok davranışları — sadece statik olmayan bloklar için tanımlı.
/// %95 blok Static (None) → hot path'te branch yok.
/// Function pointer: vtable lookup yok, cache-friendly, static dispatch.
///
/// Mod'lar Rust ile custom behavior register edebilir.
/// TOML'den yüklenemez (kod gerektirir).
pub struct BlockBehavior {
    /// Random tick davranışı (su akışı, ateş yayılması, büyüme).
    /// Frequency: ~20 tick/saniye, random pozisyonlarda.
    pub on_random_tick: Option<fn(ctx: &mut BlockContext, pos: IVec3, block_type: u16)>,

    /// Komşu blok değiştiğinde çağrılır (kum düşmesi, redstone güncelleme).
    pub on_neighbor_change: Option<fn(ctx: &mut BlockContext, pos: IVec3, neighbor: IVec3)>,

    /// Blok yerleştirildiğinde çağrılır (yön hesaplama, state ayarlama).
    /// Return: final variant ID (yön bilgisi vb.)
    pub on_place: Option<fn(ctx: &BlockContext, pos: IVec3, placer: Entity) -> u16>,

    /// Oyuncu sağ-tıkladığında (kapı açma, sandık açma, crafting table).
    /// Return: true = etkileşim başarılı (item kullanımı tüketildi)
    pub on_interact: Option<fn(ctx: &mut BlockContext, pos: IVec3, player: Entity) -> bool>,
}

/// Block behavior'a erişim için context.
/// World'a güvenli erişim sağlar (sector okuma/yazma, entity spawn).
pub struct BlockContext<'a> {
    pub world: &'a mut World,
    pub registry: &'a BlockRegistryInner,
}
```

### 17.3 Registry Entegrasyonu

```rust
// BlockRegistryInner'a eklenir:
pub behaviors: Vec<Option<BlockBehavior>>,  // warm path, index = block_type

// Query API:
impl BlockRegistryInner {
    /// Blok dinamik davranışa sahip mi? (Hot check: Option None kontrolü)
    #[inline(always)]
    pub fn has_behavior(&self, block_type: u16) -> bool {
        unsafe { self.behaviors.get_unchecked(block_type as usize) }.is_some()
    }

    /// Random tick davranışı çağır.
    pub fn random_tick(&self, ctx: &mut BlockContext, pos: IVec3, block_type: u16) {
        if let Some(behavior) = &self.behaviors[block_type as usize] {
            if let Some(tick_fn) = behavior.on_random_tick {
                tick_fn(ctx, pos, block_type);
            }
        }
    }
}
```

### 17.4 Performans Analizi


| Yaklaşım                     | Maliyet             | Cache              | Mod Support      |
| ---------------------------- | ------------------- | ------------------ | ---------------- |
| **Trait Object (dyn)**       | vtable lookup ~5ns  | kötü (indirection) | İyi              |
| **Enum Dispatch**            | match branch ~1ns   | İyi                | Kötü (recompile) |
| **Function Pointer (bizim)** | Option check ~0.5ns | İyi (inline)       | İyi (register)   |


%95 blok `None` olduğu için, `Option::is_none()` check = tek bir null pointer karşılaştırması = ~0.5ns. Dinenamik bloklar için fn pointer call = ~1ns. Vtable lookup'a göre **5x hızlı**.

---

## 18. Neighbor Visibility LUT (Greedy Meshing Optimizasyonu)

### 18.1 Sorun

Greedy meshing sırasında en sık yapılan sorgu: "Bu blok ile komşu blok arasında yüz render edilmeli mi?" Bu sorgu her voxel yüzü için tekrar tekrar yapılır. Bir sektor için ~98K yüz = 98K query.

Mevcut yaklaşım:

```rust
// Her yüz için:
let neighbor_flags = registry.flags(neighbor_type);
let visible = neighbor_flags.has(BlockFlags::TRANSPARENT);
```

Bu zaten hızlı (~0.5ns) ama daha da hızlandırılabilir.

### 18.2 Çözüm: Precomputed 2D Visibility Table

```rust
/// Komşuluk görünürlük tablosu — precomputed, cache-line aligned.
/// face_visible[source_type][neighbor_type] → bool
///
/// Boyut: 256 × 256 bit = 8 KB (L1 cache'e sığar)
/// NOT: Sadece ilk 256 block type için precompute edilir.
/// 256+ block type'lar için runtime fallback (flags check).
///
/// Meshing'de kullanım: face_visible[source][neighbor] → tek bir bit okuma
/// Maliyet: ~0.3ns (L1 cache hit, bit extraction)
pub struct VisibilityTable {
    /// 256 × 256 bit matrix, packed into [u8; 8192].
    /// Row = source block type, Col = neighbor block type.
    /// Bit set = face visible (neighbor transparent or different).
    data: Box<[u8; 8192]>,
}

impl VisibilityTable {
    /// Init phase'da BlockRegistry'den oluşturulur.
    pub fn build(registry: &BlockRegistryInner) -> Self {
        let mut data = Box::new([0u8; 8192]);
        for src in 0..256u16 {
            for nbr in 0..256u16 {
                let visible = if src == nbr {
                    false // aynı blok → yüz gizli
                } else {
                    // Komşu şeffaf mı veya farklı render tipi mi?
                    registry.flags(nbr).has(BlockFlags::TRANSPARENT)
                        || registry.appearance.get(nbr as usize)
                            .map_or(false, |a| a.render_type() != RenderType::Solid)
                };
                if visible {
                    let idx = (src as usize) * 256 + (nbr as usize);
                    data[idx / 8] |= 1 << (idx % 8);
                }
            }
        }
        Self { data }
    }

    /// O(1) yüzey görünürlük sorgusu.
    #[inline(always)]
    pub fn is_face_visible(&self, source: u16, neighbor: u16) -> bool {
        if source >= 256 || neighbor >= 256 {
            return true; // Fallback: 256+ block type'lar için conservative
        }
        let idx = (source as usize) * 256 + (neighbor as usize);
        (self.data[idx / 8] >> (idx % 8)) & 1 != 0
    }
}
```

### 18.3 Performans Karşılaştırması


| Yaklaşım             | Maliyet/yüz      | 98K yüz (1 sektör)  |
| -------------------- | ---------------- | ------------------- |
| Flags check (mevcut) | ~0.5ns           | ~49μs               |
| **Visibility LUT**   | **~0.3ns**       | **~29μs**           |
| Kazanç               | **%40 hızlanma** | **~20μs tasarrufu** |


---

## 19. Save/Load ID Stability (String-Based Serialization)

### 19.1 Sorun

Dünya dosyasında block type'lar `u16` ID ile saklanırsa:

- Mod eklenince/çıkarılınca ID'ler kayar
- Dünya dosyası bozulur
- Minecraft'ın ilk sürümlerinde yaşanan klasik problem

### 19.2 Çözüm: String-ID Mapping Table

```rust
/// Save dosyasında her sektör için ID mapping tablosu saklanır.
/// Runtime ID'leri değişebilir, ama string isimleri sabittir.
///
/// Save format:
///   sector_header {
///     palette: [(string_id: StringId, count: u32), ...]
///     // string_id → "strata:stone", "mymod:custom_ore"
///   }
///
/// Load süreci:
///   1. String ID'leri oku
///   2. BlockRegistry.name_to_id ile runtime ID'lerini resolve et
///   3. Eşleşmeyen ID'ler → AIR (veya fallback blok)
///   4. SectorPalette'i yeniden oluştur
///
/// Maliyet: Sadece load time (sektör başına ~30-60 string lookup).
/// Runtime'da etkisi yok.
pub struct SectorSaveHeader {
    /// Palette mapping: local_index → string name.
    /// Load time'da resolve edilir.
    pub palette_entries: Vec<(StringId, u32)>, // (name, count)
    /// Schema version (migration için).
    pub schema_version: u32,
}

impl SectorSaveHeader {
    /// Load time: string ID'lerini runtime ID'lerine çevir.
    pub fn resolve(&self, registry: &BlockRegistryInner) -> Vec<PaletteEntry> {
        self.palette_entries.iter().map(|(name_id, _count)| {
            match registry.name_to_id.get(name_id) {
                Some(&block_type) => PaletteEntry { block_type, variant: 0 },
                None => PaletteEntry { block_type: 0, variant: 0 }, // AIR fallback
            }
        }).collect()
    }
}
```

### 19.3 Mod Ekleme/Çıkarma Senaryosu

```
1. Dünya oluşturulur: stone(1), dirt(2), mymod:custom_ore(2048)
2. Save edilir:  ["strata:stone", "strata:dirt", "mymod:custom_ore"]
3. Mod kaldırılır
4. Load edilir:
   - "strata:stone" → ID 1 ✓
   - "strata:dirt"   → ID 2 ✓
   - "mymod:custom_ore" → not found → AIR (veya kullanıcıya uyarı)
5. Mod geri eklenir → tüm ID'ler doğru resolve edilir
```

---

## 20. Palette Overflow Handling (256 Sınırı — Fail-Fast)

### 20.1 Problem

`SectorPalette` en fazla **256** farklı `(block_type, variant)` tutar. Teorik olarak (çok modlu sunucu, yapı botu) bu sınır aşılabilir.

### 20.2 Kesinleşmiş strateji: Fail-Fast (LRU yok)

**Neden LRU kullanılmıyor:** Eviction, mevcut voxel'lerin yanlış `PaletteEntry` ile çözümlenmesine ve sessiz dünya bozulmasına yol açar. Debug maliyeti yüksek; oyun bütünlüğü için kabul edilemez.

**Politika:**
1. `entries.len() >= 256` → `get_or_insert` **`Err(PaletteFullError)`** döner; blok yerleştirme **reddedilir**.
2. `entries.len() >= 200` → `tracing::warn!` + metrik (`palette_near_full`).
3. İstemciye/sunucuya: kullanıcıya “sector palette full” (opsiyonel UI); creative modda admin override **kapalı** (güvenlik).

**Pratik beklenti:** Tipik sektör 20–40 entry; mod-heavy 60–80; 256 üstü yalnızca stress test.

```rust
#[derive(Debug, thiserror::Error)]
#[error("sector palette full (256 unique block+variant combinations)")]
pub struct PaletteFullError {
    pub sector: IVec3,
    pub attempted: PaletteEntry,
}

pub struct PaletteOverflowStats {
    pub reject_count: u32,
    pub peak_entries: u8,
    pub near_full_warn_count: u32,
}

impl SectorPalette {
    /// Yeni (block_type, variant) için local index; doluysa hata.
    pub fn get_or_insert(&mut self, entry: PaletteEntry) -> Result<u8, PaletteFullError> {
        if let Some(&idx) = self.reverse.get(&entry) {
            return Ok(idx);
        }
        if self.entries.len() >= 256 {
            return Err(PaletteFullError { /* sector, entry */ });
        }
        if self.entries.len() >= 200 {
            tracing::warn!("SectorPalette near full: {} entries", self.entries.len());
        }
        let idx = self.entries.len() as u8;
        self.entries.push(entry);
        self.reverse.insert(entry, idx);
        Ok(idx)
    }
}
```

**Gelecek (plan dışı):** Sektör bölme veya palet shard — yalnızca fail-fast yetersiz kalırsa değerlendirilir (06 §2.1 notu).

---

## 21. Crate Organizasyonu (Güncel)

```
crates/
  core/
    ├── registry/
    │   ├── mod.rs           ← BlockRegistry, BlockRegistryBuilder, BlockRegistryPlugin
    │   ├── flags.rs         ← BlockFlags (u16 bitmask)
    │   ├── hot_path.rs      ← LightingCompact, AppearanceCompact, BlockGpuProps
    │   ├── physics.rs       ← PhysicsExtra
    │   ├── gameplay.rs      ← GameplayProperties, DropDefinition, ToolRequirement
    │   ├── connectivity.rs  ← ConnectivityRules, FaceRule, ConnectionShape
    │   ├── state.rs         ← StateDefinition, BlockStatePalette, PaletteEntry
    │   ├── behavior.rs      ← BlockBehavior, BlockContext (function pointer table)
    │   ├── visibility.rs    ← VisibilityTable (precomputed neighbor LUT)
    │   ├── interner.rs      ← StringInterner, StringId
    │   ├── sounds.rs        ← SoundSet
    │   ├── palette.rs       ← GlobalPalette, SectorPalette (XBrickMap bridge)
    │   ├── namespace.rs     ← ModNamespace, namespace registry
    │   ├── serialization.rs ← SectorSaveHeader (save/load ID stability)
    │   └── loader.rs        ← TOML parsing, BlockTomlDef
    └── block_id.rs          ← Block type ID encoding/decoding
```

---

## 22. Tasarım Karşılaştırması (Final)


| Özellik                   | Orijinal Tasarım                | Final Tasarım                                      |
| ------------------------- | ------------------------------- | -------------------------------------------------- |
| **Veri Yapısı**           | AoS (`Vec<BlockDefinition>`)    | SoA (hot/warm/cold ayrı array'ler)                 |
| **Thread Safety**         | Rc/RefCell (Bevy ile uyumsuz)   | Arc (Send + Sync)                                  |
| **Flags Boyutu**          | u64 (8 byte)                    | u16 (2 byte, 4x tasarruf)                          |
| **String Kullanımı**      | `String` her yerde (heap)       | `StringId` (u32, interner)                         |
| **State Encoding**        | Block ID üst 4 bit (16 variant) | Packed PaletteEntry (sınırsız variant)             |
| **State Storage**         | ECS Entity (per-voxel)          | SectorPalette + SubBrick index (0 ek entity)       |
| **Block Type Limit**      | 4096 (12 bit)                   | 65535 (16 bit)                                     |
| **Palette Mapping**       | Direkt (u8=u16 çelişkisi)       | SectorPalette u8→u16 bridge                        |
| **GPU Entegrasyon**       | Yok                             | `BlockGpuProps` SSBO (WGSL-aligned, zero-copy)     |
| **XBrickMap Entegrasyon** | Yok                             | 4-seviyeli palet zinciri                           |
| **Mod Support**           | Yok                             | Namespace sistemi (namespace:block_name)           |
| **Cache Performansı**     | Düşük (pointer chase, AoS)      | Yüksek (SoA, cache-line packed)                    |
| **Registry Mutability**   | Runtime `&mut self`             | Init-only builder → immutable Arc                  |
| **Hardness Duplikasyon**  | Hot + Warm (2x)                 | Sadece Hot (f16)                                   |
| **Block Behavior**        | Yok                             | Function pointer table (static dispatch, %95 None) |
| **Meshing Optimizasyonu** | Yok                             | VisibilityTable LUT (%40 hızlanma)                 |
| **Save Compatibility**    | Yok                             | String-based ID resolution (mod-safe)              |
| **Palette Overflow**      | Tanımsız                        | Fail-fast (`PaletteFullError`) + warn @200         |
| **State vs BlockEntity**  | Belirsiz                        | Hibrit (03 §10.6.1 matrisi)                        |

---

## 23. Araştırma Doğrulamaları ve Öneriler (2026-06)

> **Kaynak:** 5 worker ile 40+ WebSearch sorgusu, Rust ekosistem crate'leri, akademik voxel motor literatürü.

### 23.1 Doğrulanan Kararlar

| Karar | Doğrulama |
|-------|-----------|
| SoA hot/warm/cold layout | Cache-friendly, SIMD-iteration ready |
| u16 BlockID (65535 max) | Sektör palette u8 bridge ile uyumlu |
| SectorPalette fail-fast | LRU eviction sessiz dünya bozulmasına yol açar |
| VisibilityTable LUT | Precomputed neighbor visibility, %40 meshing hızlanma |
| Function pointer behavior table | %95 None → static dispatch, vtable lookup yok |

### 23.2 P0 — `bitflags` Crate Geçişi (Kesin)

**Problem:** Mevcut `BlockFlags` manuel bitmask implementasyonu (`pub const OPAQUE: u16 = 1 << 0`).

**Çözüm:** `bitflags` crate kullanımı.

```rust
// Eski (manuel):
impl BlockFlags {
    pub const OPAQUE: u16 = 1 << 0;
    pub fn has(self, flag: u16) -> bool { self.0 & flag != 0 }
}

// Yeni (bitflags):
bitflags::bitflags! {
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    pub struct BlockFlags: u16 {
        const OPAQUE           = 1 << 0;
        const TRANSPARENT      = 1 << 1;
        const PASSABLE         = 1 << 2;
        const GRAVITY_AFFECTED = 1 << 3;
        const CLIMBABLE        = 1 << 4;
        const EMITS_LIGHT      = 1 << 5;
        const HAS_INVENTORY    = 1 << 6;
        const CONNECTS         = 1 << 7;
        const REDSTONE         = 1 << 8;
        const IS_FLUID         = 1 << 9;
        const IS_LEAVES        = 1 << 10;
        const IS_LOG           = 1 << 11;
        const IS_ORE           = 1 << 12;
        const BLAST_RESISTANT  = 1 << 13;
        const BOUNCY           = 1 << 14;
        const SLOWING          = 1 << 15;
    }
}
```

**Avantajlar:**
- `#[repr(transparent)]` ile u16 layout korunur, zero overhead
- Type-safe API: `flags.contains(BlockFlags::OPAQUE)` vs `flags.has(BlockFlags::OPAQUE)`
- Debug format: `OPAQUE | IS_FLUID` (okunabilir)
- Serde string desteği (TOML'de `"OPAQUE | TRANSPARENT"`)
- AGENTS.md "tekerleği yeniden icat etme" kuralıyla uyumlu

**Entegrasyon:** Phase 1, §4 BlockFlags implementasyonu güncellenmeli.

### 23.3 P1 — TOML+RON Hibrit Format

**Problem:** Basit bloklar TOML ile tanımlanabilir ama state/enum tanımları TOML'de zor.

**Çözüm:** Hibrit format — basit bloklar TOML (content creator dostu), state/enum tanımları RON (Rust-native enum desteği).

```toml
# blocks/stone.toml — basit blok (TOML)
[name]
id = 1
display_name = "Stone"

[appearance]
render_type = "solid"
transparency = "opaque"
```

```ron
// blocks/door.states.ron — state tanımları (RON)
(
    block_type: "strata:door",
    states: [
        (name: "facing", type: Enum(["north", "south", "east", "west"])),
        (name: "half", type: Enum(["upper", "lower"])),
        (name: "open", type: Boolean),
        (name: "powered", type: Boolean),
    ],
    default_variant: 0,
)
```

**Avantaj:** Bevy zaten RON kullanıyor — ekosistem uyumlu. Content creator'lar basit bloklar için TOML'de kalır.

### 23.4 P2 — `soa-rs` Crate ve AoSoA Layout (Benchmark Gerekli)

**Problem:** Mevcut SoA layout manuel. `soa-rs` derive macro ile tek allocation, SIMD-friendly iteration.

**Değerlendirme:**

```rust
// soa-rs ile:
#[derive(soa_rs::StructOfArray)]
pub struct BlockGpuProps {
    pub packed_word0: u32,
    pub packed_word1: u32,
}

// Otomatik oluşturulan:
pub struct BlockGpuPropsVec {
    pub packed_word0: Vec<u32>,
    pub packed_word1: Vec<u32>,
}
```

**Etki:** GPU-critical hot path'de AoSoA %20-30 hızlanma (teorik). **Benchmark gerekli** — Phase 2'de değerlendir.

### 23.5 P3 — Tagged Palette Entry (Gelecek)

**Problem:** Mevcut `PaletteEntry` 4 byte (`block_type: u16` + `variant: u16`). Basit bloklar (variant=0) için fazla.

**Çözüm:** Basit bloklar için 4→2 byte packing:

```rust
// Tagged union — 2 byte (basit) veya 4 byte (karmaşık)
#[repr(C)]
pub union PaletteEntryPacked {
    simple: u16,           // block_type only (variant=0)
    full: PaletteEntry,    // block_type + variant
    tag: u32,              // tag bit + data
}
```

**Etki:** Sektör bellek %30-40 tasarrufu (tipik sektör %70 basit blok). **Phase 5+** — karmaşıklık maliyeti yüksek, ancak bellek kritik senaryolarda değerlendir.


