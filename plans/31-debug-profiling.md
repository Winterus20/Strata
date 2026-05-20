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
    /// Timestamp query pool.
    query_set: wgpu::QuerySet,

    /// Query sonuç buffer'ı.
    resolve_buffer: wgpu::Buffer,

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
            label: Some("gpu_profiler"),
            count: max_queries * 2, // start + end per query
            ty: wgpu::QueryType::Timestamp,
        });

        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_profiler_resolve"),
            size: max_queries as u64 * 2 * 8, // 8 bytes per timestamp
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        Self {
            query_set,
            resolve_buffer,
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

    /// Sonuçları oku.
    pub fn resolve(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<(String, f32)> {
        // Timestamp query sonuçlarını oku
        // GPU timestamp → ms dönüşümü
        // ...
        Vec::new()
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

    /// GPU buffer'ları.
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
                max: (origin + IVec3::new(32, 128, 32)).as_vec3(),
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
    │   └── gpu.rs          ← GPU profiling (timestamp query)
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
