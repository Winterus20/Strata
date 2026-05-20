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
