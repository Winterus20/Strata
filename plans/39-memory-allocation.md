# 39 — Memory Allocation Stratejisi

## 1. Genel Bakış

Strata, voxel dünyasında sürekli allocation/deallocation yapan bir oyun motorudur. System default allocator (glibc/Windows heap) bu iş yükü için yetersizdir — thread contention, fragmentation ve RSS bloat sorunları üretir.

**Karar: mimalloc v3 global allocator + hybrid per-subsystem strateji.**

### Neden mimalloc?

| Allocator | Performans | Windows MSVC | Bellek Overhead | Geliştirme | Oyun Motoru Kullanımı |
|-----------|-----------|--------------|-----------------|------------|----------------------|
| System (glibc) | Baseline | ✅ | Düşük | — | — |
| jemalloc | +30-50% | ❌ | Orta-yüksek | Azalan | Yok major |
| **mimalloc v3** | **+30-50%** | **✅** | **Orta (+5-10%)** | **Aktif (Microsoft)** | **Unreal, Roblox, Xbox** |
| snmalloc | ~mimalloc | ✅ | Düşük | Aktif | Yok |
| rpmalloc | ~mimalloc | ✅ | Yüksek | Orta | Zig |

**Neden jemalloc değil?** Windows MSVC desteği yok (dealbreaker). Geliştirme aktivitesi azalıyor. mimalloc benzer performansı Windows dahil tüm platformlarda veriyor.

### Gerçek Dünya Doğrulaması

| Proje | Allocator | Bulgular |
|-------|-----------|----------|
| **Unreal Engine 5** | mimalloc | `MallocMimalloc.cpp` — production'da kullanımda |
| **Roblox** | mimalloc | GPU OOM debugging'de referanslanmış |
| **Factorio** | mimalloc + large pages | %20-30 UPS artışı |
| **CPython 3.13+** | mimalloc | Free-threaded (no-GIL) build'in default'u |
| **Xbox** | mimalloc | Platform-level entegrasyon |
| **Veloren** | SPECS ECS (system alloc) | 181 eşzamanlı oyuncu — mimalloc olsa daha iyi olurdu |

---

## 2. Mimarisi

```
┌─────────────────────────────────────────────────────────────────┐
│                    mimalloc v3 (Global Allocator)                │
│  #[global_allocator] static GLOBAL: MiMalloc = MiMalloc;        │
│                                                                  │
│  Runtime Tuning:                                                 │
│  - PURGE_DELAY=500 (client) / 2000 (server)                     │
│  - ARENA_EAGER_COMMIT=1                                          │
│  - ALLOW_THP=1 (Linux) / ALLOW_LARGE_OS_PAGES=1 (Windows)       │
│  - PURGE_DECOMMITS=1 (Windows)                                   │
└──────────────────────────┬──────────────────────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        ▼                  ▼                  ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ Bevy ECS     │  │ Chunk Heaps  │  │ Network      │
│              │  │              │  │ Heaps        │
│ Archetype    │  │ mi_heap_new  │  │ mi_heap_new  │
│ Tables (SoA) │  │ /destroy     │  │ /destroy     │
│              │  │              │  │              │
│ Bevy yönetir │  │ Bulk unload  │  │ Per-conn     │
│ dokunma      │  │ O(1) free    │  │ disconnect   │
└──────────────┘  └──────────────┘  └──────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        ▼                  ▼                  ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│ Mesh Gen     │  │ Pathfinding  │  │ Network      │
│ Arena        │  │ Arena        │  │ Packet Pool  │
│              │  │              │  │              │
│ bumpalo      │  │ bumpalo      │  │ slab         │
│ 4MB pre-alloc│  │ per-query    │  │ pre-alloc    │
│ reset/frame  │  │ drop         │  │ recycle      │
└──────────────┘  └──────────────┘  └──────────────┘
```

---

## 3. Global Allocator: mimalloc v3

### 3.1 Kurulum

```toml
# Cargo.toml
[dependencies]
mimalloc = "0.1"
libmimalloc-sys = "0.1"
```

```rust
// main.rs
use mimalloc::MiMalloc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;
```

### 3.2 Runtime Tuning (App::new()'dan ÖNCE)

```rust
fn configure_allocator() {
    unsafe {
        // RSS dengesi — 500ms client, 2000ms server
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_purge_delay, 500
        );

        // Commit spike'larını önler (Windows/macOS)
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_arena_eager_commit, 1
        );

        // Terk edilmiş sayfaları geri kazan
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_abandoned_reclaim_on_free, 1
        );

        // OOM'da GC dene
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_retry_on_oom, 1
        );

        // Temiz kapanış
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_destroy_on_exit, 1
        );
    }
}
```

