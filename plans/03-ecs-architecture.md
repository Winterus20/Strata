# 14 — ECS Mimarisi (Bevy ECS)

## 1. Genel Bakış

Strata, **Bevy ECS 0.18+** kullanır. ECS, tüm oyun mantığının temelidir — render hariç her sistem ECS üzerinden çalışır.

### Temel Prensipler

- **Plugin-first:** Her alt sistem bir ECS plugin'dir
- **Data-oriented:** Cache-friendly component layout
- **Parallel-safe:** Sistemler otomatik paralel çalışır
- **Replicon-ready:** Network için component'lar replicon-compatible

---

## 2. Plugin Mimarisi

```rust
/// Strata ana plugin — tüm alt plugin'leri yükler.
pub struct StrataPlugin;

impl Plugin for StrataPlugin {
    fn build(&self, app: &mut App) {
        app
            // Çekirdek plugin'ler
            .add_plugins(BlockRegistryPlugin)
            .add_plugins(WorldGenPlugin)
            .add_plugins(StreamingPlugin)
            .add_plugins(PhysicsPlugin)
            .add_plugins(LightingPlugin)
            .add_plugins(RenderPlugin)
            .add_plugins(NetworkPlugin)
            .add_plugins(StoragePlugin)
            .add_plugins(PlayerPlugin)
            .add_plugins(EntityPlugin)
            .add_plugins(AudioPlugin)
            .add_plugins(UiPlugin)
            .add_plugins(DebugPlugin)
            // Ortak ayarlar
            .insert_resource(StrataConfig::default())
            .init_resource::<WorldState>();
    }
}
```

---

## 3. Component Tasarımı

### 3.1 World Components

```rust
/// Dünya entity'si — root entity.
#[derive(Component)]
pub struct WorldRoot;

/// Bir sector entity'si.
#[derive(Component)]
pub struct SectorEntity {
    pub coord: SectorCoord,
    pub tier: Tier,
    pub dirty: bool,
}

/// Sector'ün XBrickMap verisi.
#[derive(Component)]
pub struct SectorData(pub Arc<RwLock<Sector>>);

/// Sector'ün SVDAG root index'i.
#[derive(Component)]
pub struct SvdagRoot(pub Option<u32>);
```

### 3.2 Player Components

```rust
/// Oyuncu entity'si.
#[derive(Component)]
pub struct Player {
    pub name: String,
    pub client_id: Option<ClientId>,
}

/// Oyuncu pozisyonu.
#[derive(Component)]
pub struct PlayerPosition(pub Vec3);

/// Oyuncu orientasyonu.
#[derive(Component)]
pub struct PlayerOrientation(pub Quat);

/// Oyuncu velocity (fizik).
#[derive(Component)]
pub struct PlayerVelocity(pub Vec3);

/// Oyuncu health.
#[derive(Component)]
pub struct Health {
    pub current: u16,
    pub max: u16,
}

/// Oyuncu inventory.
#[derive(Component)]
pub struct Inventory {
    pub slots: [Option<ItemStack>; 36],
    pub hotbar_index: u8,
}

/// Oyuncu game mode.
#[derive(Component)]
pub enum GameMode {
    Survival,
    Creative,
    Spectator,
    Adventure,
}
```

### 3.3 Entity Components

```rust
/// Genel entity (mob, item, vehicle vb.).
#[derive(Component)]
pub struct Entity {
    pub entity_type: EntityType,
    pub health: u16,
    pub max_health: u16,
}

#[derive(Clone, Copy)]
pub enum EntityType {
    Zombie,
    Skeleton,
    Creeper,
    Spider,
    Cow,
    Pig,
    Sheep,
    Chicken,
    Item,
    ExperienceOrb,
    Arrow,
    Minecart,
    Boat,
}

/// Entity AI state.
#[derive(Component)]
pub struct AiState {
    pub current_goal: Option<AiGoal>,
    pub target: Option<Entity>,
    pub path: Option<Vec<IVec3>>,
    pub last_update: Instant,
}

#[derive(Clone)]
pub enum AiGoal {
    Wander,
    Flee(Entity),
    Attack(Entity),
    Follow(Entity),
    Idle,
}

/// Entity render component.
#[derive(Component)]
pub struct EntityRender {
    pub model_id: String,
    pub animation_state: AnimationState,
}
```

