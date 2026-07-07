# Thread Model — Gelişmiş Hybrid Mimarisi

## 1. Genel Bakış

Strata'nın thread modeli **Advanced Hybrid** yaklaşımını kullanır. Bevy'nin dahili task pool'ları üzerine inşa edilir, özel thread yönetimi gerektirmez.

**Karar: Advanced Hybrid (37/40)**

| Model | Skor | Neden |
|-------|------|-------|
| Tek Main Thread | 25/40 | 8+ core CPU'da kayıp, 600+ oyuncu yetiştiremez |
| Tam Paralel ECS | 23/40 | Deadlock riski, debug zor, determinizm kaybı |
| **Advanced Hybrid** | **37/40** | Dengeli: deterministik kritik yol + paralel ağır iş |

---

## 2. Temel Mimarisi

```
┌─────────────────────────────────────────────────────────────────┐
│                        MAIN THREAD                               │
│  (ECS Schedule — deterministik sıra)                             │
│                                                                  │
│  Input → Simulation → WorldGen → Meshing → NetworkSync →        │
│  RenderPrep → UI                                                 │
│                                                                  │
│  Her set içinde sistemler paralel çalışır                        │
│  Set'ler arası sıra sabit (deterministik)                        │
└──────────────┬───────────────────────────────────────────────────┘
               │
    ┌──────────┼──────────────────────┐
    ▼          ▼                      ▼
┌────────┐ ┌────────────────┐ ┌────────────┐
│Compute │ │AsyncCompute    │ │I/O Pool    │
│Pool    │ │Pool            │ │            │
│        │ │                │ │Network I/O │
│Tier 1: │ │Tier 2:         │ │Disk I/O    │
│4-8 thr │ │2-4 thr         │ │Asset load  │
│P0-P1   │ │SVDAG bake      │ │            │
│3ms     │ │2ms             │ │1 thread    │
│        │ │                │ │0.5ms       │
│Tier 2: │ │Tier 3:         │ │            │
│2 thr   │ │1-2 thr         │ │            │
│P2-P3   │ │Distant gen     │ │            │
│2ms     │ │1ms             │ │            │
└────────┘ └────────────────┘ └────────────┘
               │
               ▼
        ┌─────────────┐
        │Double Buffer │
        │Mesh Upload   │
        │(swap/frame)  │
        └─────────────┘
```

---

## 3. Bevy Task Pool'ları

Bevy zaten 3 task pool sunar — bunları kullanır, yenilerini yazmayız:

| Pool | Amaç | Thread Sayısı | Bevy Tipi |
|------|------|---------------|-----------|
| `ComputeTaskPool` | Frame-bounded CPU işi | CPU core sayısı | `Res<ComputeTaskPool>` |
| `AsyncComputeTaskPool` | Multi-frame CPU işi | CPU core sayısı | `Res<AsyncComputeTaskPool>` |
| `IoTaskPool` | I/O (network, disk) | Düşük (2-4) | `Res<IoTaskPool>` |