### 3.3 Platform-Specific Ayarlar

```rust
#[cfg(target_os = "linux")]
fn platform_allocator_config() {
    unsafe {
        // Transparent huge pages — 2MB sayfalar
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_allow_thp, 1
        );
    }
}

#[cfg(target_os = "windows")]
fn platform_allocator_config() {
    unsafe {
        // Purging gerçekten RSS düşürür (MEM_RESET değil, decommit)
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_purge_decommits, 1
        );

        // Large OS pages — SeLockMemoryPrivilege gerektirir
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_allow_large_os_pages, 1
        );
    }
}
```

### 3.4 Profil Seçenekleri

**Client (60 FPS hedefi):**
```
PURGE_DELAY=500
ARENA_EAGER_COMMIT=1
ALLOW_THP=1 (Linux) / ALLOW_LARGE_OS_PAGES=1 (Windows)
PURGE_DECOMMITS=1 (Windows)
```

**Server (600+ oyuncu):**
```
PURGE_DELAY=2000
ARENA_EAGER_COMMIT=1
RESERVE_HUGE_OS_PAGES=4 (4GB reserve)
SHOW_STATS=1
```

**Development / Profiling:**
```
PURGE_DELAY=0
SHOW_STATS=1
VERBOSE=1
STATS=1
```

---

## 4. Per-Subsystem Heap İzolasyonu

### 4.1 mimalloc Custom Heap API

v3'te heap'ler **thread-independent** — herhangi bir thread'den herhangi bir heap'e allocate edilebilir.

```rust
use libmimalloc_sys::{mi_heap_new, mi_heap_destroy, mi_heap_collect};

// Dedicated heap oluştur
let heap = unsafe { mi_heap_new() };

// Bu heap'ten allocate et
let ptr = unsafe { libmimalloc_sys::mi_heap_malloc(heap, size) };

// Heap'i yok et — O(1) bulk free!
unsafe { mi_heap_destroy(heap); }

// Veya sadece purge et (yok etmeden)
unsafe { mi_heap_collect(heap, true); }
```

### 4.2 Chunk Heap (Sector Yükleme/Boşaltma)

Sektör verisi yükleme ve boşaltma için dedicated heap. Oyuncu dünyada hareket ettikçe chunk'lar yüklenir/boşaltılır.

```rust
use libmimalloc_sys::*;
use std::collections::HashMap;
use bevy::prelude::*;

/// Sector bazlı heap yönetimi.
#[derive(Resource)]
pub struct ChunkHeapManager {
    heaps: HashMap<IVec3, *mut mi_heap_t>,
}

impl ChunkHeapManager {
    pub fn new() -> Self {
        Self {
            heaps: HashMap::new(),
        }
    }

    /// Sector yükle — yeni heap oluştur.
    pub fn alloc_sector(&mut self, coord: IVec3) -> *mut mi_heap_t {
        let heap = unsafe { mi_heap_new() };
        self.heaps.insert(coord, heap);
        heap
    }

    /// Sector boşalt — heap'i O(1) yok et.
    pub fn free_sector(&mut self, coord: IVec3) {
        if let Some(heap) = self.heaps.remove(&coord) {
            unsafe { mi_heap_destroy(heap); }
        }
    }

    /// Toplu sector boşaltma (world unload, disconnect).
    pub fn free_all(&mut self) {
        for (_, heap) in self.heaps.drain() {
            unsafe { mi_heap_destroy(heap); }
        }
    }

    /// Periyodik RSS kontrolü — nazik purge.
    pub fn periodic_purge(&self) {
        for (_, &heap) in &self.heaps {
            unsafe { mi_heap_collect(heap, false); }
        }
    }
}

impl Drop for ChunkHeapManager {
    fn drop(&mut self) {
        self.free_all();
    }
}
```

### 4.3 Network Heap (Oyuncu Bağlantıları)

Her oyuncu bağlantısı kendi heap'ini alır — disconnect'te bulk free.

