# 35 — Asset Pipeline

## 1. Genel Bakış

Strata'nın asset pipeline'ı **texture, model, ses, ve diğer asset'lerin** yüklenmesi, cache'lenmesi ve hot-reload'unu yönetir.

### Temel Prensipler

- **Lazy loading:** İhtiyaç duyulunca yükle
- **Cache:** Yüklü asset'leri cache'le
- **Hot-reload:** Dosya değişikliğinde otomatik güncelle
- **Format desteği:** PNG, KTX2, glTF, WAV, OGG

---

## 2. Asset Manager

```rust
pub struct AssetManager {
    /// Yüklü asset'ler.
    pub cache: HashMap<AssetId, Asset>,

    /// Yükleme kuyruğu.
    pub load_queue: Vec<AssetRequest>,

    /// Dosya watcher (hot-reload).
    pub watcher: Option<NotifyWatcher>,
}

pub enum Asset {
    Texture(Texture2D),
    Model(Model),
    Sound(SoundBuffer),
    Font(Font),
    Data(Vec<u8>),
}
```

---

## 3. Texture Loading

```rust
pub struct TextureLoader {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl TextureLoader {
    pub fn load_png(&self, path: &Path) -> Result<Texture2D> {
        // PNG decode → wgpu texture upload
    }

    pub fn load_ktx2(&self, path: &Path) -> Result<Texture2D> {
        // KTX2 (compressed) → wgpu texture
    }
}
```

---

## 4. Hot Reload

```rust
pub struct NotifyWatcher {
    pub watcher: RecommendedWatcher,
    pub callbacks: HashMap<PathBuf, Box<dyn Fn()>>,
}

// Dosya değiştiğinde callback çağrılır
// Asset yeniden yüklenir
// GPU texture güncellenir
```

---

## 5. Crate Organizasyonu

```
crates/
  assets/
    ├── mod.rs
    ├── manager.rs
    ├── loader.rs
    ├── cache.rs
    ├── hot_reload.rs
    └── formats/
        ├── texture.rs
        ├── model.rs
        └── audio.rs
```