**Kaynak:** [bevy_tasks docs](https://docs.rs/bevy/latest/bevy/tasks/index.html)

### 3.1 ComputeTaskPool — Anında Tamamlanması Gereken İş

```rust
fn chunk_generation_system(
    pool: Res<ComputeTaskPool>,
    mut queue: ResMut<PriorityTaskQueue>,
) {
    // Priority'den task al, pool'a spawn et
    while let Some(task) = queue.pop_by_priority() {
        pool.spawn(async move {
            generate_chunk_data(task.coord, task.seed)
        }).detach();
    }
}
```

**Kullanım alanları:**
- Chunk generation (noise, structure placement)
- Greedy mesh generation
- Lighting BFS propagation
- A* pathfinding

### 3.2 AsyncComputeTaskPool — Multi-Frame İş

```rust
fn svdag_bake_system(
    pool: Res<AsyncComputeTaskPool>,
    query: Query<&SectorData, NeedsSvdagBake>,
) {
    for data in &query {
        let data = Arc::clone(&data.0);
        pool.spawn(async move {
            // Bu işlem birkaç frame sürebilir
            let svdag = build_svdag(&data);
            svdag
        }).detach();
    }
}
```

**Kullanım alanları:**
- SVDAG bake/unbake
- Save snapshot generation
- Large-scale world edit operations
- Compression/decompression

### 3.3 IoTaskPool — I/O İşleri

```rust
fn network_receive_system(
    pool: Res<IoTaskPool>,
    mut receiver: ResMut<NetworkReceiver>,
) {
    pool.spawn(async move {
        // Non-blocking I/O
        let packets = receiver.receive_all().await;
        packets
    }).detach();
}
```

**Kullanım alanları:**
- Network packet receive/send
- Disk I/O (chunk save/load)
- Asset loading (textures, models)
- Cloud backup

---

## 4. Senkronizasyon Mekanizmaları

Task pool'dan main thread'e veri transferi için 3 mekanizma:

### 4.1 Crossbeam Channel (ComputePool → Main)

```rust
#[derive(Resource)]
pub struct ChunkResultChannel {
    pub sender: crossbeam::channel::Sender<ChunkResult>,
    pub receiver: crossbeam::channel::Receiver<ChunkResult>,
}

// ComputePool'da: sender.send(result)
// Main thread'de: receiver.try_recv() ile topla
fn collect_chunk_results(
    mut commands: Commands,
    channel: Res<ChunkResultChannel>,
) {
    while let Ok(result) = channel.receiver.try_recv() {
        // ECS'e ekle
        commands.entity(result.entity).insert(result.data);
    }
}
```

### 4.2 Bevy Task<T> (AsyncPool → Main)

```rust
#[derive(Resource)]
pub struct PendingTasks {
    pub tasks: Vec<Task<SvdagResult>>,
}

fn poll_svdag_tasks(
    mut commands: Commands,
    mut pending: ResMut<PendingTasks>,
) {
    pending.tasks.retain_mut(|task| {
        if let Some(result) = future::block_on(future::poll_once(task)) {
            // Sonucu ECS'e aktar
            false // Tamamlandı, listeden kaldır
        } else {
            true // Hâlâ çalışıyor
        }
    });
}
```

### 4.3 Bevy Event (IOPool → Main)

```rust
// IOPool'da event yaz
fn network_event_writer(
    mut events: EventWriter<NetworkPacketEvent>,
    packets: Vec<Packet>,
) {
    for packet in packets {
        events.send(NetworkPacketEvent { data: packet });
    }
}

// Main thread'de event oku
fn network_event_reader(
    mut reader: EventReader<NetworkPacketEvent>,
) {
    for event in reader.read() {
        // Paketi işle
    }
}
```

---

## 5. İyileştirme 1: Priority-Based Task Scheduling

Tüm chunk'lar eşit öncelikli değil. Oyuncunun altındaki chunk, 500m uzaktaki chunk'tan kritik.

### 5.1 Öncelik Seviyeleri

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChunkPriority {
    Critical = 0,   // Oyuncunun bulunduğu chunk
    Predictive = 1,  // Hareket yönündeki chunk'lar
    Visible = 2,     // Frustum içinde
    Background = 3,  // Streaming mesafesi
    Maintenance = 4, // SVDAG bake, cleanup
}
```

### 5.2 Priority Queue Implementasyonu

```rust
#[derive(Resource)]
pub struct PriorityTaskQueue {
    queues: [VecDeque<ChunkTask>; 5],
    total_count: usize,
}

impl PriorityTaskQueue {
    pub fn new() -> Self {
        Self {
            queues: Default::default(),
            total_count: 0,
        }
    }

    pub fn push(&mut self, task: ChunkTask) {
        self.queues[task.priority as usize].push_back(task);
        self.total_count += 1;
    }

    pub fn pop(&mut self) -> Option<ChunkTask> {
        for queue in &mut self.queues {
            if let Some(task) = queue.pop_front() {
                self.total_count -= 1;
                return Some(task);
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.total_count
    }

    pub fn is_empty(&self) -> bool {
        self.total_count == 0
    }
}

pub struct ChunkTask {
    pub coord: SectorCoord,
    pub priority: ChunkPriority,
    pub task_type: ChunkTaskType,
}

pub enum ChunkTaskType {
    Generate { seed: WorldSeed },
    Mesh { data: Arc<CompressedChunkData> },
    Light { light_data: SectorLightMap },
    SvdagBake { data: Arc<CompressedChunkData> },
}
```

**Kaynak:** [Priority based chunk generation - Reddit](https://www.reddit.com/r/VoxelGameDev/comments/1qrk3j7/i_think_i_have_finally_mastered_priority_based)

---

## 6. İyileştirme 2: Frame Budget / Time Slicing

Her frame'de sabit süre ayrılmalı, sonsuz task döngüsü frame rate'i düşürür.

### 6.1 Frame Budget Dağılımı

```
Frame Budget (16.6ms @ 60 FPS):

┌──────────────────────────────────────────────────────────────┐
│  Input (0.1ms) │ Physics (2ms) │ Render Prep (1ms) │ GPU (0.5ms) │
├──────────────────────────────────────────────────────────────┤
│  Chunk Gen     │ Mesh Gen      │ Lighting           │ Network     │
│  (budget: 2ms) │ (budget: 2ms) │ (budget: 1.5ms)    │ (budget: 1ms)│
├──────────────────────────────────────────────────────────────┤
│  Buffer: ~7ms (frame spike tolerance)                        │
└──────────────────────────────────────────────────────────────┘
```

### 6.2 Time Slicing Implementasyonu

```rust
#[derive(Resource)]
pub struct FrameBudget {
    pub chunk_gen_ms: u64,
    pub mesh_gen_ms: u64,
    pub lighting_ms: u64,
    pub network_ms: u64,
}

impl Default for FrameBudget {
    fn default() -> Self {
        Self {
            chunk_gen_ms: 2,
            mesh_gen_ms: 2,
            lighting_ms: 1,
            network_ms: 1,
        }
    }
}

fn time_sliced_chunk_gen(
    pool: Res<ComputeTaskPool>,
    budget: Res<FrameBudget>,
    mut queue: ResMut<PriorityTaskQueue>,
) {
    let start = Instant::now();
    let max_time = Duration::from_millis(budget.chunk_gen_ms);

    while start.elapsed() < max_time && !queue.is_empty() {
        if let Some(task) = queue.pop() {
            pool.spawn(async move {
                generate_chunk(task)
            }).detach();
        }
    }
}

fn time_sliced_mesh_gen(
    pool: Res<ComputeTaskPool>,
    budget: Res<FrameBudget>,
    mut dirty: Query<&mut SectorData, With<NeedsRemesh>>,
) {
    let start = Instant::now();
    let max_time = Duration::from_millis(budget.mesh_gen_ms);

    for mut data in dirty.iter_mut() {
        if start.elapsed() >= max_time { break; }

        let chunk_data = Arc::clone(&data.0);
        pool.spawn(async move {
            greedy_mesh(&chunk_data)
        }).detach();
    }
}
```

**Kaynak:** [Game Programming: Time Slicing](https://allenchou.net/2021/05/time-slicing)

---

## 7. İyileştirme 3: Tier-Aware Thread Allocation

4-tier streaming ile uyumlu thread dağılımı:

### 7.1 Tier-Thread Eşlemesi

| Tier | Mesafe | Thread | Öncelik | Bütçe | Hedef |
|------|--------|--------|---------|-------|-------|
| **ACTIVE** | 0-96m | 4-8 | P0-P1 | 3ms | Anında chunk hazır |
| **WARM** | 96-384m | 2-4 | P2-P3 | 2ms | Yumuşak geçiş |
| **DISTANT** | 384m-1.5km | 1-2 | P3 | 1ms | Arka plan |
| **ARCHIVE** | 1.5km+ | 1 | P4 | 0.5ms | Boş zaman |

### 7.2 Tier-Based Task Dağılımı

```rust
fn tier_aware_distribution(
    queue: Res<PriorityTaskQueue>,
    streaming: Res<StreamingState>,
) -> HashMap<Tier, usize> {
    let mut allocation = HashMap::new();

    match streaming.player_speed {
        // Statik oyuncu: tüm tier'lar aktif
        0.0..=1.0 => {
            allocation.insert(Tier::Active, 8);
            allocation.insert(Tier::Warm, 4);
            allocation.insert(Tier::Distant, 2);
            allocation.insert(Tier::Archive, 1);
        }
        // Hareket halinde: Tier 1'e ağırlık ver
        1.0..=10.0 => {
            allocation.insert(Tier::Active, 6);
            allocation.insert(Tier::Warm, 2);
            allocation.insert(Tier::Distant, 1);
            allocation.insert(Tier::Archive, 0); // Durdur
        }
        // Koşuyor: sadece Tier 1
        _ => {
            allocation.insert(Tier::Active, 4);
            allocation.insert(Tier::Warm, 1);
            allocation.insert(Tier::Distant, 0);
            allocation.insert(Tier::Archive, 0);
        }
    }

    allocation
}
```

---

## 8. İyileştirme 4: Predictive Streaming Entegrasyonu

`plans/08-streaming.md`'deki `StreamingPredictor` ile thread scheduling entegrasyonu:

### 8.1 Predictive Prefetch

```rust
fn predictive_prefetch(
    predictor: Res<StreamingPredictor>,
    player: Query<&Transform, With<Player>>,
    mut queue: ResMut<PriorityTaskQueue>,
    loaded: Res<LoadedSectors>,
) {
    let transform = player.single();
    let predicted = predictor.predict_position(transform.translation);
    let predicted_sector = SectorCoord::from_world(predicted);

    // 2 saniye sonraki pozisyondaki chunk'ları şimdi generate et
    for offset in SPHERE_RADIUS_3.iter() {
        let coord = predicted_sector.0 + offset;
        if !loaded.contains(coord) {
            let dist = (coord.as_vec3() - predicted).length();
            let priority = if dist < 32.0 {
                ChunkPriority::Predictive
            } else {
                ChunkPriority::Background
            };
            queue.push(ChunkTask {
                coord,
                priority,
                task_type: ChunkTaskType::Generate { seed: *predictor.seed },
            });
        }
    }
}
```

---

## 9. İyileştirme 5: Double-Buffered Mesh Upload

Mesh generation ve GPU upload arasındaki darboğazı çözer:

### 9.1 Double Buffer Yapısı

```rust
#[derive(Resource)]
pub struct MeshDoubleBuffer {
    pub front: Vec<PendingMesh>,  // GPU'nun okuduğu
    pub back: Vec<PendingMesh>,   // CPU'nun yazdığı
}

pub struct PendingMesh {
    pub entity: Entity,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub aabb: Aabb,
}

impl MeshDoubleBuffer {
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.front, &mut self.back);
        self.back.clear();
    }

    pub fn push_back(&mut self, mesh: PendingMesh) {
        self.back.push(mesh);
    }

    pub fn drain_front(&mut self) -> Vec<PendingMesh> {
        std::mem::take(&mut self.front)
    }
}
```

### 9.2 Upload Döngüsü

```
Frame N:
  CPU: Mesh gen (chunk A, B, C)  →  back buffer'a yaz
  GPU: front buffer'dan upload (önceki frame'in mesh'leri)
  Swap: front ↔ back

Frame N+1:
  CPU: Mesh gen (chunk D, E, F)  →  back buffer'a yaz
  GPU: front buffer'dan upload
  Swap: front ↔ back
```

```rust
fn mesh_upload_system(
    mut buffers: ResMut<MeshDoubleBuffer>,
    queue: Res<wgpu::Queue>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
) {
    // Front buffer'daki mesh'leri GPU'ya upload et
    for pending in buffers.drain_front() {
        let mesh_handle = mesh_assets.add(Mesh::from(&pending));
        // Entity'ye mesh component'ini ekle
    }

    // Buffer swap
    buffers.swap();
}
```

**Kaynak:** [Multi-Threaded Chunk Loading](https://rtarun9.github.io/blogs/async_copy)

---

## 10. İyileştirme 6: System Set Optimizasyonu

1000+ sistem bottleneck'ini önlemek için ([Bevy Issue #11378](https://github.com/bevyengine/bevy/issues/11378)):

### 10.1 System Set Tanımları

```rust
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameSet {
    Input,           // Input handling
    Simulation,      // Physics, AI, entities
    WorldGen,        // Chunk generation, lighting
    Meshing,         // Mesh generation
    NetworkSync,     // Network tick
    RenderPrep,      // Frustum culling, visibility
    UI,              // UI update
}
```

### 10.2 Set Sıralaması

```rust
impl Plugin for StrataPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (
            GameSet::Input,
            GameSet::Simulation.after(GameSet::Input),
            GameSet::WorldGen.after(GameSet::Simulation),
            GameSet::Meshing.after(GameSet::WorldGen),
            GameSet::NetworkSync.after(GameSet::Simulation),
            GameSet::RenderPrep.after(GameSet::Meshing),
            GameSet::UI.after(GameSet::RenderPrep),
        ));
    }
}
```

### 10.3 Set İçi Paralellik

```
Her set içindeki sistemler paralel çalışır:
  GameSet::Simulation:
    ├── physics_movement ─┐
    ├── entity_ai ────────┤  ← Bevy scheduler bunları paralel çalıştırır
    └── ground_check ─────┘     (resource conflict yoksa)

