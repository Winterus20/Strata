# 28 — Client Binary & Başlatma

## 1. Genel Bakış

Strata'nın game client'ı **winit + wgpu** ile oluşturulur. Tüm subsystem'ler plugin olarak yüklenir ve ECS üzerinden çalışır.

### Temel Prensipler

- **Plugin-first:** Tüm subsystem'ler plugin olarak yüklenir
- **ECS-driven:** Oyun mantığı tamamen ECS
- **Async init:** Ağır işlemler (world load, shader compile) async
- **Graceful shutdown:** Tüm kaynaklar düzgün temizlenir

---

## 2. Client Binary

```rust
// bin/client/main.rs

use strata_client::{Client, ClientConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "strata=info,wgpu=warn".into()),
        )
        .init();

    // Konfigürasyon yükle
    let config = ClientConfig::load("client.toml")?;

    // Client oluştur ve çalıştır
    let mut client = Client::new(config)?;
    client.run()
}
```

---

## 3. Client Config

```toml
# client.toml

[window]
title = "Strata"
width = 1920
height = 1080
fullscreen = false
vsync = true
max_fps = 0  # 0 = unlimited

[render]
render_distance = 12
quality = "high"  # low, medium, high, ultra
fovs = 70.0
shadows = true
ambient_occlusion = true
foveated_rendering = false

[network]
server_address = "127.0.0.1"
server_port = 25565
username = "Player"

[controls]
sensitivity = 1.0
invert_y = false

[audio]
master_volume = 1.0
music_volume = 0.5
ambient_volume = 0.7
block_volume = 1.0
```

```rust
/// Client konfigürasyonu.
#[derive(Deserialize)]
pub struct ClientConfig {
    pub window: WindowConfig,
    pub render: RenderConfig,
    pub network: NetworkConfig,
    pub controls: ControlsConfig,
    pub audio: AudioConfig,
}

#[derive(Deserialize)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    pub max_fps: u32,
}

#[derive(Deserialize)]
pub struct RenderConfig {
    pub render_distance: u32,
    pub quality: String,
    pub fov: f32,
    pub shadows: bool,
    pub ambient_occlusion: bool,
    pub foveated_rendering: bool,
}

#[derive(Deserialize)]
pub struct NetworkConfig {
    pub server_address: String,
    pub server_port: u16,
    pub username: String,
}

#[derive(Deserialize)]
pub struct ControlsConfig {
    pub sensitivity: f32,
    pub invert_y: bool,
}

#[derive(Deserialize)]
pub struct AudioConfig {
    pub master_volume: f32,
    pub music_volume: f32,
    pub ambient_volume: f32,
    pub block_volume: f32,
}
```

---

## 4. Client Runtime

