# Implementation Plan — Faz 4: Network & Multiplayer

**Süre:** Hafta 13-18 (6 hafta)
**Hedef:** Server-authoritative multiplayer altyapısı, headless server, 1000+ oyuncu kapasitesi

---

## 0. Faz 4 Başlangıç Kontrol Listesi

- [ ] Faz 1-3 tüm milestone'ları tamamlandı ve `dev` branch'inde stabilize
- [ ] `bevy_renet2 0.13+`, `bevy_replicon 0.39+`, `bevy_replicon_renet2 0.14+` crate versiyonları doğrulandı
- [ ] `postcard 1.1+` network serialization için hazır
- [ ] Mevcut ECS component'leri (`Position`, `Velocity`, `Health`, `Inventory`) `Serialize + Deserialize` trait'lerini implement ediyor
- [ ] `bin/client` ve `bin/server` binary'leri workspace'te mevcut (boş da olsa)

---

## 1. Hafta 13 — Network Crate İskeleti & Transport Katmanı

### 1.1. Workspace & Dependency Kurulumu

**Dosya:** `Cargo.toml` (workspace root)

```toml
[workspace.dependencies]
bevy_renet2 = "0.13"
bevy_replicon = "0.39"
bevy_replicon_renet2 = "0.14"
postcard = { version = "1.1", features = ["alloc"] }
serde = { version = "1", features = ["derive"] }
```

**Dosya:** `crates/network/Cargo.toml`

```toml
[package]
name = "strata-network"
version = "0.1.0"
edition = "2024"

[dependencies]
strata-core = { path = "../core" }
strata-ecs = { path = "../ecs" }
bevy_ecs = "0.18"
bevy_renet2 = { workspace = true }
bevy_replicon = { workspace = true }
bevy_replicon_renet2 = { workspace = true }
postcard = { workspace = true }
serde = { workspace = true }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
```

### 1.2. Crate Yapısı

```
crates/network/src/
├── lib.rs              # Public exports, NetworkPlugin
├── protocol.rs         # NetworkProtocolPlugin (channel config, replication setup)
├── server.rs           # ServerPlugin (renet2 server, tick loop)
├── client.rs           # ClientPlugin (renet2 client, prediction hooks)
├── chunk_sync.rs       # Manuel chunk request/response (postcard + zstd)
├── visibility.rs       # Interest management (spatial partitioning)
├── events.rs           # Remote event tanımları (input, chat, block interaction)
└── config.rs           # NetworkConfig (port, tick rate, bandwidth limits)
```

### 1.3. NetworkConfig

**Dosya:** `crates/network/src/config.rs`

```rust
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub server_port: u16,
    pub tick_rate: u8,          // 20 TPS (Minecraft standardı)
    pub client_send_rate: u8,   // 30 input packets/sec
    pub chunk_view_distance: u8, // 8-16 chunk radius
    pub max_clients: u16,        // 1000+
    pub heartbeat_interval_ms: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            server_port: 27015,
            tick_rate: 20,
            client_send_rate: 30,
            chunk_view_distance: 10,
            max_clients: 1024,
            heartbeat_interval_ms: 1000,
        }
    }
}
```

### 1.4. NetworkPlugin (Ana Plugin)

**Dosya:** `crates/network/src/lib.rs`

```rust
use bevy_ecs::prelude::*;
use bevy_renet2::RenetPlugins;
use bevy_replicon::prelude::*;
use bevy_replicon_renet2::*;

mod protocol;
mod server;
mod client;
mod chunk_sync;
mod visibility;
mod events;
mod config;

pub use config::NetworkConfig;
pub use events::*;
pub use chunk_sync::*;

pub struct NetworkPlugin {
    pub config: NetworkConfig,
    pub mode: NetworkMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkMode {
    Server,
    Client,
    SinglePlayer, // Local loopback (test)
}

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.config.clone())
            .add_plugins(RenetPlugins)
            .add_plugins(RepliconPlugins);

        match self.mode {
            NetworkMode::Server => {
                app.add_plugins(server::ServerPlugin { config: self.config.clone() });
            }
            NetworkMode::Client => {
                app.add_plugins(client::ClientPlugin { config: self.config.clone() });
            }
            NetworkMode::SinglePlayer => {}
        }

        app.add_plugins(protocol::NetworkProtocolPlugin);
    }
}
```

### 1.5. Channel & Replication Setup

**Dosya:** `crates/network/src/protocol.rs`

```rust
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::*;
use strata_ecs::components::{Position, Velocity, Health, Inventory};

pub struct NetworkProtocolPlugin;

impl Plugin for NetworkProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Component replication — server → client
        app.replicate::<Position>()
           .replicate::<Velocity>()
           .replicate::<Health>()
           .replicate::<Inventory>();

        // Remote events — client → server
        app.add_client_trigger::<PlayerInputEvent>(ChannelKind::Unordered)
           .add_client_trigger::<BlockInteractEvent>(ChannelKind::ReliableOrdered)
           .add_client_trigger::<ChatMessageEvent>(ChannelKind::ReliableOrdered)
           .add_server_trigger::<ChunkDataEvent>(ChannelKind::ReliableOrdered)
           .add_server_trigger::<EntitySpawnEvent>(ChannelKind::ReliableUnordered);
    }
}
```

