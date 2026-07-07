# 16 — Debug & Profiling Sistemi

## 1. Genel Bakış

Strata'nın debug ve profiling sistemi, **performans hedeflerini** doğrulamak ve **runtime sorunlarını** tespit etmek için tasarlanmıştır.

### Temel Prensipler

- **Zero-cost when disabled:** Debug feature kapalıyken overhead = 0
- **Real-time metrics:** Runtime metrikler HUD'da gösterilir
- **Tracing-based:** `tracing` crate ile structured logging
- **GPU profiling:** Render pass'leri GPU timeline'da izlenir

---

## 2. Debug HUD

```rust
/// Debug HUD component'i.
#[derive(Component)]
pub struct DebugHud {
    pub visible: bool,
    pub panels: Vec<DebugPanel>,
}

#[derive(Clone)]
pub enum DebugPanel {
    /// FPS ve frame time.
    Performance,

    /// Memory kullanımı.
    Memory,

    /// World bilgisi (sector sayısı, tier dağılımı).
    World,

    /// Network istatistikleri.
    Network,

    /// Render istatistikleri.
    Render,

    /// Physics istatistikleri.
    Physics,

    /// Lighting istatistikleri.
    Lighting,

    /// Storage istatistikleri.
    Storage,

    /// Chunk meshing bilgisi.
    Meshing,

    /// Entity bilgisi.
    Entities,
}
```

### 2.1 Performance Panel

```
┌─────────────────────────────────────────┐
│ Performance                              │
├─────────────────────────────────────────┤
│ FPS:          144.2                      │
│ Frame Time:   6.93ms                     │
│ ├─ Input:     0.12ms (1.7%)             │
│ ├─ Player:    0.05ms (0.7%)             │
│ ├─ Streaming: 0.31ms (4.5%)             │
│ ├─ Physics:   1.24ms (17.9%)            │
│ ├─ Lighting:  0.87ms (12.6%)            │
│ ├─ Network:   0.15ms (2.2%)             │
│ ├─ Render:    3.42ms (49.3%)            │
│ └─ Storage:   0.12ms (1.7%)             │
│                                          │
│ CPU Threads:  8/16                       │
│ GPU:          45.2%                      │
└─────────────────────────────────────────┘
```

### 2.2 World Panel

```
┌─────────────────────────────────────────┐
│ World                                    │
├─────────────────────────────────────────┤
│ Loaded Sectors:  342                     │
│ ├─ Active:       27  (0-96m)            │
│ ├─ Warm:         89  (96-384m)          │
│ ├─ Distant:      226 (384m-1.5km)       │
│ └─ Archive:      0   (1.5km+)           │
│                                          │
│ Dirty Sectors:   3                       │
│ Pending Bake:    2                       │
│ Pending Unbake:  1                       │
│                                          │
│ Player Position: 1247.3, 64.0, -892.1    │
│ Current Biome:   Plains                  │
│ Time of Day:     14:32                   │
└─────────────────────────────────────────┘
```

### 2.3 Render Panel

```
┌─────────────────────────────────────────┐
│ Render                                   │
├─────────────────────────────────────────┤
│ Visible Sectors:   87                    │
│ Culled Sectors:    255                   │
│                                          │
│ Draw Calls:        142                   │
│ Triangles:         2.4M                  │
│ Vertices:          1.8M                  │
│                                          │
│ VRAM:              1.2GB / 2.0GB         │
│ ├─ Textures:       456MB                 │
│ ├─ Buffers:        312MB                 │
│ ├─ SVDAG Pool:     8.2MB                 │
│ └─ Vertex Pool:    128MB                 │
│                                          │
│ Foveated:          ON                    │
│ Ray/Pixel Ratio:   0.34x                 │
└─────────────────────────────────────────┘
```

### 2.4 Network Panel

```
┌─────────────────────────────────────────┐
│ Network                                  │
├─────────────────────────────────────────┤
│ Connected:         47 clients            │
│                                          │
│ Bandwidth:                              │
│ ├─ Upload:         245 KB/s              │
│ └─ Download:       1.2 MB/s              │
│                                          │
│ Per Client:        5.2 KB/s avg          │
│ RTT:               42ms avg              │
│ Packet Loss:       0.3% avg              │
│                                          │
│ Packets/s:         1,247                 │
│ Delta Updates:     89/s                  │
│ Snapshots:         3/s                   │
└─────────────────────────────────────────┘
```

---

## 3. Tracing & Logging

