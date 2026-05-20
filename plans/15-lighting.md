# 07 — Aydınlatma Sistemi

## 1. Aydınlatma Sistemi — 5-Kademeli Hybrid Mimari

### 1.1 Genel Bakış

| Kademe | Ad | Yöntem | Frekans | Performans |
|---|---|---|---|---|
| **L0** | Direct Light | Analytic (sun, point lights) | Her frame | ~0.1ms |
| **L1** | Block Light (BFS) | CPU SIMD flood-fill + two-phase removal | Değişiklikte | <100µs/torch |
| **L2** | Sky Light | Column-first + heightmap (Starlight-style) | Chunk load/değişiklik | <0.5ms/sector |
| **L3** | Indirect GI (near) | Clustered Voxel GI + visibility buffer | Her 5 frame | <3ms |
| **L4** | Indirect GI (far) | SVDAG ray march + Hi-Z occlusion | Her 10 frame | <2ms |

### Temel Prensipler

- **L0 = Direct:** Anlık, maliyetsiz — mesh'e doğrudan bake
- **L1 = Block:** BFS zaten gerekli, SIMD ile ultra-hızlı — mesh vertex color'a bake
- **L2 = Sky:** Starlight-style column-first, XBrickMap slab bitmask'inden heightmap O(1)
- **L3 = Indirect near:** Clustered GI — oyuncuya yakın alanlarda doğru GI
- **L4 = Indirect far:** SVDAG ray march — uzaktaki alanlarda yaklaşık GI

---

### 1.2 Light Data Formatı (16-bit Packed)

```
┌─────────────────────────────────────────┐
│ Light Data (16 bit per voxel)           │
├─────────────────────────────────────────┤
│ Bits 0-3:   Sky Light (0-15)            │
│ Bits 4-7:   Block Light R (0-15)        │
│ Bits 8-11:  Block Light G (0-15)        │
│ Bits 12-15: Block Light B (0-15)        │
└─────────────────────────────────────────┘
```

```rust
#[repr(transparent)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LightData(pub u16);

impl LightData {
    #[inline]
    pub fn sky(&self) -> u8 { (self.0 & 0xF) as u8 }

    #[inline]
    pub fn block_r(&self) -> u8 { ((self.0 >> 4) & 0xF) as u8 }

    #[inline]
    pub fn block_g(&self) -> u8 { ((self.0 >> 8) & 0xF) as u8 }

    #[inline]
    pub fn block_b(&self) -> u8 { ((self.0 >> 12) & 0xF) as u8 }

    #[inline]
    pub fn new(sky: u8, r: u8, g: u8, b: u8) -> Self {
        Self(
            (sky & 0xF) as u16
            | ((r & 0xF) as u16) << 4
            | ((g & 0xF) as u16) << 8
            | ((b & 0xF) as u16) << 12,
        )
    }

    #[inline]
    pub fn wlp_max(a: u32, b: u32) -> u32 {
        let lt = Self::wlp_less_than(a, b);
        a ^ ((a ^ b) & lt)
    }

    #[inline]
    pub fn wlp_less_than(a: u32, b: u32) -> u32 {
        const COMPONENT_MASK: u32 = 0x0F0F0F0F;
        const BORROW_GUARD: u32 = 0x08080808;
        const CARRY_MASK: u32 = 0x10101010;
        let d = (((a & COMPONENT_MASK) | BORROW_GUARD) - (b & COMPONENT_MASK)) & CARRY_MASK;
        (d | (d >> 3) | (d >> 4)) & COMPONENT_MASK
    }

    #[inline]
    pub fn wlp_decrement(x: u32) -> u32 {
        let d = ((x & 0x0F0F0F0F) | 0x02020202) - 0x01010101;
        let b = d & 0x10101010;
        (d + (b >> 4)) & 0x0F0F0F0F
    }
}
```

**Bellek Hesabı:**
- Sub-brick başına: 8 voxel × 16 bit = 16 byte
- Brick başına: ~64 sub-brick × ~16 byte = ~1KB (left-packed)
- Slab başına: ~64 brick × ~1KB = ~64KB (sparse)
- Sector: ~128-256KB (ortalama arazi)

