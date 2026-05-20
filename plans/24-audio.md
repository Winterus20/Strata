# 17 — Audio Sistemi

## 1. Genel Bakış

Strata'nın audio sistemi **spatial 3D ses** destekler. Bloklar, entity'ler ve ortam sesleri konuma göre işlenir.

### Temel Prensipler

- **3D Spatial:** Mesafe ve yöne göre ses azalır
- **Block-based:** Her blok tipinin kendine özgü sesleri var
- **Ambient:** Ortam sesleri (rüzgar, yağmur, gece sesleri)
- **Occlusion:** Bloklar arkasındaki sesler azaltılır

---

## 2. Audio Engine

```rust
/// Audio engine — cpal + rodio tabanlı.
pub struct AudioEngine {
    /// Output stream.
    stream: Stream,

    /// Mixer — tüm ses kaynaklarını karıştırır.
    mixer: Arc<Mixer>,

    /// Spatial listener (kamera pozisyonu).
    listener: SpatialListener,

    /// Ses kaynakları.
    sources: HashMap<SoundId, AudioSource>,

    /// Ambient ses controller'ı.
    ambient: AmbientController,
}

impl AudioEngine {
    /// Yeni audio engine oluştur.
    pub fn new() -> Result<Self> {
        let (stream, stream_handle) = rodio::OutputStream::try_default()?;
        let sink = rodio::Sink::try_new(&stream_handle)?;

        let mixer = Arc::new(Mixer::new());

        Ok(Self {
            stream,
            mixer,
            listener: SpatialListener::default(),
            sources: HashMap::new(),
            ambient: AmbientController::new(),
        })
    }

    /// Listener pozisyonunu güncelle (kamera).
    pub fn update_listener(&mut self, position: Vec3, orientation: Quat) {
        self.listener.position = position;
        self.listener.orientation = orientation;
    }

    /// Bir ses çal (3D spatial).
    pub fn play_spatial(&mut self, sound_id: SoundId, position: Vec3, volume: f32) {
        if let Some(source) = self.sources.get(&sound_id) {
            let distance = (position - self.listener.position).length();

            // Mesafe bazlı volume azalma
            let attenuation = self.compute_attenuation(distance);
            let final_volume = volume * attenuation;

            // Stereo pan (yön bazlı)
            let pan = self.compute_pan(position);

            // Occlusion check
            let occlusion = self.compute_occlusion(position);
            let occluded_volume = final_volume * occlusion;

            // Ses çal
            if occluded_volume > 0.01 {
                let spatial_source = source.clone()
                    .spatialized(position, self.listener.position)
                    .volume(occluded_volume)
                    .pan(pan);

                self.mixer.append(spatial_source);
            }
        }
    }

    /// Mesafe bazlı attenuation.
    fn compute_attenuation(&self, distance: f32) -> f32 {
        // Inverse square law
        let reference_distance = 1.0;
        let max_distance = 64.0;

        if distance > max_distance {
            return 0.0;
        }

        1.0 / (1.0 + (distance / reference_distance).powi(2))
    }

    /// Stereo pan hesapla.
    fn compute_pan(&self, position: Vec3) -> f32 {
        let relative = position - self.listener.position;
        let right = self.listener.orientation.mul_vec3(Vec3::X);

        // -1.0 (sol) ile 1.0 (sağ) arası
        relative.dot(right).clamp(-1.0, 1.0)
    }

    /// Occlusion hesapla (bloklar arkasındaki sesler).
    fn compute_occlusion(&self, position: Vec3) -> f32 {
        // Ray cast ile engel kontrolü
        let dir = (position - self.listener.position).normalize();
        let distance = (position - self.listener.position).length();

        // Basit occlusion: her blok %20 azaltır
        let mut occlusion = 1.0;
        let mut t = 0.0;

        while t < distance {
            let check_pos = self.listener.position + dir * t;
            // XBrickMap'ten blok kontrolü
            if self.is_block_at(check_pos) {
                occlusion *= 0.8; // %20 azalma per blok
            }
            t += 1.0;
        }

        occlusion
    }
}
```

---

## 3. Sound Registry

```rust
/// Ses kayıt defteri.
pub struct SoundRegistry {
    /// Ses tanımları.
    sounds: HashMap<String, SoundDefinition>,

    /// Ses buffer'ları (yüklü).
    buffers: HashMap<String, Arc<rodio::Source>>,
}

pub struct SoundDefinition {
    /// Ses ismi.
    pub name: String,

    /// Ses dosya yolu.
    pub file_path: PathBuf,

    /// Ses kategorisi.
    pub category: SoundCategory,

    /// Maksimum mesafe.
    pub max_distance: f32,

    /// Rastgele varyantlar (aynı sesin farklı versiyonları).
    pub variants: Vec<PathBuf>,

    /// Loop flag.
    pub looped: bool,

    /// Pitch varyasyonu.
    pub pitch_variation: f32, // 0.0 = yok, 0.1 = ±10%
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SoundCategory {
    Master,
    Music,
    Ambient,
    Block,
    Entity,
    Player,
    Weather,
    Ui,
}
```

---

## 4. Block Sesleri

