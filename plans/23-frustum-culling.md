# 26 — Frustum Culling & Render Optimizasyonları

## 1. Genel Bakış

Strata'nın frustum culling sistemi **GPU compute-based**'tir. Her render pass öncesinde görünür sector'lar belirlenir ve sadece onlar render edilir.

### Temel Prensipler

- **GPU compute:** Culling GPU'da yapılır (CPU bottleneck yok)
- **Hierarchical:** Sector → Slab → Brick hiyerarşik culling
- **Hi-Z occlusion:** Derinlik buffer ile görünmeyen sector'lar atlanır
- **Temporal coherence:** Önceki frame'in culling sonuçları reuse edilir

---

## 2. Frustum

```rust
/// View frustum — 6 plane ile tanımlanır.
#[derive(Clone)]
pub struct Frustum {
    pub planes: [Plane; 6],
}

impl Frustum {
    /// View-projection matrix'ten frustum oluştur.
    pub fn from_matrix(view_proj: Mat4) -> Self {
        let rows = view_proj.to_cols_array_2d();

        let planes = [
            // Left
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][0], rows[1][3] + rows[1][0], rows[2][3] + rows[2][0]),
                rows[3][3] + rows[3][0],
            ),
            // Right
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][0], rows[1][3] - rows[1][0], rows[2][3] - rows[2][0]),
                rows[3][3] - rows[3][0],
            ),
            // Bottom
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][1], rows[1][3] + rows[1][1], rows[2][3] + rows[2][1]),
                rows[3][3] + rows[3][1],
            ),
            // Top
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][1], rows[1][3] - rows[1][1], rows[2][3] - rows[2][1]),
                rows[3][3] - rows[3][1],
            ),
            // Near
            Plane::from_vecs(
                Vec3::new(rows[0][3] + rows[0][2], rows[1][3] + rows[1][2], rows[2][3] + rows[2][2]),
                rows[3][3] + rows[3][2],
            ),
            // Far
            Plane::from_vecs(
                Vec3::new(rows[0][3] - rows[0][2], rows[1][3] - rows[1][2], rows[2][3] - rows[2][2]),
                rows[3][3] - rows[3][2],
            ),
        ];

        Self { planes }
    }

    /// AABB frustum içinde mi?
    pub fn contains_aabb(&self, aabb: &Aabb) -> FrustumIntersection {
        let mut result = FrustumIntersection::Inside;

        for plane in &self.planes {
            let p_vertex = plane.positive_vertex(aabb);
            let n_vertex = plane.negative_vertex(aabb);

            if plane.distance(p_vertex) < 0.0 {
                return FrustumIntersection::Outside;
            }

            if plane.distance(n_vertex) < 0.0 {
                result = FrustumIntersection::Intersect;
            }
        }

        result
    }

    /// Sphere frustum içinde mi?
    pub fn contains_sphere(&self, center: Vec3, radius: f32) -> FrustumIntersection {
        let mut result = FrustumIntersection::Inside;

        for plane in &self.planes {
            let distance = plane.distance(center);

            if distance < -radius {
                return FrustumIntersection::Outside;
            }

            if distance < radius {
                result = FrustumIntersection::Intersect;
            }
        }

        result
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrustumIntersection {
    Outside,
    Intersect,
    Inside,
}

/// Plane tanımı.
#[derive(Clone, Copy)]
pub struct Plane {
    pub normal: Vec3,
    pub distance: f32,
}

impl Plane {
    pub fn from_vecs(normal: Vec3, distance: f32) -> Self {
        let len = normal.length();
        Self {
            normal: normal / len,
            distance: distance / len,
        }
    }

    pub fn distance(&self, point: Vec3) -> f32 {
        self.normal.dot(point) + self.distance
    }

    /// AABB'nin plane'e en uzak köşesi.
    pub fn positive_vertex(&self, aabb: &Aabb) -> Vec3 {
        Vec3::new(
            if self.normal.x > 0.0 { aabb.max.x } else { aabb.min.x },
            if self.normal.y > 0.0 { aabb.max.y } else { aabb.min.y },
            if self.normal.z > 0.0 { aabb.max.z } else { aabb.min.z },
        )
    }

    /// AABB'nin plane'e en yakın köşesi.
    pub fn negative_vertex(&self, aabb: &Aabb) -> Vec3 {
        Vec3::new(
            if self.normal.x < 0.0 { aabb.max.x } else { aabb.min.x },
            if self.normal.y < 0.0 { aabb.max.y } else { aabb.min.y },
            if self.normal.z < 0.0 { aabb.max.z } else { aabb.min.z },
        )
    }
}
```

