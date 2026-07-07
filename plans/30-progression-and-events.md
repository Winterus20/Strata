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


# 42 — Tutorial & Onboarding

## 1. Genel Bakış

Strata'nın tutorial sistemi yeni oyunculara oyun mekaniklerini öğretir. İsteğe bağlıdır ve atlanabilir.

### Temel Prensipler

- **Interactive:** Adım adım interaktif rehberlik
- **Contextual:** Oyuncunun bulunduğu duruma göre ipuçları
- **Skippable:** İstendiğinde atlanabilir
- **Non-intrusive:** Deneyimli oyuncuları rahatsız etmez

---

## 2. Tutorial System

```rust
pub struct TutorialManager {
    pub completed_steps: HashSet<String>,
    pub active_tutorial: Option<String>,
    pub current_step: usize,
}

#[derive(Serialize, Deserialize)]
pub struct Tutorial {
    pub id: String,
    pub title: String,
    pub steps: Vec<TutorialStep>,
    pub trigger: TutorialTrigger,
}

pub enum TutorialTrigger {
    OnStart,
    OnBlockBreak,
    OnFirstDeath,
    OnEnterDimension,
    OnCraft,
    Custom(String),
}

pub struct TutorialStep {
    pub id: String,
    pub text: String,
    pub highlight: Option<WorldRegion>,
    pub wait_for: Option<TutorialEvent>,
    pub next_step: Option<String>,
}
```

---

## 3. Hint System

```rust
pub struct HintSystem {
    pub hints: Vec<Hint>,
    pub cooldown: Duration,
    pub last_hint: Instant,
}

pub struct Hint {
    pub condition: HintCondition,
    pub text: String,
    pub priority: u8,
    pub shown: bool,
}

pub enum HintCondition {
    Stuck { duration: Duration },
    LowHealth,
    LowHunger,
    NewBlockType,
    NightFalling,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  tutorial/
    ├── mod.rs
    ├── manager.rs
    ├── registry.rs
    ├── steps.rs
    └── hints.rs
```


# 48 — Dynamic Events & Quests

## 1. Genel Bakış

Strata'nın dinamik event ve görev sistemi dünyaya canlılık katar. Prosedürel olarak tetiklenen olaylar ve oyuncu odaklı görevler içerir.

### Temel Prensipler

- **Event-driven:** Dünya durumuna göre otomatik event tetikleme
- **Quest chains:** Zincirleme görevler
- **Rewards:** Görev tamamlama ödülleri
- **Dynamic:** Oyuncu seviyesine ve konumuna göre uyarlanma

---

## 2. Event System

```rust
pub struct WorldEventManager {
    pub active_events: Vec<WorldEvent>,
    pub event_pool: Vec<EventType>,
    pub cooldowns: HashMap<EventType, Instant>,
}

pub struct WorldEvent {
    pub id: String,
    pub event_type: EventType,
    pub location: Option<IVec3>,
    pub start_time: u64,
    pub duration: Duration,
    pub participants: Vec<PlayerId>,
    pub state: EventState,
}

pub enum EventType {
    MeteorShower,
    Eclipse,
    TreasureSpawn,
    BossSpawn,
    StructureAppear,
    WeatherAnomaly,
    Custom(String),
}

pub enum EventState {
    Starting,
    Active,
    Completing,
    Ended,
}
```

---

## 3. Quest System

```rust
#[derive(Serialize, Deserialize)]
pub struct Quest {
    pub id: String,
    pub title: String,
    pub description: String,
    pub giver: Option<EntityId>,
    pub objectives: Vec<QuestObjective>,
    pub rewards: Vec<QuestReward>,
    pub status: QuestStatus,
}

pub struct QuestObjective {
    pub id: String,
    pub description: String,
    pub condition: ObjectiveCondition,
    pub progress: u32,
    pub required: u32,
    pub completed: bool,
}

pub enum ObjectiveCondition {
    KillEntity { entity_type: u16 },
    CollectItem { item_id: u16 },
    ReachLocation { location: IVec3, radius: f32 },
    CraftItem { item_id: u16 },
    TalkToNpc { npc_id: u16 },
}

pub enum QuestReward {
    Item { item_id: u16, count: u8 },
    Xp(u32),
    Currency(u32),
    Unlock(String),
}

pub enum QuestStatus {
    Available,
    InProgress,
    Completed,
    Failed,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  events/
    ├── mod.rs
    ├── manager.rs
    ├── types.rs
    └── rewards.rs
  quests/
    ├── mod.rs
    ├── registry.rs
    ├── tracker.rs
    ├── objectives.rs
    └── rewards.rs
```