Set'ler arası sıra sabit (deterministik):
  Input → Simulation → WorldGen → Meshing → NetworkSync → RenderPrep → UI
```

---

## 11. İyileştirme 7: Adaptive Thread Pool Sizing

Sabit thread sayısı her duruma uymaz, dinamik ayarlama:

### 11.1 Metrics Toplama

```rust
#[derive(Resource)]
pub struct FrameMetrics {
    pub avg_frame_time: Duration,
    pub frame_times: VecDeque<Duration>,  // Son 60 frame
    pub pool_sizes: PoolSizes,
}

pub struct PoolSizes {
    pub compute: usize,
    pub async_compute: usize,
    pub io: usize,
}
```

### 11.2 Adaptive Sizing

```rust
fn adaptive_pool_sizing(
    mut metrics: ResMut<FrameMetrics>,
    time: Res<Time>,
) {
    metrics.frame_times.push_back(time.delta());
    if metrics.frame_times.len() > 60 {
        metrics.frame_times.pop_front();
    }

    metrics.avg_frame_time = metrics.frame_times.iter().sum::<Duration>()
        / metrics.frame_times.len() as u32;

    let max_threads = num_cpus::get();

    if metrics.avg_frame_time > Duration::from_millis(16) {
        // Frame budget aşıldı → azalt
        metrics.pool_sizes.compute =
            (metrics.pool_sizes.compute - 1).max(2);
    } else if metrics.avg_frame_time < Duration::from_millis(10) {
        // Budget altında → artır
        metrics.pool_sizes.compute =
            (metrics.pool_sizes.compute + 1).min(max_threads - 2);
    }
}
```

---

## 12. İyileştirme 8: Cache-Aware Chunk Sıralaması

Chunk'ları işlerken mekansal yakınlık önemli:

### 12.1 Morton-Ordered Processing

```rust
fn cache_aware_chunk_order(
    chunks: &mut Vec<SectorCoord>,
    player_pos: Vec3,
) {
    // Morton kodu sıralaması (Z-order curve)
    // Komşu chunk'lar bellekte yakın durur
    chunks.sort_by_key(|coord| {
        SectorMap::morton_encode(
            (coord.x + SectorMap::BIAS as i32) as u32,
            (coord.y + SectorMap::BIAS as i32) as u32,
            (coord.z + SectorMap::BIAS as i32) as u32,
        )
    });
}
```

### 12.2 Spatial Batching

```rust
fn spatial_batch_generation(
    pool: Res<ComputeTaskPool>,
    mut queue: ResMut<PriorityTaskQueue>,
) {
    // Aynı chunk grubunu tek batch'te işle
    let batch_size = 4; // 4 chunk tek task'ta
    let mut batch = Vec::with_capacity(batch_size);

    while let Some(task) = queue.pop() {
        batch.push(task);
        if batch.len() >= batch_size {
            let batch = std::mem::replace(&mut batch, Vec::with_capacity(batch_size));
            pool.spawn(async move {
                for task in batch {
                    generate_chunk(task);
                }
            }).detach();
        }
    }
}
```

---

## 13. İyileştirme 9: Graceful Degradation

CPU yükünde sistem tamamen durmamalı, kalite düşmeli:

### 13.1 Degradation Seviyeleri

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceLevel {
    Normal,    // 60 FPS
    High,      // 45-60 FPS
    Critical,  // <30 FPS
}

impl PerformanceLevel {
    pub fn from_fps(fps: f32) -> Self {
        if fps >= 55.0 { Self::Normal }
        else if fps >= 30.0 { Self::High }
        else { Self::Critical }
    }
}
```