---

### 1.3 L1 — Block Light (SIMD BFS Flood-Fill)

#### Propagation (Işık Yerleştirme)

```rust
pub struct BlockLightEngine {
    queue: VecDeque<BfsNode>,
    visited: AHashMap<IVec3, u8>,
    buffer_pool: BufferPool,
}

#[repr(C)]
pub struct BfsNode {
    pub pos: IVec3,
    pub light_level: u8,
}

impl BlockLightEngine {
    pub fn place_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        level: u8,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        updates.push(LightUpdate { pos, light: level, color });

        self.queue.clear();
        self.queue.push_back(BfsNode { pos, light_level: level });
        self.visited.clear();
        self.visited.insert(pos, level);

        while let Some(node) = self.queue.pop_front() {
            let current_level = node.light_level;
            if current_level <= 1 { continue; }

            for dir in DIRECTIONS_6 {
                let neighbor = node.pos + dir;
                if self.visited.contains_key(&neighbor) { continue; }
                if sector.is_opaque(neighbor) { continue; }

                let new_level = current_level - 1;
                let existing = sector.get_light(neighbor);

                if existing + 2 <= new_level {
                    self.visited.insert(neighbor, new_level);
                    self.queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: new_level,
                    });
                    updates.push(LightUpdate {
                        pos: neighbor,
                        light: new_level,
                        color,
                    });
                }
            }
        }

        updates
    }
}
```

#### Two-Phase Removal (Işık Kaldırma)

```rust
impl BlockLightEngine {
    pub fn remove_light(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        color: LightColor,
    ) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        let boundary_sources = self.zero_dependents(sector, pos, color, &mut updates);

        for source in boundary_sources {
            let new_updates = self.place_light(sector, source.pos, source.level, color);
            updates.extend(new_updates);
        }

        updates
    }

    fn zero_dependents(
        &mut self,
        sector: &Sector,
        pos: IVec3,
        color: LightColor,
        updates: &mut Vec<LightUpdate>,
    ) -> Vec<BoundarySource> {
        let old_level = sector.get_light_at(pos, color);
        let mut boundary_sources = Vec::new();

        self.queue.clear();
        self.queue.push_back(BfsNode { pos, light_level: old_level });

        while let Some(node) = self.queue.pop_front() {
            for dir in DIRECTIONS_6 {
                let neighbor = node.pos + dir;
                let neighbor_level = sector.get_light_at(neighbor, color);

                if neighbor_level < node.light_level {
                    updates.push(LightUpdate { pos: neighbor, light: 0, color });
                    self.queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: neighbor_level,
                    });
                } else if neighbor_level >= node.light_level {
                    boundary_sources.push(BoundarySource {
                        pos: neighbor,
                        level: neighbor_level,
                    });
                }
            }
        }

        boundary_sources
    }
}
```

#### SIMD Acceleration (15x Hızlanma)

```rust
use wide::{u32x4, u64x4};

pub fn propagate_simd(
    slab: &mut Slab,
    light_data: &mut [LightData],
    queue: &mut BfsQueue,
) {
    while let Some(node) = queue.pop() {
        let level = node.light_level;
        let current = u32x4::from([level as u32; 4]);
        let neighbor = load_neighbor_light_simd(light_data, node.pos);
        let should_update = wlp_less_than_simd(neighbor + 2, current);

        if should_update.any() {
            let new_level = wlp_decrement_simd(current);
            store_neighbor_light_simd(light_data, node.pos, new_level);
            queue.push_bulk(neighbor_positions(should_update));
        }
    }
}
```

**Performans (Ryzen 9 7900, voxel-light crate):**

| Operasyon | Level 7 | Level 10 | Level 14 |
|---|---|---|---|
| Propagation (scalar) | 17µs | 60µs | 174µs |
| Propagation (SIMD) | ~5µs | ~18µs | ~52µs |
| Removal (tek kaynak) | 105µs | — | 432µs |
| Full place+remove cycle | — | — | ~300µs (SIMD) |

