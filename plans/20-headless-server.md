# 27 — Headless Server

## 1. Genel Bakış

Strata'nın headless server'ı **tokio async runtime** üzerinde çalışır. Render pipeline'ı yoktur — sadece ECS, network, physics, world gen, storage ve lighting çalışır.

### Temel Prensipler

- **Headless:** wgpu/winit yok, sadece tokio + ECS
- **Server-authoritative:** Tüm validasyon server'da
- **Multi-thread:** Physics, world gen, storage paralel
- **Low memory:** <512MB/100 oyuncu hedefi

---

## 2. Server Binary

```rust
// bin/server/main.rs

use strata_server::{Server, ServerConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "strata=info".into()),
        )
        .init();

    // Konfigürasyon yükle
    let config = ServerConfig::load("server.toml")?;

    tracing::info!("Starting Strata Headless Server v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Listening on {}:{}", config.bind_address, config.port);

    // Server oluştur
    let mut server = Server::new(config).await?;

    // Başlat
    server.start().await?;

    // CTRL+C bekle
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    // Kapat
    server.shutdown().await?;

    Ok(())
}
```

---

## 3. Server Config

```toml
# server.toml

[network]
bind_address = "0.0.0.0"
port = 25565
max_players = 100
tick_rate = 20
view_distance = 12

[world]
seed = 12345
generator_version = 1
max_sectors_loaded = 500
save_interval_seconds = 30

[physics]
tick_rate = 60
max_entities = 10000

[storage]
world_path = "./worlds/default"
cache_size_mb = 256
flush_interval_seconds = 60

[performance]
worker_threads = 0  # 0 = CPU sayısı
max_memory_mb = 2048

[security]
max_reach = 6.0
max_block_place_per_second = 10
max_block_break_per_second = 20
auto_ban_threshold = 50.0
```

```rust
/// Server konfigürasyonu.
#[derive(Deserialize)]
pub struct ServerConfig {
    pub network: NetworkConfig,
    pub world: WorldConfig,
    pub physics: PhysicsConfig,
    pub storage: StorageConfig,
    pub performance: PerformanceConfig,
    pub security: SecurityConfig,
}

#[derive(Deserialize)]
pub struct NetworkConfig {
    pub bind_address: String,
    pub port: u16,
    pub max_players: u32,
    pub tick_rate: u32,
    pub view_distance: u32,
}

#[derive(Deserialize)]
pub struct WorldConfig {
    pub seed: u64,
    pub generator_version: u32,
    pub max_sectors_loaded: u32,
    pub save_interval_seconds: u64,
}

#[derive(Deserialize)]
pub struct PhysicsConfig {
    pub tick_rate: u32,
    pub max_entities: u32,
}

#[derive(Deserialize)]
pub struct StorageConfig {
    pub world_path: PathBuf,
    pub cache_size_mb: u32,
    pub flush_interval_seconds: u64,
}

#[derive(Deserialize)]
pub struct PerformanceConfig {
    pub worker_threads: u32,
    pub max_memory_mb: u32,
}

#[derive(Deserialize)]
pub struct SecurityConfig {
    pub max_reach: f32,
    pub max_block_place_per_second: u32,
    pub max_block_break_per_second: u32,
    pub auto_ban_threshold: f32,
}
```

---

## 4. Server Runtime