### 1.6. Remote Events

**Dosya:** `crates/network/src/events.rs`

```rust
use serde::{Deserialize, Serialize};
use glam::{Vec3, IVec2};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PlayerInputEvent {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sprint: bool,
    pub look_delta: (f32, f32), // (yaw, pitch)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BlockInteractEvent {
    pub block_pos: IVec3,
    pub face: u8,
    pub is_break: bool, // true = break, false = place
    pub block_id: Option<u16>, // place edilecek blok (is_break=false ise)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChatMessageEvent {
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkDataEvent {
    pub x: i32,
    pub z: i32,
    pub data: Vec<u8>, // zstd compressed
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EntitySpawnEvent {
    pub entity_id: u32,
    pub entity_type: u16,
    pub position: Vec3,
}
```

### 1.7. ServerPlugin (renet2 Server)

**Dosya:** `crates/network/src/server.rs`

```rust
use bevy_ecs::prelude::*;
use bevy_renet2::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet2::server::*;
use tokio::runtime::Runtime;
use crate::config::NetworkConfig;

pub struct ServerPlugin {
    pub config: NetworkConfig,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let runtime = Runtime::new().expect("Failed to create tokio runtime");

        app.insert_resource(RenetServer::new(ServerConfig {
            current_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap(),
            max_clients: self.config.max_clients as usize,
            protocol_id: 0,
        }))
        .insert_resource(ServerTransport::new_tokio(
            runtime,
            ServerTransportConfig {
                port: self.config.server_port,
            },
        ).expect("Failed to create server transport"))
        .add_plugins(RepliconServerPlugin)
        .add_systems(Update, (
            server_tick_system,
            handle_client_connections,
            handle_client_disconnections,
            process_client_events,
        ));
    }
}

fn server_tick_system(
    mut server: ResMut<RenetServer>,
    config: Res<NetworkConfig>,
    // ... ECS world access
) {
    let tick_duration = std::time::Duration::from_millis(1000 / config.tick_rate as u64);
    // 20 TPS sabit tick loop
    server.update(tick_duration);
}

fn handle_client_connections(
    mut server_events: EventReader<ServerEvent>,
) {
    for event in server_events.read() {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                tracing::info!("Client {} connected", client_id);
                // Initial world state snapshot gönder
            }
            ServerEvent::ClientDisconnected { client_id, reason } => {
                tracing::info!("Client {} disconnected: {:?}", client_id, reason);
                // Entity cleanup
            }
        }
    }
}
```

### 1.8. ClientPlugin (renet2 Client + Prediction)

**Dosya:** `crates/network/src/client.rs`

```rust
use bevy_ecs::prelude::*;
use bevy_renet2::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet2::client::*;
use crate::config::NetworkConfig;

pub struct ClientPlugin {
    pub config: NetworkConfig,
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RenetClient::new(ClientConfig {
            client_id: 0, // runtime'da atanacak
            protocol_id: 0,
        }))
        .add_plugins(RepliconClientPlugin)
        .add_systems(Update, (
            client_input_system,
            handle_server_events,
            client_prediction_system,
        ));
    }
}

fn client_input_system(
    mut triggers: TriggerSender<PlayerInputEvent>,
    // input state access
) {
    // Input topla ve server'a gönder (30 Hz)
}

fn client_prediction_system(
    // Local prediction: input'u hemen uygula, server confirmation'da reconcile
) {
    // bevy_replicon built-in prediction hooks kullan
}
```

### 1.9. Teslim Edilebilirler (Hafta 13)

- [ ] `crates/network` crate oluşturuldu, derleniyor
- [ ] `NetworkPlugin` hem server hem client modunda çalışıyor
- [ ] Channel konfigürasyonu tamamlandı (Reliable, Unreliable, ReliableUnordered)
- [ ] Component replication (`Position`, `Velocity`, `Health`, `Inventory`) aktif
- [ ] Remote event'ler tanımlandı ve trigger edilebiliyor
- [ ] Server client bağlantı/disconnect event'lerini handle ediyor
- [ ] Client input server'a gönderiliyor

---

## 2. Hafta 14 — Chunk Sync Sistemi

### 2.1. Mimari Kararlar

| Karar | Detay |
|-------|-------|
| Chunk sync yöntemi | Manuel (replication dışında) — chunk data büyük, delta compression uygun değil |
| Compression | zstd (level 3 — hız/kompresyon dengesi) |
| Request pattern | Client request → Server response (pull-based) |
| Priority | Oyuncuya en yakın chunk'lar önce gönderilir |
| Rate limit | Frame başına max 2 chunk gönderimi (bandwidth koruma) |
| Cache | Client tarafında loaded chunk cache (aynı chunk tekrar istenmez) |

