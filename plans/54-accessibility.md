# 54 — Accessibility

## 1. Genel Bakış

Strata'nın erişilebilirlik sistemi farklı ihtiyaçlara sahip oyuncular için oyun deneyimini uyarlar.

### Temel Prensipler

- **Colorblind modes:** Renk körlüğü desteği
- **Subtitles:** Altyazı ve görsel bildirimler
- **Screen reader:** Ekran okuyucu desteği
- **Customizable UI:** Ölçeklenebilir arayüz
- **Motor accessibility:** Tek elle oynanabilirlik

---

## 2. Colorblind Support

```rust
pub enum ColorblindMode {
    None,
    Protanopia,
    Deuteranopia,
    Tritanopia,
    Monochromacy,
}

pub struct ColorblindFilter {
    pub mode: ColorblindMode,
    pub intensity: f32,
}

impl ColorblindFilter {
    pub fn apply_to_color(&self, color: Color) -> Color;
    pub fn apply_to_texture(&self, texture: &wgpu::Texture);
}
```

---

## 3. Subtitle & Visual Notifications

```rust
pub struct SubtitleSystem {
    pub enabled: bool,
    pub font_size: f32,
    pub background: bool,
    pub speaker_name: bool,
    pub sound_indicators: bool,
}

pub struct Subtitle {
    pub text: String,
    pub speaker: Option<String>,
    pub duration: Duration,
    pub priority: u8,
}

pub struct VisualNotification {
    pub icon: String,
    pub text: String,
    pub color: Color,
    pub position: NotificationPosition,
    pub duration: Duration,
}

pub enum NotificationPosition {
    TopCenter,
    BottomCenter,
    TopLeft,
    TopRight,
}
```

---

## 4. UI Scaling & Customization

```rust
pub struct AccessibilitySettings {
    pub ui_scale: f32,
    pub font_scale: f32,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    pub screen_shake: f32,
    pub flash_effects: bool,
    pub one_handed_mode: bool,
}
```

---

## 5. Crate Organizasyonu

```
crates/
  accessibility/
    ├── mod.rs
    ├── colorblind.rs
    ├── subtitles.rs
    ├── notifications.rs
    └── settings.rs
```