```rust
/// Headless server.
pub struct Server {
    /// ECS world.
    world: World,

    /// Network manager.
    network: NetworkManager,

    /// Tick handle.
    tick_handle: JoinHandle<()>,

    /// Storage flush handle.
    storage_handle: JoinHandle<()>,

    /// Konfigürasyon.
    config: ServerConfig,

    /// Çalışıyor mu?
    running: Arc<AtomicBool>,
}

impl Server {
    /// Yeni server oluştur.
    pub async fn new(config: ServerConfig) -> Result<Self> {
        // Tokio runtime (ayrı worker threads)
        let worker_threads = if config.performance.worker_threads == 0 {
            num_cpus::get()
        } else {
            config.performance.worker_threads as usize
        };

        // ECS world oluştur
        let mut world = World::new();

        // Server plugin'lerini yükle
        let mut app = App::new();
        app.add_plugins(ServerPlugins { config: config.clone() });

        // Network manager
        let network = NetworkManager::new(&config.network).await?;

        Ok(Self {
            world,
            network,
            tick_handle: JoinHandle::dummy(),
            storage_handle: JoinHandle::dummy(),
            config,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Server'ı başlat.
    pub async fn start(&mut self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);

        // Tick loop başlat
        let tick_rate = self.config.network.tick_rate;
        let tick_interval = Duration::from_secs_f32(1.0 / tick_rate as f32);

        let running = self.running.clone();
        let mut world = self.world.clone();
        let mut network = self.network.clone();

        self.tick_handle = tokio::spawn(async move {
            let mut tick_count: u64 = 0;
            let mut ticker = tokio::time::interval(tick_interval);

            while running.load(Ordering::SeqCst) {
                ticker.tick().await;

                // 1. Network receive
                network.receive(&mut world).await;

                // 2. ECS tick
                world.run_systems();

                // 3. Network send
                network.send(&world).await;

                tick_count += 1;

                // Her 100 tick'te log
                if tick_count % 100 == 0 {
                    let tps = tick_count as f32 / ticker.elapsed().as_secs_f32();
                    tracing::debug!("Server tick: {} (TPS: {:.1})", tick_count, tps);
                }
            }
        });

        // Storage flush loop
        let flush_interval = Duration::from_secs(self.config.storage.flush_interval_seconds);
        let running = self.running.clone();
        let mut storage = StorageManager::new(&self.config.storage)?;

        self.storage_handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(flush_interval);

            while running.load(Ordering::SeqCst) {
                ticker.tick().await;
                storage.flush_dirty().await;
            }
        });

        tracing::info!("Server started (tick rate: {} TPS)", tick_rate);
        Ok(())
    }

    /// Server'ı kapat.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);

        // Client'ları bilgilendir
        self.network.broadcast("Server shutting down...").await;

        // Son flush
        // ...

        // Task'ları bekle
        if let Err(e) = self.tick_handle.await {
            tracing::error!("Tick task error: {}", e);
        }

        if let Err(e) = self.storage_handle.await {
            tracing::error!("Storage task error: {}", e);
        }

        tracing::info!("Server shut down complete");
        Ok(())
    }
}
```

---

## 5. Server Plugins

```rust
/// Server-only plugin'lar.
pub struct ServerPlugins {
    pub config: ServerConfig,
}

impl PluginsState for ServerPlugins {
    fn build(&self, app: &mut App) {
        app
            // Çekirdek
            .add_plugins(BlockRegistryPlugin)
            .add_plugins(EcsPlugin)

            // World
            .add_plugins(WorldGenPlugin)
            .add_plugins(StreamingPlugin)
            .add_plugins(StoragePlugin)

            // Gameplay
            .add_plugins(PhysicsPlugin)
            .add_plugins(LightingPlugin)
            .add_plugins(EntityPlugin)
            .add_plugins(AiPlugin)

            // Network
            .add_plugins(NetworkPlugin::server(&self.config.network))

            // Security
            .add_plugins(SecurityPlugin)

            // Server'a özel
            .add_plugins(ServerConsolePlugin)
            .add_plugins(MetricsPlugin)

            // Kaynaklar
            .insert_resource(self.config.clone());
    }
}
```

---

## 6. Server Console

