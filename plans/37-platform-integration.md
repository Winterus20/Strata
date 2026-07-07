# 51 — Platform Integration

## 1. Genel Bakış

Strata'nın platform entegrasyonu Steam, Epic gibi platformlarla arayüz kurar.

### Temel Prensipler

- **Steamworks:** Steam overlay, achievements, friends
- **Cross-platform:** Windows öncelikli, genişletilebilir
- **Feature flags:** Platform bazlı özellikler
- **Graceful degradation:** Platform yoksa temel özellikler

---

## 2. Steam Integration

```rust
pub struct SteamIntegration {
    pub initialized: bool,
    pub user_id: Option<String>,
    pub overlay_enabled: bool,
}

impl SteamIntegration {
    pub fn init() -> Result<Self>;
    pub fn activate_overlay(&self);
    pub fn set_achievement(&self, achievement_id: &str);
    pub fn get_friends(&self) -> Vec<FriendInfo>;
    pub fn set_rich_presence(&self, key: &str, value: &str);
    pub fn request_stats(&self);
}

pub struct FriendInfo {
    pub id: String,
    pub name: String,
    pub status: FriendStatus,
    pub game_id: Option<String>,
}

pub enum FriendStatus {
    Offline,
    Online,
    Busy,
    Away,
    InGame,
}
```

---

## 3. Platform Abstraction

```rust
pub trait PlatformProvider {
    fn init(&self) -> Result<()>;
    fn user_id(&self) -> Option<String>;
    fn user_name(&self) -> Option<String>;
    fn activate_overlay(&self);
    fn set_achievement(&self, id: &str);
    fn get_friends(&self) -> Vec<FriendInfo>;
    fn set_rich_presence(&self, key: &str, value: &str);
}

pub enum Platform {
    Steam,
    Epic,
    Gog,
    Standalone,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  platform/
    ├── mod.rs
    ├── provider.rs
    ├── steam.rs
    ├── epic.rs
    └── standalone.rs
```