```rust
/// Strata tracing subscriber konfigürasyonu.
pub fn init_tracing() {
    use tracing_subscriber::{
        EnvFilter,
        fmt::format::FmtSpan,
    };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("strata=info,wgpu=warn"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_thread_ids(true)
        .with_target(false)
        .init();
}

/// Kullanım örnekleri:
///
/// ```rust
/// // Info
/// tracing::info!(sector = ?coord, "Sector loaded");
///
/// // Warning
/// tracing::warn!(tier = ?old_tier, "Tier transition slow");
///
/// // Error
/// tracing::error!(error = %e, "Failed to bake SVDAG");
///
/// // Span (performance measurement)
/// #[tracing::instrument(skip(sector))]
/// fn bake_svgdag(sector: &Sector) -> Result<u32> {
///     // ...
/// }
///
/// // Custom metrics
/// tracing::debug!(
///     bake_time_ms = elapsed.as_millis(),
///     node_count = nodes.len(),
///     "SVDAG bake complete"
/// );
/// ```
```

---

## 4. Performance Metrics

```rust
/// Performance metrik toplayıcı.
#[derive(Resource)]
pub struct MetricsCollector {
    /// Frame time history (son 120 frame).
    frame_times: RingBuffer<f32>,

    /// Sistem bazlı süreler.
    system_times: HashMap<String, RingBuffer<f32>>,

    /// Memory kullanımı.
    memory_usage: MemoryUsage,

    /// GPU metrikleri.
    gpu_metrics: GpuMetrics,

    /// Counter'lar.
    counters: HashMap<String, AtomicU64>,

    /// Gauge'lar.
    gauges: HashMap<String, AtomicF32>,
}

impl MetricsCollector {
    /// Frame başlangıcı.
    pub fn begin_frame(&mut self) {
        self.frame_start = Instant::now();
    }

    /// Frame sonu.
    pub fn end_frame(&mut self) {
        let frame_time = self.frame_start.elapsed().as_secs_f32();
        self.frame_times.push(frame_time);

        // FPS hesapla
        let fps = 1.0 / self.frame_times.average();
        self.gauges.get("fps").unwrap().store(fps);
    }

    /// Sistem süresi ölç.
    pub fn measure_system<F, T>(&mut self, name: &str, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let start = Instant::now();
        let result = f();
        let elapsed = start.elapsed().as_secs_f32();

        if let Some(buffer) = self.system_times.get_mut(name) {
            buffer.push(elapsed);
        }

        result
    }

    /// Counter artır.
    pub fn increment_counter(&self, name: &str) {
        if let Some(counter) = self.counters.get(name) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Gauge ayarla.
    pub fn set_gauge(&self, name: &str, value: f32) {
        if let Some(gauge) = self.gauges.get(name) {
            gauge.store(value);
        }
    }

    /// Metrik raporunu al.
    pub fn get_report(&self) -> MetricsReport {
        MetricsReport {
            fps: self.gauges.get("fps").unwrap().load(),
            frame_time_p99: self.frame_times.percentile(99),
            frame_time_avg: self.frame_times.average(),
            system_times: self.system_times.iter()
                .map(|(k, v)| (k.clone(), v.average()))
                .collect(),
            memory: self.memory_usage.snapshot(),
            gpu: self.gpu_metrics.snapshot(),
            counters: self.counters.iter()
                .map(|(k, v)| (k.clone(), v.load()))
                .collect(),
        }
    }
}
```

---

## 5. GPU Profiling

```rust
/// GPU profiling — wgpu timestamp query.
pub struct GpuProfiler {
    /// Query set (timestamp).
    query_set: wgpu::QuerySet,

    /// Resolve buffer (timestamp sonuçları).
    resolve_buffer: wgpu::Buffer,

    /// Read buffer (CPU'ya okuma).
    read_buffer: wgpu::Buffer,