```rust
/// Server konsol komutları.
pub struct ServerConsolePlugin;

impl Plugin for ServerConsolePlugin {
    fn build(&self, app: &mut App) {
        app
            .init_resource::<ConsoleCommands>()
            .add_systems(Update, process_console_commands);
    }
}

pub enum ConsoleCommand {
    /// Yardım.
    Help,

    /// Oyuncu listesi.
    List,

    /// Oyuncuyu kick'le.
    Kick { name: String },

    /// Oyuncuyu ban'la.
    Ban { name: String },

    /// Mesaj gönder.
    Say { message: String },

    /// Oyun modu değiştir.
    Gamemode { mode: String, player: String },

    /// Teleport.
    Tp { from: String, to: String },

    /// Dünya bilgisi.
    Status,

    /// Kaydet.
    Save,

    /// Kapat.
    Stop,
}

impl ConsoleCommand {
    pub fn parse(input: &str) -> Option<Self> {
        let parts: Vec<&str> = input.split_whitespace().collect();

        match parts.first() {
            Some(&"help") => Some(ConsoleCommand::Help),
            Some(&"list") => Some(ConsoleCommand::List),
            Some(&"kick") => {
                parts.get(1).map(|name| ConsoleCommand::Kick { name: name.to_string() })
            }
            Some(&"ban") => {
                parts.get(1).map(|name| ConsoleCommand::Ban { name: name.to_string() })
            }
            Some(&"say") => {
                Some(ConsoleCommand::Say {
                    message: parts[1..].join(" "),
                })
            }
            Some(&"gamemode") => {
                if parts.len() >= 3 {
                    Some(ConsoleCommand::Gamemode {
                        mode: parts[1].to_string(),
                        player: parts[2].to_string(),
                    })
                } else {
                    None
                }
            }
            Some(&"tp") => {
                if parts.len() >= 3 {
                    Some(ConsoleCommand::Tp {
                        from: parts[1].to_string(),
                        to: parts[2].to_string(),
                    })
                } else {
                    None
                }
            }
            Some(&"status") => Some(ConsoleCommand::Status),
            Some(&"save") => Some(ConsoleCommand::Save),
            Some(&"stop") => Some(ConsoleCommand::Stop),
            _ => None,
        }
    }
}

/// Konsol komutlarını işle.
pub fn process_console_commands(
    mut commands: EventReader<ConsoleInputEvent>,
    players: Query<&Player>,
    mut network: ResMut<NetworkManager>,
) {
    for event in commands.read() {
        if let Some(cmd) = ConsoleCommand::parse(&event.input) {
            match cmd {
                ConsoleCommand::Help => {
                    println!("Komutlar: help, list, kick, ban, say, gamemode, tp, status, save, stop");
                }
                ConsoleCommand::List => {
                    let count = players.iter().count();
                    println!("{} oyuncu çevrimiçi:", count);
                    for player in players.iter() {
                        println!("  - {}", player.name);
                    }
                }
                ConsoleCommand::Say { message } => {
                    network.broadcast(&format!("[Server] {}", message)).await;
                }
                ConsoleCommand::Status => {
                    let player_count = players.iter().count();
                    println!("Oyuncu: {}", player_count);
                    // ... daha fazla metrik
                }
                ConsoleCommand::Save => {
                    println!("Dünya kaydediliyor...");
                    // ...
                }
                ConsoleCommand::Stop => {
                    println!("Server kapatılıyor...");
                    // ...
                }
                _ => println!("Komut henüz implement edilmedi."),
            }
        }
    }
}
```

---

## 7. Server Memory Management

```rust
/// Server memory yöneticisi.
pub struct ServerMemoryManager {
    /// Maksimum bellek (MB).
    max_memory_mb: u32,

    /// Son GC zamanı.
    last_gc: Instant,

    /// GC aralığı.
    gc_interval: Duration,
}

impl ServerMemoryManager {
    /// Bellek kullanımını kontrol et.
    pub fn check_and_gc(&mut self, world: &mut World) {
        let now = Instant::now();

        if now - self.last_gc < self.gc_interval {
            return;
        }

        self.last_gc = now;

        let used_mb = self.get_memory_usage();

        if used_mb > self.max_memory_mb as u64 * 80 / 100 {
            tracing::warn!(
                used_mb = used_mb,
                max_mb = self.max_memory_mb,
                "Memory usage high, triggering GC"
            );

            // 1. En uzak sector'ları unload et
            self.unload_distant_sectors(world);

            // 2. Entity cleanup
            self.cleanup_dead_entities(world);

            // 3. Storage flush
            self.flush_storage(world);

            let new_used = self.get_memory_usage();
            tracing::info!("GC complete: {}MB → {}MB", used_mb, new_used);
        }
    }

    fn get_memory_usage(&self) -> u64 {
        // Platform-specific memory query
        // Windows: GetProcessMemoryInfo
        // Linux: /proc/self/status
        0
    }
}
```

---

## 8. Crate Organizasyonu

```
bin/
  server/
    ├── main.rs             ← Server entry point
    └── config.rs           ← ServerConfig

crates/
  server/
    ├── mod.rs              ← Server plugin
    ├── runtime.rs          ← Server runtime
    ├── console.rs          ← Server console
    ├── memory.rs           ← Memory management
    └── plugins.rs          ← Server-only plugins
```