### 3.4 Render Components

```rust
/// Render edilebilir sector.
#[derive(Component)]
pub struct RenderableSector {
    pub vertex_slice: VertexSlice,
    pub visible: bool,
    pub frustum_culled: bool,
}

/// Kamera.
#[derive(Component)]
pub struct Camera {
    pub position: Vec3,
    pub orientation: Quat,
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

/// Işık kaynağı.
#[derive(Component)]
pub struct LightSource {
    pub light_type: LightType,
    pub color: [u8; 3],
    pub intensity: u8,
    pub radius: f32,
}

#[derive(Clone, Copy)]
pub enum LightType {
    Point,
    Directional,
    Spot,
}
```

### 3.5 Network Components

```rust
/// Network entity — replicon ile sync edilir.
#[derive(Component)]
pub struct Networked;

/// Replicon client entity.
#[derive(Component)]
pub struct RepliconClient {
    pub peer_id: PeerId,
    pub rtt: Duration,
    pub packet_loss: f32,
}

/// Entity'nin network owner'ı.
#[derive(Component)]
pub struct NetworkOwner(pub ClientId);

/// Entity interpolation state.
#[derive(Component)]
pub struct InterpolationState {
    pub previous: EntityState,
    pub current: EntityState,
    pub alpha: f32,
}

#[derive(Clone)]
pub struct EntityState {
    pub position: Vec3,
    pub orientation: Quat,
    pub velocity: Vec3,
    pub timestamp: Instant,
}
```

---

## 4. Sistem Mimarisi

### 4.1 Sistem Setleri

```rust
/// Sistem setleri — doğru sıralama için.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum StrataSystemSet {
    // Input
    Input,

    // Player
    PlayerMovement,
    PlayerInventory,

    // World
    WorldStreaming,
    WorldGeneration,
    WorldEditing,

    // Physics
    PhysicsUpdate,
    PhysicsCollision,
    PhysicsCharacter,

    // Lighting
    LightingUpdate,
    LightingBake,

    // Entity
    EntityAi,
    EntityUpdate,
    EntitySpawn,
    EntityDespawn,

    // Network
    NetworkSend,
    NetworkReceive,
    NetworkSync,

    // Render
    RenderPrepare,
    RenderCulling,
    RenderMeshing,
    RenderSubmit,

    // Storage
    StorageFlush,
    StorageGc,

    // Debug
    DebugRender,
    DebugMetrics,
}
```

### 4.2 Sistem Sıralaması

```rust
/// Sistem sıralaması konfigürasyonu.
pub fn configure_system_sets(app: &mut App) {
    app.configure_sets(
        (
            // 1. Input işleme
            StrataSystemSet::Input,

            // 2. Player güncelleme
            StrataSystemSet::PlayerMovement,
            StrataSystemSet::PlayerInventory,

            // 3. Network receive (server'dan gelen)
            StrataSystemSet::NetworkReceive,
            StrataSystemSet::NetworkSync,

            // 4. World streaming (tier güncelleme)
            StrataSystemSet::WorldStreaming,
            StrataSystemSet::WorldGeneration,

            // 5. World editing (blok yerleştirme/kırma)
            StrataSystemSet::WorldEditing,

            // 6. Physics
            StrataSystemSet::PhysicsUpdate,
            StrataSystemSet::PhysicsCollision,
            StrataSystemSet::PhysicsCharacter,

            // 7. Entity AI ve güncelleme
            StrataSystemSet::EntityAi,
            StrataSystemSet::EntityUpdate,
            StrataSystemSet::EntitySpawn,
            StrataSystemSet::EntityDespawn,

            // 8. Lighting
            StrataSystemSet::LightingUpdate,
            StrataSystemSet::LightingBake,

            // 9. Network send (client'a giden)
            StrataSystemSet::NetworkSend,

            // 10. Render hazırlık
            StrataSystemSet::RenderPrepare,
            StrataSystemSet::RenderCulling,
            StrataSystemSet::RenderMeshing,
            StrataSystemSet::RenderSubmit,

            // 11. Storage (arka plan)
            StrataSystemSet::StorageFlush,
            StrataSystemSet::StorageGc,

            // 12. Debug
            StrataSystemSet::DebugRender,
            StrataSystemSet::DebugMetrics,
        )
            .chain(),
    );
}
```

