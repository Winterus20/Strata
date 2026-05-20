# 05 — Render Pipeline

## 1. Unified Visibility Buffer Render Pipeline

### 1.1 Genel Akış

Tüm tier'lar **aynı 64-bit visibility buffer'a** yazar. Bu, iki farklı render pipeline'ını tek bir shading pass'inde birleştirir.

```
┌──────────────────────────────────────────────────────────────┐
│                    RENDER FRAME                               │
├──────────────────────────────────────────────────────────────┤
│ Pass 1: Frustum Culling (GPU Compute)                        │
│   → Görünür sector'ları belirle, tier'lara göre sınıfla      │
│   → Çıktı: sector_list (buffer)                              │
├──────────────────────────────────────────────────────────────┤
│ Pass 2: Tier 1 — XBrickMap Ray Trace (GPU Compute)           │
│   → 4-level bitmask space skipping                           │
│   → Visibility buffer'a yaz (depth + normal + sector + voxel)│
├──────────────────────────────────────────────────────────────┤
│ Pass 3: Tier 2 — XBrickMap + SVDAG (GPU Compute)             │
│   → Brick varsa brick'ten, yoksa SVDAG'den                   │
│   → Aynı visibility buffer'a (depth test otomatik)           │
├──────────────────────────────────────────────────────────────┤
│ Pass 4: Tier 3 — SVDAG Ray March (GPU Compute)               │
│   → Hi-Z occlusion culling ile                               │
│   → Aynı visibility buffer'a                                 │
├──────────────────────────────────────────────────────────────┤
│ Pass 5: Color Resolve (GPU Compute)                          │
│   → Visibility buffer'dan tüm pikselleri tek seferde shade et│
│   → G-buffer → final frame buffer                            │
├──────────────────────────────────────────────────────────────┤
│ Pass 6: Build Hi-Z (GPU Compute)                             │
│   → Depth buffer'dan hierarchical Z-buffer oluştur           │
│   → Sonraki frame occlusion culling için                     │
└──────────────────────────────────────────────────────────────┘
```

### 1.2 Visibility Buffer Layout (64-bit)

| Bit Aralığı | İçerik | Açıklama |
|---|---|---|
| 0-23 (24 bit) | Depth | Z-depth, 16M+ hassasiyet |
| 24-26 (3 bit) | Normal | Axis-aligned normal (X+/X-/Y+/Y-/Z+/Z-) |
| 27-39 (13 bit) | Sector ID | Hangi sector'den geldi |
| 40-63 (24 bit) | Voxel Pos | Voxel koordinatı (sector içinde) |

#### WGSL 64-bit Atomik Stratejisi

| Platform | Feature | Depth Write |
|---|---|---|
| Vulkan (VK_KHR_shader_atomic_int64) | `SHADER_INT64_ATOMIC_ALL_OPS` | `atomic<u64>` native |
| DX12 (SM 6.6+) | `SHADER_INT64_ATOMIC_ALL_OPS` | `atomic<u64>` native |
| Metal (Apple8+) | `SHADER_INT64_ATOMIC_MIN_MAX` | `atomic<vec2<u32>>` + `atomicStoreMin` |

```rust
let use_native_u64 = adapter.features().contains(
    wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS
);

let required_features = if use_native_u64 {
    wgpu::Features::SHADER_INT64_ATOMIC_ALL_OPS
} else {
    wgpu::Features::SHADER_INT64_ATOMIC_MIN_MAX
};
```

```wgsl
// Path A: Native u64 atomic (Vulkan, DX12)
#ifdef NATIVE_U64_ATOMIC
struct VisibilityEntry {
    depth_and_normal: u64,
    sector_and_voxel: u64,
}

fn visibility_depth_write(entry: ptr<storage, atomic<u64>, read_write>, new_depth: u32) {
    atomicMin(entry, u64(new_depth));
}

// Path B: vec2<u32> fallback (Metal)
#else
struct VisibilityEntry {
    depth_and_normal: vec2<u32>,
    sector_and_voxel: vec2<u32>,
}

fn visibility_depth_write(entry: ptr<storage, atomic<vec2<u32>>, read_write>, new_depth: u32) {
    let packed = vec2<u32>(new_depth, 0u);
    atomicStoreMin(entry, packed);
}
#endif
```

### 1.3 Hi-Z Occlusion Culling

```wgsl
@compute @workgroup_size(8, 8)
fn svdag_ray_march_hiz(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let tile = id.xy;
    let tile_depth = hiz_buffer_load(tile);

    if (tile_depth == MAX_DEPTH) {
        return;
    }

    let lod_level = select_lod(tile_depth);
    svdag_ray_march(tile, lod_level);
}
```

## 2. Vertex Pooling (Nick McDonald)