### 13.2 Seviye Bazlı Ayarlar

```rust
fn graceful_degradation(
    metrics: Res<FrameMetrics>,
    mut config: ResMut<RuntimeConfig>,
) {
    let fps = 1.0 / metrics.avg_frame_time.as_secs_f32();
    let level = PerformanceLevel::from_fps(fps);

    match level {
        PerformanceLevel::Normal => {
            config.chunk_priorities = vec![
                ChunkPriority::Critical,
                ChunkPriority::Predictive,
                ChunkPriority::Visible,
                ChunkPriority::Background,
                ChunkPriority::Maintenance,
            ];
            config.mesh_detail = MeshDetail::Full;
            config.lighting_level = LightingLevel::Full;
            config.active_tiers = vec![
                Tier::Active, Tier::Warm, Tier::Distant, Tier::Archive,
            ];
        }
        PerformanceLevel::High => {
            config.chunk_priorities = vec![
                ChunkPriority::Critical,
                ChunkPriority::Predictive,
                ChunkPriority::Visible,
            ];
            config.mesh_detail = MeshDetail::Greedy;
            config.lighting_level = LightingLevel::L0L1;
            config.active_tiers = vec![
                Tier::Active, Tier::Warm, Tier::Distant,
            ];
            // Tier::Archive duraklat
        }
        PerformanceLevel::Critical => {
            config.chunk_priorities = vec![
                ChunkPriority::Critical,
            ];
            config.mesh_detail = MeshDetail::Minimal;
            config.lighting_level = LightingLevel::L0Only;
            config.active_tiers = vec![
                Tier::Active,
            ];
            // Sadece oyuncunun bulunduğu chunk
        }
    }
}
```

