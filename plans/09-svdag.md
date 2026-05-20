# 03 — SVDAG Veri Yapısı

## 1. SVDAG — Uzak Alan Veri Yapısı

### 1.1 Shared Node Pool

Tüm sektörlerin SVDAG'ları **tek bir global node havuzunu** paylaşır. Bu, aynı geometrinin birden fazla sector'de **tek node** olarak saklanmasını sağlar.

```rust
pub struct SharedNodePool {
    nodes: Vec<SvdagNode>,
    free_slots: Vec<u32>,
    ref_counts: Vec<u32>,
    gpu_free_head: wgpu::Buffer,
    capacity: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SvdagNode {
    pub child_mask: u8,
    pub child_indices: [u32; 8],
    pub material: u16,
}
```

**GPU Allocator (32-bit atomic):**

```wgsl
struct NodePool {
    free_head: atomic<u32>,
    capacity: u32,
    nodes: array<SvdagNode, 262144>,
}

fn node_alloc(pool: ptr<storage, NodePool>) -> u32 {
    let idx = atomicAdd(&pool.free_head, 1u);
    if (idx >= pool.capacity) {
        return 0xFFFFFFFFu;
    }
    return idx;
}
```

### 1.2 Node Bellek Hesabı

| Bileşen | Boyut |
|---|---|
| child_mask | 1 byte |
| child_indices | 32 byte (8 × u32) |
| material | 2 byte |
| Padding | 5 byte (16-byte alignment) |
| **Toplam** | **40 byte** |

32×128×32 sector için tipik SVDAG:
- Boş/homojen alan: **~8-12KB**
- Karmaşık arazi: **~25-40KB**
- Deduplication ile: **%20-30 ek tasarruf**

**GPU Node Pool Kapasitesi:** 256K node × 40B = **~10MB**

### 1.3 Brick → SVDAG Bake (GPU Compute)

```rust
pub struct SvdagBaker {
    edit_buffer: GpuRingBuffer<EditOp>,
    hash_table: GpuHashTable,
    node_pool: GpuNodePool,
}

#[repr(C)]
pub struct EditOp {
    pub sector: IVec3,
    pub pos: IVec3,
    pub old_material: u16,
    pub new_material: u16,
}
```

**Bake Pipeline:**

```
1. Brickmap'ten voxel array çıkar (CPU → GPU upload, ~2ms)
2. GPU compute: Geçici SVO oluştur (bottom-up, ~5ms)
3. GPU compute: Mevcut SVDAG'e merge et (HashDAG algorithm, ~5ms)
4. GPU compute: Duplicate node'ları temizle (hash-based, ~3ms)
5. CPU: Node pool'dan root index al, sector'a ata
6. Eski node'ların ref count'unu azalt (GC için işaretle)
```

**Toplam süre: ~15ms** (CPU'daki 200ms'lik süreye kıyasla)

### 1.4 Transform-Aware Deduplication (SIGGRAPH 2025)

**Transform-Aware SVDAG** (Molenaar & Eisemann, SIGGRAPH 2025) simetri ve dönüşümleri kullanarak ek **%20-45** tasarruf sağlar.

#### Simetri Tipleri

| Simetri | Açıklama | Tasarruf |
|---|---|---|
| **Mirror X/Y/Z** | Eksenlerde ayna | %10-15 |
| **Rotation 90°/180°/270°** | Y ekseni etrafında dönüş | %10-20 |
| **Translation** | Öteleme ile eşleştirme | %5-10 |
| **Kombinasyonlar** | Mirror + Rotation | %20-45 |

#### Transform-Aware Node Yapısı

```rust
#[repr(u8)]
pub enum SvdagTransform {
    Identity = 0,
    MirrorX = 1,
    MirrorY = 2,
    MirrorZ = 3,
    MirrorXY = 4,
    MirrorXZ = 5,
    MirrorYZ = 6,
    MirrorXYZ = 7,
    Rotate90 = 8,
    Rotate180 = 9,
    Rotate270 = 10,
    Rotate90MirrorX = 11,
    Rotate180MirrorX = 12,
    Rotate270MirrorX = 13,
}

pub struct SvdagNode {
    pub child_mask: u8,
    pub child_indices: [u32; 8],
    pub material: u16,
    pub transform: SvdagTransform,
}
```

