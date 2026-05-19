# Project Design: Strata (Rust Voxel Engine)

## 1. Proje Özeti

Rust ile sıfırdan geliştirilecek, performans ve optimizasyon odaklı, Windows platformuna özel bir voksel oyun motoru. Hedef: 100+ chunk görüş mesafesi, 60+ FPS, binlerce oyuncu destekleyen headless sunucu mimarisi.

---

## 2. Teknoloji Yığını

### 2.1. Çekirdek

| Alan | Teknoloji | Versiyon | Neden |
|------|-----------|----------|-------|
| Dil | Rust | 2024 Edition | Bellek güvenliği, fearless concurrency, GC yok |
| Build System | Cargo (Workspace) | - | Multi-crate yapısı, modüler derleme |
| Async Runtime | tokio | 1.x | Mature, geniş ecosystem, renet2 ile uyumlu |

### 2.2. ECS & Veri Yönetimi

| Alan | Teknoloji | Versiyon | Neden |
|------|-----------|----------|-------|
| ECS Framework | Bevy ECS | 0.18+ | En mature Rust ECS, Wasvy modding desteği, parallel system execution |
| Math Library | glam | 0.29+ | SIMD optimized, Bevy ile native uyumlu, mathbench'te en hızlı |
| Serialization (Disk) | rkyv | 0.8+ | Zero-copy deserialization, chunk disk I/O için kritik |
| Serialization (Network) | postcard | 1.1+ | Bevy_replicon ile native uyumlu, küçük binary footprint, no_std |
| Chunk Voxel Data | `Vec<u16>` (flat array) | - | ndarray overkill; flat array CPU dostu, SIMD-friendly, cache-optimal |
| Noise Generation | fastnoise2 | 0.4+ | Modern, SIMD-optimized, FBM desteği, node-graph tabanlı kompozisyon. C++ build dependency kabul edildi (performans > pure Rust) |

### 2.3. Render Pipeline

| Alan | Teknoloji | Versiyon | Neden |
|------|-----------|----------|-------|
| Grafik API | wgpu | 29+ | Vulkan/DX12/Metal abstraction, WebGPU standardı |
| Window/Input | winit | 0.30+ | Cross-platform window management (Windows öncelikli) |
| Font/Text | glyphon | 0.12+ | GPU-accelerated text rendering, cosmic-text 0.18+ tabanlı, OpenType feature desteği |
| Texture | image | 0.25+ | Texture loading, Texture2DArray desteği |

### 2.4. Meshing Algoritması

**Soyutlama:** Tüm meshing algoritmaları ortak `MeshData` formatı üretir. Render crate'i hangi algoritmanın çalıştığını bilmez. Algoritma değişimi `Mesher` trait üzerinden tek satırla yapılır.

| Faz | Yaklaşım | Performans | Durum |
|-----|----------|------------|-------|
| Faz 1 | Klasik Greedy Meshing (CPU) | 200-500µs/chunk | İlk implementasyon, multi-texture safe |
| Faz 2 | GPU Compute Shader Greedy | <50µs/chunk | İleri optimizasyon, bitwise parallelism GPU'da |