    /// Aktif query'ler.
    active_queries: Vec<GpuQuery>,
}

pub struct GpuQuery {
    pub name: String,
    pub start_index: u32,
    pub end_index: u32,
}

impl GpuProfiler {
    /// Yeni GPU profiler oluştur.
    pub fn new(device: &wgpu::Device, max_queries: u32) -> Self {
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("GpuProfiler"),
            count: max_queries * 2, // start + end per query
            ty: wgpu::QueryType::Timestamp,
        });

        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("GpuProfiler Resolve"),
            size: max_queries as u64 * 2 * 8, // 8 bytes per timestamp
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let read_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("GpuProfiler Read"),
            size: max_queries as u64 * 2 * 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            query_set,
            resolve_buffer,
            read_buffer,
            active_queries: Vec::new(),
        }
    }

    /// GPU query başlat.
    pub fn begin_query(&mut self, name: &str, encoder: &mut wgpu::CommandEncoder) {
        let index = self.active_queries.len() as u32;
        encoder.write_timestamp(&self.query_set, index * 2);

        self.active_queries.push(GpuQuery {
            name: name.to_string(),
            start_index: index * 2,
            end_index: index * 2 + 1,
        });
    }

    /// GPU query bitir.
    pub fn end_query(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(query) = self.active_queries.last() {
            encoder.write_timestamp(&self.query_set, query.end_index);
        }
    }

    /// Sonuçları resolve et (GPU tarafı).
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(
            &self.query_set,
            0..self.active_queries.len() as u32 * 2,
            &self.resolve_buffer,
            0,
        );
        encoder.copy_buffer_to_buffer(
            &self.resolve_buffer,
            0,
            &self.read_buffer,
            0,
            self.active_queries.len() as u64 * 2 * 8,
        );
    }

    /// Sonuçları oku (CPU tarafı — async).
    pub fn read_results(&self, device: &wgpu::Device) -> Vec<(String, f32)> {
        let buffer_slice = self.read_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);

        let data = buffer_slice.get_mapped_range();
        let timestamps: &[u64] = bytemuck::cast_slice(&data);

        let mut results = Vec::new();
        for (i, query) in self.active_queries.iter().enumerate() {
            let start = timestamps[i * 2];
            let end = timestamps[i * 2 + 1];
            // Timestamp → ms: (end - start) * timestamp_period / 1_000_000
            let duration_ms = (end - start) as f32 * 0.000001; // placeholder
            results.push((query.name.clone(), duration_ms));
        }

        self.read_buffer.unmap();
        results
    }
}
```

---

## 6. Debug Render

```rust
/// Debug render sistemi — wireframe, AABB, ray visualization.
pub struct DebugRenderer {
    /// Wireframe line buffer.
    line_buffer: Vec<DebugLine>,

    /// AABB box buffer.
    box_buffer: Vec<DebugBox>,

    /// Text overlay buffer.
    text_buffer: Vec<DebugText>,

    /// GPU buffer'ları (wgpu).
    line_gpu: wgpu::Buffer,
    box_gpu: wgpu::Buffer,
}

pub struct DebugLine {
    pub start: Vec3,
    pub end: Vec3,
    pub color: [f32; 4],
}

pub struct DebugBox {
    pub min: Vec3,
    pub max: Vec3,
    pub color: [f32; 4],
}

pub struct DebugText {
    pub position: Vec3,
    pub text: String,
    pub color: [f32; 4],
}

impl DebugRenderer {
    /// Sector AABB'lerini çiz.
    pub fn draw_sector_aabbs(&mut self, sectors: &[(SectorCoord, Tier)]) {
        for (coord, tier) in sectors {
            let origin = coord.world_origin();
            let color = match tier {
                Tier::Active => [0.0, 1.0, 0.0, 0.5],
                Tier::Warm => [1.0, 1.0, 0.0, 0.3],
                Tier::Distant => [1.0, 0.5, 0.0, 0.2],
                Tier::Archive => [1.0, 0.0, 0.0, 0.1],
            };

            self.box_buffer.push(DebugBox {
                min: origin.as_vec3(),
                max: (origin + IVec3::new(32, 32, 32)).as_vec3(),
                color,
            });
        }
    }

    /// Ray trace sonucunu çiz.
    pub fn draw_ray(&mut self, origin: Vec3, direction: Vec3, hit: Option<HitResult>) {
        let end = if let Some(hit) = hit {
            origin + direction * hit.t
        } else {
            origin + direction * 100.0
        };

        let color = if hit.is_some() {
            [1.0, 0.0, 0.0, 1.0]
        } else {
            [0.0, 1.0, 1.0, 0.5]
        };

        self.line_buffer.push(DebugLine {
            start: origin,
            end,
            color,
        });
    }

    /// Collider'ları çiz.
    pub fn draw_colliders(&mut self, colliders: &[(Vec3, ColliderType)]) {
        for (pos, collider) in colliders {
            let color = [0.0, 0.5, 1.0, 0.5];

            match collider {
                ColliderType::Aabb(size) => {
                    self.box_buffer.push(DebugBox {
                        min: pos - size / 2.0,
                        max: pos + size / 2.0,
                        color,
                    });
                }
                ColliderType::Capsule(radius, height) => {
                    // Capsule visualization
                }
                ColliderType::Voxels(bounds) => {
                    self.box_buffer.push(DebugBox {
                        min: bounds.min,
                        max: bounds.max,
                        color,
                    });
                }
            }
        }
    }
}
```

---

## 7. Benchmark Sistemi

```rust
/// Benchmark runner.
pub struct BenchmarkRunner {
    /// Benchmark sonuçları.
    results: Vec<BenchmarkResult>,
}

