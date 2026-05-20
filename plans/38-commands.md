# 34 — Command & Console System

## 1. Genel Bakış

Strata'nın komut sistemi **server/client console**, **debug komutları** ve **admin yetkilendirme** destekler.

### Temel Prensipler

- **Command registry:** Komutlar runtime'da kaydedilir
- **Permission-based:** Yetki seviyeleri (player, mod, admin, console)
- **Tab completion:** Otomatik tamamlama
- **Server & Client:** Hem server hem client komutları

---

## 2. Command Registry

```rust
pub struct CommandRegistry {
    pub commands: HashMap<String, Command>,
}

pub struct Command {
    pub name: String,
    pub description: String,
    pub permission: PermissionLevel,
    pub handler: Box<dyn Fn(&[&str]) -> CommandResult>,
    pub tab_completer: Option<Box<dyn Fn(&str) -> Vec<String>>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    Player = 0,
    Moderator = 1,
    Admin = 2,
    Console = 3,
}
```

---

## 3. Console

```rust
#[derive(Component)]
pub struct Console {
    /// Komut geçmişi.
    pub history: Vec<String>,

    /// Aktif input.
    pub input: String,

    /// Log mesajları.
    pub messages: Vec<ConsoleMessage>,

    /// Görünürlük.
    pub visible: bool,
}

pub struct ConsoleMessage {
    pub text: String,
    pub level: LogLevel,
    pub timestamp: f64,
}
```

---

## 4. Built-in Komutlar

```
/gamemode <mode>          — Oyun modu değiştir
/give <player> <item> <n> — Item ver
/tp <x> <y> <z>           — Işınlan
/time set <value>         — Zaman ayarla
/weather <type>           — Hava durumu
/seed                     — Dünya seed'i göster
/list                     — Oyuncu listesi
/kick <player>            — Oyuncuyu at
/ban <player>             — Oyuncuyu banla
```

---

## 5. Crate Organizasyonu

```
crates/
  commands/
    ├── mod.rs
    ├── registry.rs
    ├── console.rs
    ├── builtins/
    └── permissions.rs
```
