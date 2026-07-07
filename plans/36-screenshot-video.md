# 46 — Screenshot & Video Capture

## 1. Genel Bakış

Strata'nın ekran görüntüsü ve video kayıt sistemi oyun içinden medya yakalamayı sağlar.

### Temel Prensipler

- **Screenshot:** PNG/JPEG formatında anlık görüntü
- **Video capture:** WebM/MP4 formatında kayıt
- **GPU-accelerated:** wgpu image readback (queue.write_buffer)
- **HUD toggle:** HUD'siz çekim desteği

---

## 2. Screenshot System

```rust
pub struct ScreenshotManager {
    pub output_dir: PathBuf,
    pub format: ImageFormat,
    pub include_hud: bool,
    pub quality: u8,
}

pub enum ImageFormat {
    Png,
    Jpeg { quality: u8 },
    Bmp,
}

impl ScreenshotManager {
    pub async fn capture(&self, device: &wgpu::Device, texture: &wgpu::Texture) -> Result<PathBuf>;
    pub async fn capture_no_hud(&self, device: &wgpu::Device, texture: &wgpu::Texture) -> Result<PathBuf>;
}
```

---

## 3. Video Capture

```rust
pub struct VideoCapture {
    pub is_recording: bool,
    pub output_path: PathBuf,
    pub fps: u32,
    pub codec: VideoCodec,
    pub frame_buffer: Vec<wgpu::Buffer>,
}

pub enum VideoCodec {
    WebM,
    Mp4,
}

impl VideoCapture {
    pub fn start(&mut self) -> Result<()>;
    pub fn stop(&mut self) -> Result<PathBuf>;
    pub fn capture_frame(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture);
}
```

---

## 4. Replay System (placeholder)

```rust
pub struct ReplaySystem {
    pub recording: bool,
    pub events: Vec<ReplayEvent>,
    pub player_state: Vec<PlayerSnapshot>,
}

pub struct ReplayEvent {
    pub timestamp: f32,
    pub event_type: ReplayEventType,
    pub data: Vec<u8>,
}
```

---

## 5. Crate Organizasyonu

```
crates/
  media/
    ├── mod.rs
    ├── screenshot.rs
    ├── video.rs
    └── replay.rs
```