pub struct BenchmarkResult {
    pub name: String,
    pub iterations: u32,
    pub mean_ms: f32,
    pub median_ms: f32,
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub min_ms: f32,
    pub max_ms: f32,
}

impl BenchmarkRunner {
    /// Benchmark çalıştır.
    pub fn run<F>(&mut self, name: &str, iterations: u32, mut f: F) -> BenchmarkResult
    where
        F: FnMut(),
    {
        let mut times = Vec::with_capacity(iterations as usize);

        // Warmup
        for _ in 0..3 {
            f();
        }

        // Benchmark
        for _ in 0..iterations {
            let start = Instant::now();
            f();
            times.push(start.elapsed().as_secs_f32() * 1000.0); // ms
        }

        times.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let result = BenchmarkResult {
            name: name.to_string(),
            iterations,
            mean_ms: times.iter().sum::<f32>() / times.len() as f32,
            median_ms: times[times.len() / 2],
            p95_ms: times[(times.len() as f32 * 0.95) as usize],
            p99_ms: times[(times.len() as f32 * 0.99) as usize],
            min_ms: times[0],
            max_ms: times[times.len() - 1],
        };

        self.results.push(result.clone());
        result
    }

    /// Tüm benchmark'ları çalıştır.
    pub fn run_all(&mut self) {
        // Render benchmarks
        self.run("xbrickmap_ray_trace", 1000, || {
            // ...
        });

        self.run("svdag_ray_march", 1000, || {
            // ...
        });

        // Physics benchmarks
        self.run("collider_update_single", 10000, || {
            // ...
        });

        self.run("collider_update_region", 1000, || {
            // ...
        });

        // Lighting benchmarks
        self.run("bfs_propagation_level14", 1000, || {
            // ...
        });

        self.run("sky_light_propagation", 100, || {
            // ...
        });

        // Streaming benchmarks
        self.run("sector_bake", 100, || {
            // ...
        });

        self.run("sector_unbake", 100, || {
            // ...
        });

        // Storage benchmarks
        self.run("sector_serialize", 10000, || {
            // ...
        });

        self.run("sector_deserialize", 10000, || {
            // ...
        });
    }

    /// Sonuçları raporla.
    pub fn report(&self) -> String {
        let mut output = String::from("=== Benchmark Results ===\n\n");

        for result in &self.results {
            output.push_str(&format!(
                "{}\n  Iterations: {}\n  Mean: {:.2}ms | Median: {:.2}ms | P95: {:.2}ms | P99: {:.2}ms\n  Min: {:.2}ms | Max: {:.2}ms\n\n",
                result.name,
                result.iterations,
                result.mean_ms,
                result.median_ms,
                result.p95_ms,
                result.p99_ms,
                result.min_ms,
                result.max_ms,
            ));
        }

        output
    }
}
```

---

## 8. Crate Organizasyonu

```
crates/
  debug/
    ├── mod.rs              ← Debug plugin entry point
    ├── hud/
    │   ├── mod.rs          ← Debug HUD
    │   ├── panels.rs       ← Debug panel'leri
    │   └── renderer.rs     ← HUD render (glyphon)
    ├── metrics/
    │   ├── mod.rs          ← MetricsCollector
    │   ├── counters.rs     ← Counter'lar
    │   ├── gauges.rs       ← Gauge'lar
    │   └── report.rs       ← MetricsReport
    ├── profiler/
    │   ├── mod.rs          ← Profiler
    │   ├── cpu.rs          ← CPU profiling
    │   └── gpu.rs          ← GPU profiling (wgpu timestamp query)
    ├── render/
    │   ├── mod.rs          ← DebugRenderer
    │   ├── lines.rs        ← Debug line rendering
    │   ├── boxes.rs        ← Debug box rendering
    │   └── text.rs         ← Debug text overlay
    ├── benchmark/
    │   ├── mod.rs          ← BenchmarkRunner
    │   └── suites.rs       ← Benchmark suite'leri
    └── tracing.rs          ← Tracing setup
```


# 22 — Testing & Benchmark Stratejisi

## 1. Genel Bakış

Strata'nın testing stratejisi **4 katmanlı** test piramidi kullanır: unit tests, integration tests, benchmark tests, ve visual regression tests.

### Temel Prensipler

- **Test piramidi:** Çok unit, az integration, az benchmark
- **Deterministik:** Aynı girdi = aynı çıktı (world gen, physics)
- **CI/CD:** Her commit'te testler otomatik çalışır
- **Performance regression:** Benchmark sonuçları takip edilir

---

## 2. Unit Tests

### 2.1 Block Registry Tests

```rust
#[cfg(test)]
mod block_registry_tests {
    use super::*;

