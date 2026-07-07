# 27 — Headless Server

## 1. Genel Bakış

Strata'nın headless server'ı **tokio async runtime** üzerinde çalışır. Render pipeline'ı yoktur — sadece ECS, network, physics, world gen, storage ve lighting çalışır.

### Temel Prensipler

- **Headless:** wgpu/winit yok, sadece tokio + Bevy ECS
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

        // Bevy ECS world oluştur
        let mut world = World::new();

        // Server plugin'lerini yükle (custom plugin API)
        let mut app = App::new();
        app.add_plugin(ServerPlugins { config: config.clone() });

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

impl Plugin for ServerPlugins {
    fn build(&self, app: &mut App) {
        app
            // Çekirdek
            .add_plugin(BlockRegistryPlugin)
            .add_plugin(EcsPlugin)

            // World
            .add_plugin(WorldGenPlugin)
            .add_plugin(StreamingPlugin)
            .add_plugin(StoragePlugin)

            // Gameplay
            .add_plugin(PhysicsPlugin)
            .add_plugin(LightingPlugin)
            .add_plugin(EntityPlugin)
            .add_plugin(AiPlugin)

            // Network
            .add_plugin(NetworkPlugin::server(&self.config.network))

            // Security
            .add_plugin(SecurityPlugin)

            // Server'a özel
            .add_plugin(ServerConsolePlugin)
            .add_plugin(MetricsPlugin)

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
            .add_system(process_console_commands);
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
    commands: &mut EventReader<ConsoleInputEvent>,
    players: &Query<&Player>,
    network: &mut ResMut<NetworkManager>,
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


# 21 — Security & Validation Sistemi

## 1. Genel Bakış

Strata'nın güvenlik sistemi **server-authoritative** modeline dayanır. Client tarafı sadece input gönderir, tüm validasyon server'da yapılır.

### Temel Prensipler

- **Server-authoritative:** Client hiçbir şeye güvenmez
- **Input validation:** Tüm client input'ları validate edilir
- **Rate limiting:** Spam ve exploit önleme
- **State verification:** Client state periyodik doğrulanır

---

## 2. Input Validation

```rust
/// Input validator — client input'larını doğrular.
pub struct InputValidator {
    /// Rate limiter'lar (per-client).
    rate_limiters: HashMap<ClientId, RateLimiter>,

    /// Maksimum hareket mesafesi (per-tick).
    max_movement: f32,

    /// Maksimum blok yerleştirme mesafesi.
    max_reach: f32,

    /// Maksimum blok yerleştirme hızı.
    max_block_place_rate: u32,

    /// Maksimum blok kırma hızı.
    max_block_break_rate: u32,
}

pub struct RateLimiter {
    /// Son action zamanı.
    last_action: Instant,

    /// Action sayacı (pencere içinde).
    action_count: u32,

    /// Pencere süresi.
    window: Duration,

    /// Maksimum action sayısı.
    max_actions: u32,
}

impl RateLimiter {
    pub fn new(window: Duration, max_actions: u32) -> Self {
        Self {
            last_action: Instant::now(),
            action_count: 0,
            window,
            max_actions,
        }
    }

    pub fn allow(&mut self) -> bool {
        let now = Instant::now();

        // Pencere sıfırla
        if now - self.last_action > self.window {
            self.action_count = 0;
            self.last_action = now;
        }

        if self.action_count < self.max_actions {
            self.action_count += 1;
            true
        } else {
            false
        }
    }
}

impl InputValidator {
    /// Blok yerleştirme input'unu validate et.
    pub fn validate_block_place(
        &mut self,
        client_id: ClientId,
        pos: IVec3,
        player_pos: Vec3,
    ) -> Result<(), ValidationError> {
        // Rate limit kontrolü
        let limiter = self.rate_limiters
            .entry(client_id)
            .or_insert_with(|| RateLimiter::new(Duration::from_secs(1), 10));

        if !limiter.allow() {
            return Err(ValidationError::RateLimited);
        }

        // Mesafe kontrolü (reach check)
        let dist = player_pos.distance(pos.as_vec3());
        if dist > self.max_reach {
            return Err(ValidationError::TooFar {
                distance: dist,
                max: self.max_reach,
            });
        }

        // Pozisyon geçerli mi?
        if !self.is_valid_position(pos) {
            return Err(ValidationError::InvalidPosition);
        }

        Ok(())
    }

    /// Hareket input'unu validate et.
    pub fn validate_movement(
        &mut self,
        client_id: ClientId,
        new_pos: Vec3,
        old_pos: Vec3,
        dt: f32,
    ) -> Result<(), ValidationError> {
        // Hız kontrolü
        let distance = new_pos.distance(old_pos);
        let speed = distance / dt;

        if speed > self.max_movement / dt {
            return Err(ValidationError::TooFast {
                speed,
                max: self.max_movement / dt,
            });
        }

        // Teleport check (ani pozisyon değişimi)
        if distance > self.max_movement * 2.0 {
            return Err(ValidationError::TeleportSuspected);
        }

        Ok(())
    }

    /// Pozisyon geçerli mi?
    fn is_valid_position(&self, pos: IVec3) -> bool {
        // World sınırları içinde mi?
        // Y pozisyonu geçerli mi (0-128)?
        pos.y >= 0 && pos.y < 128
    }
}

#[derive(Debug)]
pub enum ValidationError {
    RateLimited,
    TooFar { distance: f32, max: f32 },
    TooFast { speed: f32, max: f32 },
    TeleportSuspected,
    InvalidPosition,
    InvalidBlock,
    InsufficientPermissions,
}
```

---

## 3. State Verification

```rust
/// State verifier — client state'i doğrular.
pub struct StateVerifier {
    /// Son doğrulama zamanı (per-client).
    last_verification: HashMap<ClientId, Instant>,

    /// Doğrulama aralığı.
    verification_interval: Duration,

    /// Tolerans (float hataları için).
    tolerance: f32,
}

impl StateVerifier {
    /// Client state'i doğrula.
    pub fn verify(
        &mut self,
        client_id: ClientId,
        client_state: &ClientState,
        server_state: &ServerState,
    ) -> Result<(), VerificationError> {
        let now = Instant::now();

        // Periyodik doğrulama
        if let Some(last) = self.last_verification.get(&client_id) {
            if now - *last < self.verification_interval {
                return Ok(()); // Henüz zamanı değil
            }
        }

        self.last_verification.insert(client_id, now);

        // Pozisyon doğrulama
        let pos_diff = client_state.position.distance(server_state.position);
        if pos_diff > self.tolerance {
            return Err(VerificationError::PositionMismatch {
                client: client_state.position,
                server: server_state.position,
                diff: pos_diff,
            });
        }

        // Velocity doğrulama
        let vel_diff = client_state.velocity.distance(server_state.velocity);
        if vel_diff > self.tolerance * 2.0 {
            return Err(VerificationError::VelocityMismatch {
                client: client_state.velocity,
                server: server_state.velocity,
                diff: vel_diff,
            });
        }

        // Inventory doğrulama
        if !self.verify_inventory(&client_state.inventory, &server_state.inventory) {
            return Err(VerificationError::InventoryMismatch);
        }

        Ok(())
    }

    /// Inventory doğrulama.
    fn verify_inventory(
        &self,
        client: &Inventory,
        server: &Inventory,
    ) -> bool {
        // Toplam item sayısı aynı mı?
        let client_total: u32 = client.slots.iter()
            .filter_map(|s| s.as_ref().map(|s| s.count as u32))
            .sum();

        let server_total: u32 = server.slots.iter()
            .filter_map(|s| s.as_ref().map(|s| s.count as u32))
            .sum();

        client_total == server_total
    }
}

#[derive(Debug)]
pub enum VerificationError {
    PositionMismatch {
        client: Vec3,
        server: Vec3,
        diff: f32,
    },
    VelocityMismatch {
        client: Vec3,
        server: Vec3,
        diff: f32,
    },
    InventoryMismatch,
}
```

---

## 4. Anti-Cheat

```rust
/// Anti-cheat sistemi.
pub struct AntiCheat {
    /// Şüpheli client'lar.
    suspicious: HashMap<ClientId, SuspicionReport>,

    /// Ban listesi.
    ban_list: HashSet<ClientId>,

    /// Aksiyon limitleri.
    limits: AntiCheatLimits,
}

pub struct SuspicionReport {
    /// Şüpheli aksiyonlar.
    pub actions: Vec<SuspiciousAction>,

    /// Şüpheli skoru (yüksek = daha şüpheli).
    pub score: f32,

    /// İlk şüpheli aksiyon zamanı.
    pub first_suspicion: Instant,
}

#[derive(Clone)]
pub enum SuspiciousAction {
    /// Hız ihlali.
    SpeedViolation { speed: f32, max: f32 },

    /// Reach ihlali.
    ReachViolation { distance: f32, max: f32 },

    /// Fly şüphesi.
    FlightSuspected,

    /// NoClip şüphesi.
    NoClipSuspected,

    /// Hızlı blok kırma.
    FastBreaking { rate: f32, max: f32 },

    /// Hızlı blok yerleştirme.
    FastPlacing { rate: f32, max: f32 },

    /// Invalid inventory.
    InvalidInventory,

    /// Packet spam.
    PacketSpam { rate: f32, max: f32 },
}

pub struct AntiCheatLimits {
    /// Maksimum şüphe skoru (auto-ban threshold).
    pub auto_ban_threshold: f32,

    /// Şüphe skoru zamanla azalır (half-life).
    pub suspicion_half_life: Duration,

    /// Aksiyon ağırlıkları.
    pub action_weights: HashMap<SuspiciousActionType, f32>,
}

impl AntiCheat {
    /// Şüpheli aksiyon kaydet.
    pub fn report_suspicion(
        &mut self,
        client_id: ClientId,
        action: SuspiciousAction,
    ) {
        let weight = self.get_action_weight(&action);

        let report = self.suspicious
            .entry(client_id)
            .or_insert_with(|| SuspicionReport {
                actions: Vec::new(),
                score: 0.0,
                first_suspicion: Instant::now(),
            });

        report.actions.push(action);
        report.score += weight;

        // Auto-ban kontrolü
        if report.score >= self.limits.auto_ban_threshold {
            self.ban_client(client_id, "Anti-cheat: suspicion threshold exceeded");
        }
    }

    /// Aksiyon ağırlığı.
    fn get_action_weight(&self, action: &SuspiciousAction) -> f32 {
        match action {
            SuspiciousAction::SpeedViolation { .. } => 5.0,
            SuspiciousAction::ReachViolation { .. } => 3.0,
            SuspiciousAction::FlightSuspected => 10.0,
            SuspiciousAction::NoClipSuspected => 10.0,
            SuspiciousAction::FastBreaking { .. } => 2.0,
            SuspiciousAction::FastPlacing { .. } => 2.0,
            SuspiciousAction::InvalidInventory => 5.0,
            SuspiciousAction::PacketSpam { .. } => 1.0,
        }
    }

    /// Client'ı banla.
    pub fn ban_client(&mut self, client_id: ClientId, reason: &str) {
        self.ban_list.insert(client_id);
        tracing::warn!(client_id = %client_id, reason, "Client banned");
    }

    /// Şüphe skorunu zamanla azalt.
    pub fn decay_suspicion(&mut self, dt: f32) {
        let decay_rate = 1.0 / self.limits.suspicion_half_life.as_secs_f32();

        for report in self.suspicious.values_mut() {
            report.score *= 1.0 - decay_rate * dt;

            if report.score < 0.1 {
                report.actions.clear();
            }
        }
    }
}
```

---

## 5. Server-Side Authority

```rust
/// Server-authoritative world update.
pub fn server_world_update(
    world: &mut ResMut<World>,
    events: &mut EventReader<StrataEvent>,
    validator: &mut ResMut<InputValidator>,
    anti_cheat: &mut ResMut<AntiCheat>,
    clients: &Query<(Entity, &RepliconClient, &PlayerPosition)>,
) {
    for event in events.read() {
        match event {
            StrataEvent::BlockPlaced { pos, block_id } => {
                // Client pozisyonunu bul
                if let Some((_, client, player_pos)) = clients.iter()
                    .find(|(_, c, _)| c.peer_id == event.client_id)
                {
                    // Validate
                    match validator.validate_block_place(
                        client.peer_id,
                        *pos,
                        player_pos.0,
                    ) {
                        Ok(()) => {
                            // Geçerli — dünyayı güncelle
                            world.set_block(*pos, *block_id);
                        }
                        Err(e) => {
                            // Geçersiz — client'a geri bildir
                            tracing::warn!(
                                client_id = %client.peer_id,
                                error = ?e,
                                "Invalid block place rejected"
                            );

                            anti_cheat.report_suspicion(
                                client.peer_id,
                                SuspiciousAction::ReachViolation {
                                    distance: player_pos.0.distance(pos.as_vec3()),
                                    max: validator.max_reach,
                                },
                            );

                            // Client state'i geri al
                            // (server doğru state'i gönderir)
                        }
                    }
                }
            }

            StrataEvent::BlockBroken { pos } => {
                // Benzer validasyon
            }
        }
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  security/
    ├── mod.rs              ← Security plugin entry point
    ├── validation/
    │   ├── mod.rs          ← InputValidator
    │   ├── movement.rs     ← Hareket validasyonu
    │   ├── block.rs        ← Blok yerleştirme/kırma validasyonu
    │   └── rate_limit.rs   ← RateLimiter
    ├── verification/
    │   ├── mod.rs          ← StateVerifier
    │   ├── position.rs     ← Pozisyon doğrulama
    │   └── inventory.rs    ← Inventory doğrulama
    ├── anti_cheat/
    │   ├── mod.rs          ← AntiCheat
    │   ├── report.rs       ← SuspicionReport
    │   ├── actions.rs      ← SuspiciousAction enum
    │   └── limits.rs       ← AntiCheatLimits
    └── ban/
        ├── mod.rs          ← Ban sistemi
        └── list.rs         ← Ban listesi
```


# 34 — Command & Console System

## 1. Genel Bakış

Strata'nın komut sistemi **server/client console**, **debug komutları** ve **admin yetkilendirme** destekler.

### Temel Prensipler

- **Command registry:** Komutlar runtime'da kaydedilir
- **Permission-based:** Yetki seviyeleri (player, mod, admin, console)
- **Tab completion:** Otomatik tamamlama
- **Server & Client:** Hem server hem client komutları

---

## 2. Command Registry

```rust
pub struct CommandRegistry {
    pub commands: HashMap<String, Command>,
}

pub struct Command {
    pub name: String,
    pub description: String,
    pub permission: PermissionLevel,
    pub handler: Box<dyn Fn(&[&str]) -> CommandResult>,
    pub tab_completer: Option<Box<dyn Fn(&str) -> Vec<String>>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PermissionLevel {
    Player = 0,
    Moderator = 1,
    Admin = 2,
    Console = 3,
}
```

---

## 3. Console

```rust
#[derive(Component)]
pub struct Console {
    /// Komut geçmişi.
    pub history: Vec<String>,

    /// Aktif input.
    pub input: String,

    /// Log mesajları.
    pub messages: Vec<ConsoleMessage>,

    /// Görünürlük.
    pub visible: bool,
}

pub struct ConsoleMessage {
    pub text: String,
    pub level: LogLevel,
    pub timestamp: f64,
}
```

---

## 4. Built-in Komutlar

```
/gamemode <mode>          — Oyun modu değiştir
/give <player> <item> <n> — Item ver
/tp <x> <y> <z>           — Işınlan
/time set <value>         — Zaman ayarla
/weather <type>           — Hava durumu
/seed                     — Dünya seed'i göster
/list                     — Oyuncu listesi
/kick <player>            — Oyuncuyu at
/ban <player>             — Oyuncuyu banla
```

---

## 5. Crate Organizasyonu

```
crates/
  commands/
    ├── mod.rs
    ├── registry.rs
    ├── console.rs
    ├── builtins/
    └── permissions.rs
```