### 2.2. Chunk Sync Protocol

**Dosya:** `crates/network/src/chunk_sync.rs`

```rust
use serde::{Deserialize, Serialize};
use zstd::stream::encode_all;
use strata_core::chunk::Chunk;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkRequestPacket {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkResponsePacket {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub compressed_data: Vec<u8>, // zstd compressed rkyv serialized chunk
}

pub fn compress_chunk(chunk: &Chunk) -> Result<Vec<u8>, std::io::Error> {
    let serialized = rkyv::to_bytes::<_, 256>(chunk).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, e)
    })?;
    encode_all(serialized.as_ref(), 3)
}

pub fn decompress_chunk(data: &[u8]) -> Result<Chunk, std::io::Error> {
    let decompressed = zstd::stream::decode_all(data)?;
    let archived = unsafe { rkyv::archived_root::<Chunk>(&decompressed) };
    Ok(archived.deserialize(&mut rkyv::Infallible).unwrap())
}
```

### 2.3. Client-Side Chunk Request Manager

```rust
use std::collections::{HashSet, VecDeque};
use glam::IVec2;

pub struct ChunkRequestManager {
    pub requested: HashSet<IVec2>,
    pub queue: VecDeque<IVec2>,
    pub chunks_per_frame: u8,
    pub view_distance: u8,
}

impl ChunkRequestManager {
    pub fn new(view_distance: u8) -> Self {
        Self {
            requested: HashSet::new(),
            queue: VecDeque::new(),
            chunks_per_frame: 2,
            view_distance,
        }
    }

    pub fn update_view(&mut self, player_chunk: IVec2) {
        // Mevcut view distance içindeki chunk'ları hesapla
        // Zaten requested olmayanları queue'ya ekle
        // En yakın → en uzak sıralama
    }

    pub fn poll_requests(&mut self) -> Vec<ChunkRequestPacket> {
        let count = self.chunks_per_frame.min(self.queue.len() as u8) as usize;
        (0..count)
            .filter_map(|_| self.queue.pop_front())
            .map(|pos| ChunkRequestPacket {
                chunk_x: pos.x,
                chunk_z: pos.y,
            })
            .collect()
    }

    pub fn mark_received(&mut self, x: i32, z: i32) {
        self.requested.insert(IVec2::new(x, z));
    }
}
```

### 2.4. Server-Side Chunk Response Handler

```rust
// Server, client'tan ChunkRequestPacket aldığında:
// 1. Chunk'u disk'ten yükle veya procedural üret
// 2. zstd + rkyv ile serialize et
// 3. ChunkResponsePacket olarak gönder
// 4. Rate limit kontrolü (frame başına max N chunk)

fn handle_chunk_requests(
    mut server: ResMut<RenetServer>,
    mut chunk_requests: EventReader<ClientChunkRequest>,
    chunk_storage: Res<ChunkStorage>,
    mut rate_limiter: ResMut<ChunkSendRateLimiter>,
) {
    for request in chunk_requests.read() {
        if rate_limiter.can_send() {
            if let Some(chunk) = chunk_storage.get(request.x, request.z) {
                if let Ok(data) = compress_chunk(&chunk) {
                    server.send_message(
                        request.client_id,
                        ChunkChannel,
                        ChunkResponsePacket {
                            chunk_x: request.x,
                            chunk_z: request.z,
                            compressed_data: data,
                        },
                    );
                    rate_limiter.record_send();
                }
            }
        }
    }
}
```

### 2.5. Teslim Edilebilirler (Hafta 14)

- [ ] Chunk request/response protocol implementasyonu tamamlandı
- [ ] zstd + rkyv compression/decompression çalışıyor
- [ ] Client-side chunk request manager (priority queue, rate limit)
- [ ] Server-side chunk response handler (rate limit, disk/procudural load)
- [ ] Chunk sync bandwidth ölçümü: <50KB/s/oyuncu hedefi doğrulandı
- [ ] Duplicate request prevention (client cache)

---

## 3. Hafta 15 — Interest Management & Entity Visibility

### 3.1. Mimari Kararlar

| Karar | Detay |
|-------|-------|
| Interest management | Spatial partitioning (grid-based) |
| Update granularity | Per-entity visibility check (her tick) |
| Optimization | Spatial hash grid — O(1) neighbor lookup |
| Entity spawn/despawn | Sadece visible entity'ler client'a replicate edilir |
| Transition zone | View distance ± 2 chunk buffer (pop-in önleme) |

### 3.2. Spatial Hash Grid

**Dosya:** `crates/network/src/visibility.rs`