    #[test]
    fn test_register_block() {
        let mut registry = BlockRegistry::new();
        let id = registry.register(BlockDefinition {
            name: "stone".into(),
            // ...
        }).unwrap();

        assert_eq!(id, 1);
        assert_eq!(registry.get_id("stone"), Some(1));
    }

    #[test]
    fn test_duplicate_name_rejected() {
        let mut registry = BlockRegistry::new();
        registry.register(BlockDefinition { name: "stone".into(), .. }).unwrap();

        let result = registry.register(BlockDefinition { name: "stone".into(), .. });
        assert!(result.is_err());
    }

    #[test]
    fn test_block_flags() {
        let flags = BlockFlags(BlockFlags::OPAQUE | BlockFlags::EMITS_LIGHT);
        assert!(flags.is_opaque());
        assert!(flags.emits_light());
        assert!(!flags.is_passable());
    }

    #[test]
    fn test_block_state_encoding() {
        let state = BlockState { type_id: 42, variant: 7 };
        let id = state.to_id();

        assert_eq!(id & 0x0FFF, 42);
        assert_eq!((id >> 12) & 0xF, 7);

        let decoded = BlockState::from_id(id);
        assert_eq!(decoded.type_id, 42);
        assert_eq!(decoded.variant, 7);
    }
}
```

### 2.2 XBrickMap Tests

```rust
#[cfg(test)]
mod xbrickmap_tests {
    use super::*;

    #[test]
    fn test_empty_sector() {
        let sector = Sector::empty();
        assert_eq!(sector.get_block(IVec3::new(0, 0, 0)), None);
    }

    #[test]
    fn test_set_and_get_block() {
        let mut sector = Sector::empty();
        sector.set_block(IVec3::new(5, 10, 15), Some(42));

        assert_eq!(sector.get_block(IVec3::new(5, 10, 15)), Some(42));
        assert_eq!(sector.get_block(IVec3::new(5, 10, 16)), None);
    }

    #[test]
    fn test_remove_block() {
        let mut sector = Sector::empty();
        sector.set_block(IVec3::new(5, 10, 15), Some(42));
        sector.set_block(IVec3::new(5, 10, 15), None);

        assert_eq!(sector.get_block(IVec3::new(5, 10, 15)), None);
    }

    #[test]
    fn test_boundary_coordinates() {
        let mut sector = Sector::empty();

        // Min boundary
        sector.set_block(IVec3::new(0, 0, 0), Some(1));
        assert_eq!(sector.get_block(IVec3::new(0, 0, 0)), Some(1));

        // Max boundary
        sector.set_block(IVec3::new(31, 127, 31), Some(2));
        assert_eq!(sector.get_block(IVec3::new(31, 127, 31)), Some(2));
    }

    #[test]
    fn test_popcnt_correctness() {
        let mask: u64 = 0b10101010_10101010_10101010_10101010_10101010_10101010_10101010_10101010;
        assert_eq!(mask.count_ones(), 32);
    }

    #[test]
    fn test_soa_layout_consistency() {
        // AOS ve SOA layout'lar aynı sonucu vermeli
        let mut aos_slab = SlabAOS::new();
        let mut soa_slab = SlabSOA::new();

        // Aynı veriyi her ikisine de yaz
        for i in 0..10 {
            aos_slab.set_brick(i, BrickData { mask: 0xFF, .. });
            soa_slab.set_brick(i, BrickData { mask: 0xFF, .. });
        }

        // Sonuçları karşılaştır
        for i in 0..10 {
            assert_eq!(aos_slab.get_brick(i), soa_slab.get_brick(i));
        }
    }
}
```

### 2.3 SVDAG Tests

```rust
#[cfg(test)]
mod svdag_tests {
    use super::*;

    #[test]
    fn test_node_pool_alloc() {
        let mut pool = SharedNodePool::new(1024);
        let idx = pool.alloc().unwrap();
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_node_pool_free() {
        let mut pool = SharedNodePool::new(1024);
        let idx = pool.alloc().unwrap();
        pool.free(idx);

        // Serbest slot yeniden kullanılabilir
        let idx2 = pool.alloc().unwrap();
        assert_eq!(idx2, idx);
    }

    #[test]
    fn test_node_pool_exhaustion() {
        let mut pool = SharedNodePool::new(2);
        pool.alloc().unwrap();
        pool.alloc().unwrap();

        assert!(pool.alloc().is_err());
    }

    #[test]
    fn test_deduplication() {
        let mut pool = SharedNodePool::new(1024);

        // Aynı geometri iki kez oluştur
        let node1 = pool.create_leaf(42);
        let node2 = pool.create_leaf(42);

        // Deduplication aynı node'u döndürmeli
        assert_eq!(node1, node2);
    }

    #[test]
    fn test_transform_aware_dedup() {
        let mut pool = SharedNodePool::new(1024);

        // Aynı geometri, farklı transform
        let node1 = pool.create_leaf(42);
        let node2 = pool.create_leaf_with_transform(42, SvdagTransform::MirrorX);

        // Transform-aware dedup farklı node döndürür
        // ama aynı geometriyi referans eder
        assert_ne!(node1, node2);
    }
}
```

### 2.4 Lighting Tests

```rust
#[cfg(test)]
mod lighting_tests {
    use super::*;