---

## 14. İyileştirme 10: Run Condition ile Boş Çalışmayı Önleme

Run condition'lar main thread'de evaluate edilir, skip edilen sistemlerin near-zero maliyeti vardır:

```rust
fn voxel_chunks_need_remeshing(
    changed_chunks: Query<(), (Changed<SectorData>, With<NeedsRemesh>)>,
) -> bool {
    !changed_chunks.is_empty()
}

fn has_dirty_chunks(
    dirty_query: Query<(), With<ChunkDirty>>,
) -> bool {
    !dirty_query.is_empty()
}

app.add_systems(Update, (
    // Sadece gerçekten kirli chunk varsa çalış
    remesh_dirty_chunks.run_if(voxel_chunks_need_remeshing),
    process_dirty_chunks.run_if(has_dirty_chunks),
    // Zaman bazlı: sadece belirli aralıkta çalış
    update_chunk_lod.run_if(on_timer(Duration::from_millis(100))),
    // FPS bazlı: sadece budget varsa çalış
    background_generation.run_if(|metrics: Res<FrameMetrics>| {
        metrics.avg_frame_time < Duration::from_millis(12)
    }),
));
```

**Etki:** Boş query'li sistemler scheduling overhead'i yaratmaz.

---

## 15. İyileştirme 11: Minimum Ordering Constraint