```rust
/// Game client.
pub struct Client {
    /// winit event loop.
    event_loop: EventLoop<()>,

    /// winit window.
    window: Window,

    /// wgpu device.
    device: wgpu::Device,
    queue: wgpu::Queue,

    /// Bevy ECS world.
    world: World,

    /// Bevy ECS stage runner.
    schedule: Schedule,

    /// Konfigürasyon.
    config: ClientConfig,

    /// Çalışıyor mu?
    running: bool,

    /// FPS counter.
    fps_counter: FpsCounter,
}

impl Client {
    /// Yeni client oluştur.
    pub fn new(config: ClientConfig) -> Result<Self> {
        // winit window oluştur
        let event_loop = EventLoop::new()?;
        let window = WindowBuilder::new()
            .with_title(&config.window.title)
            .with_inner_size(PhysicalSize::new(
                config.window.width,
                config.window.height,
            ))
            .with_resizable(true)
            .build(&event_loop)?;

        // wgpu device oluştur (async)
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = unsafe { instance.create_surface(&window) }?;

        let adapter = pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            },
        ))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("strata_device"),
                required_features: Self::required_features(&config.render),
                required_limits: adapter.limits(),
            },
            None,
        ))?;

        // ECS world ve schedule oluştur
        let mut world = World::new();
        let mut schedule = Schedule::default();

        // Plugin'leri yükle
        let mut app = App::new();
        app.add_plugins(ClientPlugins {
            config: config.clone(),
            window: window.clone(),
            device: device.clone(),
            queue: queue.clone(),
            surface,
        });

        Ok(Self {
            event_loop,
            window,
            device,
            queue,
            world,
            schedule,
            config,
            running: false,
            fps_counter: FpsCounter::new(),
        })
    }

    /// Gerekli wgpu feature'ları.
    fn required_features(render: &RenderConfig) -> wgpu::Features {
        let mut features = wgpu::Features::empty();

        // Shader int64 atomics (visibility buffer için)
        features |= wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS;

        // Timestamp query (profiling)
        features |= wgpu::Features::TIMESTAMP_QUERY;

        // Push constants (performans)
        features |= wgpu::Features::PUSH_CONSTANTS;

        features
    }

    /// Client'ı çalıştır (main loop).
    pub fn run(&mut self) -> Result<()> {
        self.running = true;

        let mut last_frame = Instant::now();

        // Main loop
        while self.running {
            // winit event'leri işle
            self.process_events()?;

            // Frame süresi
            let frame_dt = last_frame.elapsed();
            last_frame = Instant::now();

            // FPS hesapla
            self.fps_counter.update(frame_dt);

            // ECS tick
            self.world.run_systems();

            // Render
            self.render()?;

            // FPS limit
            if self.config.window.max_fps > 0 {
                let target_frame_time = Duration::from_secs_f32(
                    1.0 / self.config.window.max_fps as f32,
                );
                let elapsed = last_frame.elapsed();
                if elapsed < target_frame_time {
                    std::thread::sleep(target_frame_time - elapsed);
                }
            }
        }

        Ok(())
    }

    /// winit event'leri işle.
    fn process_events(&mut self) -> Result<()> {
        // winit event loop polling
        // ...
        Ok(())
    }

    /// Render pass.
    fn render(&mut self) -> Result<()> {
        // Swapchain'den texture al
        let frame = self.surface.get_current_texture()?;
        let view = frame.texture.create_view(&Default::default());

        // Render pass başlat
        let mut encoder = self.device.create_command_encoder(&Default::default());

        // ... render pass'leri ...

        // Submit
        self.queue.submit(Some(encoder.finish()));
        frame.present();

        Ok(())
    }
}
```

---

## 5. Client Plugins

```rust
/// Client plugin'ları.
pub struct ClientPlugins {
    pub config: ClientConfig,
    pub window: Window,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
}

impl PluginsState for ClientPlugins {
    fn build(&self, app: &mut App) {
        app
            // Çekirdek
            .add_plugins(BlockRegistryPlugin)
            .add_plugins(EcsPlugin)

            // World
            .add_plugins(WorldGenPlugin)
            .add_plugins(StreamingPlugin)
            .add_plugins(StoragePlugin)

            // Render
            .add_plugins(RenderPlugin::new(
                self.device.clone(),
                self.queue.clone(),
                self.surface.clone(),
                &self.config.render,
            ))

            // Gameplay
            .add_plugins(PhysicsPlugin)
            .add_plugins(LightingPlugin)
            .add_plugins(PlayerPlugin)
            .add_plugins(EntityPlugin)
            .add_plugins(AiPlugin)

            // UI
            .add_plugins(UiPlugin::new(
                self.device.clone(),
                self.queue.clone(),
                &self.config.window,
            ))

            // Audio
            .add_plugins(AudioPlugin)

            // Particles
            .add_plugins(ParticlePlugin)

            // Network (client mode)
            .add_plugins(NetworkPlugin::client(&self.config.network))

            // Debug
            .add_plugins(DebugPlugin)

            // Kaynaklar
            .insert_resource(self.config.clone())
            .insert_resource(self.window.clone())
            .insert_resource(self.device.clone())
            .insert_resource(self.queue.clone());
    }
}
```

---

## 6. Init Sırası

