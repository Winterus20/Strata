# 41 — Achievements & Statistics

## 1. Genel Bakış

Strata'nın achievements ve statistics sistemi oyuncu ilerlemesini takip eder. Hem lokal hem multiplayer'da çalışır.

### Temel Prensipler

- **Event-driven:** Oyun event'leri achievement unlock'larını tetikler
- **Persistent:** Unlock'lar save dosyasında saklanır
- **Conditional:** Karmaşık koşullar desteklenir
- **Statistics:** Detaylı oyuncu istatistikleri (blok kırma, mesafe, süre vb.)

---

## 2. Achievement System

```rust
#[derive(Serialize, Deserialize)]
pub struct Achievement {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub condition: AchievementCondition,
    pub unlocked: bool,
    pub unlocked_at: Option<u64>,
}

pub enum AchievementCondition {
    BlockBreak { block_type: u16, count: u32 },
    DistanceTraveled { meters: f32 },
    PlayTime { seconds: u64 },
    BlocksPlaced { block_type: u16, count: u32 },
    EntityKill { entity_type: u16, count: u32 },
    Custom { script: String },
}
```

---

## 3. Statistics Tracking

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct PlayerStatistics {
    pub blocks_broken: HashMap<u16, u64>,
    pub blocks_placed: HashMap<u16, u64>,
    pub distance_walked: f64,
    pub distance_flown: f64,
    pub distance_swum: f64,
    pub play_time_seconds: u64,
    pub entities_killed: HashMap<u16, u64>,
    pub deaths: u64,
    pub jumps: u64,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    pub items_crafted: HashMap<u16, u64>,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  achievements/
    ├── mod.rs
    ├── registry.rs
    ├── tracker.rs
    ├── conditions.rs
    └── notifications.rs
  statistics/
    ├── mod.rs
    ├── collector.rs
    ├── player_stats.rs
    └── aggregator.rs
```