---

### 1.4 L2 — Sky Light (Column-First + Heightmap)

#### Heightmap'ten Sky Source Setup (O(1))

```rust
impl Sector {
    pub fn build_sky_heightmap(&self) -> [i16; 32 * 32] {
        let mut heightmap = [128i16; 32 * 32];

        for (slab_idx, slab) in self.slabs.iter().enumerate().rev() {
            if slab.slab_mask == 0 { continue; }

            for brick_idx in slab.slab_mask.iter_ones() {
                let bx = brick_idx % 4;
                let bz = (brick_idx / 4) % 4;
                let by = brick_idx / 16;

                let world_x = bx * 8;
                let world_z = bz * 8;
                let world_y = slab_idx * 32 + by * 8;

                for dx in 0..8 {
                    for dz in 0..8 {
                        let sx = (world_x + dx) as usize;
                        let sz = (world_z + dz) as usize;
                        let idx = sx + sz * 32;
                        if world_y as i16 < heightmap[idx] {
                            heightmap[idx] = world_y as i16;
                        }
                    }
                }
            }
        }

        heightmap
    }
}
```

#### Column-First Propagation

```rust
impl SkyLightEngine {
    pub fn propagate_sky(&mut self, sector: &Sector) -> Vec<LightUpdate> {
        let mut updates = Vec::new();
        let heightmap = sector.build_sky_heightmap();

        for sx in 0..32 {
            for sz in 0..32 {
                let sky_y = heightmap[sx + sz * 32];

                for y in (0..sky_y).rev() {
                    updates.push(LightUpdate {
                        pos: IVec3::new(sx as i32, y, sz as i32),
                        light: 15,
                        color: LightColor::Sky,
                    });

                    if sx == 0 || sx == 31 || sz == 0 || sz == 31 {
                        self.horizontal_queue.push(BfsNode {
                            pos: IVec3::new(sx as i32, y, sz as i32),
                            light_level: 14,
                        });
                    }
                }
            }
        }

        self.horizontal_bfs(sector, &mut updates);
        updates
    }

    fn horizontal_bfs(&mut self, sector: &Sector, updates: &mut Vec<LightUpdate>) {
        while let Some(node) = self.horizontal_queue.pop_front() {
            if node.light_level == 0 { continue; }

            for dir in DIRECTIONS_4_HORIZONTAL {
                let neighbor = node.pos + dir;
                if sector.is_opaque(neighbor) { continue; }

                let existing = sector.get_sky_light(neighbor);
                if existing + 2 <= node.light_level {
                    let new_level = node.light_level - 1;
                    updates.push(LightUpdate {
                        pos: neighbor,
                        light: new_level,
                        color: LightColor::Sky,
                    });
                    self.horizontal_queue.push_back(BfsNode {
                        pos: neighbor,
                        light_level: new_level,
                    });
                }
            }
        }
    }
}
```

**Performans:**
- Açık arazi (çöl): ~300 queued entry (Starlight optimizasyonu)
- Vanilla: ~2000+ queued entry
- **~7x az queue işlemi**

---

### 1.5 L3 — Indirect GI (Clustered Voxel GI)

#### Mip Level'den Cluster Oluşturma

```rust
pub struct LightCluster {
    pub center: Vec3,
    pub normal: Vec3,
    pub lit_voxel_count: u32,
    pub accumulated_irradiance: Vec3,
    pub visible_from_camera: bool,
}

impl Sector {
    pub fn build_light_clusters(&self) -> Vec<LightCluster> {
        let mut clusters = Vec::new();

        for (slab_idx, slab) in self.slabs.iter().enumerate() {
            for (brick_idx, brick) in slab.bricks.iter().enumerate() {
                for group in brick.mip_quarter.iter_ones() {
                    let center = brick_quarter_center(brick_idx, group);
                    let normal = estimate_cluster_normal(brick, group);
                    let lit_count = count_lit_voxels(brick, group);

                    if lit_count > 0 {
                        clusters.push(LightCluster {
                            center,
                            normal,
                            lit_voxel_count: lit_count,
                            accumulated_irradiance: Vec3::ZERO,
                            visible_from_camera: false,
                        });
                    }
                }
            }
        }

        clusters.sort_by(|a, b| a.normal.dot(b.normal).partial_cmp(&0.5).unwrap());
        clusters
    }
}
```