#### Transform-Aware Hash Lookup

```rust
pub struct TransformAwareHashTable {
    normal_map: HashMap<u64, u32>,
    transform_map: HashMap<(u64, SvdagTransform), u32>,
}

impl TransformAwareHashTable {
    pub fn lookup_with_transforms(&self, geometry_hash: u64) -> Option<(u32, SvdagTransform)> {
        if let Some(&idx) = self.normal_map.get(&geometry_hash) {
            return Some((idx, SvdagTransform::Identity));
        }

        let transform_order = [
            SvdagTransform::MirrorX,
            SvdagTransform::MirrorY,
            SvdagTransform::MirrorZ,
            SvdagTransform::Rotate90,
            SvdagTransform::Rotate180,
            SvdagTransform::Rotate270,
        ];

        for transform in transform_order {
            if let Some(&idx) = self.transform_map.get(&(geometry_hash, transform)) {
                return Some((idx, transform));
            }
        }

        None
    }
}
```

### 1.5 Shallow SVDAG Streaming (Aokana, SIGGRAPH 2025)

Derin SVDAG traversal = çoklu indirect jump = GPU cache miss. **Aokana** yaklaşımı bu sorunu **sığ SVDAG'lar + streaming** ile çözer.

#### Temel Fikir

| Özellik | Derin SVDAG | Shallow SVDAG |
|---|---|---|
| **Max depth** | 8-12 level | 4-5 level |
| **Traversal** | Çoklu indirect jump | Az indirect jump |
| **VRAM kullanımı** | Tüm sahne | Sadece **%5** |
| **32K+ çözünürlük** | Yavaş | **2-4× daha hızlı** |
| **Streaming** | Yok | View-dependent, LOD bazlı |

#### Shallow SVDAG Yapısı

```rust
pub struct ShallowSvdag {
    roots: Vec<ShallowSvdagRoot>,
    node_pool: SharedNodePool,
    streaming_state: SvdagStreamingState,
}

pub struct ShallowSvdagRoot {
    pub tile_coord: IVec3,
    pub root_index: u32,
    pub lod_level: u8,
    pub loaded: bool,
    pub priority: f32,
}
```

#### View-Dependent Streaming

```rust
pub struct SvdagStreamingManager {
    loaded_tiles: HashMap<IVec3, ShallowSvdagRoot>,
    load_queue: PriorityQueue<IVec3, f32>,
    disk_index: SvdagDiskIndex,
}

impl SvdagStreamingManager {
    pub fn update(&mut self, camera: &Camera, frustum: &Frustum) {
        let visible_tiles = self.frustum_query(frustum);

        for tile in visible_tiles {
            let priority = self.compute_priority(tile, camera);
            self.load_queue.push(tile, priority);
        }

        self.load_tiles_from_queue(Budget::VRAM_5_PERCENT);
        self.unload_invisible_tiles();
    }

    const VRAM_BUDGET: f32 = 0.05;
}
```

**Performans (Aokana sonuçları):**
- **4.8× hız artışı**
- **9× VRAM azalması** (sadece %5 yüklü)
- **32K+ çözünürlük** HashDAG'den 2-4× daha hızlı
- **Streaming overhead:** <1ms/frame

### 1.6 SVDAG → Brick Unbake

Oyuncu bir sector'e yaklaştığında:

```
1. SVDAG root node'dan başla
2. GPU compute: SVDAG → voxel array (top-down traversal, ~3ms)
3. CPU: Voxel array → Brickmap (bitmask + materials, ~2ms)
4. GPU: Node pool'dan ref count azalt
5. Sector.dirty = false
```

**Toplam süre: ~5ms**
