# 03 — ECS Mimarisi (Bevy ECS 0.18+)

## 1. Genel Bakış

Strata, **Bevy ECS 0.18+** kullanır. ECS, tüm oyun mantığının temelidir — render hariç her sistem ECS üzerinden çalışır.

### Temel Prensipler

- **Plugin-first:** Her alt sistem bir Bevy plugin'dir
- **Data-oriented:** Cache-friendly component layout (Bevy archetype storage)
- **Otomatik paralel:** Bevy scheduler, bağımsız sistemleri otomatik paralel çalıştırır
- **Event/Message:** Bevy MessageWriter/MessageReader ile buffered event sistemi (network-safe) (Bevy 0.17+: EventWriter→MessageWriter, EventReader→MessageReader)
- **Change detection:** `Changed<T>`, `Added<T>` ile otomatik değişim tespiti
- **Reflect:** `#[derive(Reflect)]` ile runtime type introspection

### Gerçek Dünya Doğrulaması

Bu mimari, aşağıdaki gerçek dünya projelerinden doğrulanmış pattern'ler üzerine kurulmuştur:

| Proje | Teknoloji | Bulgular |
|-------|-----------|----------|
| **[Veloren](https://veloren.net)** | Rust, SPECS ECS | Tek sunucuda 181 eşzamanlı oyuncu (48-core Hetzner), tüm sunucularda toplam 400+, voxel world streaming |
| **[Bevy](https://bevyengine.org)** | Rust, Bevy ECS | 1000+ crate ekosistemi, archetype-based storage, otomatik paralel scheduler |
| **[Exofactory](https://exofactory.net/blog/2026-02-04)** | Bevy 0.18 + replicon | Server-authoritative multiplayer, headless server |

---

## 2. Bevy ECS Core

### 2.1 Component Tanımları

```rust
use bevy::prelude::*;

/// Bir sector entity'si.
#[derive(Component)]
pub struct SectorEntity {
    pub coord: SectorCoord,
    pub tier: Tier,
}

/// Sector'ün tier değişim durumu — SparseSet çünkü nadiren eklenir/kaldırılır.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct TierChange {
    pub old_tier: Tier,
    pub new_tier: Tier,
    pub timestamp: Instant,
}

/// Sector'ün XBrickMap verisi (immutable snapshot).
/// `CompressedChunkData`: `06-xbrickmap.md` §1.4 — Arc ile meshing/network thread'lerine lock-free paylaşım.
#[derive(Component)]
pub struct SectorData(pub Arc<CompressedChunkData>);

/// Sector mesh durumu — version tracking ile stale mesh tespiti.
#[derive(Component)]
pub struct SectorMeshState {
    pub data_version: u64,
    pub mesh_version: u64,
    pub pending_mesh_version: u64,
}

/// Chunk kirlilik flag'i — doğrudan mutasyon sonrası işaretlenir.
/// SparseSet çünkü sık eklenir/kaldırılır ve archetype move istenmez.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct ChunkDirty;

/// Chunk'ın yeniden mesh'e ihtiyacı olduğunu belirtir.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct NeedsRemesh;

/// Chunk'ın physics collider'ının güncellenmesi gerektiğini belirtir.
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct NeedsColliderUpdate;
```

### 2.2 Resource Tanımları

```rust
/// Global dünya durumu.
#[derive(Resource)]
pub struct WorldState {
    pub seed: WorldSeed,
    pub time_of_day: f32,
    pub weather: WeatherState,
    pub difficulty: Difficulty,
    pub day_count: u32,
}

impl Default for WorldState {
    fn default() -> Self {
        Self {
            seed: WorldSeed(0),
            time_of_day: 12.0,
            weather: WeatherState::default(),
            difficulty: Difficulty::Normal,
            day_count: 0,
        }
    }
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

impl Default for StrataConfig {
    fn default() -> Self {
        Self {
            render_distance: 12,
            max_fps: 60,
            vsync: true,
            fullscreen: false,
            fov: 70.0,
            render_quality: RenderQuality::High,
        }
    }
}

/// O(1) mekansal arama için Chunk haritası.
#[derive(Resource)]
pub struct ChunkMap(pub HashMap<SectorCoord, Entity>);

/// Meshing istek kuyuğu.
#[derive(Resource)]
pub struct MeshingQueue {
    pub sender: crossbeam::channel::Sender<MeshRequest>,
    pub receiver: crossbeam::channel::Receiver<MeshResult>,
}
```

### 2.3 Entity Spawn

```rust
// Entity oluşturma — Bevy Commands ile
commands.spawn((
    SectorEntity { coord: SectorCoord(IVec3::ZERO), tier: Tier::Active },
    SectorData(Arc::new(CompressedChunkData::empty())),
    SectorMeshState { data_version: 0, mesh_version: 0, pending_mesh_version: 0 },
    Transform::default(),
    Visibility::default(),
));

// Component ekleme
commands.entity(entity).insert(ChunkDirty);

// Component kaldırma
commands.entity(entity).remove::<ChunkDirty>();

// Entity despawn
commands.entity(entity).despawn();
```

### 2.4 Query Kullanımı

```rust
// Immutable query
fn read_positions(query: Query<&Transform, With<Player>>) {
    for transform in query.iter() {
        // ...
    }
}

// Mutable query
fn update_positions(mut query: Query<(&mut Transform, &Velocity)>) {
    for (mut transform, velocity) in query.iter_mut() {
        transform.translation += velocity.0 * dt;
    }
}

// Filtered query — sadece değişenler
fn on_health_changed(query: Query<&Health, Changed<Health>>) {
    for health in query.iter() {
        // Sadece sağlık değiştiğinde çalış
    }
}

// Entity ile birlikte
fn with_entity(query: Query<(Entity, &mut Transform)>) {
    for (entity, mut transform) in query.iter_mut() {
        // entity ID'ye erişim
    }
}

// Multiple filter
fn filtered(query: Query<&Transform, (With<Player>, Without<Dead>)>) {
    // Alive players only
}
```

### 2.5 Query Optimizasyonu

#### Filter-First Tasarım

`With<T>` ve `Without<T>` filtreleri archetype matching sırasında değerlendirilir — non-matching archetype'lar tamamen atlanır:

```rust
// KÖTÜ: Tüm entity'leri iterate et, loop içinde kontrol et
fn bad(query: Query<(&Transform, Option<&NeedsRemesh>)>) {
    for (transform, remesh) in query.iter() {
        if remesh.is_some() { /* ... */ }
    }
}

// İYİ: Archetype-level filtre, per-entity maliyet yok
fn good(query: Query<&Transform, With<NeedsRemesh>>) {
    for transform in query.iter() { /* ... */ }
}
```

**Performans etkisi:** Archetype-level filtering O(archetypes), per-entity check O(entities).

#### Minimum Component Sorgulama

Sadece gerçekten ihtiyaç duyulan component'ları sorgula — her ek component cache line kullanımını etkiler:

```rust
// KÖTÜ: Transform'u sorguluyoruz ama kullanmıyoruz
fn bad(query: Query<(&mut Velocity, &Transform)>) {
    for (mut vel, _) in query.iter_mut() { vel.0.y -= 9.8; }
}

// İYİ: Sadece ihtiyaç duyulan component
fn good(mut query: Query<&mut Velocity>) {
    for mut vel in query.iter_mut() { vel.0.y -= 9.8; }
}
```

#### Change Detection Guard

Bevy, değer gerçekten değişmese bile `&mut` erişimde change detection tetikler. Guard ile önle:

```rust
fn update_health(mut query: Query<&mut Health>) {
    for mut health in query.iter_mut() {
        let new_hp = calculate_new_hp(&health);
        if new_hp != health.current {  // Guard — sadece gerçekten değiştiyse
            health.current = new_hp;
        }
    }
}
```

#### `Ref<T>` vs `Changed<T>` Filtresi

```rust
// Filtre: sadece değişen entity'leri getir (büyük çoğunluk atlanır)
fn sync_changed(query: Query<&Transform, Changed<Transform>>) { ... }

// Ref: tüm entity'leri getir, değişen durumunu kontrol et
fn update_all(query: Query<Ref<Transform>>) {
    for transform in query.iter() {
        if transform.is_changed() {
            // özel işlem
        }
        // her zaman çalışır
    }
}
```

### 2.6 Event/Message Sistemi

```rust
/// Network message trait — ağ üzerinden iletilen mesajlar için.
#[derive(Event, Clone, Serialize, Deserialize)]
pub struct ClientBlockBreakRequest {
    pub client_id: ClientId,
    pub pos: IVec3,
    pub block_id: u16,
}

#[derive(Event, Clone, Serialize, Deserialize)]
pub struct ServerBlockBrokenBroadcast {
    pub pos: IVec3,
    pub block_id: u16,
}

/// MessageWriter ile yazma (Bevy 0.17+: EventWriter → MessageWriter)
fn send_block_break(mut events: MessageWriter<ClientBlockBreakRequest>) {
    events.send(ClientBlockBreakRequest {
        client_id: local_client_id(),
        pos: IVec3::new(5, 10, 5),
        block_id: 42,
    });
}

/// MessageReader ile okuma (Bevy 0.17+: EventReader → MessageReader)
fn handle_block_break(mut reader: MessageReader<ClientBlockBreakRequest>) {
    for event in reader.read() {
        // Blok kırma işle
    }
}
```

### 2.7 Command Queue (Deferred Mutations)

```rust
/// Deferred mutations — Bevy Commands ile
fn deferred_system(mut commands: Commands, query: Query<Entity, With<NeedsRemesh>>) {
    for entity in query.iter() {
        // Deferred: frame sonunda uygulanır
        commands.entity(entity).remove::<NeedsRemesh>();
        commands.entity(entity).insert(ChunkDirty);
    }
}

/// Deferred spawn
fn spawn_system(mut commands: Commands) {
    commands.spawn((
        SectorEntity { coord: SectorCoord(IVec3::new(1, 0, 0)), tier: Tier::Active },
        SectorData(Arc::new(CompressedChunkData::empty())),
    ));
}
```

---

## 3. Plugin Mimarisi

Plugin yükleme grafiği, bootstrap (voxel çekirdek + SubApp) ile oyun katmanını ayırır. **Tam tanım:** `04-plugin-api.md` §2–§3, §11. Bu bölüm ECS tarafındaki plugin listesi ve `StrataPlugin` sorumluluklarını tanımlar.

### 3.1 İki Katmanlı Yükleme

| Yapı | Rol | Kayıt |
|------|-----|-------|
| `StrataCorePlugins` | Motor bootstrap: SubApp (Render/Physics), `StrataSets`, `BlockRegistry`, XBrickMap, meshing | Ana `App` + SubApp world'leri |
| `StrataPlugin` | Oyun mantığı + cross-plugin `SystemSet` zinciri | Yalnızca ana `App` |
| `ModdingPlugin` | WASM / mod drain (`32-modding.md`) | `StrataCorePlugins` **sonrası** |

**Kritik:** `BlockRegistryPlugin`, `StrataRenderPlugin` ve `StrataPhysicsPlugin` **yalnızca** `StrataCorePlugins` / `StrataSubAppPlugin` içinde yüklenir — `StrataPlugin` bunları tekrar eklemez (çift kayıt ve yanlış world riski).

```rust
// client / server entry (main.rs) — 04 §3, §11
fn main() {
    App::new()
        .add_plugins(StrataCorePlugins) // bootstrap + SubApp
        .add_plugins(StrataPlugin)      // oyun katmanı (bu dosya §3.3)
        .add_plugins(ModdingPlugin)     // 32 — core sonrası
        .run();
}
```

### 3.2 StrataCorePlugins (Bootstrap — özet)

Detaylı SubApp extract, write-back ve `StrataSets` zinciri → `04-plugin-api.md` §2, §4.

```rust
/// Motor bootstrap — PluginGroup (04 §3).
pub struct StrataCorePlugins;

impl PluginGroup for StrataCorePlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(StrataSubAppPlugin)       // Render + Physics SubApp'leri
            .add(StrataSchedulingPlugin) // StrataSets / StrataPhysicsSets
            .add(BlockRegistryPlugin)      // 05 — init-only registry
            .add(XBrickMapPlugin)          // 06
            .add(StrataMeshingPlugin)
            // StrataRenderPlugin / StrataPhysicsPlugin → yalnızca SubApp (04 §2)
    }
}
```

### 3.3 StrataPlugin (Oyun Katmanı)

`StrataPlugin` ana `App` world'ünde oyun plugin'lerini ve cross-plugin bağımlılıklarını kaydeder. Voxel çekirdek ve GPU/Rapier SubApp burada **değildir**.

```rust
/// Tam oyun plugin grafiği — ana App (04 §3).
pub struct StrataPlugin;

impl Plugin for StrataPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            WorldGenPlugin,
            StreamingPlugin,
            LightingPlugin,   // CPU ışık; render pass SubApp'te (04 §2)
            NetworkPlugin,
            StoragePlugin,
            PlayerPlugin,
            EntityPlugin,
            AudioPlugin,      // şimdilik ana App (Audio SubApp ileride — 04 §1)
            UiPlugin,
            DebugPlugin,
        ));

        // Ortak resource'lar
        app.insert_resource(StrataConfig::default());
        app.insert_resource(WorldState::default());

        // Event kayıtları
        app.add_event::<ClientBlockBreakRequest>();
        app.add_event::<ServerBlockBrokenBroadcast>();

        // Network replikasyon kayıtları
        app.world_mut().resource_mut::<NetworkRegistry>()
            .replicate::<PlayerPosition>()
            .replicate::<PlayerOrientation>()
            .replicate::<PlayerVelocity>();

        // Client/Server message kayıtları
        app.world_mut().resource_mut::<MessageRegistry>()
            .register_client_message::<ClientBlockBreakRequest>(Channel::Ordered)
            .register_client_message::<ClientBlockPlaceRequest>(Channel::Ordered)
            .register_server_message::<ServerBlockBrokenBroadcast>(Channel::Unreliable)
            .register_server_message::<ServerBlockPlacedBroadcast>(Channel::Unreliable);
    }

    fn finish(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<XBrickMapPlugin>(),
            "StrataPlugin requires StrataCorePlugins (BlockRegistry, XBrickMap, SubApp)!"
        );
    }
}
```

### 3.4 Alt Plugin Örneği

```rust
/// Player plugin — oyuncu sistemlerini ana App'te kaydeder.
pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (
            input_system,
            player_movement_system.after(input_system),
            camera_system.after(player_movement_system),
            block_interaction_system.after(camera_system),
        ));

        // Oyuncu hareket ön-işleme — ana App FixedUpdate.
        // Rapier çözücü + çarpışma → Physics SubApp (StrataPhysicsPlugin, 04 §2).
        app.add_systems(FixedUpdate, (
            physics_movement,
            ground_check,
        ));
    }
}
```

---

## 4. Component Tasarımı

### 4.1 Player Components

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

impl Default for Inventory {
    fn default() -> Self {
        Self {
            slots: [None; 36],
            hotbar_index: 0,
        }
    }
}

/// Oyuncu game mode.
#[derive(Component)]
pub enum GameMode {
    Survival,
    Creative,
    Spectator,
    Adventure,
}

impl Default for GameMode {
    fn default() -> Self { Self::Survival }
}
```

### 4.2 Entity Components

```rust
/// Genel entity (mob, item, vehicle vb.).
#[derive(Component)]
pub struct GameEntity {
    pub entity_type: EntityType,
    pub health: u16,
    pub max_health: u16,
}

#[derive(Clone, Copy, Component)]
pub enum EntityType {
    Zombie, Skeleton, Creeper, Spider,
    Cow, Pig, Sheep, Chicken,
    Item, ExperienceOrb, Arrow, Minecart, Boat,
}

/// Entity AI state.
#[derive(Component)]
pub struct AiState {
    pub current_goal: Option<AiGoal>,
    pub target: Option<Entity>,
    pub path: SmallVec<[IVec3; 16]>,
    pub last_update: Instant,
}

#[derive(Clone, Component)]
pub enum AiGoal {
    Wander,
    Flee(Entity),
    Attack(Entity),
    Follow(Entity),
    Idle,
}
```

### 4.3 Render Components

```rust
/// Render edilebilir sector.
#[derive(Component)]
pub struct RenderableSector {
    pub vertex_slice: VertexSlice,
    pub visible: bool,
    pub frustum_culled: bool,
}

/// Kamera kontrol parametreleri.
#[derive(Component)]
pub struct CameraController {
    pub sensitivity: f32,
    pub smoothing: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self { sensitivity: 0.002, smoothing: 10.0 }
    }
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

### 4.4 Network Components

```rust
/// Network entity — replicon ile sync edilir.
#[derive(Component)]
pub struct Networked;

/// Network client bilgisi.
#[derive(Component)]
pub struct NetworkClientInfo {
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

// ### Network Chunk Data Prensibi
//
// Ham chunk/Sector verisi asla standart replication ile gönderilmemelidir.
// Bunun yerine:
//   - BrickDelta gibi özel RPC/kanal üzerinden gönderilmeli
//   - SVDAG snapshot sync kendi kanalında olmalı
//   - Sadece metadata (TierChange, ChunkDirty vb.) replication üzerinden gönderilmeli

#[derive(Clone)]
pub struct EntityState {
    pub position: Vec3,
    pub orientation: Quat,
    pub velocity: Vec3,
    pub timestamp: Instant,
}
```

### 4.5 Component Tasarım Optimizasyonları

#### Hot/Cold Data Split (SoA within Components)

Sık erişilen veriler ile nadir erişilen verileri ayrı component'lara ayır:

```rust
// SICAK: Her frame sorgulanır (movement, rendering)
#[derive(Component)]
pub struct SectorTransform {
    pub position: Vec3,
    pub tier: Tier,
}

// SOĞUK: Sadece debug veya nadir durumlarda erişilir
#[derive(Component)]
pub struct SectorMetadata {
    pub created_at: Instant,
    pub last_modified: Instant,
    pub source: SectorSource,
    pub debug_name: String,
}
```

**Etki:** Hot component'lar daha sıkı paketlenir, cache utilization artar. Sadece hot path'i sorgulayan sistemler cold data'yı atlar.

#### ZST Marker Component'lar (Sıfır Maliyetli Filtreleme)

Zero-sized type (ZST) marker'lar tabloda 0 byte kaplar ve archetype-level filtering sağlar:

```rust
#[derive(Component)] // Zero-sized, tabloda 0 byte
pub struct ChunkVisible;

#[derive(Component)]
pub struct InPlayerRange;

#[derive(Component)]
pub struct Frozen; // Fizik devre dışı

// Query sadece matching archetype'ları tarar — per-entity maliyet yok
fn remesh_visible_dirty(
    query: Query<&SectorData, (With<ChunkVisible>, With<NeedsRemesh>)>,
) { ... }
```

#### Immutable Component'lar (Bevy 0.18+)

Değiştirilemez component'lar sessiz mutasyonu engeller, observer/hook'lar her değişimi yakalar:

```rust
#[derive(Component)]
#[component(immutable)]
pub struct SectorCoord(pub IVec3); // Sadece insert'te set edilir

#[derive(Component)]
#[component(immutable)]
pub struct ChunkId(pub u64);

// "Değiştirmek" için insert ile yeni değer verilmeli → Remove + Insert tetiklenir (Bevy 0.17+: OnRemove→Remove, OnInsert→Insert)
```

#### Disabled Component (Bevy 0.18+)

Uzaktaki chunk'ları despawn etmeden devre dışı bırak — veri korunur, yaklaşında tekrar etkinleştirilir:

```rust
// Uzak chunk: physics, AI, movement query'lerinden otomatik elenir
commands.entity(chunk_entity).insert(Disabled);

// Yaklaştığında tekrar etkinleştir
commands.entity(chunk_entity).remove::<Disabled>();

// Bölge bazlı toplu devre dışı bırakma
commands.entity(region_entity).insert_recursive(Disabled);
```

**Etki:** Despawn/spawn döngüsü ortadan kalkar, chunk verisi korunur.

---

## 5. Sistem Mimarisi

### 5.1 Sistem Setleri (Modüler)

Her plugin kendi sistem set'ini tanımlar:

```rust
/// Player plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlayerSystems {
    Input,
    Movement,
    Inventory,
}

/// World plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum WorldSystems {
    Streaming,
    Generation,
    Editing,
}

/// Physics plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum PhysicsSystems {
    Update,
    Collision,
    Character,
}

/// Entity plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum EntitySystems {
    Ai,
    Update,
    Spawn,
    Despawn,
}

/// Lighting plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum LightingSystems {
    Update,
    Bake,
}

/// Render plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum RenderSystems {
    Prepare,
    Culling,
    Meshing,
    Submit,
}

/// Network plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum NetworkSystems {
    Receive,
    Sync,
    Send,
}

/// Storage plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum StorageSystems {
    Flush,
    Gc,
}

/// Debug plugin sistem setleri.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum DebugSystems {
    Render,
    Metrics,
}
```

### 5.2 Sistem Sıralaması

**Prensip:** Her plugin kendi iç set sıralamasını kendi `build()` içinde tanımlar. Cross-plugin bağımlılıklar yalnızca `StrataPlugin::build` içinde configure edilir. Voxel bootstrap sırası (`StrataSets`: Input → WorldGen → …) → `04-plugin-api.md` §4.B; fizik SubApp sırası (`StrataPhysicsSets`) → `04` §4.A.

```rust
/// Player plugin — kendi iç sıralamasını kendi configure eder.
impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (
            PlayerSystems::Input,
            PlayerSystems::Movement.after(PlayerSystems::Input),
            PlayerSystems::Inventory,
        ));

        app.add_systems(Update, (
            input_system.in_set(PlayerSystems::Input),
            player_movement_system.in_set(PlayerSystems::Movement),
            inventory_system.in_set(PlayerSystems::Inventory),
        ));
    }
}

/// Ana plugin — tüm cross-plugin bağımlılıklar burada.
impl Plugin for StrataPlugin {
    fn build(&self, app: &mut App) {
        // Cross-plugin bağımlılıklar
        app.configure_sets(Update, (
            NetworkSystems::Receive,
            NetworkSystems::Sync.after(NetworkSystems::Receive),
            NetworkSystems::Send.after(PhysicsSystems::Character),
        ));
    }
}
```

---

## 6. Örnek Sistemler

### 6.1 Player Movement

```rust
/// Oyuncu hareket sistemi.
fn player_movement_system(
    time: Res<Time>,
    input: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &mut PlayerVelocity, &PlayerOrientation, &GameMode)>,
) {
    let dt = time.delta_secs();

    for (mut transform, mut vel, orient, game_mode) in query.iter_mut() {
        let mut input_dir = Vec3::ZERO;

        if input.pressed(KeyCode::KeyW) { input_dir.z -= 1.0; }
        if input.pressed(KeyCode::KeyS) { input_dir.z += 1.0; }
        if input.pressed(KeyCode::KeyA) { input_dir.x -= 1.0; }
        if input.pressed(KeyCode::KeyD) { input_dir.x += 1.0; }

        if input_dir.length_squared() > 0.0 {
            input_dir = input_dir.normalize();
        }

        let forward = orient.mul_vec3(Vec3::NEG_Z);
        let right = orient.mul_vec3(Vec3::X);
        let move_dir = forward * input_dir.z + right * input_dir.x;

        vel.0.x = move_dir.x * 5.0;
        vel.0.z = move_dir.z * 5.0;
        vel.0.y -= 20.0 * dt;
        transform.translation += vel.0 * dt;
    }
}
```

### 6.2 World Streaming

```rust
/// World streaming sistemi — tier güncelleme.
fn world_streaming_system(
    camera: Res<CameraState>,
    mut commands: Commands,
    mut query: Query<(Entity, &SectorEntity, &mut TierChange)>,
) {
    let camera_pos = camera.position;

    for (entity, sector, mut tier_change) in query.iter_mut() {
        let sector_center = sector.coord.world_origin().as_vec3() + Vec3::new(16.0, 64.0, 16.0);
        let dist = (sector_center - camera_pos).length();

        let new_tier = determine_tier(dist);

        if new_tier != sector.tier {
            commands.entity(entity).insert(TierChange {
                old_tier: sector.tier,
                new_tier,
                timestamp: Instant::now(),
            });
        }
    }
}
```

### 6.3 Entity AI

```rust
/// Entity AI sistemi — 200ms throttling ile.
fn entity_ai_system(
    mut query: Query<(Entity, &mut AiState, &GameEntity)>,
    players: Query<&PlayerPosition>,
) {
    for (_entity, mut ai, _entity_data) in query.iter_mut() {
        if ai.last_update.elapsed() < Duration::from_millis(200) {
            continue;
        }

        ai.last_update = Instant::now();

        let nearest_player = players.iter()
            .min_by(|a, b| {
                let dist_a = a.0.length_squared();
                let dist_b = b.0.length_squared();
                dist_a.partial_cmp(&dist_b).unwrap()
            });

        if let Some(player_pos) = nearest_player {
            let dist = player_pos.0.length();

            ai.current_goal = if dist < 20.0 {
                Some(AiGoal::Attack(_entity))
            } else if dist < 40.0 {
                Some(AiGoal::Wander)
            } else {
                Some(AiGoal::Idle)
            };
        }
    }
}
```

### 6.4 Network Message Handler

```rust
/// Client'tan gelen blok kırma isteğini işle (Server-side).
fn handle_block_break_request(
    mut reader: MessageReader<ClientBlockBreakRequest>,
    mut writer: MessageWriter<ServerBlockBrokenBroadcast>,
) {
    for request in reader.read() {
        // Yetki kontrolü
        if !is_authorized(request.client_id, &request.pos) {
            continue;
        }

        // Blok kır
        break_block(world, &request.pos);

        // Tüm client'lara yayınla
        writer.send(ServerBlockBrokenBroadcast {
            pos: request.pos,
            block_id: request.block_id,
        });
    }
}
```

### 6.5 Dirty Flag Pattern (Doğrudan Mutasyon + Toplu İşlem)

```rust
/// Blok değiştirme — doğrudan mutasyon, event yok.
/// TNT patlaması, world edit, fluid simülasyonu gibi durumlar için.
fn modify_blocks_directly(
    mut commands: Commands,
    mut query: Query<(Entity, &SectorEntity, &mut SectorData, &mut SectorMeshState)>,
    changes: &[(IVec3, u16)],
) {
    for (entity, _sector, mut data, mut mesh_state) in query.iter_mut() {
        let mut new_data = CompressedChunkData::clone(&data.0);
        for &(pos, block_id) in changes {
            new_data.set_block(pos, block_id);
        }
        data.0 = Arc::new(new_data);
        mesh_state.data_version += 1;

        commands.entity(entity).insert(ChunkDirty);
        commands.entity(entity).insert(NeedsRemesh);
    }
}

/// Tüm kirli chunk'ları topluca işleyen sistem.
fn process_dirty_chunks(
    mut commands: Commands,
    dirty_query: Query<Entity, With<ChunkDirty>>,
    collider_query: Query<Entity, With<NeedsColliderUpdate>>,
) {
    for entity in dirty_query.iter() {
        // Remesh tetikle
        // ...
        commands.entity(entity).remove::<ChunkDirty>();
    }

    for entity in collider_query.iter() {
        // Physics collider güncelle
        // ...
        commands.entity(entity).remove::<NeedsColliderUpdate>();
    }
}
```

---

## 7. Resource'lar

```rust
/// Hava durumu.
#[derive(Resource)]
pub struct WeatherState {
    pub current: WeatherType,
    pub transition_timer: f32,
    pub intensity: f32,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            current: WeatherType::Clear,
            transition_timer: 0.0,
            intensity: 1.0,
        }
    }
}

#[derive(Clone, Copy)]
pub enum WeatherType {
    Clear, Rain, Thunderstorm, Snow, Blizzard,
}

#[derive(Clone, Copy)]
pub enum RenderQuality {
    Low, Medium, High, Ultra, RayTraced,
}

/// Kamera durumu.
#[derive(Resource)]
pub struct CameraState {
    pub position: Vec3,
    pub orientation: Quat,
    pub fov: f32,
}
```

---

## 8. Event ve Message Sistemi

### 8.1 Event/Message Ayrımı

| Trait | Kullanım | Okuyucu | Yazıcı | Örnek |
|-------|----------|---------|--------|-------|
| `Event` | Buffered, network-safe | `MessageReader<T>` | `MessageWriter<T>` | Network istekleri |
| `Event` | Local, immediate | `MessageReader<T>` | `MessageWriter<T>` | UI callback, entity lifecycle |

**Kural:** Network üzerinden iletilen her şey `Event` trait implement etmelidir. Yerel olaylar da `Event` kullanır.

### 8.2 Message Ayrıştırma Prensibi

Network mesajları **yön belirtmelidir**:

| Yön | İsimlendirme | Örnek |
|-----|-------------|-------|
| Client → Server | `Client*Request` | `ClientBlockBreakRequest` |
| Server → Client | `Server*Broadcast` | `ServerBlockBrokenBroadcast` |
| Server → Tek Client | `Server*Response` | `ServerInventoryResponse` |
| Yerel (UI, ses) | `Local*` | `LocalBlockPlaced` |

### 8.3 Local Event'ler

```rust
/// Yerel event'ler — ağ üzerinden iletilmez.
#[derive(Event, Clone)]
pub enum LocalEvent {
    SectorLoaded { coord: SectorCoord },
    SectorUnloaded { coord: SectorCoord },
    WeatherChanged { new_weather: WeatherType },
    DayChanged { day: u32 },
    UiRefresh,
}
```

### 8.4 Network Message'lar

```rust
/// Client → Server message'lar.
#[derive(Event, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    BlockBreak { pos: IVec3, block_id: u16 },
    BlockPlace { pos: IVec3, block_id: u16 },
    ChatMessage { text: String },
    Interact { entity: Entity },
}

/// Server → Client message'lar.
#[derive(Event, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    BlockBroken { pos: IVec3, block_id: u16 },
    BlockPlaced { pos: IVec3, block_id: u16 },
    EntitySpawned { entity_type: EntityType, pos: Vec3 },
    EntityDespawned { entity: Entity },
    ChatBroadcast { sender: String, text: String },
    PlayerJoined { client_id: ClientId, name: String },
    PlayerLeft { client_id: ClientId },
    EntityDied { entity: Entity, killer: Option<Entity> },
}
```

### 8.5 Bulk Message'lar (Performans)

```rust
/// Toplu blok değişim — TNT patlaması, WorldGen gibi durumlarda.
#[derive(Event, Clone)]
pub struct SectorRegionModified {
    pub coord: SectorCoord,
    pub bounds: IVec3,
    pub changes: SmallVec<[(IVec3, u16); 64]>,
}
```

---

## 9. Performans ve Ölçeklendirme Prensipleri

### 9.1 Archetype Fragmentation'ı Önleme

- `TierChange`, `ChunkDirty`, `NeedsRemesh` gibi **geçici component'lar SparseSet** olarak saklanmalıdır (`#[component(storage = "SparseSet")]`).
- **Chunk entity'lerinin component set'i mümkün olduğunca uniform olmalıdır.**

| Senaryo | Depolama |
|---------|----------|
| Nadiren eklenir/kaldırılır, sık sorgulanır | **Archetype** (varsayılan) |
| Her frame veya çok sık eklenir/kaldırılır | **SparseSet** |
| Az sayıda entity'de bulunur / geçici etki | **SparseSet** |
| Maksimum sorgu hızı gerekli | **Archetype** |

### 9.2 Event Tıkanıklığını Önleme

- Tekil blok event'ları sadece oyuncu eliyle yapılan kırmalarda kullanılır.
- Toplu değişimler için `SectorRegionModified` gibi "Bulk" event'lar yollanır.

### 9.3 Change Detection

Bevy'nin `Changed<T>` ve `Added<T>` filtreleri ile otomatik değişim tespiti:

```rust
// Kötü: Her frame kontrol et
fn bad_system(query: Query<&Health>) {
    for health in query.iter() {
        update_display(health); // Her frame çalışır
    }
}

// İyi: Sadece değiştiyse kontrol et
fn good_system(query: Query<&Health, Changed<Health>>) {
    for health in query.iter() {
        update_display(health); // Sadece değiştiğinde çalışır
    }
}
```

### 9.4 Sistem Tasarımı

- **`.run_if()` ile** sistemlerin gereksiz çalışması önlenir.
- **Event-only sistemler:** Sadece event'a tepki veren sistemlerde boş event kontrolü yapılır.

```rust
// Örnek: Sadece hava durumu değiştiğinde çalış
fn update_weather_effects(
    mut reader: MessageReader<WeatherChanged>,
    // ...
) {
    if reader.read().next().is_none() {
        return; // Event yoksa hemen dön
    }
    // ...
}
```

### 9.5 Task Pool Konfigürasyonu

```rust
/// Bevy task pool konfigürasyonu.
fn configure_task_pools(app: &mut App) {
    // Bevy'nin dahili task pool'u
    // AsyncComputeTaskPool — chunk generation, mesh generation
    // ComputeTaskPool — lighting, pathfinding
    // IoTaskPool — network, disk I/O
}
```

### 9.6 SmallVec ile Heap Allocation Azaltma

```rust
// Tipik AI pathfinding: 16 node'a kadar stack'de.
#[derive(Component)]
pub struct AiPath {
    pub waypoints: SmallVec<[IVec3; 16]>,
}

// Küçük durum efektleri: 8'e kadar stack'de.
#[derive(Component)]
pub struct ActiveEffects {
    pub effects: SmallVec<[StatusEffect; 8]>,
}
```

---

## 10. Sektörel Dersler ve İleri Optimizasyonlar

### 10.1 Palette Compression Hayatidir

- Veriler her zaman Palette-Compressed olarak tutulmalıdır.
- `SectorData(Arc<CompressedChunkData>)` bu yapıyı sarmalar (`06-xbrickmap.md` §1.4).

### 10.2 Client / Server Crate İzolasyonu

- **Client:** Sadece `Core` verilerini okur, Render ve lokal fizik tahmini yürütür.
- **Server:** Fizik, Voxel üretimi ve doğrulama. Headless çalışabilmelidir.
- **Core:** Ortak veri tipleri, event'lar, component'lar.

### 10.3 Multiplayer Mimari

> **Oyuncu sistemleri asla doğrudan dünya component'larını mutasyona uğratmamalı.** Tüm oyuncu etkileşimleri Event/Message sisteminden geçmelidir.

```rust
// 1. Aksiyon event'ları tanımla
#[derive(Event, Clone, Serialize, Deserialize)]
struct BuildAction { position: IVec3, block_id: u16 }

// 2. Client: Sadece event yazar
fn client_build_input(
    input: Res<ButtonInput<MouseButton>>,
    mut writer: MessageWriter<BuildAction>,
) {
    if input.just_pressed(MouseButton::Left) {
        writer.send(BuildAction {
            position: calculate_place_pos(),
            block_id: get_selected_block(),
        });
    }
}

// 3. Server: Event'ı alır, doğrular, mutasyon yapar
fn server_handle_build(
    mut reader: MessageReader<BuildAction>,
    // ...
) {
    for action in reader.read() {
        if is_authorized(&action.position) {
            place_block(&action.position, action.block_id);
        }
    }
}
```

### 10.4 Performans Kontrol Listesi

| Konu | Uygulama |
|------|----------|
| Archetype fragmentation | Geçici component'larda `#[component(storage = "SparseSet")]` |
| Uniform chunk component set | Tüm chunk entity'leri aynı component set'ine sahip |
| Event tıkanıklığı | Bulk event'lar + dirty flag pattern |
| Dirty flag pattern | Toplu blok değişimlerinde doğrudan mutasyon + ChunkDirty |
| Scheduler overhead | İlgili küçük sistemleri birleştir |
| Chunk bellek | Arc<> arkasında, palette-compressed |
| Multi-threaded meshing | Arc<CompressedChunkData> ile lock-free paylaşım |
| Async entity spawning | Bevy's `spawn_batch` |
| AI pathfinding | SmallVec<[IVec3; 16]> / Box<[IVec3]> |
| Network chunk data | Özel kanal (BrickDelta, SVDAG snapshot) |
| Client-side prediction | Optimistic block placement + movement prediction |
| Error handler | Production'da `DefaultErrorHandler` (Bevy 0.18+) |
| Heap allocation | Küçük koleksiyonlarda SmallVec |
| Change detection | `Changed<T>`, `Added<T>` filtreleri |
| Otomatik paralel | Bağımsız sistemler Bevy scheduler tarafından paralel çalıştırılır |
| Hot/Cold data split | Sık erişilen component'ları nadir erişilenlerden ayır |
| ZST marker component'lar | `With<T>` ile sıfır maliyetli archetype filtreleme |
| Immutable component'lar | `#[component(immutable)]` ile sessiz mutasyon engeli |
| Disabled component | Uzak chunk'ları despawn etmeden devre dışı bırak |
| ComponentHooks lifecycle | Otomatik spatial index bakım (on_add/on_remove) |
| Filter-first query design | Archetype-level filtering, per-entity check yok |
| Change detection guard | `if new_val != *comp { *comp = new_val; }` |
| Minimum ordering constraint | System set'leri ile hiyerarşik sıralama, paralelizm最大化 |
| Run condition | `.run_if()` ile boş çalışan sistemleri atla |
| Exclusive system bulk spawn | 1000+ chunk spawn'da commands overhead yok |
| Block entity pattern | Per-block entity felaketi önlemi |
| Component pre-register | Bevy 0.16.x `get` regresyonu (#19504); Strata 0.18+ — startup'ta kayıt |

### 10.5 Component Lifecycle Hooks

Chunk entity'lerinin otomatik spatial index bakımını ComponentHooks ile sağla:

```rust
app.world_mut().register_component_hooks::<SectorData>()
    .on_add(|mut world, ctx| {
        // Yeni chunk otomatik olarak spatial index'e eklenir
        let coord = world.get::<SectorCoord>(ctx.entity).unwrap();
        world.resource_mut::<SectorMap>().insert(coord.0, ctx.entity);
    })
    .on_remove(|mut world, ctx| {
        // Chunk kaldırıldığında spatial index'ten otomatik silinir
        let coord = world.get::<SectorCoord>(ctx.entity).unwrap();
        world.resource_mut::<SectorMap>().remove(coord.0);
    });
```

**Hooks vs Observers:**

| Özellik | Hooks | Observers |
|---------|-------|-----------|
| Çalışma | Senkron, inline | Ertelenebilir |
| Erişim | `DeferredWorld` (sınırlı) | Tam system parametreleri |
| Kapsam | Per-component type | Per-event veya per-entity |
| Kullanım | Structural invariants | Oyun mantığı, reaksiyonlar |
| Performans | Minimal overhead | Biraz daha fazla overhead |

### 10.6 Block Entity Pattern (Blok Entity Tasarımı)

Bireysel bloklar ASLA ECS entity olmamalı. Voxel geometri + meshing/network’te görünen state **SectorPalette** üzerinden tutulur (Bkz. 05-block-registry.md §10, 06-xbrickmap.md §2.1). ECS BlockEntity yalnızca **palete sığmayan veya seyrek** veri içindir.

```rust
// BlockRegistry block_type_id — SectorPalette çözümlemesinin çıktısı (u16).
// Voxel bellekte u8 local index; bu tip doğrudan chunk array'inde YOK.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BlockType(pub u16);

// Sadece "extended" state gerektiren pozisyonlar (seyrek HashMap)
#[derive(Component)]
pub struct BlockEntity {
    pub sector: IVec3,
    pub local_pos: UVec3,
}

#[derive(Component)]
pub struct ChunkBlockEntities {
    pub entities: HashMap<UVec3, BlockEntityData>,
}

pub enum BlockEntityData {
    Chest { inventory: [Option<Item>; 27] },
    Furnace { progress: f32, fuel_remaining: u16 },
    Sign { lines: [StringId; 4] },
    /// Repeater/comparator gibi zamanlayıcı + mod state
    RedstoneRepeater { delay_ticks: u8, locked: bool },
}
// NOT: Door open/facing, wire power 0-15, stair half → SectorPalette.variant (05 §10.5)
```

### 10.6.1 State Ownership (Palet vs BlockEntity) — Kesinleşmiş

| Veri | Depolama | Örnek | Neden |
|------|----------|-------|-------|
| Block type + meshing state | `SectorPalette` → `PaletteEntry { block_type, variant }` | Kapı açık/kapalı, facing, merdiven half, redstone gücü 0–15 | O(1) voxel okuma, remesh, network delta ile uyumlu |
| Büyük / seyrek / envanter | `ChunkBlockEntities` | Sandık 27 slot, fırın ilerleme, tabela metni | Palet 256 entry ve 4–8 byte/voxel bütçesine sığmaz |
| Statik özellikler | `BlockRegistry` (immutable) | hardness, flags, ses | Init-only SoA |

**Kurallar:**
1. Yeni blok state’i eklerken önce TOML `states` + `variant` dene; yalnızca palet yetersizse `BlockEntityData` variant’ı ekle.
2. Aynı property **iki yerde tutulmaz** (ör. `Door { facing }` entity’de **yasak** — sadece palette).
3. Redstone tel gücü: `variant` (0–15); repeater gecikmesi: `BlockEntityData::RedstoneRepeater`.
4. Block entity sistemleri `Query<&ChunkBlockEntities>` ile seyrek iterate eder; tüm sektörü taramaz.

```rust
fn update_redstone_repeaters(
    mut blocks: Query<(&SectorPosition, &mut ChunkBlockEntities)>,
    sectors: Query<&SectorPalette>,
    registry: Res<BlockRegistry>,
) {
    for (pos, mut block_entities) in blocks.iter_mut() {
        let palette = /* sector palette for pos.0 */;
        for (local, data) in block_entities.entities.iter_mut() {
            if let BlockEntityData::RedstoneRepeater { delay_ticks, locked } = data {
                // Komşu güç: palette.resolve(local) → variant
            }
        }
    }
}
```

**Neden per-block entity felaket?** 32³ voxel × entity = milyonlarca archetype — overhead felaket. Hibrit modelde entity sayısı sandık/fırın/tabela ile sınırlı kalır.

### 10.7 Bevy 0.18+ Uygulama Notları (Doğrulandı)

#### Component erişimi ve kayıt (#19504 — tarihsel)

Bevy **0.16.0**'da kayıtsız component için `EntityWorldMut::get` yolunda **~11×** regresyon raporlandı ([bevy#19504](https://github.com/bevyengine/bevy/issues/19504)); kısmi düzeltmeler **0.16.2+** ([PR #19510](https://github.com/bevyengine/bevy/pull/19510)). Strata hedefi **0.18+** — yine de:

- Tüm Strata component'lerini **startup'ta** (dummy spawn veya `register_component`) kaydet.
- Hot path'te `world.get` yerine **`Query` / `QueryState`** kullan.
- Per-frame `get` ile structural invariant kurma — bunun yerine **ComponentHooks** (§10.5).

#### Transform ve ilişkiler (0.18)

Transform dirty-bit propagation ve `ChildOf` relationship API'si 0.16+ ile geldi; 0.18'de kullanılmaya devam edilir. Sector/chunk hiyerarşisi varsa transform maliyeti otomatik düşer.

#### Relationships

`ChildOf` relationship component'i eski `Parent`/`Children` sisteminin yerini aldı. Child ekleme constant-time:

```rust
commands.spawn((Player, children![
    (RightHand, children![Sword, Shield]),
    (LeftHand, children![Glove]),
]));
```

### 10.8 Test Stratejisi

**Level 1: Unit Tests**
```rust
#[test]
fn xbrickmap_set_get_roundtrip() {
    let mut map = XBrickMap::new();
    map.set_block(IVec3::new(0, 0, 0), 42);
    assert_eq!(map.get_block(IVec3::new(0, 0, 0)), 42);
}
```

**Level 2: System Tests**
```rust
#[test]
fn player_movement_system_applies_velocity() {
    let mut app = App::new();
    app.add_systems(Update, player_movement_system);

    let entity = app.world_mut().spawn((
        Player { name: "Test".into(), client_id: None },
        Transform::default(),
        PlayerVelocity(Vec3::new(1.0, 0.0, 0.0)),
        PlayerOrientation(Quat::IDENTITY),
    )).id();

    app.update();

    let pos = app.world().get::<Transform>(entity).unwrap();
    assert!(pos.translation.x > 0.0, "Player should have moved");
}
```

**Level 3: Integration Tests**
```rust
#[test]
fn strata_plugin_loads_without_panic() {
    let mut app = App::new();
    app.add_plugins(StrataCorePlugins);
    app.add_plugins(StrataPlugin);
    app.update(); // Tek frame çalıştır
}
```

**Level 4: Determinism Tests**
```rust
#[test]
fn world_generation_deterministic() {
    let seed = WorldSeed(12345);
    let world_a = generate_world(seed, IVec2::ZERO);
    let world_b = generate_world(seed, IVec2::ZERO);
    assert_eq!(world_a, world_b, "Same seed must produce same world");
}
```

### 10.9 Multi-Threaded Meshing (Arc ile Lock-Free Paylaşım)

```
Ana Thread (ECS)              Meshing Thread
     |                              |
     |  Arc::clone(&chunk_data)     |
     |----------------------------->|
     |                              | mesh_sector(&data)
     |  channel: MeshResult         |
     |<-----------------------------|
     |                              |
     | mesh component güncelle      |
```

```rust
/// Sistem: Mesh isteklerini gönder.
fn submit_mesh_requests(
    query: Query<(&SectorData, &SectorMeshState, &SectorEntity, &NeedsRemesh)>,
    meshing_queue: Res<MeshingQueue>,
) {
    for (data, mesh_state, _sector, _needs_remesh) in query.iter() {
        let request = MeshRequest {
            sector_coord: _sector.coord,
            data: Arc::clone(&data.0),
            version: mesh_state.data_version,
        };
        if meshing_queue.sender.try_send(request).is_ok() {
            // pending_mesh_version güncelle (mutability gerektirir)
        }
    }
}
```

### 10.10 Determinizm (Fizik ve Multiplayer)

**Strata için önerilen: Authoritative Server + Client-Side Prediction**

| Yaklaşım | Artı | Eksi | Öneri |
|----------|------|------|-------|
| Authoritative server | Determinizm gereksiz, en basit | Yüksek bant genişliği | **Seçilen** |
| Rapier `enhanced-determinism` | Üretime hazır | Performans maliyeti | Gerekirse |

### 10.11 Async Entity Spawning

```rust
// Toplu oluşturma — Bevy spawn_batch ile
fn spawn_generated_sectors(
    mut commands: Commands,
    sectors: Vec<GeneratedSector>,
) {
    let entities: Vec<_> = sectors.into_iter().map(|sector| {
        (
            SectorEntity { coord: sector.coord, tier: Tier::Active },
            SectorData(Arc::new(sector.compressed)),
            Transform::default(),
            Visibility::default(),
        )
    }).collect();

    commands.spawn_batch(entities);
}
```

---

## 11. Bevy ECS API Referansı

### 11.1 Core API

| Fonksiyon | Açıklama |
|-----------|----------|
| `App::new()` | Yeni app oluştur |
| `commands.spawn(components)` | Entity oluştur |
| `commands.entity(e).despawn()` | Entity'yi kaldır |
| `commands.entity(e).insert(comp)` | Component ekle |
| `commands.entity(e).remove::<T>()` | Component kaldır |
| `query.iter()` | Query iterator |
| `query.iter_mut()` | Mutable query iterator |
| `query.get(entity)` | Tek entity component'ı |
| `app.insert_resource(res)` | Resource ekle |
| `world.resource::<T>()` | Resource referansı al |
| `world.resource_mut::<T>()` | Mutable resource referansı al |
| `writer.send(event)` | Event yaz |
| `reader.read()` | Event'leri oku |
| `commands.entity(e).insert(comp)` | Deferred component ekle |
| `commands.entity(e).remove::<T>()` | Deferred component kaldır |

### 11.2 App API

| Fonksiyon | Açıklama |
|-----------|----------|
| `App::new()` | Yeni app oluştur |
| `app.add_plugins(plugins)` | Plugin ekle |
| `app.insert_resource(res)` | Resource ekle |
| `app.add_event::<T>()` | Event tipi kaydet |
| `app.add_systems(schedule, systems)` | System ekle |
| `app.configure_sets(schedule, sets)` | System set bağımlılıkları |
| `app.run()` | Uygulamayı başlat |

### 11.3 System Parametreleri

| Parametre | Açıklama |
|-----------|----------|
| `Query<...>` | Component sorgulama |
| `Res<T>` | Immutable resource erişimi |
| `ResMut<T>` | Mutable resource erişimi |
| `Commands` | Deferred mutations |
| `MessageWriter<T>` | Event yazma (Bevy 0.17+: EventWriter → MessageWriter) |
| `MessageReader<T>` | Event okuma (Bevy 0.17+: EventReader → MessageReader) |
| `Local<T>` | System-local state |
| `In<T>` | Input parametresi (pipe) |

---

## 12. Araştırma Doğrulamaları ve Öneriler (2026-06)

> **Kaynak:** 5 worker ile 40+ WebSearch sorgusu, SIGGRAPH/akademik paper'lar, Bevy ekosistem kaynakları.

### 12.1 Doğrulanan Kararlar

| Karar | Doğrulama |
|-------|-----------|
| Archetype-based ECS | SAC 2026 benchmark: ~10× faster than OOP |
| Filter-First tasarım | Academic validation — archetype-level filtering O(archetypes) |
| ZST marker component'lar | Zero-cost filtering, archetype bitmask O(1) |
| SparseSet geçici component'lar | Bevy best practice, archetype move eliminasyonu |
| Change detection guard | `if old_val != new_val` pattern — validated |

### 12.2 Bevy 0.17+ API Değişiklikleri (Kesin)

Bevy 0.17+ ile birlikte aşağıdaki API isim değişiklikleri yapılmıştır. **Tüm plan dosyaları ve kod örnekleri bu yeni terminolojiyi kullanmalıdır.**

| Eski (≤0.16) | Yeni (0.17+) | Strata Etkisi |
|---|---|---|
| `EventWriter` | **`MessageWriter`** | Tüm sistem kodları güncellendi |
| `EventReader` | **`MessageReader`** | Tüm sistem kodları güncellendi |
| `OnAdd` / `OnRemove` | **`Add`** / **`Remove`** / **`Insert`** / **`Replace`** / **`Despawn`** | Lifecycle hook isimleri |
| `Trigger<E>` | **`On<E>`** | Observer parametre değişimi |

**Not:** `Event` trait ve `app.add_event::<T>()` aynı kalır; sadece reader/writer isimleri değişti.

### 12.3 P0 — Change Detection Optimizasyonları

#### `set_if_neq()` Kullanımı

Bevy 0.18+ `set_if_neq()` methodu, değer gerçekten farklıysa change detection tetikler. Guard pattern'in built-in versiyonu:

```rust
// Eski (manuel guard):
fn update_health(mut query: Query<&mut Health>) {
    for mut health in query.iter_mut() {
        let new_hp = calculate_new_hp(&health);
        if new_hp != health.current {
            health.current = new_hp;
        }
    }
}

// Yeni (set_if_neq):
fn update_health(mut query: Query<&mut Health>) {
    for mut health in query.iter_mut() {
        let new_hp = calculate_new_hp(&health);
        health.set_if_neq(Health { current: new_hp, ..*health });
    }
}
```

**Kullanım alanları:** Render SubApp'te transform sync, physics write-back, UI state updates.

#### `bypass_change_detection()` Kullanımı

Render/Physics SubApp'te extract fonksiyonlarında change detection tetiklemek gereksiz overhead yaratır:

```rust
// SubApp extract — change detection bypass
fn render_extract(main_world: &mut World, render_world: &mut World) {
    let mut query = main_world.query_filtered::<&Transform, With<RenderSync>>();
    for transform in query.iter(main_world) {
        // Bypass: render world'de change detection gereksiz
        render_world.entity_mut(entity).bypass_change_detection().insert(*transform);
    }
}
```

**Kullanım alanları:** SubApp extract fonksiyonları, debug overlay updates, metrics collection.

### 12.4 P2 — SparseSet Kaldırma Riski

Bevy Discussion [#19164](https://github.com/bevyengine/bevy/discussions/19164): SparseSet storage'nin geleceği belirsiz. **Strata stratejisi:**

- **Phase 1:** Mevcut `#[component(storage = "SparseSet")]` pattern'ini koru
- **Phase 3:** Bevy kararını takip et; gerekirse archetype storage'ya geç
- **Risk mitigation:** SparseSet kullanan component'lar (`ChunkDirty`, `NeedsRemesh`) zaten ZST — storage type değişikliği minimal etki

### 12.5 Observer Sıralama Uyarısı

Observer sıralama garanti değildir. Sıralama-kritik logic için:

- **Scheduler + one-shot system** kullan (Observer değil)
- **EventReader zinciri** ile batch processing (sıralama garantili)
- Observer sadece "fire and forget" reaksiyonlar için
