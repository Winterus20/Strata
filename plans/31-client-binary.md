# 31 — Client Binary & Başlatma

> **Revizyon notu (2026-07-07):** Bu plan, kapsamlı teknik denetim (web araştırmasıyla doğrulanmış) sonucunda güncellenmiştir. Düzeltmeler: derleme-kıran bağımlılık sürümleri, renderer sahipliği çelişkisi, async init/graceful shutdown gerçeklenmesi, config tip güvenliği, Bevy sürüm sabitleme. `01–16` anayasayla çelişki yoktur; `17–38` taslak aralığındadır.

## 1. Genel Bakış

Strata'nın game client'ı **Bevy 0.18** üzerine kuruludur; Bevy varsayılan olarak **wgpu + winit** kullanır. Tüm subsystem'ler Bevy plugin olarak yüklenir ve Bevy ECS üzerinden çalışır.

### Temel Prensipler

- **Plugin-first:** Tüm subsystem'ler plugin olarak yüklenir.
- **ECS-driven:** Oyun mantığı tamamen ECS.
- **Async init:** Ağır işlemler (`world load`, `shader compile`, `network connect`) `Startup` schedule + `AsyncComputeTaskPool` ile **bloklamadan** yapılır; `Client::new` yalnızca senkron kayıt yapar.
- **Graceful shutdown:** `AppExit` dinleyici sistemiyle kaynaklar sıralı temizlenir.
- **Tek wgpu cihazı:** Custom renderer, Bevy'nin `RenderDevice`/`RenderQueue`'unu kullanır; **ikinci bir wgpu cihazı/surface oluşturulmaz** (renderer sahipliği çelişkisini önler).

### Renderer Sahipliği (Kritik)

`DefaultPlugins` zaten Bevy'nin wgpu-tabanlı renderer'ını (ve winit penceresini) içerir. Strata'nın 9-pass custom pipeline'ı **Bevy render sub-app'i içinde** render-graph `Node`'ları olarak yazılır ve Bevy'nin `RenderDevice`'ı üzerinden GPU'ya erişir. Bu sayede:
- Tek wgpu cihazı/surface (çift swapchain çakışması yok).
- `bevy_ui` HUD'ı composite pass üstüne çizilmeye devam eder.
- wgpu sürümü Bevy'in çözdüğü sürümle **zorunlu olarak birleşik** olur (bkz. §8, §10).

---

## 2. Client Binary

```rust
// bin/client/main.rs

use strata_client::{Client, ClientConfig};

fn main() -> Result<AppExit, Box<dyn std::error::Error>> {
    // Logging — özel STRATA_LOG env, wgpu=warn fallback, dosya+stdout katmanlı
    let (fmt_writer, _guard) = tracing_appender::non_blocking(
        tracing_appender::rolling::daily("logs", "strata.log"),
    );
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_env("STRATA_LOG")
                .unwrap_or_else(|_| "strata=info,wgpu=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(tracing_subscriber::fmt::layer().with_writer(fmt_writer))
        .with(tracing_log::LogTracer::new()) // wgpu `log` kayıtlarını yakala
        .init();

    // Konfigürasyon yükle (figment: defaults -> gömülü TOML -> user TOML -> STRATA_* env)
    let config = ClientConfig::load()?;

    // Client oluştur ve çalıştır
    let mut client = Client::new(config)?;
    Ok(client.run())
}

// AppExit 0.18'de Termination implement eder; main doğrudan döndürebilir.
use bevy::app::AppExit;
```

> **Not:** `tracing_log::LogTracer` ve `tracing_appender` bağımlılıkları `Cargo.toml`'a eklenmelidir (bkz. §8).

---

## 3. Client Config

TOML formatı doğru seçimdir (flat client config için ideal). Sorun struct tasarımı ve yükleme stratejisindeydi; aşağıda düzeltildi.