**Klasik Greedy Meshing (Faz 1):**
- Referans: mikolalysenko / 0fps.net "Meshing in a Minecraft Game"
- Her axis için 2D mask oluştur, en büyük dikdörtgenleri bul (greedy)
- **Avantaj:** Multi-texture memory sorunu yok (tek pass'te tüm blok tipleri)
- **T-junction artifact'ları** için quad expansion + vertex snapping

**GPU Compute Shader Meshing (Faz 2):**
- Binary greedy'nin bitwise operasyonları GPU'da doğal olarak paralel çalışır
- Her thread bir face grubunu işler, VRAM'de ayrı buffer'lar
- `multi_draw_indexed_indirect` ile tek draw call'da tüm chunk'lar
- T-junction çözümü: GPU'da vertex snapping + eye-space calculation

**MeshData Ortak Formatı (algoritma bağımsız):**
```rust
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub vertex_count: usize,
    pub index_count: usize,
    pub bounds: BoundingBox,
}

pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub ao: f32,
    pub texture_id: u16,
}

pub trait Mesher: Send + Sync {
    fn generate_mesh(&self, chunk: &Chunk) -> MeshData;
    fn name(&self) -> &str;
}
```

### 2.5. Network & Multiplayer

| Alan | Teknoloji | Versiyon | Neden |
|------|-----------|----------|-------|
| Transport | renet2 (bevy_renet2) | 0.13+ | Game-specific UDP, düşük latency, Bevy plugin entegrasyonu |
| Replication | bevy_replicon | 0.39+ | Server-authoritative ECS replication, delta compression built-in, I/O agnostic |
| Backend Entegrasyon | bevy_replicon_renet2 | 0.14+ | bevy_replicon + renet2 resmi entegrasyon crate'i |
| Alternatif (P2P) | lightyear | 0.22+ | Deterministic replication, rollback desteği, P2P oyunlar için |

**Network Mimarisi (bevy_replicon + renet2):**
- Replication: Otomatik ECS component sync (server → client)
- Remote Events: Client → server input tetikleyicileri
- Channel yapısı: Reliable (ordered), Unreliable, Reliable-Unordered
- Client-side prediction + server reconciliation (built-in)
- Entity visibility kontrolü (interest management)

**Server Referans Mimarisi (Hyperion.rs):**
- Entity'ler thread'lere partition edilir (çekirdek sayısı kadar)
- Her tick sonunda `bytes::BytesMut` buffer'ları tokio egress task'a gönderilir
- postcard serialization (renet2 backend)
- Packet switch logic parallel çalışır

### 2.6. Modding Sistemi

| Alan | Teknoloji | Versiyon | Neden |
|------|-----------|----------|-------|
| Wasm Runtime | wasmtime | 30+ | En mature WASI runtime, fuel metering, resource limits, Bytecode Alliance güvenlik altyapısı |
| Interface | WIT (WebAssembly Interface Types) | Component Model | Type-safe contract tanımlama |
| Referans | Wasvy | - | Bevy ECS + Wasm modding entegrasyonu |

**İki Katmanlı Modding:**

Katman 1 - Güvenli Wasm Modları:
- Yeni eşyalar, bloklar, yaratıklar, biyomlar
- Sandbox içinde çalışır, asla motoru çökertemez
- Hot-reload desteği
- Diller: Rust, Python, Go, C# (Wasm'a compile edebilen her dil)

Katman 2 - Native Core-Mods:
- Network protokolü, render pipeline, bellek yönetimi değişiklikleri
- `.dll` dinamik kütüphane olarak yüklenir
- Güvenlik uyarısı ile kurulur
- Stabilite garantisi yoktur

### 2.7. Chunk Storage

| Faz | Yaklaşım | Neden |
|-----|----------|-------|
| Faz 1 | Custom binary format + zstd compression | Basit, hızlı implementasyon, zlib'den 3-5x hızlı |
| Faz 2 | fjall 3.0 (LSM-tree KV store) | Yazma-heavy workload için optimize, pure Rust, otomatik compaction |

**Neden fjall yerine redb değil:** fjall LSM-tree tabanlıdır; chunk save/load gibi yazma-ağırlıklı işlerde B-tree (redb) tabanlı veritabanlarından daha yüksek throughput sağlar. Okuma-ağırlıklı senaryolarda redb tercih edilebilir.

**Chunk Format (Faz 1):**
```
[Header: 16 bytes]
  - Magic: 4 bytes ("VXCL")
  - Version: 2 bytes
  - Chunk X: 4 bytes (i32)
  - Chunk Z: 4 bytes (i32)
  - Data Length: 2 bytes (u16)

[Compressed Data: variable]
  - zstd compressed voxel array (Vec<u16>, 65,536 entries)
  - Entity data (varsa)
  - Light data (varsa)
```

---

## 3. Mimari Tasarım

### 3.1. Cargo Workspace Yapısı

```
strata/
├── Cargo.toml                 # Workspace root
├── README.md
├── project_design.md          # Bu dosya
│
├── crates/
│   ├── core/                  # Temel veri yapıları
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── block.rs       # Block registry, block properties
│   │   │   ├── chunk.rs       # Chunk data structure
│   │   │   ├── world.rs       # World koordinat sistemi
│   │   │   └── registry.rs    # Block/Item/Entity registry
│   │   └── Cargo.toml
│   │
│   ├── ecs/                   # ECS components & systems (Bevy 0.18+)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── components/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── position.rs
│   │   │   │   ├── velocity.rs
│   │   │   │   ├── health.rs
│   │   │   │   ├── inventory.rs
│   │   │   │   └── render.rs
│   │   │   ├── systems/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── physics.rs
│   │   │   │   ├── ai.rs
│   │   │   │   └── lifecycle.rs
│   │   │   └── plugin.rs      # ECS plugin base trait
│   │   └── Cargo.toml
│   │
│   ├── world-gen/             # Prosedürel dünya üretimi
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── noise.rs       # fastnoise2 0.4+ FBM (SIMD-optimized, node-graph)
│   │   │   ├── biome.rs       # Biome tanımları
│   │   │   ├── terrain.rs     # Terrain generation
│   │   │   ├── structure.rs   # Yapılar (köyler, zindanlar)
│   │   │   └── generator.rs   # Chunk generator pipeline
│   │   └── Cargo.toml
│   │
│   ├── meshing/               # Voxel mesh oluşturma (Mesher trait + algoritmalar)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── mesher.rs         # Mesher trait + MeshData struct (algoritma bağımsız)
│   │   │   ├── classic_greedy.rs # Klasik greedy meshing (Faz 1)
│   │   │   ├── gpu_compute.rs    # GPU compute shader meshing (Faz 2)
│   │   │   └── chunk_mesh.rs     # Chunk mesh wrapper
│   │   └── Cargo.toml
│   │
│   ├── render/                # Render pipeline (wgpu)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── engine.rs      # wgpu engine initialization
│   │   │   ├── pipeline.rs    # Render pipeline setup
│   │   │   ├── camera.rs      # Camera system
│   │   │   ├── frustum.rs     # Frustum culling
│   │   │   ├── lighting.rs    # Işıklandırma sistemi
│   │   │   ├── chunk_renderer.rs  # Chunk render logic
│   │   │   └── shaders/       # WGSL shaders
│   │   │       ├── chunk.wgsl
│   │   │       ├── lighting.wgsl
│   │   │       └── compute_mesher.wgsl  # Faz 2
│   │   └── Cargo.toml
│   │
│   ├── network/               # Network protokolü
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── replication.rs    # bevy_replicon protocol setup
│   │   │   ├── server.rs         # renet2 server + replication
│   │   │   ├── client.rs         # renet2 client + prediction
│   │   │   ├── chunk_sync.rs     # Manuel chunk sync (compressed)
│   │   │   └── visibility.rs     # Interest management
│   │   └── Cargo.toml
│   │
│   ├── storage/               # Chunk persistence
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── region.rs      # Region dosya yönetimi (Faz 1)
│   │   │   ├── format.rs      # Binary chunk format + zstd
│   │   │   ├── cache.rs       # LRU chunk cache
│   │   │   ├── loader.rs      # Async chunk loader
│   │   │   └── fjall_store.rs # fjall 3.0 backend (Faz 2)
│   │   └── Cargo.toml
│   │
│   ├── modding/               # Wasm modding altyapısı
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── runtime.rs     # wasmtime runtime
│   │   │   ├── loader.rs      # Mod yükleme
│   │   │   ├── sandbox.rs     # Resource limits, fuel metering
│   │   │   ├── wit/           # WIT interface tanımları
│   │   │   │   ├── block_api.wit
│   │   │   │   ├── entity_api.wit
│   │   │   │   └── event_api.wit
│   │   │   └── native/        # Core-mod (.dll) loader
│   │   │       └── mod.rs
│   │   └── Cargo.toml
│   │
│   ├── physics/               # Fizik motoru
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── rapier_plugin.rs  # bevy_rapier 0.33 wrapper & config
│   │   │   ├── aabb.rs           # Axis-aligned bounding box helpers
│   │   │   ├── collision.rs      # Çarpışma tespiti
│   │   │   └── raycast.rs        # Işın çarpışma testi
│   │   └── Cargo.toml
│   │
│   ├── lighting/              # Işıklandırma motoru (plugin)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── sunlight.rs    # Güneş ışığı propagasyonu
│   │   │   ├── block_light.rs # Blok ışığı (meşale vb.)
│   │   │   └── propagate.rs   # Light propagation algorithm
│   │   └── Cargo.toml
│   │
│   └── plugin-api/            # Plugin framework base
│       ├── src/
│       │   ├── lib.rs
│       │   ├── trait.rs       # Plugin trait tanımı
│       │   ├── registry.rs    # Plugin registry
│       │   ├── hook.rs        # Hook system
│       │   └── lifecycle.rs   # Plugin lifecycle
│       └── Cargo.toml
│
├── bin/
│   ├── client/                # Oyun istemcisi
│   │   ├── src/
│   │   │   └── main.rs
│   │   └── Cargo.toml
│   │
│   └── server/                # Headless sunucu
│       ├── src/
│       │   └── main.rs
│       └── Cargo.toml
│
├── assets/
│   ├── textures/
│   ├── shaders/
│   └── mods/                  # Varsayılan modlar
│
└── tools/
    ├── wit-gen/               # WIT binding generator
    └── chunk-tool/            # Chunk inspect/debug aracı
```

### 3.2. Plugin-First Mimarisi

Ana motor monolitik DEĞİLDİR. Her alt sistem bir plugin olarak tasarlanır:

```
┌─────────────────────────────────────────────────┐
│                   Core Engine                    │
│  ┌───────────────────────────────────────────┐  │
│  │           Plugin Registry                  │  │
│  │  - Plugin yükleme/sıralama                │  │
│  │  - Hook management                        │  │
│  │  - Lifecycle yönetimi                     │  │
│  └───────────────────────────────────────────┘  │
│                      │                          │
│    ┌─────────────────┼─────────────────┐        │
│    │                 │                 │        │
│  ┌─▼──────┐  ┌──────▼──────┐  ┌──────▼───┐    │
│  │ Render │  │  Lighting   │  │ Physics  │    │
│  │ Plugin │  │   Plugin    │  │ Plugin   │    │
│  └────────┘  └─────────────┘  └──────────┘    │
│                                                  │
│    ┌─────────────────┬─────────────────┐        │
│    │                 │                 │        │
│  ┌─▼──────┐  ┌──────▼──────┐  ┌──────▼───┐    │
│  │ World  │  │  Meshing    │  │ Network  │    │
│  │ Gen    │  │   Plugin    │  │ Plugin   │    │
│  └────────┘  └─────────────┘  └──────────┘    │
└─────────────────────────────────────────────────┘
```

**Plugin Trait:**
```rust
pub trait GamePlugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> semver::Version;
    fn dependencies(&self) -> Vec<&str>;
    
    fn on_register(&self, registry: &mut PluginRegistry);
    fn on_startup(&self, app: &mut App);
    fn on_shutdown(&self, app: &mut App);
    
    // Opsiyonel hook'lar
    fn on_chunk_generated(&self, chunk: &Chunk) { ... }
    fn on_block_placed(&self, pos: BlockPos, block: BlockId) { ... }
    fn on_block_broken(&self, pos: BlockPos, block: BlockId) { ... }
}
```

**Varsayılan Plugin'ler:**
- `DefaultRenderPlugin` - wgpu render pipeline
- `DefaultLightingPlugin` - Classic Minecraft light propagation
- `DefaultMeshingPlugin` - Klasik greedy meshing (Faz 1), GPU compute (Faz 2)
- `DefaultWorldGenPlugin` - fastnoise2 FBM terrain
- `DefaultPhysicsPlugin` - bevy_rapier 0.33 (AABB collision + gravity + enhanced-determinism desteği)
- `DefaultNetworkPlugin` - renet2 + bevy_replicon server/client

### 3.3. ECS Component Yapısı

**Temel Component'ler:**
```rust
// Pozisyon
#[derive(Component)]
pub struct Position(pub Vec3);

// Hız
#[derive(Component)]
pub struct Velocity(pub Vec3);

// Chunk koordinatı
#[derive(Component)]
pub struct ChunkPosition(pub IVec2);

// Renderable (mesh'e sahip entity)
#[derive(Component)]
pub struct Renderable {
    pub mesh_handle: Handle<Mesh>,
    pub material_handle: Handle<Material>,
}

// Health
#[derive(Component)]
pub struct Health {
    pub current: u16,
    pub max: u16,
}

// Inventory
#[derive(Component)]
pub struct Inventory {
    pub slots: [Option<ItemStack>; 36],
    pub selected_slot: u8,
}

// Player (marker component)
#[derive(Component)]
pub struct Player;

// Entity (marker component)
#[derive(Component)]
pub struct Entity;

// Network ownership (hangi client bu entity'yi kontrol ediyor)
#[derive(Component)]
pub struct NetworkOwner(pub u64); // client ID
```

### 3.4. Chunk Sistemi

**Chunk Yapısı:**
- Boyut: 16×256×16 = 65,536 blok (Minecraft uyumlu)
- Her blok: `u16` block ID (65,536 tip desteği)
- Flat array: `Vec<u16>` (1D, cache-friendly, SIMD-optimize)
- Toplam: 65,536 × 2 byte = **128 KB/chunk** (uncompressed)
- 100 chunk RAM: ~12.8 MB (modern sistemlerde ihmal edilebilir)

**Neden flat `Vec<u16>`:**
- ndarray voxel için overkill ve düşük boyutta 5x yavaş
- Bit-packed'e göre daha hızlı erişim (shift/mask overhead yok)
- GPU'ya doğrudan upload edilebilir
- Cache-line'da 32 blok sığar (64 byte / 2 byte)

**Heightmap Optimizasyonu (Sector's Edge yaklaşımı):**
```rust
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: Vec<u16>,                 // Flat array: [x + z*16 + y*256]
    pub heightmap_top: [u16; 256],        // Her column'daki en üst dolu blok
    pub heightmap_bottom: [u16; 256],     // Her column'daki en alt dolu blok
    pub dirty: bool,                       // Mesh rebuild gerekli mi?
    pub mesh_handle: Option<Handle<Mesh>>,
}

impl Chunk {
    #[inline]
    pub fn get_block(&self, x: usize, y: usize, z: usize) -> u16 {
        self.blocks[x + z * 16 + y * 256]
    }

    #[inline]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, id: u16) {
        self.blocks[x + z * 16 + y * 256] = id;
        self.dirty = true;
    }
}
```

**Chunk Yükleme Pipeline:**
```
1. Oyuncu pozisyonuna göre gerekli chunk'ları belirle
2. Öncelik sırasına göre sırala (en yakın → en uzak)
3. Disk'ten yükle (varsa) veya prosedürel üret
4. Mesh oluştur (async, worker thread)
5. Render pipeline'a gönder
6. Dirty chunk'ları batch olarak işle (frame başına N chunk)
```

**Lazy Loading (frame throttling):**
```rust
// Her N frame'de 1 chunk yükle
pub struct ChunkLoader {
    queue: VecDeque<ChunkPos>,
    chunks_per_frame: u8,    // Örn: 2
    frame_counter: u32,
    load_interval: u32,      // Örn: 3 (her 3 frame'de bir)
}
```

### 3.5. Network Protokol Tasarımı

**Replication (bevy_replicon ile otomatik):**
```rust
// Component'ler otomatik replicate edilir
app.replicate::<Position>()
   .replicate::<Velocity>()
   .replicate::<Health>();

// Client → server input için remote events
app.add_client_trigger::<PlayerInput>(ChannelKind::Unordered);
```

**Manuel Packet Yapısı (postcard serialized, özel durumlar için):**
```rust
#[derive(Serialize, Deserialize)]
pub enum ManualPacket {
    // Handshake
    Handshake { version: u32, username: String },
    HandshakeResponse { success: bool, reason: Option<String> },

    // Chunk (manuel sync, replication dışında)
    ChunkData { x: i32, z: i32, data: Vec<u8> }, // zstd compressed
    ChunkRequest { x: i32, z: i32 },

    // Chat
    ChatMessage { message: String },

    // Heartbeat
    Ping { timestamp: u64 },
    Pong { timestamp: u64 },
}
```

**Channel Routing (renet2):**
| Kanal | Tip | Kullanım |
|-------|-----|----------|
| Reliable Ordered | renet2 Reliable | Chunk data, handshake, chat |
| Unreliable | renet2 Unreliable | Pozisyon güncellemeleri, entity hareketi |
| Reliable Unordered | renet2 ReliableUnordered | Block updates, entity spawn/despawn |

**Delta Compression (bevy_replicon built-in):**
- Otomatik component delta tracking
- Sadece değişen component'ler gönderilir
- İlk bağlantıda full snapshot, sonrasında incremental diff

### 3.6. Işıklandırma Sistemi

**Classic Minecraft Light Propagation:**
- 15 seviye güneş ışığı (sky light)
- 15 seviye blok ışığı (block light: meşale, lava, vb.)
- BFS propagation algoritması

**Optimizasyon:**
- Dirty chunk'lar için incremental update
- Compute shader'a taşıma (Faz 2)
- Her chunk kendi light array'ini tutar: `light: [u8; 16×256×16]` (2 byte/voxel: 4 bit sky + 4 bit block)

```rust
pub struct LightData {
    pub sky_light: BitArray,    // 4 bit per voxel
    pub block_light: BitArray,  // 4 bit per voxel
    pub dirty_regions: Vec<ChunkSectionPos>,
}
```

### 3.7. Wasm Modding API (WIT Interfaces)

**block_api.wit:**
```wit
package strata:block-api;

interface block-registry {
    register-block(name: string, properties: list<property>) -> u16;
    get-block(id: u16) -> block-info;
}

record block-info {
    id: u16,
    name: string,
    hardness: f32,
    blast-resistance: f32,
    transparent: bool,
    light-emission: u8,
}
```

**entity_api.wit:**
```wit
package strata:entity-api;

interface entity-registry {
    spawn-entity(entity-type: u16, position: vec3) -> u32;
    add-component(entity-id: u32, component: component-data);
    remove-component(entity-id: u32, component-type: string);
}
```

**event_api.wit:**
```wit
package strata:event-api;

interface event-hooks {
    on-block-placed(position: block-pos, block-id: u16);
    on-block-broken(position: block-pos, block-id: u16);
    on-entity-spawned(entity-id: u32);
    on-entity-died(entity-id: u32);
    on-player-chat(player-id: u32, message: string);
}
```

### 3.8. Render Pipeline

**Pipeline Adımları:**
```
1. Frustum Culling (CPU)
   - Görünür chunk'ları belirle
   - Distance-based LOD seçimi

2. Chunk Mesh Bind (wgpu)
   - Görünür chunk mesh'lerini GPU buffer'a yükle
   - Instanced rendering için batch'le

3. Lighting Pass
   - Ambient light
   - Directional light (güneş)
   - Block light (meşale vb.)

4. Main Render Pass
   - Chunk geometry
   - Entity geometry
   - Particle effects

5. Post-Processing
   - Bloom (opsiyonel)
   - Tone mapping
   - Vignette
```

**Draw Call Optimizasyonu:**
- Aynı materyale sahip chunk'ları tek draw call'da birleştir
- `multi_draw_indexed_indirect` (wgpu) ile batch rendering
- Ortak `MeshData` formatı sayesinde algoritma değişse de render pipeline değişmez
- Front-to-back sorting ile z-buffer overdraw azaltma (Faz 2)

---

## 4. Geliştirme Fazları

### Faz 1: Temel Altyapı (Hafta 1-4)
- [ ] Cargo workspace kurulumu
- [ ] `core` crate: Block registry, Chunk data structure (`Vec<u16>` flat array)
- [ ] `ecs` crate: Bevy ECS 0.18 entegrasyonu, temel component'ler
- [ ] `world-gen` crate: fastnoise2 0.4+ FBM terrain generation
- [ ] `meshing` crate: `Mesher` trait + `MeshData` struct, Klasik greedy meshing (CPU)
- [ ] `storage` crate: Custom binary chunk format + zstd compression, disk I/O
- [ ] `bin/client`: winit window, wgpu initialization
- [ ] `physics` crate: bevy_rapier entegrasyonu (AABB, gravity, raycast)
- [ ] İlk oynanabilir: prosedürel dünya, blok kırma/yerleştirme

### Faz 2: Render & Işıklandırma (Hafta 5-8)
- [ ] `render` crate: Full render pipeline
- [ ] `lighting` crate: Light propagation (BFS)
- [ ] Frustum culling
- [ ] Chunk lazy loading + dirty-flag system
- [ ] Heightmap optimizasyonu
- [ ] Texture2DArray block rendering
- [ ] GPU Compute Shader meshing (compute_mesher.wgsl, <50µs/chunk hedefi)
- [ ] Debug overlay (FPS, chunk count, memory)

### Faz 3: Fizik & Entity (Hafta 9-12)
- [ ] `physics` crate: AABB collision, gravity, raycast
- [ ] Player controller (hareket, zıplama, sprint)
- [ ] Entity sistemi (yaratık spawn, AI basics)
- [ ] Inventory sistemi
- [ ] Block interaction (sağ/sol tık)

### Faz 4: Network & Multiplayer (Hafta 13-18)
- [ ] `network` crate: bevy_renet2 0.13+ transport + bevy_replicon 0.39+ replication + bevy_replicon_renet2 0.14+ backend
- [ ] Component replication setup (Position, Velocity, Health, etc.)
- [ ] Remote events: client input → server trigger
- [ ] Chunk sync (manuel: client request → server compressed response)
- [ ] Client-side prediction + server reconciliation (bevy_replicon built-in)
- [ ] Entity visibility / interest management
- [ ] `bin/server`: Headless server binary
- [ ] Multiplayer test: 2+ oyuncu aynı dünyada

### Faz 5: Modding Sistemi (Hafta 19-24)
- [ ] `modding` crate: wasmtime runtime
- [ ] WIT interface tanımları
- [ ] Wasm mod loading/unloading
- [ ] Hot-reload desteği
- [ ] Resource limits + fuel metering
- [ ] `plugin-api` crate: Plugin framework
- [ ] Varsayılan plugin'lerin plugin olarak refactor edilmesi
- [ ] Native core-mod (.dll) loader

### Faz 6: Optimizasyon & Polish (Hafta 25-30)
- [ ] GPU light propagation
- [ ] fjall 3.0 chunk storage migration
- [ ] Memory profiling + leak detection
- [ ] Anti-cheat basics
- [ ] Performance benchmark suite
- [ ] 100 chunk render @ 60+ FPS hedefi
- [ ] Aokana (SVDAG) paper araştırma — uzun vadeli GPU-driven rendering için

---

## 5. Performans Hedefleri

| Metrik | Hedef | Referans |
|--------|-------|----------|
| FPS (100 chunk) | 60+ | Vanilla Minecraft: 30-40 chunk @ 60 FPS |
| Chunk mesh generation (CPU) | <500µs/chunk | Klasik greedy: 200-500µs |
| Chunk mesh generation (GPU) | <50µs/chunk | Compute shader greedy |
| Chunk load time (disk) | <50ms/chunk | SSD, uncompressed |
| Server TPS | 20 (sabit) | Vanilla: 20 TPS (düşebilir) |
| Max oyuncu (sunucu) | 1000+ | Vanilla: 50-100 |
| Entity count (sunucu) | 10.000+ | Vanilla: birkaç yüz |
| Bellek kullanımı (client) | <2GB | Vanilla: 4-8GB |
| Bellek kullanımı (server) | <512MB/100 oyuncu | Vanilla: 2-4GB |
| Network bant genişliği | <50KB/s/oyuncu | Vanilla: 100-200KB/s |

---

## 6. Riskler ve Mitigasyon

| Risk | Olasılık | Etki | Mitigasyon |
|------|----------|------|------------|
| Bevy ECS modding entegrasyonu zorluğu | Orta | Yüksek | Wasvy projesini referans al, gerekirse custom ECS |
| wgpu Vulkan driver uyumsuzlukları | Düşük | Orta | Geniş GPU test matrisi, fallback DX12 |
| renet2 + bevy_replicon game için yetersiz kalması | Düşük | Orta | lightyear'e geçiş planı hazır tut (deterministic replication) |
| Wasm modding performance overhead | Orta | Orta | Native core-mod katmanı ile bypass imkanı |
| Klasik greedy CPU bottleneck (fazla blok değişimi) | Orta | Orta | Faz 2'de GPU compute shader'a geçiş, `Mesher` trait ile tek satırda değişim |
| GPU compute shader wgpu feature flag olgunluğu | Düşük | Orta | Fallback CPU greedy hazır, `multi_draw_indexed_indirect` desteği takip edilecek |
| Deterministik fizik zorluğu | Yüksek | Yüksek | İlk fazta server-authoritative, lockstep sonraya bırak |
| fjall 3.0 production maturity | Düşük | Orta | Pure Rust, aktif bakım (Ocak 2026 v3), fallback custom binary format. Benchmark: redb'den yazma-heavy işlerde daha hızlı, okuma'da daha yavaş |
| Bevy API instability (sık breaking change) | Yüksek | Orta | Migration guide takibi, Faz geçişlerinde buffer süre |

---

## 7. Referans Projeler

| Proje | Açıklama | İncelenecek Yön |
|-------|----------|-----------------|
| [Minerust](https://github.com/B4rtekk1/Minerust) | Rust Minecraft clone | GPU-driven rendering |
| [voxel-rs](https://github.com/Technici4n/voxel-rs) | Rust voxel engine | Client-server architecture, Wasm modding planı |
| [voxelize](https://github.com/voxelize/voxelize) | Full-stack voxel engine | ECS yapısı, server architecture |
| [Luxelith](https://github.com/JSKF/Luxelith) | Unity GPU voxel engine | GPU compute shader meshing |
| [binary-greedy-meshing](https://github.com/cgerikj/binary-greedy-meshing) | Binary greedy meshing | Bitwise meshing algoritması |
| [Hyperion.rs](https://hyperion.rs) | Rust Minecraft server | ECS network sync, high-performance server |
| [Wasvy](https://github.com/wasvy-org/wasvy) | Bevy Wasm modding | WIT interfaces, hot-reload |
| [Sector's Edge](https://vercidium.com) | Voxel FPS | Mesh optimizasyonları, heightmap tracking |
| [bevy_replicon](https://github.com/simgine/bevy_replicon) | Bevy networking | ECS replication, server-authoritative model, bevy_replicon_renet2 backend |
| [renet2](https://github.com/UkoeHB/renet2) | Game networking | UDP transport, Bevy backend (bevy_renet2 0.13+) |
| [Rapier](https://rapier.rs) | Rust physics engine | bevy_rapier 0.33 plugin, AABB collision, enhanced-determinism |
| [fjall](https://github.com/fjall-rs/fjall) | LSM-tree KV store | Chunk persistence, yazma-heavy optimization |
| [Aokana (ACM 2025)](https://arxiv.org/abs/2505.02017) | GPU-driven voxel rendering | SVDAG + LOD + streaming, 9x memory azaltma, 4.8x hız artışı (Faz 6+ araştırma) |

---

## 8. Karar Kayıtları (ADRs)

### ADR-001: Platform Seçimi
**Karar:** Sadece Windows (x64)
**Neden:** Geliştirme süresini kısaltmak, cross-platform abstraction overhead'inden kaçınmak
**Tarih:** 2026-05-17

### ADR-002: ECS Framework
**Karar:** Bevy ECS
**Neden:** En mature Rust ECS, Wasvy modding desteği, parallel system execution
**Alternatifler:** Custom ECS (daha fazla iş), Hecs (minimalist ama modding yok)
**Tarih:** 2026-05-17

### ADR-003: Grafik API
**Karar:** wgpu (WebGPU abstraction)
**Neden:** Vulkan + DX12 otomatik destek, daha az boilerplate
**Alternatifler:** Doğrudan Vulkan (daha fazla kontrol ama 2-3x iş)
**Tarih:** 2026-05-17

### ADR-004: Network Protokolü
**Karar:** renet2 (UDP transport, `bevy_renet2 0.13+`) + bevy_replicon (`0.39+`, ECS replication) + bevy_replicon_renet2 (`0.14+`, backend entegrasyon)
**Neden:** Game için düşük latency, ECS-native otomatik replication, delta compression built-in, Bevy ecosystem ile tam uyumlu. bevy_replicon_renet2 resmi entegrasyon crate'i ile backend soyutlaması sağlanır.
**Alternatifler:** QUIC/quinn (TLS overhead, game için fazla ağır), lightyear (P2P ve deterministic replication için Faz 6'da değerlendirilecek), bevy_replicon_quinnet (QUIC transport, gereksiz TLS overhead)
**Tarih:** 2026-05-17

### ADR-005: Chunk Storage
**Karar:** Faz 1: Custom binary + zstd compression, Faz 2: fjall 3.0 (LSM-tree KV store)
**Neden:** zstd zlib'den 3-5x hızlı decompression; fjall yazma-heavy chunk workload'unda B-tree tabanlı redb'den daha yüksek throughput sağlar
**Tarih:** 2026-05-17

### ADR-006: Meshing Algoritması
**Karar:** Faz 1: Klasik Greedy (CPU, `Mesher` trait ile soyutlanmış), Faz 2: GPU Compute Shader Greedy
**Neden:** Binary greedy'nin multi-texture memory sorunu gerçek (500+ blok tipinde 64MB/chunk). Klasik greedy bu sorunu doğal olarak çözer (tek pass). `Mesher` trait + `MeshData` ortak formatı ile algoritma değişimi render pipeline'ı bozmadan yapılır. GPU compute shader Faz 2'de bitwise parallelism'den tam yararlanır.
**Tarih:** 2026-05-17

### ADR-007: Spatial Indexing
**Karar:** Chunk-grid + heightmap (Octree KULLANILMAYACAK)
**Neden:** Minecraft dense dünyasında octree overhead yaratıyor, heightmap daha pratik
**Tarih:** 2026-05-17

### ADR-008: Deterministic Lockstep
**Karar:** İlk fazta UYGULANMAYACAK
**Neden:** Floating point determinism zorluğu, geliştirme süresini 2-3x artırır
**Plan:** Server-authoritative + basit client interpolation, lockstep Faz 6+
**Tarih:** 2026-05-17

### ADR-009: Chunk Voxel Data Structure
**Karar:** `Vec<u16>` flat array (ndarray KULLANILMAYACAK, bit-packed FAZ 6'DA değerlendirilecek)
**Neden:** ndarray düşük boyutlu işlemlerde 5x yavaş; bit-packed CPU overhead getirir (shift/mask). Flat array SIMD-friendly, GPU'ya doğrudan upload edilebilir. 100 chunk = 12.8 MB RAM (ihmal edilebilir).
**Tarih:** 2026-05-17

### ADR-010: Physics Engine
**Karar:** bevy_rapier 0.33 (Rapier physics + Bevy plugin)
**Neden:** Bevy ile native uyumlu, aktif bakım (Dimforge), AABB + raycast + gravity desteği, ECS-driven. `enhanced-determinism` feature'ı ile cross-platform determinism desteği mevcut (IEEE 754-2008 compliant platformlarda). 0.33 versiyonu Bevy 0.18 ile tam uyumlu.
**Alternatifler:** Custom physics (daha fazla iş, kontrol avantajı)
**Tarih:** 2026-05-17

---

## 9. Geliştirme Kuralları

### Kod Stili
- Rust 2024 Edition
- `clippy` warnings = error
- `rustfmt` ile formatlanmış kod
- Documentation comments (`///`) tüm public API'ler için

### Branch Stratejisi
- `main`: Production-ready kod
- `dev`: Aktif geliştirme
- `feature/*`: Yeni özellikler
- `hotfix/*`: Kritik düzeltmeler

### Test Stratejisi
- Unit testler: Her crate'te `#[cfg(test)]` modülleri
- Integration testler: `tests/` dizininde
- Benchmarklar: `criterion` ile performans testleri
- Meshing doğrulama: Referans çıktılarla karşılaştırma

### Commit Mesajları
- Format: `type(scope): description`
- Tipler: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `chore`
- Örnek: `feat(meshing): implement classic greedy meshing algorithm with Mesher trait`