```rust
/// Per-connection heap.
pub struct ConnectionHeap {
    heap: *mut mi_heap_t,
    client_id: ClientId,
}

impl ConnectionHeap {
    pub fn new(client_id: ClientId) -> Self {
        Self {
            heap: unsafe { mi_heap_new() },
            client_id,
        }
    }

    /// Bu heap'ten allocate et.
    pub fn alloc(&self, size: usize) -> *mut u8 {
        unsafe { mi_heap_malloc(self.heap, size) as *mut u8 }
    }
}

impl Drop for ConnectionHeap {
    fn drop(&mut self) {
        // Bağlantı koptuğunda tüm bellek tek seferde serbest
        unsafe { mi_heap_destroy(self.heap); }
    }
}
```

---

## 5. Hot Path: bumpalo Arena

### 5.1 Per-Frame Mesh Generation

Mesh generation en sıcak allocation path'i. Her chunk değişiminde binlerce vertex/index üretiliyor.

```rust
use bumpalo::Bump;
use bevy::prelude::*;

/// Per-thread mesh generation arena'sı.
#[derive(Resource)]
pub struct MeshScratchAllocator {
    bump: Bump,
}

impl MeshScratchAllocator {
    pub fn new() -> Self {
        Self {
            // 4MB pre-allocated — çoğu chunk mesh'i sığar
            bump: Bump::with_capacity(4 * 1024 * 1024),
        }
    }

    /// Arena'yı sıfırla — O(1), bellek geri alınmaz, yeniden kullanılır.
    pub fn reset(&mut self) {
        self.bump.reset();
    }

    /// Arena referansı — mesh generation fonksiyonuna verilir.
    pub fn bump(&self) -> &Bump {
        &self.bump
    }
}

/// Mesh generation — bumpalo arena kullanır.
fn generate_chunk_mesh(
    chunk_data: &CompressedChunkData,
    arena: &Bump,
) -> ChunkMesh {
    // Vertex ve index array'leri arena'dan allocate edilir
    let mut vertices = bumpalo::collections::Vec::new_in(arena);
    let mut indices = bumpalo::collections::Vec::new_in(arena);

    // ... greedy meshing algoritması ...

    // Sonuç — arena drop edildiğinde scratch memory serbest kalır
    ChunkMesh {
        vertices: vertices.to_vec(), // Kalıcı kopya (mimalloc global)
        indices: indices.to_vec(),
    }
}

/// Per-frame reset sistemi.
fn reset_mesh_arena(mut arena: ResMut<MeshScratchAllocator>) {
    arena.reset();
}
```

### 5.2 Per-Query Pathfinding

A* gibi algoritmalar sorgu başına yüzlerce node üretiyor.

```rust
use bumpalo::Bump;

fn find_path(start: IVec3, goal: IVec3, world: &World) -> Option<Vec<IVec3>> {
    let arena = Bump::new();

    // Open set ve closed set arena'dan allocate edilir
    let mut open_set = BinaryHeap::new_in(&arena);
    let mut came_from = HashMap::new_in(&arena);

    // ... A* algoritması ...

    // Arena drop edilir — tüm node'lar tek seferde serbest
    Some(path)
}
```

### 5.3 bumpalo + mimalloc Etkileşimi

**Çatışma yok.** Katmanlı çalışırlar:

```
mimalloc (global)  ←── bumpalo'nun backing chunk'ları buradan gelir
    │
    └── bumpalo arena  ←── Vertex/index/node allocation'ları burada
            │
            └── 1-2ns/allocation (mimalloc'dan 10x daha hızlı)
                Frame sonunda reset = O(1)
```

bumpalo kendi büyük chunk'larını mimalloc'dan alır, içindekileri bump pointer ile yönetir. En iyi iki dünya.

---

## 6. Network Packet Pool: slab

Sabit boyutlu network paketleri için pre-allocated pool.

```rust
use slab::Slab;

/// Pre-allocated network packet buffer pool'u.
#[derive(Resource)]
pub struct PacketPool {
    pool: Slab<PacketBuffer>,
    buffer_size: usize,
}

impl PacketPool {
    pub fn new(capacity: usize, buffer_size: usize) -> Self {
        let mut pool = Slab::with_capacity(capacity);
        // Startup'ta pre-allocate
        for _ in 0..capacity {
            pool.insert(PacketBuffer::new(buffer_size));
        }
        Self { pool, buffer_size }
    }

    /// Pool'dan buffer al — O(1).
    pub fn acquire(&mut self) -> Option<usize> {
        if self.pool.len() < self.pool.capacity() {
            Some(self.pool.insert(PacketBuffer::new(self.buffer_size)))
        } else {
            None // Pool dolu — yeni allocate et (fallback)
        }
    }

    /// Buffer'ı geri koy — O(1).
    pub fn release(&mut self, key: usize) {
        if self.pool.contains(key) {
            self.pool.remove(key);
        }
    }
}

/// Sabit boyutlu packet buffer.
pub struct PacketBuffer {
    data: Vec<u8>,
}

impl PacketBuffer {
    fn new(size: usize) -> Self {
        Self {
            data: vec![0u8; size],
        }
    }
}
```