---

## 5. Örnek Sistemler

### 5.1 Player Movement

```rust
/// Oyuncu hareket sistemi.
pub fn player_movement_system(
    time: Res<Time>,
    input: Res<Input<KeyCode>>,
    mut players: Query<(
        &Player,
        &mut PlayerPosition,
        &mut PlayerOrientation,
        &mut PlayerVelocity,
    )>,
) {
    let dt = time.delta_secs();

    for (player, mut pos, mut orient, mut vel) in players.iter_mut() {
        // Input vektörü
        let mut input_dir = Vec3::ZERO;

        if input.pressed(KeyCode::W) { input_dir.z -= 1.0; }
        if input.pressed(KeyCode::S) { input_dir.z += 1.0; }
        if input.pressed(KeyCode::A) { input_dir.x -= 1.0; }
        if input.pressed(KeyCode::D) { input_dir.x += 1.0; }

        if input_dir.length_squared() > 0.0 {
            input_dir = input_dir.normalize();
        }

        // Kamera yönüne göre rotate et
        let forward = orient.mul_vec3(Vec3::NEG_Z);
        let right = orient.mul_vec3(Vec3::X);

        let move_dir = forward * input_dir.z + right * input_dir.x;

        // Hız uygula
        let speed = 5.0;
        vel.0.x = move_dir.x * speed;
        vel.0.z = move_dir.z * speed;

        // Yerçekimi
        vel.0.y -= 20.0 * dt;

        // Zıplama
        if input.pressed(KeyCode::Space) {
            // Ground check gerekli
        }

        // Pozisyon güncelle
        pos.0 += vel.0 * dt;
    }
}
```

### 5.2 World Streaming

```rust
/// World streaming sistemi — tier güncelleme.
pub fn world_streaming_system(
    camera: Query<&Camera>,
    mut sectors: Query<(
        Entity,
        &SectorEntity,
        &mut SectorData,
    )>,
    mut commands: Commands,
) {
    let camera_pos = camera.single().position;

    for (entity, sector, mut data) in sectors.iter_mut() {
        let sector_center = sector.coord.world_origin().as_vec3() + Vec3::new(16.0, 64.0, 16.0);
        let dist = (sector_center - camera_pos).length();

        let new_tier = determine_tier(dist);

        if new_tier != sector.tier {
            // Tier değişti — streaming işlemi başlat
            commands.entity(entity).insert(TierChange {
                old_tier: sector.tier,
                new_tier,
                timestamp: Instant::now(),
            });
        }
    }
}
```

### 5.3 Entity AI