```toml
# client.toml

[window]
title = "Strata"
width = 1920
height = 1080
fullscreen = false
vsync = true
max_fps = 0  # 0 yorumu kaldırıldı; bkz. Option<NonZeroU32>

[render]
render_distance = 12
quality = "high"  # enum: low | medium | high | ultra
fov = 70.0       # 'fovs' yazım hatası düzeltildi
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
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

/// Client konfigürasyonu (tek Resource olarak enjekte edilir).
#[derive(Deserialize, Serialize, Clone, Debug, Resource)]
#[serde(default)] // eksik alanlar Default::default() ile doldurulur
pub struct ClientConfig {
    pub window: WindowConfig,
    pub render: RenderConfig,
    pub network: NetworkConfig,
    pub controls: ControlsConfig,
    pub audio: AudioConfig,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            render: RenderConfig::default(),
            network: NetworkConfig::default(),
            controls: ControlsConfig::default(),
            audio: AudioConfig::default(),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)] // bilinmeyen anahtar = yazım hatası uyarısı
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    pub max_fps: Option<NonZeroU32>, // None = sınırsız; '0' sentinel kaldırıldı
}

#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RenderConfig {
    pub render_distance: u32,
    pub quality: Quality, // String yerine enum
    #[serde(default = "default_fov")]
    pub fov: f32,
    pub shadows: bool,
    pub ambient_occlusion: bool,
    pub foveated_rendering: bool,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")] // "low" | "medium" | "high" | "ultra"
pub enum Quality {
    #[default]
    Low,
    Medium,
    High,
    Ultra,
}

// NetworkConfig, ControlsConfig, AudioConfig benzer şekilde;
// ses seviyeleri için 0.0..=1.0 doğrulaması ClientConfig::validate() içinde.

impl ClientConfig {
    /// figment ile katmanlı yükleme: defaults -> gömülü TOML -> user TOML -> STRATA_* env.
    /// User yolu dirs::config_dir()/strata/client.toml; dosya yoksa gömülü default kullanılır.
    pub fn load() -> Result<Self, ConfigError> {
        let user_path = dirs::config_dir()
            .map(|p| p.join("strata").join("client.toml"))
            .filter(|p| p.exists());

        let mut fig = Figment::from(Serialized::defaults(ClientConfig::default()));
        fig = fig.merge(Toml::file("client.toml")); // gömülü/relative fallback
        if let Some(p) = user_path {
            fig = fig.merge(Toml::file(p));
        }
        fig = fig.merge(Env::prefixed("STRATA_"));

        let cfg: ClientConfig = fig.extract()?;
        cfg.validate()?; // range check: volume 0..=1, width/height > 0, render_distance makul
        Ok(cfg)
    }
}
```

> **TOML+RON hibrit:** Bu hybrid **yalnızca** block/asset tanımları içindir (Plan 05). `client.toml` için serde enum'ları yeterlidir; kullanıcıyı RON'a zorlamayın.

---

## 4. Client Runtime

```rust
use bevy::app::{App, AppExit};
use bevy::prelude::*;

/// Game client.
pub struct Client {
    app: App,
}

impl Client {
    /// Yeni client oluştur (senkron; yalnızca plugin kaydı).
    pub fn new(config: ClientConfig) -> Result<Self> {
        let mut app = App::new();

        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: config.window.title.clone(),
                resolution: WindowResolution::new(
                    config.window.width as f32,
                    config.window.height as f32,
                ),
                resizable: true,
                ..default()
            }),
            ..default()
        }));

        // Client plugin'ları yükle (config tek Resource olarak enjekte)
        app.add_plugins(ClientPlugins);
        app.insert_resource(config);

        // Async init: loading state + AppExit listener
        app.add_systems(OnEnter(ClientState::Loading), begin_init);
        app.add_systems(
            Update,
            poll_init.run_if(in_state(ClientState::Loading)),
        );
        app.add_systems(Update, on_app_exit.run_if(on_event::<AppExit>()));

        Ok(Self { app })
    }

    /// Client'ı çalıştır. AppExit döner (Termination).
    pub fn run(&mut self) -> AppExit {
        self.app.run()
    }
}
```

> **Async init gerçeği:** `Client::new` senkron kalır (Bevy `App` kurulumu senkron olmalıdır). Ağır iş `begin_init` (`Startup`/`OnEnter(Loading)`) sisteminde `AsyncComputeTaskPool`'a spawn edilir; `poll_init` tamamlanınca `ClientState::Ready`'e geçilir. Shader derleme Bevy asset pipeline'ında zaten async'tır.

> **Graceful shutdown:** `app.run()` Ctrl+C + pencere kapanmasında `AppExit` üretir. `on_app_exit` sistemi storage flush (SQLite), network disconnect, GPU finish sıralı yapar. Garanti gerektiren async teardown için `App::set_runner` manual loop'a geçilebilir.

---

## 5. Client Plugins

```rust
/// Client plugin'ları.
pub struct ClientPlugins;

impl Plugin for ClientPlugins {
    fn build(&self, app: &mut App) {
        let config = app.world().resource::<ClientConfig>().clone(); // tek kaynaktan oku

        app
            // Çekirdek
            .add_plugins((
                BlockRegistryPlugin,
                EcsPlugin,
            ))
            // World
            .add_plugins((
                WorldGenPlugin,
                StreamingPlugin,
                StoragePlugin,
            ))
            // Render (Bevy RenderDevice üzerine custom 9-pass)
            .add_plugins(RenderPlugin::new(&config.render))
            // Gameplay
            .add_plugins((
                PhysicsPlugin,
                LightingPlugin,
                PlayerPlugin,
                EntityPlugin,
                AiPlugin,
            ))
            .add_plugins(UiPlugin)
            .add_plugins(AudioPlugin)
            .add_plugins(ParticlePlugin)
            // Network (client mode)
            .add_plugins(NetworkPlugin::client(&config.network))
            .add_plugins(DebugPlugin);
    }
}
```