```rust
/// Blok ses tanımları.
pub struct BlockSounds {
    /// Yerleştirme sesi.
    pub place: SoundId,

    /// Kırma sesi.
    pub break_sound: SoundId,

    /// Üzerinde yürüme sesi.
    pub step: SoundId,

    /// Düşme sesi.
    pub fall: SoundId,

    /// Hasar alma sesi.
    pub hit: SoundId,
}

impl BlockSounds {
    /// Blok tanımından sesleri yükle.
    pub fn from_definition(def: &BlockDefinition) -> Self {
        Self {
            place: SoundId::new(&def.gameplay.place_sound),
            break_sound: SoundId::new(&def.gameplay.break_sound),
            step: SoundId::new(&def.gameplay.step_sound),
            fall: SoundId::new("generic_fall"),
            hit: SoundId::new("generic_hit"),
        }
    }
}

/// Ses çalma event handler'ı.
pub fn on_block_placed(
    audio: Res<AudioEngine>,
    event: ReadEvents<StrataEvent>,
    sounds: Res<BlockSoundRegistry>,
) {
    for event in event.read() {
        if let StrataEvent::BlockPlaced { pos, block_id } = event {
            if let Some(sound) = sounds.get(block_id) {
                audio.play_spatial(
                    sound.place,
                    pos.as_vec3(),
                    0.8,
                );
            }
        }
    }
}
```

---

## 5. Ambient Ses Sistemi

```rust
/// Ambient ses controller'ı.
pub struct AmbientController {
    /// Aktif ambient sesleri.
    active_sounds: HashMap<AmbientType, AmbientSound>,

    /// Ambient durumuna göre ses seçimi.
    rules: Vec<AmbientRule>,
}

pub struct AmbientSound {
    pub sound_id: SoundId,
    pub volume: f32,
    pub target_volume: f32,
    pub fade_speed: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmbientType {
    Wind,
    Rain,
    Thunder,
    Night,
    Cave,
    Underground,
    Forest,
    Ocean,
    Desert,
}

pub struct AmbientRule {
    /// Hangi koşulda aktif.
    pub condition: AmbientCondition,

    /// Hangi ses çalınır.
    pub sound: SoundId,

    /// Baz volume.
    pub base_volume: f32,
}

pub enum AmbientCondition {
    /// Hava durumu.
    Weather(WeatherType),

    /// Gece/gündüz.
    TimeOfDay { start: f32, end: f32 },

    /// Kapalı alan (mağara).
    Enclosed,

    /// Yeraltı.
    Underground,

    /// Biome.
    Biome(BiomeId),
}

impl AmbientController {
    /// Ambient sesleri güncelle (her frame).
    pub fn update(
        &mut self,
        weather: &WeatherState,
        time_of_day: f32,
        player_pos: Vec3,
        world: &World,
    ) {
        // Hava durumu ambient
        match weather.current {
            WeatherType::Rain => {
                self.set_target(AmbientType::Rain, 0.5);
            }
            WeatherType::Thunderstorm => {
                self.set_target(AmbientType::Rain, 0.6);
                self.set_target(AmbientType::Thunder, 0.3);
            }
            WeatherType::Clear => {
                self.set_target(AmbientType::Rain, 0.0);
                self.set_target(AmbientType::Thunder, 0.0);
            }
            _ => {}
        }

        // Gece ambient
        if time_of_day > 20.0 || time_of_day < 6.0 {
            self.set_target(AmbientType::Night, 0.3);
        } else {
            self.set_target(AmbientType::Night, 0.0);
        }

        // Kapalı alan kontrolü
        if self.is_enclosed(player_pos, world) {
            self.set_target(AmbientType::Cave, 0.4);
        } else {
            self.set_target(AmbientType::Cave, 0.0);
        }

        // Volume fade
        for sound in self.active_sounds.values_mut() {
            let diff = sound.target_volume - sound.volume;
            if diff.abs() > 0.01 {
                sound.volume += diff * sound.fade_speed;
            } else {
                sound.volume = sound.target_volume;
            }
        }
    }

    fn set_target(&mut self, ambient_type: AmbientType, volume: f32) {
        if let Some(sound) = self.active_sounds.get_mut(&ambient_type) {
            sound.target_volume = volume;
        }
    }

    fn is_enclosed(&self, pos: Vec3, world: &World) -> bool {
        // Oyuncunun etrafındaki blokları kontrol et
        // 6 yönde de blok varsa = enclosed
        let directions = [
            IVec3::new(1, 0, 0),
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 1, 0),
            IVec3::new(0, -1, 0),
            IVec3::new(0, 0, 1),
            IVec3::new(0, 0, -1),
        ];

        directions.iter().all(|dir| {
            world.is_solid((pos.as_ivec3() + dir * 2).as_vec3())
        })
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  audio/
    ├── mod.rs              ← Audio plugin entry point
    ├── engine.rs           ← AudioEngine
    ├── listener.rs         ← SpatialListener
    ├── mixer.rs            ← Ses mixer
    ├── registry/
    │   ├── mod.rs          ← SoundRegistry
    │   ├── definition.rs   ← SoundDefinition
    │   └── loader.rs       ← Ses dosyası yükleme
    ├── spatial/
    │   ├── mod.rs          ← Spatial audio
    │   ├── attenuation.rs  ← Mesafe azalma
    │   ├── panning.rs      ← Stereo pan
    │   └── occlusion.rs    ← Blok occlusion
    ├── block_sounds.rs     ← Blok sesleri
    ├── ambient/
    │   ├── mod.rs          ← AmbientController
    │   ├── rules.rs        ← Ambient kuralları
    │   └── types.rs        ← AmbientType enum
    └── events.rs           ← Audio event handler'ları
```