```rust
/// Entity AI sistemi.
pub fn entity_ai_system(
    time: Res<Time>,
    entities: Query<(Entity, &mut AiState, &Entity)>,
    players: Query<&PlayerPosition>,
) {
    let dt = time.delta_secs();

    for (entity, mut ai, entity_data) in entities.iter_mut() {
        // AI update throttling (her frame güncelleme yok)
        if ai.last_update.elapsed() < Duration::from_millis(200) {
            continue;
        }

        ai.last_update = Instant::now();

        // En yakın oyuncuyu bul
        let nearest_player = players.iter()
            .min_by(|a, b| {
                let dist_a = a.0.length_squared();
                let dist_b = b.0.length_squared();
                dist_a.partial_cmp(&dist_b).unwrap()
            });

        if let Some((_, player_pos)) = nearest_player {
            let dist = player_pos.0.length();

            ai.current_goal = if dist < 20.0 {
                Some(AiGoal::Attack(entity))
            } else if dist < 40.0 {
                Some(AiGoal::Wander)
            } else {
                Some(AiGoal::Idle)
            };
        }
    }
}
```

---

## 6. Resource'lar

```rust
/// Global dünya durumu.
#[derive(Resource)]
pub struct WorldState {
    pub seed: WorldSeed,
    pub time_of_day: f32, // 0-24 saat
    pub weather: WeatherState,
    pub difficulty: Difficulty,
    pub day_count: u32,
}

/// Hava durumu.
#[derive(Resource)]
pub struct WeatherState {
    pub current: WeatherType,
    pub transition_timer: f32,
    pub intensity: f32,
}

#[derive(Clone, Copy)]
pub enum WeatherType {
    Clear,
    Rain,
    Thunderstorm,
    Snow,
    Blizzard,
}

/// Oyun konfigürasyonu.
#[derive(Resource)]
pub struct StrataConfig {
    pub render_distance: u32,
    pub max_fps: u32,
    pub vsync: bool,
    pub fullscreen: bool,
    pub fov: f32,
    pub render_quality: RenderQuality,
}

#[derive(Clone, Copy)]
pub enum RenderQuality {
    Low,
    Medium,
    High,
    Ultra,
    RayTraced,
}
```

---

## 7. Event Sistemi

```rust
/// Strata event'leri.
#[derive(Event)]
pub enum StrataEvent {
    /// Blok yerleştirildi.
    BlockPlaced { pos: IVec3, block_id: u16 },

    /// Blok kırıldı.
    BlockBroken { pos: IVec3, block_id: u16 },

    /// Sector yüklendi.
    SectorLoaded { coord: SectorCoord },

    /// Sector boşaltıldı.
    SectorUnloaded { coord: SectorCoord },

    /// Oyuncu katıldı.
    PlayerJoined { client_id: ClientId, name: String },

    /// Oyuncu ayrıldı.
    PlayerLeft { client_id: ClientId },

    /// Entity öldü.
    EntityDied { entity: Entity, killer: Option<Entity> },

    /// Hava durumu değişti.
    WeatherChanged { new_weather: WeatherType },

    /// Gün/değişti.
    DayChanged { day: u32 },
}
```

---

## 8. Crate Organizasyonu

```
crates/
  ecs/
    ├── mod.rs              ← ECS plugin entry point
    ├── components/
    │   ├── mod.rs          ← Component tanımları
    │   ├── world.rs        ← World/Sector components
    │   ├── player.rs       ← Player components
    │   ├── entity.rs       ← Entity/Mob components
    │   ├── render.rs       ← Render components
    │   └── network.rs      ← Network components
    ├── systems/
    │   ├── mod.rs          ← Sistem tanımları
    │   ├── player.rs       ← Player movement, inventory
    │   ├── streaming.rs    ← World streaming
    │   ├── editing.rs      ← Block placement/break
    │   ├── entity_ai.rs    ← Entity AI
    │   └── network.rs      ← Network sync
    ├── resources/
    │   ├── mod.rs          ← Resource tanımları
    │   ├── world_state.rs  ← WorldState
    │   ├── weather.rs      ← WeatherState
    │   └── config.rs       ← StrataConfig
    ├── events/
    │   ├── mod.rs          ← Event tanımları
    │   └── handlers.rs     ← Event handler'ları
    └── sets.rs             ← SystemSet tanımları
```