    #[test]
    fn test_light_data_packing() {
        let light = LightData::new(15, 10, 8, 5);

        assert_eq!(light.sky(), 15);
        assert_eq!(light.block_r(), 10);
        assert_eq!(light.block_g(), 8);
        assert_eq!(light.block_b(), 5);
    }

    #[test]
    fn test_light_data_clamping() {
        // 4-bit max = 15
        let light = LightData::new(20, 20, 20, 20);

        assert_eq!(light.sky(), 15);
        assert_eq!(light.block_r(), 15);
        assert_eq!(light.block_g(), 15);
        assert_eq!(light.block_b(), 15);
    }

    #[test]
    fn test_wlp_max() {
        let a = 0x00030005; // R=5, G=3
        let b = 0x00070002; // R=2, G=7

        let result = LightData::wlp_max(a, b);

        // Her kanal için max alınmalı
        assert_eq!(result & 0xF, 7);  // G: max(3, 7) = 7
        assert_eq!((result >> 4) & 0xF, 5); // R: max(5, 2) = 5
    }

    #[test]
    fn test_bfs_propagation() {
        let mut engine = BlockLightEngine::new();
        let mut sector = Sector::empty();

        // Işık kaynağı yerleştir
        let updates = engine.place_light(&sector, IVec3::new(5, 10, 5), 15, LightColor::Red);

        assert!(!updates.is_empty());

        // En yakın komşu level 14 olmalı
        let neighbor_light = sector.get_light(IVec3::new(6, 10, 5));
        assert_eq!(neighbor_light, 14);
    }

    #[test]
    fn test_bfs_removal() {
        let mut engine = BlockLightEngine::new();
        let mut sector = Sector::empty();

        // Işık kaynağı yerleştir
        engine.place_light(&sector, IVec3::new(5, 10, 5), 15, LightColor::Red);

        // Işık kaynağını kaldır
        let updates = engine.remove_light(&sector, IVec3::new(5, 10, 5), LightColor::Red);

        // Tüm light level'lar sıfırlanmalı
        for update in &updates {
            assert_eq!(update.light, 0);
        }
    }
}
```

---

## 3. Integration Tests

### 3.1 World Generation Integration

```rust
#[cfg(test)]
mod world_gen_integration {
    use super::*;

    #[test]
    fn test_deterministic_generation() {
        let seed = WorldSeed { seed: 12345, .. };
        let gen1 = TerrainGenerator::from_seed(seed);
        let gen2 = TerrainGenerator::from_seed(seed);

        let sector1 = gen1.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector2 = gen2.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));

        // Aynı seed = aynı sonuç
        assert_eq!(sector1, sector2);
    }

    #[test]
    fn test_different_sectors_different() {
        let gen = TerrainGenerator::from_seed(WorldSeed { seed: 12345, .. });

        let sector1 = gen.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector2 = gen.generate_sector(SectorCoord(IVec3::new(1, 0, 0)));

        // Farklı koordinatlar = farklı sonuç (büyük ihtimalle)
        assert_ne!(sector1, sector2);
    }

    #[test]
    fn test_sector_boundary_continuity() {
        let gen = TerrainGenerator::from_seed(WorldSeed { seed: 12345, .. });

        let sector_a = gen.generate_sector(SectorCoord(IVec3::new(0, 0, 0)));
        let sector_b = gen.generate_sector(SectorCoord(IVec3::new(1, 0, 0)));

        // Boundary'deki bloklar uyumlu olmalı
        for y in 0..128 {
            for z in 0..32 {
                let block_a = sector_a.get_block(IVec3::new(31, y, z));
                let block_b = sector_b.get_block(IVec3::new(0, y, z));

                assert_eq!(block_a, block_b);
            }
        }
    }
}
```

### 3.2 Physics Integration

```rust
#[cfg(test)]
mod physics_integration {
    use super::*;

