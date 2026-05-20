# 38 — Game State Save/Load

## 1. Genel Bakış

Strata'nın save/load sistemi **oyuncu verisi, dünya metadata, ve session** yönetimini sağlar. Chunk storage'dan ayrıdır.

### Temel Prensipler

- **Player data:** Envanter, pozisyon, sağlık, XP
- **World metadata:** Seed, zaman, hava durumu, keşif verisi
- **Session:** Çoklu dünya desteği
- **Auto-save:** Belirli aralıklarla otomatik kaydetme

---

## 2. Player Save Data

```rust
#[derive(Serialize, Deserialize)]
pub struct PlayerSaveData {
    /// Oyuncu UUID.
    pub uuid: String,

    /// Pozisyon.
    pub position: [f32; 3],

    /// Rotasyon.
    pub rotation: [f32; 2],

    /// Sağlık.
    pub health: f32,

    /// Açlık.
    pub hunger: f32,

    /// XP.
    pub xp: f32,
    pub xp_level: u32,

    /// Envanter.
    pub inventory: Vec<Option<ItemStack>>,

    /// Oyun modu.
    pub game_mode: u8,

    /// Keşfedilen alanlar.
    pub explored_chunks: Vec<ChunkCoord>,
}
```

---

## 3. World Metadata

```rust
#[derive(Serialize, Deserialize)]
pub struct WorldMetadata {
    /// Dünya ismi.
    pub name: String,

    /// Seed.
    pub seed: u64,

    /// Oluşturulma zamanı.
    pub created_at: u64,

    /// Son oynanma zamanı.
    pub last_played: u64,

    /// Toplam oynanma süresi.
    pub playtime_seconds: u64,

    /// Zaman of day.
    pub time_of_day: f32,

    /// Hava durumu.
    pub weather: WeatherState,

    /// Spawn noktası.
    pub spawn_point: [i32; 3],

    /// Generator versiyonu.
    pub generator_version: u32,
}
```

---

## 4. Save Manager

```rust
pub struct SaveManager {
    /// Aktif session.
    pub session: Option<Session>,

    /// Auto-save interval (saniye).
    pub auto_save_interval: f32,

    /// Auto-save timer.
    pub auto_save_timer: f32,
}

impl SaveManager {
    pub fn save_world(&mut self, world: &World, players: &[PlayerSaveData]) -> Result<()> {
        // World metadata kaydet
        // Player data kaydet
        // Chunk'ları flush et
    }

    pub fn load_world(&self, world_name: &str) -> Result<(WorldMetadata, Vec<PlayerSaveData>)> {
        // World metadata yükle
        // Player data yükle
    }
}
```

---

## 5. Session Management

```rust
pub struct Session {
    pub world_id: String,
    pub players: Vec<String>, // UUID listesi
    pub started_at: u64,
    pub is_multiplayer: bool,
}
```

---

## 6. Crate Organizasyonu

```
crates/
  save/
    ├── mod.rs
    ├── manager.rs
    ├── player_data.rs
    ├── world_metadata.rs
    ├── session.rs
    └── auto_save.rs
```
