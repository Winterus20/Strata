# 45 — Controller & Gamepad

## 1. Genel Bakış

Strata'nın gamepad desteği Xbox, PlayStation ve generic gamepad'leri kapsar.

### Temel Prensipler

- **Multi-vendor:** XInput, DirectInput, SDL gamepad
- **Configurable:** Özelleştirilebilir tuş atamaları
- **Haptic feedback:** Titreşim desteği
- **Deadzone:** Analog stick ölü bölge ayarı

---

## 2. Gamepad Input

```rust
pub struct GamepadManager {
    pub connected: HashMap<GamepadId, GamepadState>,
    pub config: GamepadConfig,
}

pub struct GamepadConfig {
    pub deadzone_left: f32,
    pub deadzone_right: f32,
    pub vibration_enabled: bool,
    pub aim_assist: bool,
    pub button_map: HashMap<GamepadAction, GamepadButton>,
    pub axis_map: HashMap<GamepadAxis, InputAction>,
}

pub struct GamepadState {
    pub id: GamepadId,
    pub name: String,
    pub buttons: u64,
    pub left_stick: Vec2,
    pub right_stick: Vec2,
    pub left_trigger: f32,
    pub right_trigger: f32,
}
```

---

## 3. Vibration / Haptic

```rust
pub struct VibrationController {
    pub left_motor: f32,
    pub right_motor: f32,
    pub duration: Duration,
}

impl VibrationController {
    pub fn rumble(&mut self, gamepad: GamepadId, intensity: f32, duration: Duration);
    pub fn trigger_pulse(&mut self, gamepad: GamepadId, trigger: TriggerSide, frequency: f32);
}
```

---

## 4. Crate Organizasyonu

```
crates/
  input/
    └── gamepad/
        ├── mod.rs
        ├── manager.rs
        ├── config.rs
        └── vibration.rs
```