Gereksiz `.before()` / `.after()` constraint'leri paralelizmi kısıtlar. Sadece gerçek bağımlılıklar için ordering ekle:

```rust
// KÖTÜ: Her sistemi ayrı ayrı sırala (paralelizm kısıtlanır)
app.add_systems(Update, (
    system_a.before(system_b),
    system_b.before(system_c),
    system_c.before(system_d),
    system_d.before(system_e),
));

// İYİ: System set'leri ile hiyerarşik sıralama
// Aynı set içindeki sistemler otomatik paralel çalışır
app.configure_sets(Update, (
    ChunkSystems::Generation.before(ChunkSystems::Meshing),
    ChunkSystems::Meshing.before(ChunkSystems::Rendering),
));

app.add_systems(Update, (
    // Bu üçü paralel çalışır (aynı set, aralarında ordering yok)
    generate_chunk_terrain.in_set(ChunkSystems::Generation),
    generate_chunk_caves.in_set(ChunkSystems::Generation),
    generate_chunk_structures.in_set(ChunkSystems::Generation),
    // Bu iki de paralel çalışır
    greedy_mesh.in_set(ChunkSystems::Meshing),
    compute_lighting.in_set(ChunkSystems::Meshing),
));
```

**Kural:** Sadece veri bağımlılığı olan sistemler için ordering ekle. `A.before(B)` sadece A'nın output'u B'nin input'u ise gerekli.