Her sector için ayrı mesh buffer = yüksek driver overhead. **Vertex Pool** ile tüm mesh'ler tek büyük buffer'da yönetilir.

### 2.1 Temel Yapı

```rust
pub struct VertexPool {
    gpu_buffer: wgpu::Buffer,
    staging_buffer: Vec<u8>,
    sector_slices: HashMap<SectorCoord, VertexSlice>,
    allocator: PoolAllocator,
    capacity: u32,
}

pub struct VertexSlice {
    pub offset: u32,
    pub count: u32,
    pub index_offset: u32,
    pub index_count: u32,
    pub version: u32,
}
```

### 2.2 Pool Allocator

```rust
pub struct PoolAllocator {
    bump_offset: u32,
    free_list: Vec<(u32, u32)>,
    high_water_mark: u32,
}

impl PoolAllocator {
    pub fn alloc(&mut self, vertex_count: u32) -> Option<u32> {
        for (i, &(offset, count)) in self.free_list.iter().enumerate() {
            if count >= vertex_count {
                self.free_list.remove(i);
                if count > vertex_count {
                    self.free_list.push((offset + vertex_count, count - vertex_count));
                }
                return Some(offset);
            }
        }

        if self.bump_offset + vertex_count <= self.capacity {
            let offset = self.bump_offset;
            self.bump_offset += vertex_count;
            self.high_water_mark = self.high_water_mark.max(self.bump_offset);
            return Some(offset);
        }

        None
    }

    pub fn free(&mut self, offset: u32, count: u32) {
        self.free_list.push((offset, count));
        self.merge_free_list();
    }
}
```

### 2.3 Performans

| Metrik | Ayrı VBO | Vertex Pool | Fark |
|---|---|---|---|
| **Frame time** | 16.7ms | **10.0ms** | **-40%** |
| **Meshing time** | 8.3ms | **6.2ms** | **-25%** |
| **Driver overhead** | Yüksek | Düşük | **-60%** |
| **GPU memory** | Fragmented | Contiguous | **-15%** |
| **Rebuild cost** | VBO create + upload | Sadece upload | **-50%** |

## 3. Foveated Rendering

İnsan gözünün **peripheral vision** sınırlarını kullanarak render maliyetini düşürür.

### 3.1 Fovea Bölgeleri

```rust
pub struct FoveatedConfig {
    pub fovea_center: Vec2,
    pub fovea_radius: f32,
    pub mid_radius: f32,
    pub fovea_scale: f32,
    pub mid_scale: f32,
    pub peripheral_scale: f32,
}
```

| Bölge | Çözünürlük | Kapsam | Ray/Pixel Oranı |
|---|---|---|---|
| **Fovea** | 1.0× (tam) | Merkez %10 | 1.0× |
| **Orta** | 0.5× (yarım) | %10-30 | 0.25× |
| **Periferik** | 0.25× (çeyrek) | %30-100 | 0.0625× |

### 3.2 GPU Compute Entegrasyonu

```wgsl
@compute @workgroup_size(8, 8)
fn foveated_ray_march(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let screen_size = vec2f(textureDimensions(visibility_buffer));

    let pixel_norm = vec2f(pixel) / screen_size;
    let dist_to_fovea = distance(pixel_norm, fovea_center);

    var step_multiplier: f32;
    if (dist_to_fovea < fovea_radius) {
        step_multiplier = 1.0;
    } else if (dist_to_fovea < mid_radius) {
        step_multiplier = 2.0;
    } else {
        step_multiplier = 4.0;
    }

    let ray = camera_get_ray(pixel);
    let hit = ray_march_adaptive(ray, step_multiplier);

    visibility_buffer_write(pixel, hit);
}
```

### 3.3 Peripheral Animasyon Durdurma

```rust
pub struct FoveatedAnimationController {
    animated_entities: Vec<AnimatedEntity>,
}

impl FoveatedAnimationController {
    pub fn update(&mut self, fovea_center: Vec2, screen_positions: &HashMap<Entity, Vec2>) {
        for entity in &mut self.animated_entities {
            let screen_pos = screen_positions[&entity.id];
            let dist = distance(screen_pos, fovea_center);

            entity.update_hz = if dist < 0.1 {
                60.0
            } else if dist < 0.3 {
                20.0
            } else {
                5.0
            };
        }
    }
}
```

### 3.4 Performans

| Metrik | Uniform Rendering | Foveated | Azalma |
|---|---|---|---|
| **Ray/pixel sayısı** | 1.0× | **0.2-0.4×** | **-60-80%** |
| **Animasyon update** | 60Hz tüm ekran | **Adaptive** | **-99.3%** (periferik) |
| **Frame time** | 16.7ms | **6-10ms** | **-40-65%** |
| **GPU power** | 100% | **30-50%** | **-50-70%** |