---

## 7. GPU Buffer Yönetimi

### 7.1 wgpu Buffer Pool

wgpu kendi içinde sub-allocation yapıyor ama create/destroy churn'i önlemek için pool gerekli.

```rust
use bevy::prelude::*;
use wgpu::Buffer;

/// Pre-allocated GPU vertex buffer pool'u.
#[derive(Resource)]
pub struct GpuBufferPool {
    /// Kullanılmayan buffer'lar.
    free_vertex: Vec<wgpu::Buffer>,
    free_index: Vec<wgpu::Buffer>,

    /// Buffer boyutları.
    vertex_buffer_size: u64,
    index_buffer_size: u64,
}

impl GpuBufferPool {
    pub fn new(
        device: &wgpu::Device,
        max_buffers: usize,
        vertex_size: u64,
        index_size: u64,
    ) -> Self {
        let mut free_vertex = Vec::with_capacity(max_buffers);
        let mut free_index = Vec::with_capacity(max_buffers);

        // Startup'ta pre-allocate
        for _ in 0..max_buffers {
            free_vertex.push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Pool Buffer"),
                size: vertex_size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            free_index.push(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Index Pool Buffer"),
                size: index_size,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }

        Self {
            free_vertex,
            free_index,
            vertex_buffer_size: vertex_size,
            index_buffer_size: index_size,
        }
    }

    /// Vertex buffer al — pool'dan veya yeni oluştur.
    pub fn acquire_vertex(&mut self, device: &wgpu::Device) -> wgpu::Buffer {
        self.free_vertex.pop().unwrap_or_else(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Vertex Pool Buffer (overflow)"),
                size: self.vertex_buffer_size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
    }

    /// Vertex buffer geri koy.
    pub fn release_vertex(&mut self, buffer: wgpu::Buffer) {
        if self.free_vertex.len() < self.free_vertex.capacity() {
            self.free_vertex.push(buffer);
        }
        // Doluysa drop et — GPU memory serbest kalır
    }
}
```

### 7.2 Staging Buffer (CPU → GPU Transfer)

```rust
/// Triple-buffered staging buffer — async upload için.
#[derive(Resource)]
pub struct StagingRing {
    buffers: [wgpu::Buffer; 3],
    current: usize,
    size: u64,
}

impl StagingRing {
    pub fn new(device: &wgpu::Device, size: u64) -> Self {
        let buffers = [
            Self::create_staging(device, size),
            Self::create_staging(device, size),
            Self::create_staging(device, size),
        ];
        Self {
            buffers,
            current: 0,
            size,
        }
    }

    fn create_staging(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Staging Buffer"),
            size,
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_WRITE,
            mapped_at_creation: false,
        })
    }

    /// Sıradaki staging buffer'ı al.
    pub fn next(&mut self) -> &wgpu::Buffer {
        let buf = &self.buffers[self.current];
        self.current = (self.current + 1) % 3;
        buf
    }
}
```

---

## 8. Thread Lifecycle Yönetimi

### 8.1 "mimalloc Cigarette" Probleminden Kaçınma

Bir thread ağır allocation yapıp uyursa, belleği stranded kalır. Çözüm:

```rust
/// Worker thread işini bitirdiğinde çağrılır.
fn on_worker_thread_done() {
    unsafe {
        // Unused pages'i nazikçe bırak
        libmimalloc_sys::mi_collect(false);

        // Thread-local cache'leri geri ver
        libmimalloc_sys::mi_thread_done();
    }
}

/// Worker thread yeniden başlatıldığında.
fn on_worker_thread_init() {
    unsafe {
        libmimalloc_sys::mi_thread_init();
    }
}
```

### 8.2 Periyodik RSS Kontrolü

```rust
/// Her N saniyede bir çalışır (loading screen veya idle anında).
fn periodic_memory_cleanup() {
    unsafe {
        // Nazik purge — frame sırasında ASLA aggressive yapma
        libmimalloc_sys::mi_collect(false);
    }
}

/// Loading screen veya world unload sırasında.
fn aggressive_memory_cleanup() {
    unsafe {
        // Agresif GC — tüm unused pages
        libmimalloc_sys::mi_collect(true);
    }
}
```

