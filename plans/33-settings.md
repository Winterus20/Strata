# 36 — Settings & Configuration

## 1. Genel Bakış

Strata'nın ayarlar sistemi **grafik, ses, kontrol, ve oyun** ayarlarını yönetir. Runtime'da değiştirilebilir.

### Temel Prensipler

- **Runtime config:** Oyun sırasında değiştirilebilir
- **Preset'ler:** Low, Medium, High, Ultra grafik preset'leri
- **Keybinding:** Özelleştirilebilir tuş atamaları
- **Persist:** Ayarlar dosyaya kaydedilir

---

## 2. Graphics Settings

```rust
#[derive(Serialize, Deserialize)]
pub struct GraphicsSettings {
    pub resolution: (u32, u32),
    pub fullscreen: bool,
    pub vsync: bool,
    pub fps_limit: Option<u32>,
    pub render_distance: u32,
    pub graphics_preset: GraphicsPreset,
    pub shadows: bool,
    pub ambient_occlusion: bool,
    pub anti_aliasing: AntiAliasingMode,
    pub texture_quality: TextureQuality,
    pub cloud_quality: CloudQuality,
    pub particle_limit: u32,
}

#[derive(Clone, Copy)]
pub enum GraphicsPreset {
    Low,
    Medium,
    High,
    Ultra,
}
```

---

## 3. Audio Settings

```rust
#[derive(Serialize, Deserialize)]
pub struct AudioSettings {
    pub master_volume: f32,
    pub music_volume: f32,
    pub ambient_volume: f32,
    pub block_volume: f32,
    pub entity_volume: f32,
    pub weather_volume: f32,
}
```

---

## 4. Control Settings

```rust
#[derive(Serialize, Deserialize)]
pub struct ControlSettings {
    pub key_bindings: HashMap<InputAction, KeyCode>,
    pub mouse_bindings: HashMap<InputAction, MouseButton>,
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    pub crouch_toggle: bool,
}
```

---

## 5. Config File

```rust
pub struct GameConfig {
    pub graphics: GraphicsSettings,
    pub audio: AudioSettings,
    pub controls: ControlSettings,
    pub game: GameSettings,
}

impl GameConfig {
    pub fn load() -> Result<Self> {
        // config.toml veya config.json yükle
    }

    pub fn save(&self) -> Result<()> {
        // Dosyaya kaydet
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  config/
    ├── mod.rs
    ├── graphics.rs
    ├── audio.rs
    ├── controls.rs
    ├── game.rs
    └── persistence.rs
```