> Config her plugin'e `clone()` ile dağıtılmaz; `Res<ClientConfig>` ile okunur. `RenderPlugin` ve `NetworkPlugin` yalnızca kendi alt dilimini alır (build sırasında bir kez okunur).

---

## 6. Init Sırası (Düzeltildi)

```
Client Başlatma Sırası:
  ┌─────────────────────────────────────────┐
  │ 1. App::new() + DefaultPlugins            │
  │    (window, input, audio, asset, UI)     │
  ├─────────────────────────────────────────┤
  │ 2. ClientPlugins build() — senkron kayıt │
  │    (sistemler/resource'lar register edilir,│
  │     I/O ÇALIŞTIRILMAZ)                    │
  ├─────────────────────────────────────────┤
  │ 3. app.run() başlar:                     │
  │    a. RenderPlugin lifecycle ready()/     │
  │       finish() → wgpu adapter/device/    │
  │       queue ASENKRON burada oluşur       │
  │       (new() sırasında DEĞİL)            │
  │    b. Startup / OnEnter(Loading):        │
  │       world-gen + network async spawn    │
  │    c. poll_init → Ready (loading ekranı) │
  │    d. Per-frame: Update (sim) ‖ Render    │
  │       (Core3d, custom Node'lar)          │
  ├─────────────────────────────────────────┤
  │ 4. AppExit → on_app_exit teardown        │
  │    (storage flush, disconnect, GPU finish)│
  └─────────────────────────────────────────┘
```

> wgpu device bir **runtime resource**'tur (`Res<RenderDevice>`), `new()`'de hazır değildir.

---

## 7. Workspace Yapısı

```
Strata/
├── Cargo.toml              ← Workspace root
├── AGENTS.md               ← Agent talimatları
├── client.toml             ← Client konfigürasyonu (user override: ~/.config/strata/client.toml)
├── server.toml             ← Server konfigürasyonu
├── plans/                  ← Plan dokümanları
│   ├── 01-overview.md
│   ├── 06-xbrickmap.md
│   ├── ...
│   └── 31-client-binary.md
├── crates/
│   ├── core/  ecs/  world-gen/  meshing/  render/  network/  storage/
│   ├── modding/  physics/  lighting/  plugin-api/  player/  audio/
│   ├── ui/  particles/  ai/  security/  debug/  server/  commands/
│   ├── animation/  fluids/  daynight/  building/  crafting/  map/  events/
└── bin/
    ├── client/             ← Game client (Bevy + wgpu/winit)
    └── server/             ← Headless server (tokio)
```

> **Crate parçalama:** 24+ crate aşırı değil; ancak `events/commands/map/daynight/building/crafting/animation` gibi mikro-crate'ler `strata-gameplay` altında toplanabilir. `wasmtime` (modding) ve `rusqlite` (storage) feature arkasına alınmalı (client build'i ödemesin).

---

## 8. Cargo.toml (Workspace — Düzeltildi)

> **Derleme-kıran düzeltmeler:** `bevy_rapier3d 0.22→0.34`, `rapier3d 0.22→0.32`, `glyphon 0.7→0.11`, `bevy_replicon_renet2 0.13→0.17` (ayrı `renet2` pini kaldırıldı), `glam 0.29→0.32`, `tokio full→trimmed`, `wgpu` Bevy'in çözdüğü sürümle birleştirildi. `edition="2024"` ve `resolver="2"` doğru.