---

## 16. İyileştirme 12: Frame-Budgeted Task Polling

Async task'ları poll ederken frame bütçesini aşmamak kritik:

```rust
#[derive(Resource)]
pub struct ChunkTasks {
    pub generating: HashMap<IVec3, Task<ChunkData>>,
    pub meshing: HashMap<IVec3, Task<(Handle<Mesh>, MeshData)>>,
}

fn receive_generated_chunks(
    mut tasks: ResMut<ChunkTasks>,
    mut commands: Commands,
    time: Res<Time>,
) {
    let budget = Duration::from_millis(5); // max 5ms/frame
    let start = Instant::now();

    tasks.generating.retain(|coord, task| {
        if start.elapsed() > budget {
            return true; // Zaman bitti, sonraki frame'e bırak
        }

        if let Some(chunk_data) = block_on(future::poll_once(task)) {
            commands.spawn((
                ChunkCoord(*coord),
                chunk_data,
                ChunkState::NeedsRemesh,
            ));
            false // Tamamlandı, listeden kaldır
        } else {
            true // Hâlâ çalışıyor
        }
    });
}
```

**Etki:** Frame spike'ları önlenir, chunk generation frame bütçesini aşmaz. 100 chunk tamamlanmış olsa bile sadece 5ms'lik kısmı işlenir.

---

## 17. Bevy Implementasyonu

### 17.1 Plugin Yapısı

```rust
pub struct ThreadingPlugin;

impl Plugin for ThreadingPlugin {
    fn build(&self, app: &mut App) {
        app
            // Resource'lar
            .insert_resource(PriorityTaskQueue::new())
            .insert_resource(FrameBudget::default())
            .insert_resource(FrameMetrics::default())
            .insert_resource(MeshDoubleBuffer::new())
            .insert_resource(RuntimeConfig::default())
            // System sets
            .configure_sets(Update, (
                GameSet::Input,
                GameSet::Simulation.after(GameSet::Input),
                GameSet::WorldGen.after(GameSet::Simulation),
                GameSet::Meshing.after(GameSet::WorldGen),
                GameSet::NetworkSync.after(GameSet::Simulation),
                GameSet::RenderPrep.after(GameSet::Meshing),
                GameSet::UI.after(GameSet::RenderPrep),
            ))
            // Systems (run condition'lar ile boş çalışmayı önle)
            .add_systems(Update, (
                predictive_prefetch.in_set(GameSet::WorldGen),
                time_sliced_chunk_gen
                    .in_set(GameSet::WorldGen)
                    .run_if(has_dirty_chunks),
                time_sliced_mesh_gen
                    .in_set(GameSet::Meshing)
                    .run_if(voxel_chunks_need_remeshing),
                collect_chunk_results.in_set(GameSet::Meshing),
                mesh_upload_system.in_set(GameSet::RenderPrep),
                adaptive_pool_sizing.in_set(GameSet::UI),
                graceful_degradation.in_set(GameSet::UI),
            ));
    }
}
```

### 17.2 System Çakışma Analizi

```
Paralel çalışabilen sistemler (resource conflict yok):
  ┌─ time_sliced_chunk_gen ──── PriorityTaskQueue (ResMut)
  │                             ComputeTaskPool (Res)
  │
  ├─ time_sliced_mesh_gen ───── NeedsRemesh (Query)
  │                             ComputeTaskPool (Res)
  │
  ├─ predictive_prefetch ────── StreamingPredictor (Res)
  │                             PriorityTaskQueue (ResMut)  ← ÇAKIŞMA!
  │
  └─ collect_chunk_results ──── ChunkResultChannel (Res)

Sıralı çalışması gerekenler:
  predictive_prefetch → time_sliced_chunk_gen (PriorityTaskQueue'a yazıyor)
  time_sliced_mesh_gen → mesh_upload_system (mesh data transfer)
```