    #[test]
    fn test_character_ground_check() {
        let mut sector = Sector::empty();

        // Zemin oluştur
        for x in 0..32 {
            for z in 0..32 {
                sector.set_block(IVec3::new(x, 0, z), Some(STONE));
            }
        }

        let controller = CharacterController::new();
        let ground = controller.ground_check_xbrickmap(
            &sector,
            Vec3::new(16.0, 2.0, 16.0),
            0.3,
        );

        assert!(matches!(ground, GroundState::Grounded { .. }));
    }

    #[test]
    fn test_character_air_check() {
        let sector = Sector::empty();

        let controller = CharacterController::new();
        let ground = controller.ground_check_xbrickmap(
            &sector,
            Vec3::new(16.0, 50.0, 16.0),
            0.3,
        );

        assert!(matches!(ground, GroundState::Air));
    }

    #[test]
    fn test_collider_incremental_update() {
        let mut sector = Sector::empty();
        let mut collider = sector.build_collider();

        // Tek blok ekle
        sector.set_block(IVec3::new(5, 10, 5), Some(STONE));
        let changes = sector.get_changes();

        sector.update_collider(&mut collider, &changes);

        // Collider güncellenmiş olmalı
        assert!(collider.is_valid());
    }
}
```

### 3.3 Network Integration

```rust
#[cfg(test)]
mod network_integration {
    use super::*;

    #[test]
    fn test_delta_encoding_roundtrip() {
        let mut encoder = DeltaEncoder::new();
        let entity = Entity::from_raw(1);

        let pos = Vec3::new(10.5, 20.3, 30.7);
        let rot = Quat::from_rotation_y(0.5);

        let encoded = encoder.encode_entity(entity, pos, rot);
        assert!(!encoded.is_empty());

        // Decode ve doğrula
        let decoded = decode_entity(&encoded);
        assert!((decoded.position.x - pos.x).abs() < 0.01);
    }

    #[test]
    fn test_quantization_precision() {
        let pos = Vec3::new(100.123, 50.456, -200.789);
        let quantized = QuantizedPosition::from_vec3(pos);
        let restored = quantized.to_vec3();

        // 1cm hassasiyet
        assert!((restored.x - pos.x).abs() < 0.01);
        assert!((restored.y - pos.y).abs() < 0.01);
        assert!((restored.z - pos.z).abs() < 0.01);
    }

    #[test]
    fn test_aoi_subscription() {
        let mut manager = InterestManager::new();
        let player = Entity::from_raw(1);

        // Oyuncuyu ekle
        manager.add_player(player, 100.0);

        // Pozisyon güncelle
        manager.update_player_position(player, Vec3::new(0.0, 0.0, 0.0));

        // Abonelikleri hesapla
        manager.update(0.016);

        // Yakın sector'lar abonelikte olmalı
        let subscriptions = manager.get_subscriptions(player);
        assert!(!subscriptions.is_empty());
    }
}
```

---

## 4. Benchmark Tests

```rust
#[cfg(test)]
mod benchmarks {
    use super::*;
    use criterion::{criterion_group, criterion_main, Criterion};

    fn benchmark_xbrickmap_get_block(c: &mut Criterion) {
        let mut sector = Sector::empty();

        // Dolu sector oluştur
        for x in 0..32 {
            for y in 0..128 {
                for z in 0..32 {
                    if rand::random::<f32>() > 0.5 {
                        sector.set_block(IVec3::new(x, y, z), Some(STONE));
                    }
                }
            }
        }

        c.bench_function("xbrickmap_get_block", |b| {
            b.iter(|| {
                let x = rand::random::<i32>() % 32;
                let y = rand::random::<i32>() % 128;
                let z = rand::random::<i32>() % 32;
                black_box(sector.get_block(IVec3::new(x, y, z)));
            });
        });
    }

    fn benchmark_xbrickmap_set_block(c: &mut Criterion) {
        let mut sector = Sector::empty();

        c.bench_function("xbrickmap_set_block", |b| {
            b.iter(|| {
                let x = rand::random::<i32>() % 32;
                let y = rand::random::<i32>() % 128;
                let z = rand::random::<i32>() % 32;
                black_box(sector.set_block(IVec3::new(x, y, z), Some(STONE)));
            });
        });
    }

    fn benchmark_lighting_propagation(c: &mut Criterion) {
        let mut engine = BlockLightEngine::new();
        let sector = Sector::empty();

        c.bench_function("lighting_propagation_level14", |b| {
            b.iter(|| {
                black_box(engine.place_light(
                    &sector,
                    IVec3::new(16, 64, 16),
                    15,
                    LightColor::Red,
                ));
            });
        });
    }