---

## 3. GPU Frustum Culling

```wgsl
// GPU frustum culling compute shader
@group(0) @binding(0)
var<storage, read> sector_aabbs: array<SectorAabb>;

@group(0) @binding(1)
var<storage, read_write> sector_visible: array<u32>;

@group(0) @binding(2)
var<uniform> frustum: FrustumUniform;

struct FrustumUniform {
    planes: array<vec4f, 6>,
    sector_count: u32,
}

struct SectorAabb {
    min: vec3f,
    max: vec3f,
}

@compute @workgroup_size(256)
fn frustum_cull(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let idx = id.x;
    if (idx >= frustum.sector_count) { return; }

    let aabb = sector_aabbs[idx];

    // 6 plane kontrolü
    var visible = true;

    for (var i: u32 = 0u; i < 6u; i = i + 1u) {
        let plane = frustum.planes[i];
        let normal = plane.xyz;

        // Positive vertex
        var p_vertex: vec3f;
        p_vertex.x = select(aabb.min.x, aabb.max.x, normal.x > 0.0);
        p_vertex.y = select(aabb.min.y, aabb.max.y, normal.y > 0.0);
        p_vertex.z = select(aabb.min.z, aabb.max.z, normal.z > 0.0);

        // Plane distance
        let dist = dot(normal, p_vertex) + plane.w;

        if (dist < 0.0) {
            visible = false;
            break;
        }
    }

    sector_visible[idx] = u32(visible);
}
```

---

## 4. Hi-Z Occlusion

```wgsl
// Hi-Z occlusion culling
@group(0) @binding(0)
var<storage, read> sector_aabbs: array<SectorAabb>;

@group(0) @binding(1)
var<storage, read_write> sector_visible: array<u32>;

@group(0) @binding(2)
var<texture_2d<f32>> hiz_texture;

@group(0) @binding(3)
var<uniform> params: OcclusionParams;

struct OcclusionParams {
    sector_count: u32,
    texture_size: vec2u,
    view_proj: mat4x4f,
}

@compute @workgroup_size(64)
fn hiz_occlusion_cull(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let idx = id.x;
    if (idx >= params.sector_count) { return; }

    // Önce frustum'dan geçmiş mi kontrol et
    if (sector_visible[idx] == 0u) { return; }

    let aabb = sector_aabbs[idx];

    // AABB'yi screen space'e projekte et
    let min_screen = project_to_screen(aabb.min, params.view_proj);
    let max_screen = project_to_screen(aabb.max, params.view_proj);

    // Hi-Z texture'den depth oku
    let min_depth = read_hiz_depth(min_screen.xy, max_screen.xy);

    // AABB'nin en yakın noktası Hi-Z depth'den uzakta mı?
    let aabb_near = min(aabb.min.z, aabb.max.z);

    if (aabb_near > min_depth) {
        // Occluded — görünmez
        sector_visible[idx] = 0u;
    }
}

fn project_to_screen(pos: vec3f, view_proj: mat4x4f) -> vec4f {
    let clip = view_proj * vec4f(pos, 1.0);
    return clip / clip.w;
}

fn read_hiz_depth(min_uv: vec2f, max_uv: vec2f) -> f32 {
    // AABB'nin kapladığı Hi-Z bölgesinden minimum depth oku
    // Hierarchical: en uygun LOD level'dan oku
    let size = max_uv - min_uv;
    let max_dim = max(size.x, size.y);

    // LOD level seç
    let lod = u32(log2(max_dim * f32(textureDimensions(hiz_texture).x)));

    // Center UV'den oku
    let center = (min_uv + max_uv) * 0.5;
    return textureSampleLevel(hiz_texture, my_sampler, center, f32(lod)).r;
}
```

---

## 5. Hi-Z Buffer Oluşturma