### 8.3 RSS Monitoring

```rust
/// Anlık RSS bilgisi (Windows).
#[cfg(target_os = "windows")]
fn get_rss_bytes() -> u64 {
    unsafe {
        let mut info: libmimalloc_sys::mi_process_info_t = std::mem::zeroed();
        libmimalloc_sys::mi_process_info(&mut info);
        info.current_rss
    }
}

/// Anlık RSS bilgisi (Linux).
#[cfg(target_os = "linux")]
fn get_rss_bytes() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            return line.split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u64>().ok())
                .map(|kb| kb * 1024)
                .unwrap_or(0);
        }
    }
    0
}
```

---

## 9. Large Page Desteği

### 9.1 Neden Önemli?

Voxel dünyası milyonlarca chunk'a erişiyor. Standart 4KB sayfalarla:
- 4GB working set = 1M TLB entry (L2 TLB: 1536-2048 entry)
- 2MB sayfalarla = 2048 entry — L2 TLB'ye sığar

Factorio'da large pages ile **%20-30 UPS artışı** ölçülmüş.

### 9.2 Platform Kurulumu

**Windows:**
1. Group Policy: `Computer Configuration > Windows Settings > Security Settings > Local Policies > User Rights Assignment > "Lock pages in memory"`
2. Kullanıcıyı ekle
3. Yeniden giriş yap

**Linux:**
```bash
# 4GB 2MB huge page reserve
echo 2048 | sudo tee /proc/sys/vm/nr_hugepages

# Veya transparent huge pages
echo always | sudo tee /sys/kernel/mm/transparent_hugepage/enabled
```

### 9.3 mimalloc'da Aktifleştirme

```rust
unsafe {
    // Large OS pages (2-4MB)
    libmimalloc_sys::mi_option_set(
        libmimalloc_sys::mi_option_allow_large_os_pages, 1
    );

    // Huge OS pages (1GB) — 4GB reserve
    // Environment variable: MIMALLOC_RESERVE_HUGE_OS_PAGES=4
}
```

### 9.4 Uyarılar

| Uyarı | Açıklama |
|-------|----------|
| NUMA | Large page yanlış NUMA node'a allocate edebilir — test et |
| fork() | Linux'ta large page + fork() CoW kopyalama maliyetini artırır |
| SeLockMemoryPrivilege | Windows'ta Group Policy gerektirir |

---

## 10. Profiling ve Teşhis

### 10.1 Built-in Stats

```rust
/// Stats'ları aktifleştir (development builds).
fn enable_allocator_stats() {
    unsafe {
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_stats, 1
        );
        libmimalloc_sys::mi_option_set(
            libmimalloc_sys::mi_option_verbose, 1
        );
    }
}

/// Stats'ları yazdır.
fn print_allocator_stats() {
    unsafe {
        libmimalloc_sys::mi_stats_print_out(None, std::ptr::null_mut());
    }
}

/// Stats'ları sıfırla — periyodik ölçüm için.
fn reset_allocator_stats() {
    unsafe {
        libmimalloc_sys::mi_stats_reset();
    }
}
```

### 10.2 Per-Frame Profiling

```rust
fn profiling_frame(frame_num: u64) {
    unsafe {
        libmimalloc_sys::mi_stats_reset();
    }

    // ... frame çalıştır ...

    println!("--- Frame {} ---", frame_num);
    unsafe {
        libmimalloc_sys::mi_stats_print_out(None, std::ptr::null_mut());
    }
}
```

### 10.3 Harici Araçlar

| Araç | Platform | Ne Yapar |
|------|----------|----------|
| `MIMALLOC_SHOW_STATS=1` | Her yer | Exit'te stats dump |
| `tracy` | Her yer | Frame-by-frame memory + CPU tracking |
| `heaptrack` | Linux | RSS growth over time, allocation hotspots |
| `VMMap` | Windows | Virtual/physical memory breakdown |
| `DHAT` (Valgrind) | Linux | Detaylı heap analizi |
| `Windows Performance Analyzer` | Windows | ETW heap tracing |

---

## 11. Cargo.toml Bağımlılıkları

```toml
[dependencies]
# Global allocator
mimalloc = "0.1"
libmimalloc-sys = "0.1"

# Hot path arena allocator
bumpalo = { version = "3", features = ["collections"] }

# Object pool (network packets)
slab = "0.4"

# Entity pool (zaten mevcut — XBrickMap için)
slotmap = "1"
```