#### Visibility Test (3D Bresenham)

```rust
pub fn test_cluster_visibility(
    cluster: &LightCluster,
    camera_pos: Vec3,
    sector: &Sector,
) -> bool {
    let dir = (camera_pos - cluster.center).normalize();
    let steps = (cluster.center.distance(camera_pos)).ceil() as i32;

    let mut pos = cluster.center.as_ivec3();
    for _ in 0..steps {
        if sector.is_opaque(pos) {
            return false;
        }
        pos += dir.as_ivec3();
    }

    true
}
```

#### Irradiance Gathering

```rust
impl IndirectGIEngine {
    pub fn gather_irradiance(
        &mut self,
        clusters: &mut [LightCluster],
        sector: &Sector,
    ) {
        for cluster in clusters.iter_mut() {
            if !cluster.visible_from_camera { continue; }

            let mut total_irradiance = Vec3::ZERO;
            let mut visible_count = 0;

            for other in clusters.iter() {
                if other.lit_voxel_count == 0 { continue; }

                if is_visible(cluster.center, other.center, sector) {
                    let dist = cluster.center.distance(other.center);
                    let attenuation = 1.0 / (1.0 + dist * dist);
                    total_irradiance += other.accumulated_irradiance * attenuation;
                    visible_count += 1;
                }
            }

            if visible_count > 0 {
                cluster.accumulated_irradiance = total_irradiance / visible_count as f32;
            }
        }
    }
}
```

**Avantaj:** 131.072 voxel → ~500-1000 cluster → **100x daha az visibility test**.

---

### 1.6 L4 — Indirect GI (SVDAG Cone Tracing)

#### SVDAG Cone March (WGSL)

```wgsl
@compute @workgroup_size(64)
fn svdag_cone_trace(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let ray = camera_get_ray(pixel);

    let hit = visibility_buffer_load(pixel);
    if (hit.depth == MAX_DEPTH) { return; }

    var irradiance: vec3f = vec3f(0.0);
    let directions = get_hemisphere_directions(hit.normal);

    for (var i = 0u; i < 6u; i++) {
        let cone_dir = directions[i];
        let cone_aperture = 0.5;

        let radiance = svdag_cone_march(
            hit.position,
            cone_dir,
            cone_aperture,
            svdag_root,
        );
        irradiance += radiance;
    }

    irradiance /= 6.0;
    irradiance_cache_store(hit.voxel_coord, irradiance);
}

fn svdag_cone_march(
    origin: vec3f,
    direction: vec3f,
    aperture: f32,
    root: u32,
) -> vec3f {
    var t: f32 = 0.0;
    var radiance: vec3f = vec3f(0.0);
    var cone_width: f32 = aperture;

    for (var i = 0u; i < 64u; i++) {
        let pos = origin + direction * t;
        let (node, lod) = svdag_query_lod(root, pos, cone_width);

        if (node.is_leaf) {
            radiance += node.radiance * node.opacity;
            break;
        }

        t += get_node_size(lod);
        cone_width *= 1.5;
    }

    return radiance;
}
```

---

### 1.7 Hierarchical Light Culling

```rust
pub struct LightCullingMask {
    pub slab_light_mask: u64,
    pub brick_light_mask: u64,
    pub sorted_lights: Vec<LightSource>,
}

impl LightCullingMask {
    pub fn sort_lights_morton(&mut self) {
        self.sorted_lights.sort_by_key(|l| {
            morton_encode_3d(
                l.pos.x as u32,
                l.pos.y as u32,
                l.pos.z as u32,
            )
        });
    }

    #[inline]
    pub fn slab_has_light(&self, brick_index: usize) -> bool {
        self.slab_light_mask & (1 << brick_index) != 0
    }

    #[inline]
    pub fn brick_has_light(&self, sub_index: usize) -> bool {
        self.brick_light_mask & (1 << sub_index) != 0
    }
}
```

