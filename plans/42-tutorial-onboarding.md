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