```toml
[workspace]
members = [
    "crates/core", "crates/ecs", "crates/world-gen", "crates/meshing",
    "crates/render", "crates/network", "crates/storage", "crates/modding",
    "crates/physics", "crates/lighting", "crates/plugin-api", "crates/player",
    "crates/audio", "crates/ui", "crates/particles", "crates/ai",
    "crates/security", "crates/debug", "crates/server", "crates/commands",
    "crates/animation", "crates/fluids", "crates/daynight", "crates/building",
    "crates/crafting", "crates/map", "crates/events",
    "bin/client", "bin/server",
]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
# ECS (Bevy) — SABİT sürüm (0.19 render API'sını kırar)
bevy = "=0.18"
bevy_ecs = "=0.18"
bevy_app = "=0.18"
bevy_render = "=0.18"
bevy_asset = "=0.18"
bevy_audio = "=0.18"
bevy_ui = "=0.18"

# Render — wgpu sürümü BEVY 0.18'İN ÇÖZDÜĞÜ SÜRÜMLE AYNI OLMALI.
# Doğrula: `cargo tree -p bevy_render -i wgpu` (0.18.1 → wgpu 28).
# render crate'i ayrı wgpu cihazı kurmaz; Bevy'nin RenderDevice'ını kullanır.
wgpu = "0.28"
winit = "0.30"
glam = "0.32"
glyphon = "0.11"        # wgpu 0.28/29 uyumlu (0.7 → wgpu 0.23, ÇAKIŞIR)
bytemuck = { version = "1", features = ["derive"] }

# Async
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time", "net"] }

# Network — renet2 ayrı pinlenmez (bevy_replicon_renet2 re-export eder)
bevy_replicon = "0.40"
bevy_replicon_renet2 = "0.17"   # bevy_replicon 0.40 + bevy 0.18 ile uyumlu
bevy_rapier3d = "0.34"           # bevy 0.18 uyumlu (0.22 ~3 yıl eski, DERLENMEZ)
rapier3d = { version = "0.32", features = ["enhanced-determinism"] }

# Serialization
rkyv = "0.8"
postcard = "1.1"
figment = { version = "0.13", features = ["toml", "env"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"

# Noise
fastnoise2 = "0.4"

# Storage
rusqlite = { version = "0.32", features = ["bundled"] }  # ya da ["sqlite3"] (CI hızı)
dirs = "5"

# Compression / Hash
zstd = "0.13"
blake3 = "1.5"
xxhash-rust = { version = "0.8", features = ["xxh64"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tracing-appender = "0.2"
tracing-log = "0.2"

# SIMD / Utils
wide = "0.7"
slotmap = "1.0"
ahash = "0.8"

# Modding (yalnızca modding crate'inde)
wasmtime = "45"
```

### Build-time optimizasyon
- **cargo-hakari:** workspace-hack crate ile wgpu/rapier/wasmtime gibi ağır bağımlılıklar tek seferde (union feature) derlenir.
- **rust-lld (Windows):** `.cargo/config.toml` → `target.'cfg(windows)'.rustflags = ["-C", "link-arg=-fuse-ld=lld"]` (rapier/fastnoise2/rusqlite C-interop için).
- **MSRV:** `rust-toolchain.toml` ≥ 1.89 (bevy 0.18 floor).

---

## 9. Build Komutları (CI gating ile)

```bash
# Geliştirme (yerel)
cargo build --workspace
cargo clippy --workspace --all-targets        # -D warnings YOK (dev için çok sert)
cargo fmt
cargo test --workspace

# Sadece client / server
cargo build -p strata-client
cargo build -p strata-server

# CI (katı)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo nextest run --workspace && cargo test --doc   # nextest doctest çalıştırmaz
cargo build --workspace --profile distribution       # thin-LTO, strip

# Benchmark
cargo bench

# Çalıştır
cargo run -p strata-client
cargo run -p strata-server
```

> Dağıtım profili (`Cargo.toml`): `distribution = { inherits = "release", lto = "thin", codegen-units = 1, strip = true }`.

---

## 10. Renderer Entegrasyonu & wgpu Birleştirme (Ek Not)

- **Tek cihaz:** `crates/render` kendi `wgpu::Instance/Device/Surface`'unu **kurmaz**. Bevy'nin `RenderApp` sub-app'ı içinde `RenderDevice`/`RenderQueue`'a erişir (`bevy_render::renderer::RenderDevice`). 9-pass pipeline render-graph `Node`/`ViewNode` olarak `Core3d` subgraph'a eklenir; Bevy'nin `MainPass3d` değiştirilir/düzenlenir, `bevy_ui` HUD üstte çizilir.
- **wgpu sürüm birliği:** Workspace'teki `wgpu` pininin Bevy 0.18'in çözdüğü sürümle (0.18.1 → `wgpu 28`) **aynı** olması zorunludur; aksi halde iki wgpu kopyası linklenir ve tipler uyumsuz olur. `glyphon` gibi her şey bu major'ı takip etmelidir.
- **Visibility buffer atomics:** `atomic<u64>` yerine `TEXTURE_INT64_ATOMIC` (`r64uint` + `textureAtomicMax`) öner; fallback gerekiyorsa `atomicStore` değil CAS-loop kullan (wgpu #5887 race).
- **Native-only:** 9-pass compute + int64-atomic + GPU-feedback tasarımı **native** (Vulkan/DX12/Metal); tarayıcı/WebGPU hedefi ayrı rasterizer fallback gerektirir. Plan bunu açıkça belirtmelidir.