**Avantaj:**
- Boş slab → tüm light propagation atla (O(1))
- Boş brick → 64 voxel atla
- Morton order → nearby light'lar aynı bitmask sector'de
- 10.000+ light için bile etkili

---

### 1.8 Temporal Accumulation (TAA-Style)

```wgsl
@compute @workgroup_size(8, 8)
fn temporal_accumulate(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;

    let current = current_frame_irradiance(pixel);
    let history = history_buffer_load(pixel);

    let motion = motion_vector_buffer_load(pixel);
    let prev_pixel = pixel - motion;
    let history_sample = history_buffer_sample(prev_pixel);

    let blend_factor = compute_voxel_blend_factor(pixel);
    let result = mix(history_sample, current, blend_factor);

    final_irradiance_store(pixel, result);
    history_buffer_store(pixel, result);
}
```

---

### 1.9 Mesh'e Light Bake

```rust
pub fn bake_light_to_mesh(
    mesh: &mut MeshData,
    sector: &Sector,
    face_vertices: &[IVec3],
) {
    for vertex in face_vertices {
        let light_samples = [
            sector.get_light(*vertex),
            sector.get_light(*vertex + IVec3::new(1, 0, 0)),
            sector.get_light(*vertex + IVec3::new(0, 1, 0)),
            sector.get_light(*vertex + IVec3::new(1, 1, 0)),
        ];

        let smooth_light = smooth_lighting(light_samples);
        vertex.color = light_to_color(smooth_light);
    }
}

fn smooth_lighting(samples: [LightData; 4]) -> LightData {
    let mut sum = 0u32;
    let mut count = 0u32;

    for sample in samples {
        if sample.sky() > 0 || sample.block_r() > 0 {
            sum += sample.0 as u32;
            count += 1;
        }
    }

    if count == 0 {
        LightData::default()
    } else {
        LightData((sum / count) as u16)
    }
}
```

---

### 1.10 Tier-Bazlı Lighting Stratejisi

| Tier | Yöntem | Güncelleme | Not |
|---|---|---|---|
| **ACTIVE** (0-96m) | L0+L1+L2+L3 (CPU BFS + Clustered GI) | Her değişiklikte | En doğru, mesh'e baked |
| **WARM** (96-384m) | L0+L1+L2 (CPU BFS) | Her 3 frame | Yumuşak geçiş |
| **DISTANT** (384m-1.5km) | L0+L4 (SVDAG cone trace) | Her 10 frame | Yaklaşık GI |
| **ARCHIVE** (1.5km+) | L0 sadece | — | Render edilmez |

---

### 1.11 GPU Lighting Pipeline

```
┌──────────────────────────────────────────────────────────────┐
│                    LIGHTING FRAME                             │
├──────────────────────────────────────────────────────────────┤
│ Pass 1: Direct Light (CPU)                                   │
│   → Sun, point lights — analytic, mesh'e bake                │
├──────────────────────────────────────────────────────────────┤
│ Pass 2: Block Light BFS (CPU SIMD)                           │
│   → Dirty sector'lar için BFS flood-fill                     │
│   → Two-phase removal + re-propagation                       │
├──────────────────────────────────────────────────────────────┤
│ Pass 3: Sky Light (CPU)                                      │
│   → Column-first + heightmap (O(1) source setup)             │
│   → Yatay BFS spread (overhang/mağara)                       │
├──────────────────────────────────────────────────────────────┤
│ Pass 4: Clustered GI (GPU Compute)                           │
│   → Cluster build (mip level'den)                            │
│   → Visibility test (3D Bresenham)                           │
│   → Irradiance gathering                                     │
├──────────────────────────────────────────────────────────────┤
│ Pass 5: SVDAG Cone Trace (GPU Compute)                       │
│   → Hi-Z occlusion culling                                   │
│   → Hierarchical LOD cone march                              │
│   → Temporal accumulation                                    │
├──────────────────────────────────────────────────────────────┤
│ Pass 6: Light → Mesh Bake (CPU)                              │
│   → Smooth lighting (4-vertex average)                       │
│   → Vertex color write                                       │
└──────────────────────────────────────────────────────────────┘
```