```rust
use std::collections::HashMap;
use glam::IVec2;

const CELL_SIZE: i32 = 16; // 1 chunk = 1 cell

#[derive(Default)]
pub struct SpatialGrid {
    cells: HashMap<IVec2, Vec<EntityId>>,
}

impl SpatialGrid {
    pub fn insert(&mut self, entity: EntityId, position: IVec2) {
        let cell = position / CELL_SIZE;
        self.cells.entry(cell).or_default().push(entity);
    }

    pub fn remove(&mut self, entity: EntityId, position: IVec2) {
        let cell = position / CELL_SIZE;
        if let Some(entities) = self.cells.get_mut(&cell) {
            entities.retain(|&e| e != entity);
        }
    }

    pub fn get_nearby(&self, position: IVec2, radius: i32) -> Vec<EntityId> {
        let center = position / CELL_SIZE;
        let mut result = Vec::new();

        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let cell = center + IVec2::new(dx, dz);
                if let Some(entities) = self.cells.get(&cell) {
                    result.extend(entities.iter().copied());
                }
            }
        }

        result
    }

    pub fn update(&mut self, entity: EntityId, old_pos: IVec2, new_pos: IVec2) {
        self.remove(entity, old_pos);
        self.insert(entity, new_pos);
    }
}
```

### 3.3. Visibility System (Bevy ECS)

```rust
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::*;
use strata_ecs::components::Position;

#[derive(Component)]
pub struct Visibility {
    pub visible_to: HashSet<ClientId>,
}

#[derive(Resource)]
pub struct InterestManager {
    pub spatial_grid: SpatialGrid,
    pub view_distance_chunks: u8,
}

fn update_entity_visibility(
    mut query: Query<(Entity, &Position, &mut Visibility)>,
    clients: Query<&ClientId>,
    mut interest: ResMut<InterestManager>,
    mut commands: Commands,
) {
    for (entity, pos, mut visibility) in query.iter_mut() {
        let player_pos = pos.0.truncate().as_ivec2();
        let nearby = interest.spatial_grid.get_nearby(
            player_pos,
            interest.view_distance_chunks as i32,
        );

        let new_visible: HashSet<ClientId> = nearby
            .iter()
            .filter_map(|e| clients.get(*e).ok())
            .map(|c| *c)
            .collect();

        // Yeni visible olanlar → replicate
        for client in new_visible.difference(&visibility.visible_to) {
            commands.entity(entity).observe_replicate(*client);
        }

        // Artık visible olmayanlar → stop replicate
        for client in visibility.visible_to.difference(&new_visible) {
            commands.entity(entity).observe_despawn(*client);
        }

        visibility.visible_to = new_visible;
    }
}
```

### 3.4. Chunk-Based Interest (Oyuncu Başına)

```rust
// Her oyuncu için ilgi alanı (chunk bazlı):
// - View distance içindeki chunk'lar: FULL sync
// - View distance + 2 buffer: POSITION only (entity position sync, mesh yok)
// - Dışında: NO sync

fn calculate_player_interest(
    player_pos: ChunkPos,
    view_distance: u8,
) -> PlayerInterest {
    let mut full_sync = HashSet::new();
    let mut position_only = HashSet::new();

    for dx in -(view_distance as i32)..=(view_distance as i32) {
        for dz in -(view_distance as i32)..=(view_distance as i32) {
            let dist = (dx.abs().max(dz.abs())) as u8;
            let chunk = player_pos + IVec2::new(dx, dz);

            if dist <= view_distance {
                full_sync.insert(chunk);
            } else if dist <= view_distance + 2 {
                position_only.insert(chunk);
            }
        }
    }

    PlayerInterest { full_sync, position_only }
}
```

### 3.5. Teslim Edilebilirler (Hafta 15)

