# 39 — Localization (i18n)

## 1. Genel Bakış

Strata'nın localization sistemi **çoklu dil desteği** sağlar. UI metinleri, item isimleri, ve sistem mesajları çevrilebilir.

### Temel Prensipler

- **Key-based:** Her metin bir key ile tanımlanır
- **Runtime switching:** Dil değiştirilebilir
- **Fallback:** Eksik çeviriler için varsayılan dil (İngilizce)
- **Mod desteği:** Mod'lar kendi çevirilerini ekleyebilir

---

## 2. Localization System

```rust
pub struct Localization {
    /// Aktif dil.
    pub current_locale: String,

    /// Çeviri dosyaları.
    pub translations: HashMap<String, TranslationMap>,

    /// Fallback dil.
    pub fallback_locale: String,
}

pub type TranslationMap = HashMap<String, String>;

impl Localization {
    pub fn get(&self, key: &str) -> &str {
        // Önce aktif dilde ara
        // Bulunamazsa fallback'te ara
        // O da yoksa key'i döndür
    }

    pub fn set_locale(&mut self, locale: &str) {
        self.current_locale = locale.to_string();
    }
}
```

---

## 3. Translation Files

```json
// locales/en.json
{
  "ui.menu.play": "Play",
  "ui.menu.settings": "Settings",
  "ui.menu.quit": "Quit",
  "block.stone": "Stone",
  "block.dirt": "Dirt",
  "item.wooden_pickaxe": "Wooden Pickaxe",
  "chat.player.joined": "{player} joined the game",
  "death.attack.zombie": "{player} was killed by a Zombie"
}
```

---

## 4. Desteklenen Diller

```
en — English
tr — Türkçe
de — Deutsch
fr — Français
es — Español
ru — Русский
zh — 中文
ja — 日本語
```

---

## 5. Crate Organizasyonu

```
crates/
  localization/
    ├── mod.rs
    ├── locale.rs
    ├── manager.rs
    └── locales/
        ├── en.json
        ├── tr.json
        └── ...
```