---

### 1.12 Neural Irradiance Volume (Faz 6 Vision)

**Adobe NIV (2024)** tekniği — uzun vadeli optimizasyon:

```
Neural Irradiance Volume:
  - Pre-computed irradiance field (MLP ile sıkıştırılmış)
  - 1-5MB bellek (geleneksel probe'lardan 10x küçük)
  - ~1ms inference (consumer GPU, 1080p)
  - G-buffer input (position + normal)
  - Noise-free, ray tracing/denoising gerektirmez

Strata Entegrasyonu:
  - Tier 3 (Distant) için NIV kullan
  - SVDAG'den training data üret (offline)
  - Runtime'da G-buffer → NIV inference → indirect diffuse
  - Dynamic objeler için de çalışır (unseen objects)
```

---

### 1.13 Crate Organizasyonu (Aydınlatma)

```
crates/
  lighting/
    ├── mod.rs                  ← Lighting plugin entry point
    ├── light_data.rs           ← 16-bit packed light data
    ├── engine.rs               ← LightEngine (orchestrator)
    ├── direct/
    │   ├── mod.rs              ← Direct lighting
    │   ├── sun.rs              ← Directional sun light
    │   └── point.rs            ← Point/spot lights
    ├── block/
    │   ├── mod.rs              ← Block light
    │   ├── bfs_cpu.rs          ← CPU BFS flood-fill
    │   ├── bfs_simd.rs         ← SIMD-accelerated BFS
    │   ├── removal.rs          ← Two-phase removal
    │   └── colored.rs          ← RGB channel propagation
    ├── sky/
    │   ├── mod.rs              ← Sky light system
    │   ├── column_first.rs     ← Column-first propagation
    │   ├── heightmap.rs        ← Slab bitmask'ten heightmap
    │   └── day_night.rs        ← Day/night cycle
    ├── indirect/
    │   ├── mod.rs              ← Indirect GI system
    │   ├── clustered.rs        ← Clustered Voxel GI
    │   ├── cone_trace.rs       ← Voxel cone tracing
    │   ├── irradiance_cache.rs ← Per-face irradiance cache
    │   └── visibility.rs       ← 3D Bresenham visibility test
    ├── culling/
    │   ├── mod.rs              ← Light culling system
    │   ├── hierarchical.rs     ← Hierarchical bitmask
    │   ├── morton.rs           ← Morton Z-order sorting
    │   └── priority.rs         ← Light update priority queue
    ├── mesh_bake.rs            ← Light data → vertex color
    ├── tier.rs                 ← Tier-bazlı lighting stratejisi
    └── gpu/
        ├── mod.rs              ← GPU lighting pipelines
        ├── svdag_light.rs      ← SVDAG cone tracing
        ├── hi_z.rs             ← Hi-Z occlusion for lighting
        ├── temporal.rs         ← Temporal accumulation
        └── neural_irradiance.rs← Neural Irradiance Volume (Faz 6)
```

---

### 1.14 Performans Hedefleri (Aydınlatma)

| Metrik | Hedef | Not |
|---|---|---|
| Tek torch propagation (SIMD) | <100µs | Level-14, wide crate |
| Torch removal + re-propagate | <300µs | Two-phase + SIMD |
| Sector skylight (açık arazi) | <0.5ms | Heightmap O(1) + column-first |
| Clustered GI (near) | <3ms | 100x az visibility test |
| SVDAG cone trace (far) | <2ms | Hi-Z + hierarchical LOD |
| Light culling (10K lights) | <0.5ms | Hierarchical bitmask + Morton |
| Light → mesh bake | <2ms/sector | Smooth lighting (4-vertex avg) |
| Temporal accumulation | <1ms/frame | Voxel-specific TAA |
| Bellek (light data) | 16 bit/voxel | Sky 4-bit + RGB 4×4-bit |
| GPU irradiance cache | <1ms/frame | Temporal accumulation |