- [ ] Spatial hash grid implementasyonu (O(1) neighbor lookup)
- [ ] Entity visibility system (per-client visible entity tracking)
- [ ] Chunk-based interest management (full sync vs position-only)
- [ ] Entity spawn/despawn replication (sadece visible entity'ler)
- [ ] Transition zone (buffer) implementasyonu (pop-in önleme)
- [ ] Visibility system performans testi: 1000 entity, 100 oyuncu

---

## 4. Hafta 16 — Headless Server Binary

### 4.1. Server Binary Yapısı

**Dosya:** `bin/server/Cargo.toml`

```toml
[package]
name = "strata-server"
version = "0.1.0"
edition = "2024"

[dependencies]
strata-core = { path = "../../crates/core" }
strata-ecs = { path = "../../crates/ecs" }
strata-network = { path = "../../crates/network" }
strata-world-gen = { path = "../../crates/world-gen" }
strata-storage = { path = "../../crates/storage" }
bevy_ecs = "0.18"
bevy_renet2 = { workspace = true }
bevy_replicon = { workspace = true }
bevy_replicon_renet2 = { workspace = true }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
clap = { version = "4", features = ["derive"] }
```

### 4.2. Server Main

**Dosya:** `bin/server/src/main.rs`

```rust
use bevy_ecs::prelude::*;
use strata_network::{NetworkPlugin, NetworkMode, NetworkConfig};
use strata_world_gen::WorldGenPlugin;
use strata_storage::StoragePlugin;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "strata-server", version, about = "Strata Headless Server")]
struct Args {
    #[arg(short, long, default_value_t = 27015)]
    port: u16,

    #[arg(short, long, default_value_t = 20)]
    tick_rate: u8,

    #[arg(short, long, default_value_t = 1024)]
    max_players: u16,

    #[arg(short, long, default_value_t = 10)]
    view_distance: u8,

    #[arg(short, long, default_value = "world")]
    world_name: String,

    #[arg(long, default_value_t = false)]
    creative_mode: bool,
}

fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("strata_server=info".parse().unwrap())
                .add_directive("strata_network=info".parse().unwrap())
        )
        .init();

    let mut app = App::new();

    app.add_plugins(StoragePlugin {
        world_name: args.world_name.clone(),
    })
    .add_plugins(WorldGenPlugin::default())
    .add_plugins(NetworkPlugin {
        config: NetworkConfig {
            server_port: args.port,
            tick_rate: args.tick_rate,
            max_clients: args.max_players,
            chunk_view_distance: args.view_distance,
            ..Default::default()
        },
        mode: NetworkMode::Server,
    });

    // Server-only systems
    app.add_systems(Update, (
        server_tick,
        world_save_system,
        player_management_system,
        console_command_system,
    ));

    tracing::info!("Strata Server starting on port {}", args.port);
    tracing::info!("Tick rate: {} TPS, Max players: {}", args.tick_rate, args.max_players);

    // Headless run loop (render yok, sadece ECS + network)
    loop {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(
            1000 / args.tick_rate as u64,
        ));
    }
}
```

### 4.3. Server-Only Systems

```rust
fn server_tick(
    mut tick_counter: ResMut<TickCounter>,
    // ... server state
) {
    tick_counter.0 += 1;
    // 20 TPS sabit tick
}

fn world_save_system(
    tick: Res<TickCounter>,
    chunk_storage: ResMut<ChunkStorage>,
    dirty_chunks: Query<&ChunkPos, With<Dirty>>,
) {
    // Her 600 tick'te bir (30 saniye) dirty chunk'ları kaydet
    if tick.0 % 600 == 0 {
        for pos in dirty_chunks.iter() {
            chunk_storage.save(pos);
        }
        tracing::info!("World saved ({} dirty chunks)", dirty_chunks.iter().count());
    }
}

fn player_management_system(
    server_events: EventReader<ServerEvent>,
    mut players: ResMut<PlayerRegistry>,
) {
    for event in server_events.read() {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                players.register(*client_id);
                tracing::info!("Player {} joined ({} online)", client_id, players.count());
            }
            ServerEvent::ClientDisconnected { client_id, .. } => {
                players.unregister(*client_id);
                tracing::info!("Player {} left ({} online)", client_id, players.count());
            }
        }
    }
}

fn console_command_system(
    // stdin'den komut oku (stop, save, kick, list, etc.)
) {
    // Basit console parser
    // Komutlar: stop, save, kick <player>, list, tps, mem
}
```

### 4.4. Memory & Performance Monitoring

```rust
#[derive(Resource)]
pub struct ServerStats {
    pub tick_rate_current: f32,
    pub tick_rate_avg: f32,
    pub connected_players: u16,
    pub loaded_chunks: u32,
    pub memory_usage_mb: f32,
    pub network_bandwidth_kbps: f32,
}

fn update_server_stats(
    mut stats: ResMut<ServerStats>,
    // ... various resources
) {
    stats.memory_usage_mb = get_process_memory_usage() as f32 / 1024.0 / 1024.0;
    // Her 5 saniyede bir log
}
```

### 4.5. Teslim Edilebilirler (Hafta 16)

- [ ] `bin/server` headless binary oluşturuldu
- [ ] CLI argument parsing (port, tick rate, max players, view distance)
- [ ] Server tick loop (20 TPS sabit)
- [ ] World save system (periodic dirty chunk flush)
- [ ] Player management (join/leave tracking)
- [ ] Console command interface (stop, save, kick, list, tps, mem)
- [ ] Memory & performance monitoring
- [ ] Server 100 oyuncu ile 20 TPS stabil çalışıyor

---

## 5. Hafta 17 — Client-Side Prediction & Server Reconciliation

### 5.1. Mimari Kararlar

| Karar | Detay |
|-------|-------|
| Prediction | Client input'u hemen uygula (local simulation) |
| Reconciliation | Server state geldiğinde, client state'i düzelt |
| Input buffering | Son 64 input server'da tutulur (reconciliation için) |
| Snap correction | Server-client farkı > threshold ise snap (teleport) |
| Interpolation | Diğer oyuncu entity'leri interpolate edilir (50ms buffer) |

### 5.2. Client-Side Prediction System

**Dosya:** `crates/network/src/client.rs` (ekleme)

```rust
use std::collections::VecDeque;

#[derive(Component)]
pub struct PredictedPosition(pub Vec3);

#[derive(Component)]
pub struct InputHistory {
    pub inputs: VecDeque<(u16, PlayerInputEvent)>, // (tick, input)
    pub max_size: usize,
}

impl InputHistory {
    pub fn new() -> Self {
        Self {
            inputs: VecDeque::with_capacity(64),
            max_size: 64,
        }
    }

    pub fn push(&mut self, tick: u16, input: PlayerInputEvent) {
        self.inputs.push_back((tick, input));
        while self.inputs.len() > self.max_size {
            self.inputs.pop_front();
        }
    }

    pub fn get_up_to(&self, tick: u16) -> Vec<PlayerInputEvent> {
        self.inputs
            .iter()
            .filter(|(t, _)| *t <= tick)
            .map(|(_, input)| input.clone())
            .collect()
    }
}

fn client_prediction_system(
    mut query: Query<(&mut PredictedPosition, &mut InputHistory, &Player)>,
    input_state: Res<InputState>,
    tick: Res<TickCounter>,
) {
    for (mut pos, mut history, _) in query.iter_mut() {
        let input = input_state.current_input();
        history.push(tick.0 as u16, input.clone());

        // Input'u local olarak uygula (prediction)
        apply_input_to_position(&mut pos, &input);
    }
}
```

### 5.3. Server Reconciliation

```rust
fn server_reconciliation_system(
    mut query: Query<(&mut PredictedPosition, &mut InputHistory, &ServerConfirmedTick)>,
    confirmed_state: Res<ServerConfirmedState>,
) {
    for (mut pos, mut history, mut confirmed_tick) in query.iter_mut() {
        if let Some(confirmed) = confirmed_state.get(confirmed_tick.0) {
            // Server state'i al
            let server_pos = confirmed.position;

            // Input history'den server tick'inden sonrasını yeniden uygula
            let replay_inputs = history.get_up_to(confirmed_tick.0);

            // Reconciliation
            let mut reconciled_pos = server_pos;
            for input in replay_inputs {
                apply_input_to_position(&mut reconciled_pos, &input);
            }

            // Snap threshold kontrolü
            let diff = (pos.0 - reconciled_pos).length();
            if diff > 0.5 {
                // Snap (teleport) — büyük fark
                pos.0 = reconciled_pos;
            } else {
                // Smooth correction
                pos.0 = pos.0.lerp(reconciled_pos, 0.3);
            }

            // History'yi temizle (server tick'e kadar olanlar)
            while history.inputs.front().map_or(false, |(t, _)| *t <= confirmed_tick.0) {
                history.inputs.pop_front();
            }
        }
    }
}
```

### 5.4. Entity Interpolation (Diğer Oyuncular)

```rust
#[derive(Component)]
pub struct InterpolatedPosition {
    pub previous: Vec3,
    pub current: Vec3,
    pub alpha: f32, // 0.0 - 1.0 interpolation factor
}

fn interpolation_system(
    mut query: Query<&mut InterpolatedPosition, Without<Player>>,
    time: Res<Time>,
) {
    let delta = time.delta_secs();
    let interpolation_delay = 0.05; // 50ms buffer

    for mut interp in query.iter_mut() {
        interp.alpha += delta / interpolation_delay;
        interp.alpha = interp.alpha.min(1.0);

        // Interpolated render position
        let render_pos = interp.previous.lerp(interp.current, interp.alpha);
        // Render pipeline'a gönder
    }
}

fn update_interpolation_buffers(
    mut query: Query<(&Position, &mut InterpolatedPosition), Without<Player>>,
) {
    for (pos, mut interp) in query.iter_mut() {
        interp.previous = interp.current;
        interp.current = pos.0;
        interp.alpha = 0.0;
    }
}
```

### 5.5. Lag Compensation (Opsiyonel — Faz 4 sonu)

```rust
// Server-side lag compensation (opsiyonel, Faz 4'te basic seviye)
// Client'ın input timestamp'ine göre world state'i rewind et

#[derive(Resource)]
pub struct WorldStateHistory {
    pub states: VecDeque<(u16, WorldSnapshot)>,
    pub max_history: usize,
}

impl WorldStateHistory {
    pub fn get_state_at_tick(&self, tick: u16) -> Option<&WorldSnapshot> {
        self.states.iter().find(|(t, _)| *t == tick).map(|(_, s)| s)
    }

    pub fn add_state(&mut self, tick: u16, state: WorldSnapshot) {
        self.states.push_back((tick, state));
        while self.states.len() > self.max_history {
            self.states.pop_front();
        }
    }
}
```

### 5.6. Teslim Edilebilirler (Hafta 17)

- [ ] Client-side prediction (local input application)
- [ ] Input history buffer (64 input)
- [ ] Server reconciliation (replay + snap threshold)
- [ ] Entity interpolation (50ms buffer, diğer oyuncular için)
- [ ] Smooth correction (lerp) vs snap (teleport) kararı
- [ ] Prediction doğrulama: 100ms latency'de akıcı hareket
- [ ] Reconciliation doğrulama: server-client pozisyon farkı < 0.5 birim

---

## 6. Hafta 18 — Multiplayer Test & Entegrasyon

### 6.1. End-to-End Test Senaryoları

| Test | Açıklama | Beklenen Sonuç |
|------|----------|----------------|
| T1: Bağlantı | 2 client aynı server'a bağlanır | Her iki client da connected |
| T2: Görünürlük | Client A, Client B'yi görür | Entity replication aktif |
| T3: Hareket sync | Client A hareket eder, B görür | <100ms latency ile sync |
| T4: Blok kırma | Client A blok kırar, B görür | Block update replication |
| T5: Blok yerleştirme | Client A blok yerleştirir, B görür | Block update replication |
| T6: Chunk sync | Client yeni bölgeye gider | Chunk'lar sırayla yüklenir |
| T7: Disconnect | Client A disconnect olur | Entity despawn, B görür |
| T8: Reconnect | Client A tekrar bağlanır | World state snapshot, continue |
| T9: Stress test | 100 client aynı anda | 20 TPS stabil, <512MB RAM |
| T10: Chat | Client A mesaj gönderir, B alır | Chat event replication |

### 6.2. Automated Test Suite

```rust
#[cfg(test)]
mod network_tests {
    use super::*;

    #[test]
    fn test_chunk_compression_decompression() {
        let chunk = Chunk::test_fixture();
        let compressed = compress_chunk(&chunk).unwrap();
        let decompressed = decompress_chunk(&compressed).unwrap();

        assert_eq!(chunk.blocks, decompressed.blocks);
        assert!(compressed.len() < chunk.blocks.len() * 2); // Compression ratio
    }

    #[test]
    fn test_spatial_grid_nearby() {
        let mut grid = SpatialGrid::default();
        grid.insert(EntityId(1), IVec2::new(0, 0));
        grid.insert(EntityId(2), IVec2::new(16, 0));
        grid.insert(EntityId(3), IVec2::new(100, 100));

        let nearby = grid.get_nearby(IVec2::new(0, 0), 1);
        assert!(nearby.contains(&EntityId(1)));
        assert!(nearby.contains(&EntityId(2)));
        assert!(!nearby.contains(&EntityId(3)));
    }

    #[test]
    fn test_prediction_reconciliation() {
        // Client input uygula → server state gelince reconcile
        let mut pos = PredictedPosition(Vec3::ZERO);
        let input = PlayerInputEvent { forward: true, ..Default::default() };

        apply_input_to_position(&mut pos, &input);
        assert!(pos.0.z < 0.0); // Forward = -Z

        // Server confirmation geldiğinde
        let server_pos = Vec3::new(0.0, 0.0, -1.0);
        reconcile_position(&mut pos, server_pos, &[]);

        assert!((pos.0 - server_pos).length() < 0.01);
    }

    #[test]
    fn test_interest_management_visibility() {
        let mut manager = InterestManager {
            spatial_grid: SpatialGrid::default(),
            view_distance_chunks: 10,
        };

        manager.spatial_grid.insert(EntityId(1), IVec2::new(0, 0));
        manager.spatial_grid.insert(EntityId(2), IVec2::new(200, 200));

        let nearby = manager.spatial_grid.get_nearby(IVec2::new(0, 0), 10);
        assert!(nearby.contains(&EntityId(1)));
        assert!(!nearby.contains(&EntityId(2)));
    }
}
```

### 6.3. Performance Benchmark

```rust
// criterion benchmark suite

fn benchmark_chunk_sync(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk_sync");

    group.bench_function("compress_chunk", |b| {
        let chunk = Chunk::test_fixture();
        b.iter(|| compress_chunk(&chunk));
    });

    group.bench_function("decompress_chunk", |b| {
        let chunk = Chunk::test_fixture();
        let compressed = compress_chunk(&chunk).unwrap();
        b.iter(|| decompress_chunk(&compressed));
    });

    group.finish();
}

fn benchmark_visibility(c: &mut Criterion) {
    let mut group = c.benchmark_group("visibility");

    group.bench_function("spatial_grid_1000_entities", |b| {
        let mut grid = SpatialGrid::default();
        for i in 0..1000 {
            grid.insert(EntityId(i), IVec2::new(i as i32 % 100, i as i32 / 100));
        }
        b.iter(|| grid.get_nearby(IVec2::new(50, 50), 10));
    });

    group.finish();
}
```

### 6.4. Multiplayer Test Checklist

- [ ] T1: İki client aynı server'a bağlanabiliyor
- [ ] T2: Client'lar birbirini görebiliyor (entity replication)
- [ ] T3: Hareket sync <100ms latency ile çalışıyor
- [ ] T4: Blok kırma tüm client'lara yansıyor
- [ ] T5: Blok yerleştirme tüm client'lara yansıyor
- [ ] T6: Chunk sync sıralı ve rate-limited çalışıyor
- [ ] T7: Disconnect olan client'ın entity'si despawn oluyor
- [ ] T8: Reconnect olan client world state'i alıyor
- [ ] T9: 100 client ile 20 TPS stabil, <512MB RAM
- [ ] T10: Chat mesajları tüm client'lara ulaşıyor
- [ ] Prediction + reconciliation akıcı çalışıyor (100ms latency simülasyonu)
- [ ] Entity interpolation diğer oyuncular için smooth
- [ ] Interest management gereksiz replication'ı önlüyor
- [ ] Bandwidth kullanımı <50KB/s/oyuncu

### 6.5. Faz 4 Sonu Kriterleri

| Metrik | Hedef | Ölçüm Yöntemi |
|--------|-------|---------------|
| Server TPS | 20 (sabit) | `tracing` log'ları |
| Max oyuncu | 100+ (test), 1000+ (hedef) | Stress test |
| Network bandwidth | <50KB/s/oyuncu | `tokio` metrics |
| Client RAM | <2GB | Process monitor |
| Server RAM | <512MB/100 oyuncu | Process monitor |
| Movement latency | <100ms | Client-side timestamp |
| Chunk load time | <50ms/chunk | Server response time |
| Prediction accuracy | >95% | Server-client position diff |

---

## 7. Riskler ve Mitigasyon (Faz 4 Özel)

| Risk | Olasılık | Etki | Mitigasyon |
|------|----------|------|------------|
| bevy_replicon + renet2 versiyon uyumsuzluğu | Orta | Yüksek | Versiyon matrix'i önceden test et, `bevy_replicon_renet2` entegrasyon crate'i kullan |
| bevy_replicon 0.39 API breaking changes | Yüksek | Orta | 0.38 → 0.39 migration guide'ı önceden oku, feature flag'leri kontrol et |
| Chunk sync bandwidth aşımı | Orta | Orta | zstd level'ı dinamik ayarla (düşük bandwidth → level 5), rate limit sıkılaştır |
| 1000 oyuncu TPS düşüşü | Orta | Yüksek | Spatial grid cell size'ı optimize et, entity visibility check'lerini optimize et |
| Client prediction drift | Düşük | Orta | Snap threshold'ı agresif yap (1.0 birim), server authority kesin olsun |
| Headless server memory leak | Düşük | Yüksek | `tracing` + memory profiler ile izle, her 30 saniyede world save ile state flush |

---

## 8. Faz 4 → Faz 5 Geçiş Kriterleri

Faz 5 (Modding Sistemi) başlatılmadan önce:

- [ ] Tüm 10 multiplayer test senaryosu geçiyor
- [ ] Server 100 oyuncu ile 20 TPS stabil (10 dakika)
- [ ] Network bandwidth <50KB/s/oyuncu (ortalama)
- [ ] Client-side prediction + reconciliation akıcı
- [ ] Headless server CLI komutları çalışıyor
- [ ] `cargo clippy --workspace -- -D warnings` temiz
- [ ] `cargo test --workspace` tüm testler geçiyor
- [ ] `bin/server` ve `bin/client` ayrı ayrı derlenip çalışıyor
- [ ] Branch `dev`'e merge edildi, `main`'e cherry-pick hazır

---

## 9. Dosya Değişiklik Özeti (Faz 4)

### Yeni Dosyalar

```
crates/network/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── protocol.rs
│   ├── server.rs
│   ├── client.rs
│   ├── chunk_sync.rs
│   ├── visibility.rs
│   ├── events.rs
│   └── config.rs

bin/server/
├── Cargo.toml
└── src/
    └── main.rs
```

### Modifiye Dosyalar

```
Cargo.toml                          # Workspace dependencies
crates/ecs/src/components/*.rs      # Serialize + Deserialize derive
bin/client/src/main.rs              # NetworkPlugin entegrasyonu
```

---

## 10. Komut Referansı (Faz 4)

```bash
# Workspace build
cargo build --workspace

# Network crate build
cargo build -p strata-network

# Headless server build
cargo build -p strata-server --release

# Client build
cargo build -p strata-client --release

# Lint
cargo clippy --workspace -- -D warnings

# Test
cargo test -p strata-network

# Benchmark
cargo bench -p strata-network

# Headless server run
./target/release/strata-server --port 27015 --tick-rate 20 --max-players 100

# Client run (server'a bağlan)
./target/release/strata-client --server 127.0.0.1:27015
```