---

## 18. Risk Analizi

| Risk | Olasılık | Etki | Mitigasyon |
|------|----------|------|------------|
| ComputePool deadlock | Düşük | Yüksek | Bevy scoped fork-join deadlock-free |
| Boundary sync overhead | Orta | Orta | Channel-based async, batch transfer |
| 1000+ sistem bottleneck | Orta | Orta | 7 set grouping, sistem sayısı 200-300 |
| Determinizm kaybı | Düşük | Yüksek | Main thread sıra sabit, sadece data paralel |
| Nested parallelism | Düşük | Yüksek | Rayon kullanma, Bevy scoped task kullan |
| Frame budget aşımı | Orta | Orta | Time slicing + graceful degradation |
| Priority starvation | Düşük | Orta | Background task'lar için min 10% budget |

---

## 19. Performans Hedefleri

| Metrik | Hedef | Ölçüm |
|--------|-------|-------|
| Frame time | <16.6ms (60 FPS) | avg son 60 frame |
| Chunk gen latency | <50ms/chunk | P0 chunk için |
| Mesh gen latency | <20ms/chunk | Greedy mesh |
| Priority queue latency | <1ms | P0 task queue'dan çıkana kadar |
| Thread utilization | >70% | Tüm core'lar aktif |
| Graceful degradation | <30 FPS'de aktif | Automatic kalite düşürme |

---

## 20. Kaynaklar

| Konu | Kaynak |
|------|--------|
| Bevy Tasks API | [bevy_tasks docs](https://docs.rs/bevy/latest/bevy/tasks/index.html) |
| Bevy AsyncComputeTaskPool | [AsyncComputeTaskPool](https://docs.rs/bevy/latest/bevy/tasks/struct.AsyncComputeTaskPool.html) |
| Bevy Async Compute Example | [GitHub](https://github.com/bevyengine/bevy/blob/main/examples/async_tasks/async_compute.rs) |
| Bevy Cheat Book - Background | [Cheat Book](https://bevy-cheatbook.github.io/fundamentals/async-compute.html) |
| System Bottleneck Issue | [Issue #11378](https://github.com/bevyengine/bevy/issues/11378) |
| System Ordering | [System Order](https://bevy-cheatbook.github.io/programming/system-order.html) |
| Priority Chunk Gen | [Reddit](https://www.reddit.com/r/VoxelGameDev/comments/1qrk3j7/i_think_i_have_finally_mastered_priority_based) |
| Time Slicing | [Allen Chou](https://allenchou.net/2021/05/time-slicing) |
| Multi-Threaded Chunk Loading | [rtarun9](https://rtarun9.github.io/blogs/async_copy) |
| CPU Affinity / Core Pinning | [Medium](https://medium.com/@sagar.necindia/cpp-cpu-affinity-core-pinning-ba6749faaa65) |
| Work Stealing Scheduler | [Joe Duffy](http://joeduffyblog.com/2008/08/11/building-a-custom-thread-pool-series-part-2-a-work-stealing-queue) |
| High Perf Voxel Engine | [Nick's Blog](https://nickmcd.me/2021/04/04/high-performance-voxel-engine) |
| Chunk Loading Lag | [VoxelGameDev](https://www.reddit.com/r/VoxelGameDev/comments/oejz2d/chunk_loading_on_worker_threads_lags_main_thread) |
| Rayon Optimization | [gendignoux](https://gendignoux.com/blog/2024/11/18/rust-rayon-optimized.html) |
| Veloren Architecture | [Veloren Book](https://book.veloren.net/contributors/developers/architecture.html) |

---

## 21. Bağlantılı Planlar

| Plan | Bağlantı |
|------|----------|
| `plans/03-ecs-architecture.md` | System sets, component design, event system (BİTTİ) |
| `plans/06-xbrickmap.md` | Sector data structure, GPU feedback loop (BİTTİ) |
| `plans/08-streaming.md` | 4-tier streaming, StreamingPredictor |
| `plans/09-meshing.md` | Mesher trait, greedy mesh, GPU compute mesh |
| `plans/13-lighting.md` | Lighting BFS, wavefront propagation |
| `plans/39-memory-allocation.md` | mimalloc v3, hybrid pool strategy |