    fn benchmark_svdag_bake(c: &mut Criterion) {
        let sector = generate_test_sector();

        c.bench_function("svdag_bake", |b| {
            b.iter(|| {
                black_box(bake_sector_to_svgdag(&sector));
            });
        });
    }

    fn benchmark_pathfinding(c: &mut Criterion) {
        let world = generate_test_world();
        let mut pathfinder = VoxelPathfinder::new();

        c.bench_function("pathfinding_100_blocks", |b| {
            b.iter(|| {
                black_box(pathfinder.find_path(
                    IVec3::new(0, 64, 0),
                    IVec3::new(100, 64, 100),
                    &world,
                    1000,
                ));
            });
        });
    }

    criterion_group!(
        benches,
        benchmark_xbrickmap_get_block,
        benchmark_xbrickmap_set_block,
        benchmark_lighting_propagation,
        benchmark_svdag_bake,
        benchmark_pathfinding,
    );
    criterion_main!(benches);
}
```

---

## 5. CI/CD Pipeline

```yaml
# .github/workflows/ci.yml
name: CI

on:
  push:
    branches: [main, dev]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt

      - name: Cache dependencies
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}

      - name: Format check
        run: cargo fmt -- --check

      - name: Clippy
        run: cargo clippy --workspace -- -D warnings

      - name: Unit tests
        run: cargo test --workspace

      - name: Integration tests
        run: cargo test --workspace --test '*'

      - name: Benchmarks (quick)
        run: cargo bench -- --noplot

  benchmark:
    runs-on: windows-latest
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v4

      - name: Run benchmarks
        run: cargo bench -- --output-format bencher | tee benchmark_results.txt

      - name: Upload results
        uses: actions/upload-artifact@v3
        with:
          name: benchmark-results
          path: benchmark_results.txt
```

---

## 6. Crate Organizasyonu

```
crates/
  core/
    src/
      # ... unit tests inline (#[cfg(test)] mod tests)

  tests/
    src/
      # Integration tests
      ├── world_gen.rs
      ├── physics.rs
      ├── network.rs
      ├── lighting.rs
      └── storage.rs

  benches/
    src/
      # Benchmark tests
      ├── xbrickmap.rs
      ├── svdag.rs
      ├── lighting.rs
      ├── pathfinding.rs
      └── storage.rs
```


# 50 — Crash Reporting & Telemetry

## 1. Genel Bakış

Strata'nın hata raporlama ve telemetri sistemi çökmeleri ve kullanım verilerini toplar.

### Temel Prensipler

- **Crash dump:** Otomatik crash raporu oluşturma
- **Stack trace:** Detaylı hata izleme
- **Opt-in:** Kullanıcı onayı ile veri toplama
- **Privacy-first:** Kişisel veri toplanmaz
- **Minimized:** Minimum performans etkisi

---

## 2. Crash Reporter

```rust
pub struct CrashReporter {
    pub enabled: bool,
    pub endpoint: String,
    pub last_crash: Option<CrashReport>,
}

pub struct CrashReport {
    pub timestamp: u64,
    pub version: String,
    pub platform: String,
    pub stack_trace: Vec<StackFrame>,
    pub system_info: SystemInfo,
    pub game_state: GameStateSnapshot,
    pub logs: Vec<String>,
}

pub struct StackFrame {
    pub function: String,
    pub file: String,
    pub line: u32,
    pub module: String,
}

pub struct SystemInfo {
    pub os: String,
    pub cpu: String,
    pub gpu: String,
    pub ram_mb: u64,
    pub disk_free_gb: u64,
}

impl CrashReporter {
    pub fn capture(&self, error: &dyn std::error::Error) -> CrashReport;
    pub async fn submit(&self, report: &CrashReport) -> Result<()>;
}
```

---

## 3. Telemetry

```rust
pub struct TelemetryCollector {
    pub enabled: bool,
    pub session_id: String,
    pub events: Vec<TelemetryEvent>,
    pub flush_interval: Duration,
}

pub struct TelemetryEvent {
    pub timestamp: u64,
    pub event_type: String,
    pub properties: HashMap<String, serde_json::Value>,
}

impl TelemetryCollector {
    pub fn record(&mut self, event_type: &str, properties: HashMap<String, serde_json::Value>);
    pub async fn flush(&mut self) -> Result<()>;
}

// Örnek event'ler
// - game_start, game_end
// - block_placed, block_broken
// - entity_killed, death
// - fps_sample, memory_sample
// - settings_changed
```

---

## 4. Crate Organizasyonu

```
crates/
  telemetry/
    ├── mod.rs
    ├── crash.rs
    ├── collector.rs
    ├── events.rs
    └── privacy.rs
```