---

## 12. Bellek Tahminleri

### 12.1 Client (Render Distance = 32 chunk)

| Bileşen | Tahmini Bellek | Allocator |
|---------|---------------|-----------|
| Bevy ECS + wgpu | ~200-400 MB | mimalloc global |
| Loaded chunks (32³ ~32K sector) | ~1-2 GB | Chunk heaps |
| Mesh vertex buffers | ~500 MB - 1 GB | GPU buffer pool |
| Texture atlas | ~256 MB | mimalloc global |
| Network buffers | ~10-50 MB | Network heaps |
| Pathfinding scratch | ~1-4 MB | bumpalo |
| **Toplam** | **~2-4 GB** | |

### 12.2 Server (600 oyuncu)

| Bileşen | Tahmini Bellek | Allocator |
|---------|---------------|-----------|
| Loaded world (tüm aktif sector'ler) | ~4-8 GB | Chunk heaps |
| Player state (600 × ~10KB) | ~6 MB | mimalloc global |
| Network buffers (600 connection) | ~60-300 MB | Network heaps |
| ECS overhead | ~100-200 MB | mimalloc global |
| **Toplam** | **~5-9 GB** | |

---

## 13. Performans Hedefleri

| Metrik | Hedef | Not |
|--------|-------|-----|
| Allocation latency (hot path) | <2ns | bumpalo ile |
| Allocation latency (cold path) | <25ns | mimalloc ile |
| Frame-time allocation overhead | <0.5ms | Per-frame toplam |
| RSS growth (1 saat oyun) | <%5 | mimalloc purging ile |
| Chunk unload bulk free | O(1) | mi_heap_destroy |
| Memory fragmentation (1 saat) | <%10 | mimalloc arena-based |

---

## 14. Riskler ve Azaltma

| Risk | Seviye | Azaltma |
|------|--------|---------|
| mimalloc RSS drift (uzun oturumlar) | Orta | Periyodik `mi_collect(false)` + RSS monitoring |
| Large page NUMA sorunu | Düşük | Test et, gerekirse `MIMALLOC_USE_NUMA_NODES=1` |
| bumpalo thread safety | Düşük | Per-thread arena, paylaşım yok |
| mimalloc v3 edge case (Lean benchmarks) | Düşük | v3.3.x+ kullan, güncel tut |
| Windows SeLockMemoryPrivilege | Orta | Installer'da ayarla veya large pages opsiyonel yap |
| "mimalloc cigarette" (stranded pages) | Orta | `mi_thread_done()` worker thread çıkışında |

---

## 15. Kaynaklar

- [mimalloc Environment Options](https://microsoft.github.io/mimalloc/environment.html)
- [mimalloc Heaps API](https://microsoft.github.io/mimalloc/group__heap.html)
- [mimalloc Extended Functions](https://microsoft.github.io/mimalloc/group__extended.html)
- [mimalloc GitHub](https://github.com/microsoft/mimalloc)
- [v3 vs v2 (GitHub #1073)](https://github.com/microsoft/mimalloc/issues/1073)
- [libmimalloc-sys Rust docs](https://docs.rs/libmimalloc-sys)
- [bumpalo GitHub](https://github.com/fitzgen/bumpalo)
- [Factorio large pages (%20-30 UPS)](https://forums.factorio.com/viewtopic.php?t=96090)
- [Mimalloc Cigarette (RSS pitfall)](https://pwy.io/posts/mimalloc-cigarette)
- [Large Pages May Be Harmful on NUMA (USENIX)](https://www.usenix.org/system/files/conference/atc14/atc14-paper-gaud.pdf)
- [Unreal Engine mimalloc integration](https://github.com/EpicGames/UnrealEngine/blob/4.27/Engine/Source/Runtime/Core/Private/HAL/MallocMimalloc.cpp)
- [mimalloc Microsoft Research blog](https://www.microsoft.com/en-us/research/blog/mimalloc-a-high-performance-scalable-memory-allocator-for-the-modern-era)
- [The State of Allocators in 2026](https://cetra3.github.io/blog/state-of-allocators-2026)
- [Who uses jemalloc in 2026?](https://theconsensus.dev/p/2026/04/16/who-even-uses-jemalloc-anyway.html)
- [Meilisearch: jemalloc, bumpalo, mimalloc](https://blog.kerollmops.com/the-good-the-bad-and-the-leaky-jemalloc-bumpalo-and-mimalloc-in-meilisearch)