```
Client Başlatma Sırası:
  ┌─────────────────────────────────────────┐
  │ 1. winit window oluştur                 │
  │    (pencere, event loop)               │
  ├─────────────────────────────────────────┤
  │ 2. wgpu device oluştur                  │
  │    (adapter, device, queue, surface)   │
  ├─────────────────────────────────────────┤
  │ 3. Shader'ları compile et               │
  │    (WGSL → pipeline, async)            │
  ├─────────────────────────────────────────┤
  │ 4. Plugin'leri yükle                    │
  │    (topolojik sıralama ile)            │
  ├─────────────────────────────────────────┤
  │ 5. World yükle/oluştur                  │
  │    (seed'den world gen veya disk'ten)  │
  ├─────────────────────────────────────────┤
  │ 6. Server'a bağlan                      │
  │    (handshake, auth, world sync)       │
  ├─────────────────────────────────────────┤
  │ 7. Main loop başlat                     │
  │    (input → ECS → render)              │
  └─────────────────────────────────────────┘
```

---

## 7. Workspace Yapısı

```
Strata/
├── Cargo.toml              ← Workspace root
├── AGENTS.md               ← Agent talimatları
├── client.toml             ← Client konfigürasyonu
├── server.toml             ← Server konfigürasyonu
├── plans/                  ← Plan dokümanları
│   ├── 01-overview.md
│   ├── 02-xbrickmap.md
│   ├── ...
│   └── 28-client-binary.md
├── crates/
│   ├── core/               ← Block registry, sector, xbrickmap
│   ├── ecs/                ← Bevy ECS components & systems
│   ├── world-gen/          ← Prosedürel terrain
│   ├── meshing/            ← Mesher trait + greedy + GPU
│   ├── render/             ← wgpu pipeline, culling
│   ├── network/            ← renet2 + replicon
│   ├── storage/            ← Region files, SQLite, cache
│   ├── modding/            ← wasmtime + WIT
│   ├── physics/            ← bevy_rapier
│   ├── lighting/           ← BFS, sky, GI
│   ├── plugin-api/         ← Plugin trait, registry
│   ├── player/             ← Player controller, inventory
│   ├── audio/              ← 3D spatial audio
│   ├── ui/                 ← glyphon HUD
│   ├── particles/          ← GPU compute particles
│   ├── ai/                 ← Behavior tree, A*
│   ├── security/           ← Input validation, anti-cheat
│   ├── debug/              ← HUD, profiling, benchmarks
│   └── server/             ← Headless server runtime
└── bin/
    ├── client/             ← Game client (winit + wgpu)
    └── server/             ← Headless server (tokio)
```

---

## 8. Cargo.toml (Workspace)

```toml
[workspace]
members = [
    "crates/core",
    "crates/ecs",
    "crates/world-gen",
    "crates/meshing",
    "crates/render",
    "crates/network",
    "crates/storage",
    "crates/modding",
    "crates/physics",
    "crates/lighting",
    "crates/plugin-api",
    "crates/player",
    "crates/audio",
    "crates/ui",
    "crates/particles",
    "crates/ai",
    "crates/security",
    "crates/debug",
    "crates/server",
    "bin/client",
    "bin/server",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
# ECS
bevy_ecs = "0.18"
bevy_app = "0.18"
bevy_hierarchy = "0.18"

# Render
wgpu = "29"
winit = "0.30"
glyphon = "0.12"
glam = "0.29"

# Async
tokio = { version = "1", features = ["full"] }

# Network
renet2 = "0.13"
bevy_replicon = "0.39"
bevy_replicon_renet2 = "0.14"

# Serialization
rkyv = "0.8"
postcard = "1.1"

# Noise
fastnoise2 = "0.4"

# Physics
bevy_rapier3d = { version = "0.33", features = ["enhanced-determinism"] }

# Modding
wasmtime = "30"

# Storage
rusqlite = { version = "0.32", features = ["bundled"] }

# Compression
zstd = "0.13"

# Hash
blake3 = "1.5"
xxhash-rust = { version = "0.8", features = ["xxh64"] }

# Utils
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
slotmap = "1.0"
wide = "0.7"
ahash = "0.8"
```

---

## 9. Build Komutları

```bash
# Tüm workspace'i build et
cargo build --workspace

# Release build
cargo build --workspace --release

# Sadece client
cargo build -p strata-client

# Sadece server
cargo build -p strata-server

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt

# Test
cargo test --workspace

# Benchmark
cargo bench

# Client çalıştır
cargo run -p strata-client

# Server çalıştır
cargo run -p strata-server
```