```wgsl
// Hi-Z buffer build — depth buffer'dan hierarchical Z-buffer oluştur
@group(0) @binding(0)
var<texture_2d<f32>> depth_texture;

@group(0) @binding(1)
var<texture_storage_2d<f32, write>> hiz_texture;

@group(0) @binding(2)
var<uniform> params: HiZParams;

struct HiZParams {
    level: u32,
    src_size: vec2u,
}

@compute @workgroup_size(16, 16)
fn build_hiz_level(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let uv = vec2f(id.xy) / vec2f(params.src_size);

    if (id.x >= params.src_size.x || id.y >= params.src_size.y) { return; }

    // 2x2 bloktan minimum depth al
    let offset = vec2i(id.xy) * 2;
    let size = vec2i(2, 2);

    var min_depth: f32 = 1.0;

    for (var dy: i32 = 0i; dy < size.y; dy = dy + 1i) {
        for (var dx: i32 = 0i; dx < size.x; dx = dx + 1i) {
            let sample_pos = vec2i(offset + vec2i(dx, dy));
            let sample_uv = vec2f(sample_pos) / vec2f(params.src_size * 2u);
            let depth = textureSampleLevel(depth_texture, my_sampler, sample_uv, 0.0).r;
            min_depth = min(min_depth, depth);
        }
    }

    textureStore(hiz_texture, vec2i(id.xy), vec4f(min_depth));
}
```

---

## 6. Temporal Coherence

```rust
/// Temporal culling — önceki frame sonuçlarını reuse eder.
pub struct TemporalCulling {
    /// Önceki frame görünür sector'lar.
    previous_visible: HashSet<SectorCoord>,

    /// Frame sayısı (temporal jitter için).
    frame_count: u64,

    /// Yeniden test aralığı (her N frame'de full test).
    retest_interval: u32,
}

impl TemporalCulling {
    /// Sector görünür mü? (temporal)
    pub fn is_visible(
        &mut self,
        coord: SectorCoord,
        frustum: &Frustum,
        aabb: &Aabb,
    ) -> bool {
        let was_visible = self.previous_visible.contains(&coord);

        // Her N frame'de full test
        if self.frame_count % self.retest_interval as u64 == 0 {
            let visible = frustum.contains_aabb(aabb) != FrustumIntersection::Outside;

            if visible {
                self.previous_visible.insert(coord);
            } else {
                self.previous_visible.remove(&coord);
            }

            return visible;
        }

        // Temporal: önceki frame görünürse, bu frame de görünür varsay
        // (sadece frustum dışına çıkanları test et)
        if was_visible {
            // Hızlı test — sadece center noktası
            let center = (aabb.min + aabb.max) / 2.0;
            if frustum.contains_sphere(center, 16.0) != FrustumIntersection::Outside {
                return true;
            }
        }

        // Full test
        let visible = frustum.contains_aabb(aabb) != FrustumIntersection::Outside;

        if visible {
            self.previous_visible.insert(coord);
        } else {
            self.previous_visible.remove(&coord);
        }

        visible
    }

    /// Frame sonu — state güncelle.
    pub fn end_frame(&mut self) {
        self.frame_count += 1;
    }
}
```

---

## 7. Culling Pipeline

```
Render Frame Culling Pipeline:
  ┌─────────────────────────────────────────┐
  │ 1. CPU: Frustum oluştur                 │
  │    (view-projection matrix'ten)         │
  ├─────────────────────────────────────────┤
  │ 2. GPU: Frustum culling (compute)       │
  │    → sector_visible[] buffer            │
  ├─────────────────────────────────────────┤
  │ 3. GPU: Hi-Z occlusion culling          │
  │    → sector_visible[] güncelle          │
  ├─────────────────────────────────────────┤
  │ 4. GPU: Indirect draw count hesapla     │
  │    → visible sector sayısını say        │
  ├─────────────────────────────────────────┤
  │ 5. GPU: Indirect draw dispatch          │
  │    → sadece görünür sector'lar render   │
  └─────────────────────────────────────────┘
```

---

## 8. Performans Hedefleri

| Metrik | Hedef | Not |
|---|---|---|
| Frustum culling (GPU) | <0.5ms | 1000+ sector |
| Hi-Z occlusion (GPU) | <0.3ms | 500+ sector |
| Culling oranı | %60-80 | Görünür/Toplam sector |
| Temporal coherence hit rate | >70% | Önceki frame reuse |
| CPU culling overhead | <0.1ms | Sadece frustum build |

---

## 9. Crate Organizasyonu

```
crates/
  render/
    ├── culling/
    │   ├── mod.rs          ← Culling sistemi
    │   ├── frustum.rs      ← Frustum, Plane
    │   ├── gpu_cull.rs     ← GPU frustum culling
    │   ├── hiz.rs          ← Hi-Z occlusion
    │   └── temporal.rs     ← Temporal coherence
    └── hiz_builder.rs      ← Hi-Z buffer build
```
