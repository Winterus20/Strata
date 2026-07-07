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


# 55 — Seasons & Calendar

## 1. Genel Bakış

Strata'nın mevsim ve takvim sistemi dünyaya dinamik değişimler katar.

### Temel Prensipler

- **Seasonal changes:** Her mevsimde farklı görsel ve mekanik değişiklikler
- **Calendar system:** Oyun içi takvim ve zaman takibi
- **Weather integration:** Mevsime bağlı hava durumu
- **Crop growth:** Tarım için mevsim bağımlılığı
- **Event scheduling:** Mevsimsel event'ler

---

## 2. Season System

```rust
pub struct SeasonManager {
    pub current_season: Season,
    pub day_in_season: u32,
    pub days_per_season: u32,
    pub transition_speed: f32,
}

pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub fn weather_weights(&self) -> HashMap<WeatherType, f32>;
    pub fn color_palette(&self) -> SeasonColors;
    pub fn crop_growth_modifier(&self) -> f32;
    pub fn day_length_modifier(&self) -> f32;
}

pub struct SeasonColors {
    pub foliage: Color,
    pub ground: Color,
    pub water: Color,
    pub sky_ambient: Color,
    pub sky_direction: Color,
}
```

---

## 3. Calendar System

```rust
pub struct Calendar {
    pub year: u32,
    pub day_of_year: u32,
    pub days_per_year: u32,
    pub hours_per_day: u32,
    pub minutes_per_hour: u32,
}

impl Calendar {
    pub fn current_season(&self, days_per_season: u32) -> Season;
    pub fn day_progress(&self) -> f32;
    pub fn time_of_day(&self) -> f32;
    pub fn to_string(&self) -> String;
}
```

---

## 4. Seasonal Effects

```rust
pub struct SeasonalEffects {
    pub snow_accumulation: bool,
    pub leaf_fall: bool,
    pub flower_spawn: bool,
    pub ice_formation: bool,
    pub animal_migration: bool,
}

impl SeasonalEffects {
    pub fn apply(&self, world: &mut World, season: Season, dt: f32);
}
```

---

## 5. Crate Organizasyonu

```
crates/
  seasons/
    ├── mod.rs
    ├── manager.rs
    ├── calendar.rs
    ├── effects.rs
    └── colors.rs
```
