# 30 — Day/Night Cycle & Weather

## 1. Genel Bakış

Strata'nın gün/gece döngüsü ve hava durumu sistemi **dinamik aydınlatma**, **gökyüzü rendering** ve **biome-specific weather** destekler.

### Temel Prensipler

- **Dinamik zaman:** Gerçek zamanlı gün/gece geçişi
- **Gökyüzü:** Gradient skybox, güneş/ay, yıldızlar
- **Hava durumu:** Yağmur, kar, fırtına, sis
- **Biome-specific:** Her biome farklı hava durumu

---

## 2. Time of Day

```rust
#[derive(Component)]
pub struct DayNightCycle {
    /// Gün süresi (gerçek dakika).
    pub day_length_minutes: f32,

    /// Mevcut zaman (0-24).
    pub current_time: f32,

    /// Güneş pozisyonu.
    pub sun_position: Vec3,

    /// Ay pozisyonu.
    pub moon_position: Vec3,
}

impl DayNightCycle {
    /// Zamanı güncelle.
    pub fn update(&mut self, delta: f32) {
        self.current_time += (delta / self.day_length_minutes) * 24.0;
        if self.current_time >= 24.0 {
            self.current_time -= 24.0;
        }
        self.update_sun_moon_positions();
    }
}
```

---

## 3. Weather System

```rust
#[derive(Component)]
pub struct WeatherState {
    /// Mevcut hava durumu.
    pub current: WeatherType,

    /// Hedef hava durumu (transition için).
    pub target: Option<WeatherType>,

    /// Transition progress (0-1).
    pub transition_progress: f32,

    /// Yağmur yoğunluğu.
    pub rain_intensity: f32,

    /// Kar yoğunluğu.
    pub snow_intensity: f32,

    /// Sis yoğunluğu.
    pub fog_density: f32,
}

#[derive(Clone, Copy)]
pub enum WeatherType {
    Clear,
    Cloudy,
    Rain,
    Thunderstorm,
    Snow,
    Blizzard,
    Fog,
}
```

---

## 4. Sky Rendering

```rust
// Gökyüzü shader'ı gün/gece geçişini handle eder
// Güneş/ay pozisyonuna göre gradient değişir
// Yıldızlar gece görünür
```

---

## 5. Crate Organizasyonu

```
crates/
  daynight/
    ├── mod.rs
    ├── cycle.rs
    ├── weather.rs
    ├── sky.rs
    └── lighting_integration.rs
```
