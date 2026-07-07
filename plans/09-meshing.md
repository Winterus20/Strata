# 09 — Meshing Sistemi

> **Olgunluk:** 🔒 Kesinleşti (`01-overview.md` §1.1, 2026-06-05). Anayasa `01`–`10`; `01`–`08` ile çelişirse önce anayasa güncellenir veya `09` revize edilir. `16`+ taslaklarla çelişirse **bu dosya** esas alınır.
> **Crate:** `meshing` (`02-implementation.md` — `binary_greedy/`, `indirect/gigabuffer.rs`, `ecs/`)
> **Bağımlılıklar:** `05-block-registry.md` (§18 Visibility LUT), `06-xbrickmap.md` (32³ sektör, §2.3 PackedVertex, §1.4 snapshot, Sector LightMap, komşu sorgu), `07-svdag.md` (§1.7 Hi-Z, DISTANT raycast), `08-streaming.md` (tier mesh stratejisi, CachedGreedy), `03-ecs-architecture.md` (§Filter-First, `NeedsRemesh`)
>
> **Harici doğrulama (2026-06):** [binary-greedy-meshing](https://crates.io/crates/binary-greedy-meshing) v0.5, [block-mesh-bgm](https://docs.rs/block-mesh-bgm), [0fps.net AO](https://0fps.net/2013/07/03/ambient-occlusion-for-minecraft-like-worlds/), [Exile meshing](https://thenumb.at/Voxel-Meshing-in-Exile/), [EngineersBox transparency+BGM](https://engineersbox.github.io/website/2024/09/19/transparency-with-binary-greedy-meshing.html), [OffsetAllocator](https://github.com/sebbbi/OffsetAllocator), [Ascendant gigabuffer](https://www.vkguide.dev/docs/ascendant/ascendant_geometry/)

## 1. Genel Bakış

Strata'nın meshing sistemi **trait-based**, **algorithm-agnostic** ve **GPU-first**'tir. Render crate hangi mesher'in çalıştığını bilmez. Sadece `Mesher` trait'ini uygular ve `MeshData` alır.

### Temel Prensipler

- **Trait-based:** `Mesher` trait — algoritma değiştirilebilir
- **Algorithm-agnostic:** Render crate mesher tipini bilmez
- **GPU-first:** Vertex pulling + indirect draw ile GPU-driven rendering
- **Binary-optimized:** Bitwise operations ile CPU meshing 5-7x hız artışı
- **Compact vertex:** 8-16 byte packed vertex format (VRAM %50-75 tasarruf)
- **Tier-aware:** `08-streaming.md` tier sistemiyle uyumlu hibrit meshing stratejisi
- **Incremental:** Sadece dirty bölgeler re-mesh edilir

### Mimari Kararlar Özeti

| Karar | Seçim | Gerekçe |
|---|---|---|
| CPU mesher | **Binary Greedy** | 55-65µs/sector (32³, LUT dahil) |
| Vertex format | **PackedQuad (8B/quad)** | Vertex pulling ile %75 VRAM tasarruf |
| GPU meshing | **Branchless compute** | `firstTrailingBit` + `select` ile wavefront bölünmez |
| Draw strategy | **Multi-draw indirect + GPU cull** | 400K+ chunk CPU'da cull edilemez |
| VRAM sub-alloc | **offset-allocator (TLSF)** | Değişken mesh boyutu, ≤%12.5 overhead |
| Incremental | **ECS `NeedsRemesh` + async pool** | `World` bypass yok (`03`) |
| Tier strategy | **Hybrid (greedy/cached/SVDAG)** | Mesafe bazlı kalite/maliyet dengesi |

---

## 2. Mesher Trait

```rust
/// Mesher trait — tüm meshing algoritmaları bunu uygular.
pub trait Mesher: Send + Sync {
    /// Bir sector'ü mesh'le.
    fn mesh_sector(&self, sector: &Sector, registry: &BlockRegistry) -> MeshData;

    /// Bir yüzü mesh'le (incremental update).
    fn mesh_face(
        &self,
        sector: &Sector,
        face: BlockFace,
        pos: IVec3,
        registry: &BlockRegistry,
    ) -> Option<FaceMeshData>;

    /// Mesh tipi (transparent/opaque).
    fn mesh_type(&self) -> MeshType;

    /// Mesher'ın çıktığı vertex formatı.
    fn vertex_format(&self) -> VertexFormat;
}

/// Mesh tipi.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MeshType {
    /// Opaque bloklar (önce render edilir).
    Opaque,
    /// Transparent bloklar (sonra render edilir, depth write off).
    Transparent,
    /// Cutout bloklar (alpha test).
    Cutout,
}

/// Vertex format seçimi.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// Standart vertex (32B) — debug/legacy.
    Standard,
    /// Compact vertex (16B) — genel kullanım.
    Compact,
    /// Packed quad (8B/quad) — vertex pulling.
    PackedQuad,
}

/// Mesh verisi — GPU'ya upload-ready.
pub struct MeshData {
    /// Vertex buffer verisi (format'a göre yorumlanır).
    pub vertex_data: Vec<u8>,

    /// Index buffer verisi (None = vertex pulling, shared quad index).
    pub indices: Option<Vec<u32>>,

    /// AABB (frustum culling için).
    pub aabb: Aabb,

    /// Vertex/quad sayısı.
    pub element_count: u32,

    /// Index sayısı (None ise element_count * 6).
    pub index_count: u32,

    /// Kullanılan vertex formatı.
    pub format: VertexFormat,
}

/// AABB.
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}
```

---

## 3. Compact Vertex Formats

### 3.1 Compact Vertex (16B) — Genel Kullanım

Mevcut `Vertex` 32B idi. Aşağıdaki compact format **%50 VRAM tasarrufu** sağlar.
Referans: Ascendant Engine (Vulkan Guide), Exile Engine.

```rust
/// Compact packed vertex — 16 bytes.
/// Sector-local unorm pozisyon + octahedral normal + packed UV/AO/tex.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CompactVertex {
    /// X pozisyon (unorm, sector-local 0..32 → u16).
    pub position_x: u16,
    /// Y pozisyon (unorm, sector-local 0..32 → u16).
    pub position_y: u16,
    /// Z pozisyon (unorm, sector-local 0..32 → u16).
    pub position_z: u16,
    /// Octahedral encoded normal (düşük precision yeterli, faceted görünüm).
    pub encoded_normal: u16,
    /// UV X (unorm, texture atlas koordinatı).
    pub uv_x: u16,
    /// UV Y (unorm, texture atlas koordinatı).
    pub uv_y: u16,
    /// Block ID (texture array index + flags).
    pub block_id: u16,
    /// Extra: AO (2-bit × 4 köşe = 8-bit) + light (8-bit) + padding.
    pub extra: u16,
}
// 16 bytes — 4 vertex per quad = 64 bytes/quad
```

**Unpack shader (WGSL):**
```wgsl
fn unpack_vertex(v: CompactVertex, sector_origin: vec3<f32>) -> VertexOut {
    let pos = vec3<f32>(
        f32(v.position_x) / 65535.0 * 32.0,
        f32(v.position_y) / 65535.0 * 32.0,
        f32(v.position_z) / 65535.0 * 32.0,
    ) + sector_origin;

    let normal = octahedral_decode(v.encoded_normal);
    let uv = vec2<f32>(f32(v.uv_x) / 65535.0, f32(v.uv_y) / 65535.0);

    return VertexOut(pos, normal, uv, v.block_id);
}
```

### 3.2 Packed Quad (8B/quad) — Vertex Pulling

Binary greedy meshing çıktısı ile uyumlu. Her quad **sadece 8 byte**.
Referans: `binary-greedy-meshing` crate (cgerikj).

```rust
/// Packed quad — 8 bytes. Vertex pulling ile render edilir.
/// Shader gl_VertexID ile 4 köşeyi expand eder.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackedQuad {
    /// Packed: x(6) | y(6) | z(6) | width(6) | height(6) | face_dir(2) = 32 bits.
    pub packed_geometry: u32,
    /// Block type ID (texture + material lookup).
    pub block_type: u8,
    /// AO values: 4 × 2-bit (her köşe için 0-3 AO).
    pub ao_packed: u8,
    /// Light data (sky + block light packed).
    pub light: u8,
    /// Padding / flags.
    pub flags: u8,
}
// 8 bytes per QUAD — 4 vertex yerine! (%87.5 VRAM tasarrufu naive'e göre)
```

> **Light packing (`light: u8`):** Quad'ın pozisyonu (quad'ın taban köşesi + 1 voxel offset)
> kullanılarak `SectorLightMap`'te lookup yapılır.
> ```rust
> fn pack_light(quad_pos: IVec3, quad_center: IVec3, sector: &Sector) -> u8 {
>     let sample_pos = quad_center; // quad merkezi, AO'da kullanılan corner pozisyonu
>     // Sector LightMap: 32×32×32, 4-bit skylight + 4-bit blocklight = 8-bit/voxel
>     // (`06-xbrickmap.md` §Sector LightMap — 32KB texture_3d).
>     let (sky, block) = sector.get_light(sample_pos);
>     (sky << 4) | block // üst 4-bit skylight, alt 4-bit blocklight
> }
> ```
> `PackedQuad.new()` imzasına `light: u8` eklenir ve greedy merge sırasında
> her quad için sector lightmap'ten doldurulur. Shader'da unpack:
> ```wgsl
> let sky_light   = (quad.light >> 4u) & 0xFu;
> let block_light =  quad.light        & 0xFu;
> ```

**Vertex pulling shader (WGSL):**
```wgsl
// Shared quad index buffer (6 indices = 2 triangles, tüm quad'lar için reuse).
const QUAD_INDICES = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);

@vertex
fn vs_main(
    @builtin(vertex_index) vert_idx: u32,
    @builtin(instance_index) inst_idx: u32,
) -> VertexOutput {
    // Her instance = 1 PackedQuad
    let quad = quads[inst_idx];

    // Unpack geometry
    let x      = (quad.packed_geometry      ) & 0x3Fu;
    let y      = (quad.packed_geometry >>  6u) & 0x3Fu;
    let z      = (quad.packed_geometry >> 12u) & 0x3Fu;
    let width  = (quad.packed_geometry >> 18u) & 0x3Fu;
    let height = (quad.packed_geometry >> 24u) & 0x3Fu;
    let face   = (quad.packed_geometry >> 30u) & 0x03u;

    // Vertex ID → quad corner (0-3)
    let corner = vert_idx % 4u;
    let du = select(0u, width, corner == 1u || corner == 3u);
    let dv = select(0u, height, corner == 2u || corner == 3u);

    let local_pos = compute_corner_pos(x + du, y + dv, z, face);
    // ...
}
```

### 3.3 Legacy Vertex (32B) — Debug Only

```rust
/// Legacy vertex — sadece debug/test amaçlı.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LegacyVertex {
    pub position: [f32; 3],   // 12B
    pub normal: u8,           // 1B
    pub uv: [f32; 2],        // 8B
    pub color: [u8; 4],      // 4B
    pub tex_index: u8,       // 1B
    pub ao: u8,              // 1B
    pub _padding: [u8; 2],   // 2B — toplam 32B (cache-unfriendly)
}
```

> **NOT:** `LegacyVertex` production'da KULLANILMAZ. Sadece debug visualization ve test
> fixture'ları için tutulur.

### 3.4 Vertex Format İlişki Haritası (06 ↔ 09)

`06-xbrickmap.md` §2.3'teki `PackedVertex` (4B/vertex) ile `09`'daki mesh output formatları **farklı katmanlara** aittir — çelişki değil, tamamlayıcı:

| Format | Kaynak | Katman | Boyut | Kullanım |
|--------|--------|--------|-------|----------|
| `PackedQuad` (§3.2) | **09** | Mesh output (greedy) | 8B/quad | ACTIVE tier opaque — vertex pulling |
| `CompactVertex` (§3.1) | **09** | Mesh output (non-greedy) | 16B/vertex | Transparent/cutout, fallback |
| `PackedVertex` | **06** §2.3 | GPU buffer (ray trace) | 4B/vertex | XBrickMap DDA traversal |
| `LegacyVertex` (§3.3) | **09** | Debug | 32B/vertex | Test/visualization only |

> **NOT:** `06` PackedVertex, XBrickMap ray tracing pipeline'ının GPU vertex buffer formatıdır.
> `09` PackedQuad ise greedy meshing çıktısıdır. Render pipeline'da mesh → GPU dönüşümü
> `10-render-pipeline.md` (taslak) kapsamındadır.

---

## 4. Binary Greedy Meshing (CPU — Primary)

Naive greedy meshing yerine **bitwise operations** kullanan binary greedy algoritma.
32³ sektör için optimize edilmiş, **5-7x hız artışı** sağlar.

Referans: `binary-greedy-meshing` crate v0.5 (cgerikj), 0fps.net, Exile Engine.

> **NOT:** `binary-greedy-meshing` crate v0.5+ transparent block desteği de sunar
> (`fast_mesh` + opaque/transparency mask, ~90µs/64³). Strata, transparent blokları
> ayrı `NonGreedyMesher` (§11b) ile işler — farklı render pipeline (depth write OFF,
> back-to-front sort) gerektiğinden ayrı mesh output zorunludur.

### 4.0 Sektör sınırı ve komşu okuma (anayasa)

Meshing **32³ interior** üzerinde çalışır; yüz sınırı culling, Visibility LUT ve AO için
**±1 voksellik komşuluk** komşu yüklü sektörlerden okunur (`SectorNeighborView`, `06` cross-sector API).
64³ padded tek buffer **kullanılmaz** — Strata birimi 32³ sektör kalır (`06`, `08` ile hizalı).

| Durum | Politika |
|--------|----------|
| Komşu sektör ACTIVE/WARM ve yüklü | Gerçek vokseller; doğru seam |
| Komşu yüklenmemiş / ARCHIVE | **Conservative solid** — yüz cull + AO karanlık (güvenli, geçici pop-in yok) |
| `NeedsRemesh` komşu bleed | Edit sonrası 6 komşu sektör de işaretlenir (`§10`) |

### 4.1 Algoritma Adımları

1. **Occupancy mask** oluştur — `32×32` array of `u32`, her bit = 1 voxel (0=hava, 1=opaque)
2. **Face mask** oluştur — bitwise AND/NOT ile 6 yön × 32×32 grid'i aynı anda cull et
3. **Greedy merge** — bitwise shift + `trailing_zeros` ile face'leri birleştir

### 4.2 Implementasyon

```rust
/// Binary greedy mesher — bitwise operations ile yüksek performans.
/// Referans: binary-greedy-meshing crate v2 (cgerikj).
/// Face mask — 32×32 grid of u32 (her bit = 1 visible voxel face).
///
/// Her face için ayrı bir mask. Layer = normal ekseni, row = outer eksen.
/// 1 sektör için 6 face × 1024 u32 = 24KB (L2 cache'te kalır).
///
/// WGSL karşılığı (`07` SVDAG ghost page, `06` GPU feedback):
/// compute shader `face_masks[face * 1024 + layer * 32 + row]` → atomicOr.
///
/// Referans: `binary-greedy-meshing` crate v0.5 `face_masks: Box<[u64]>`;
/// `block-mesh-bgm` `visible_rows: Vec<u64>`.
#[derive(Clone, Copy)]
struct FaceMaskBinary {
    face: BlockFace,
    /// 32 layers × 32 rows = 1024 u32.
    rows: [[u32; 32]; 32],
}

impl FaceMaskBinary {
    #[inline(always)]
    fn set_row(&mut self, layer: usize, row: usize, val: u32) {
        self.rows[layer][row] = val;
    }

    #[inline(always)]
    fn get_row(&self, layer: usize, row: usize) -> u32 {
        self.rows[layer][row]
    }

    /// All-zero init.
    const fn zero(face: BlockFace) -> Self {
        Self {
            face,
            rows: [[0u32; 32]; 32],
        }
    }
}

/// 3D occupancy → face mask index.
/// face yönüne göre hangi (layer, row) planına bakılacağını belirler.
///
/// SECTOR_SIZE=32 olduğunda:
/// - Y- (down), Y+ (up): layer=Z, row=X, bit=Y
/// - X- (left), X+ (right): layer=Y, row=Z, bit=X
/// - Z- (front), Z+ (back): layer=Y, row=X, bit=Z
#[inline(always)]
fn index_of(face: BlockFace, layer: usize, row: usize) -> usize {
    // face'in normal eksenine göre layer/row haritası.
    // Gerçek implementasyonda BlockFace::to_axes() kullanılır.
    // Şimdilik: layer = row = 0 placeholder (doğru implementasyon §crate organizasyonu).
    layer * 32 + row
}

pub struct BinaryGreedyMesher {
    mesh_type: MeshType,
    /// `05-block-registry.md` §18 — precomputed görünürlük tablosu.
    /// face_visible[source][neighbor] → bool (8KB, L1 cache'e sığar).
    /// ~%40 face culling hızlanması sağlar.
    visibility: VisibilityTable,
}

impl BinaryGreedyMesher {
    /// Sector boyutu (32×32×32).
    const SECTOR_SIZE: usize = 32;
    /// Occupancy mask stack boyutu: 32×32 = 1024 u32 = 4KB (stack'e sığar).
    const OCCUPANCY_SIZE: usize = Self::SECTOR_SIZE * Self::SECTOR_SIZE;

    pub fn new(mesh_type: MeshType, registry: &BlockRegistry) -> Self {
        Self {
            mesh_type,
            visibility: VisibilityTable::build(registry),
        }
    }

    /// Occupancy mask oluştur — 32×32 array of u32 (32 sektör için).
    /// Her bit = 1 voxel. 0 = hava, 1 = dolu.
    ///
    /// **Neden Vec değil de [u32; 1024]?**
    /// Boyut sabit (1024 × 4B = 4KB), L1 cache'e sığar.
    /// `Vec` heap allocation'ı fragmentasyon yaratır (`AGENTS.md` §3.B.3).
    /// Stack array alloc-free'dir ve `IncrementalMesher` scratch buffer'dan okur.
    ///
    /// Referans: `binary-greedy-meshing` crate v0.5 → `opaque_mask: &[u64]`
    /// pre-computed mask kullanır, heap alloc yok. 0fps.net binary greedy.
    fn build_occupancy_mask(
        &self,
        sector: &Sector,
        scratch: &mut OccupancyScratch,
    ) -> &[u32; Self::OCCUPANCY_SIZE] {
        let mask = &mut scratch.occupancy;
        mask.fill(0);
        for z in 0..Self::SECTOR_SIZE {
            for x in 0..Self::SECTOR_SIZE {
                let mut column: u32 = 0;
                for y in 0..Self::SECTOR_SIZE {
                    if sector.get_block(IVec3::new(x as i32, y as i32, z as i32)).is_some() {
                        column |= 1 << y;
                    }
                }
                mask[z * Self::SECTOR_SIZE + x] = column;
            }
        }
        mask
    }

/// Reusable scratch buffer — BinaryGreedyMesher'ın tüm temporary buffer'ları.
///
/// `IncrementalMesher.rebuild_dirty` içinde bir kez oluşturulur,
/// her mesh_sector çağrısında `clear()` ile sıfırlanır.
/// Heap allocation sayısı: 1 → 0'a düşer.
///
/// Referans: `binary-greedy-meshing` crate `MeshData::clear()` pattern.
#[derive(Default)]
struct OccupancyScratch {
    /// 32×32 occupancy mask (1024 u32 = 4KB, stack-like).
    occupancy: [u32; BinaryGreedyMesher::OCCUPANCY_SIZE],
    /// 6 face mask × 1024 u32 = 24KB (en sıcak buffer).
    face_masks: [FaceMaskBinary; 6],
    /// Quad output (capacity: ~800 = ~6.4KB).
    quads: Vec<PackedQuad>,
}

impl OccupancyScratch {
    fn clear(&mut self) {
        self.occupancy.fill(0);
        self.quads.clear();
    }
}

    /// Face mask oluştur — 6 yüz × 32×32 grid of u32.
    /// İki katmanlı culling:
    ///   1. Occupancy: bitwise AND/NOT ile hava bloklarını cull et (fast path).
    ///   2. Visibility LUT (`05` §18): dolu-dolu komşular arasında block type
    ///      görünürlük sorgusu (~0.3ns/query, L1 cache hit).
    ///
    /// **Referans:** `block-mesh-bgm` crate `prep` modülü → `build_axis_columns()`
    /// aynı iki-katmanlı yaklaşımı kullanır. `binary-greedy-meshing` crate
    /// `fast_face_culling()` aynı `opaque_mask & trans_mask` pattern'i.
    fn build_face_masks(
        &self,
        sector: &Sector,
        registry: &BlockRegistry,
        occupancy: &[u32; Self::OCCUPANCY_SIZE],
        scratch: &mut OccupancyScratch,
    ) {
        for face_idx in 0..6 {
            let face = BlockFace::from_index(face_idx);
            let mask = &mut scratch.face_masks[face_idx];
            mask.face = face;

            for layer in 0..Self::SECTOR_SIZE {
                for row in 0..Self::SECTOR_SIZE {
                    let current = occupancy[index_of(face, layer, row)];
                    let neighbor = occupancy[index_of(face, layer + 1, row)];

                    // Fast path: occupancy bazlı culling (hava = invisible).
                    let mut visible = current & !neighbor;

                    // Slow path: her iki taraf dolu ama farklı block type.
                    // Visibility LUT (`05` §18) ile ~0.3ns/query.
                    let both = current & neighbor;
                    if both != 0 {
                        // LUT refinement: aynı type → gizli, farklı/şeffaf → görünür
                        visible |= self.visibility.refine_face(
                            sector, registry, face, layer, row, both,
                        );
                    }

                    mask.set_row(layer, row, visible);
                }
            }
        }
    }

    /// Greedy merge — bitwise ile 32 face'i aynı anda birleştir.
    /// Çıktı doğrudan `scratch.quads`'a yazılır (heap alloc yok).
    ///
    /// **Referans:** `binary-greedy-meshing` crate v0.5 `face_merging()`:
    /// aynı carry-based greedy row merge algoritması. `block-mesh-bgm` crate
    /// `merge::mesh_face_rows()` aynı pattern.
    fn greedy_merge_binary(
        &self,
        face_idx: usize,
        sector: &Sector,
        registry: &BlockRegistry,
        scratch: &mut OccupancyScratch,
    ) {
        let face_mask = &mut scratch.face_masks[face_idx];
        let quads = &mut scratch.quads;
        let face = face_mask.face;

        for layer in 0..Self::SECTOR_SIZE {
            for row in 0..Self::SECTOR_SIZE {
                let mut mask = face_mask.get_row(layer, row);
                while mask != 0 {
                    let start = mask.trailing_zeros() as usize;

                    let width = self.find_same_type_run(
                        sector, registry, face, layer, row, start, mask,
                    );
                    let width_mask = ((1u32 << width) - 1) << start;

                    let mut height = 1usize;
                    while layer + height < Self::SECTOR_SIZE {
                        let next_row = face_mask.get_row(layer + height, row);
                        let next_run = self.find_same_type_run(
                            sector, registry, face, layer + height, row, start, next_row,
                        );
                        if next_run < width || (next_row & width_mask) != width_mask {
                            break;
                        }
                        height += 1;
                    }

                    let origin = face.voxel_at(layer, row, start);
                    let ao = compute_ao_packed(origin, face, sector);
                    let light = pack_quad_light(origin, sector);

                    quads.push(PackedQuad::new(
                        start as u32,
                        layer as u32,
                        row as u32,
                        width as u32,
                        height as u32,
                        face,
                        ao,
                        light,
                    ));

                    let clear_mask = ((1u32 << width) - 1) << start;
                    for h in 0..height {
                        let cleared = face_mask.get_row(layer + h, row) & !clear_mask;
                        face_mask.set_row(layer + h, row, cleared);
                    }
                    mask = face_mask.get_row(layer, row);
                }
            }
        }
    }

    /// Aynı `merge_value` (block type + variant) için contiguous 1-bit run.
    /// `binary-greedy-meshing` v0.5: merge sırasında voxel buffer lookup.
    #[inline]
    fn find_same_type_run(
        &self,
        sector: &Sector,
        registry: &BlockRegistry,
        face: BlockFace,
        layer: usize,
        row: usize,
        start: usize,
        row_mask: u32,
    ) -> usize {
        let base = sector.palette_index_at(face, layer, row, start);
        let mut run = 1usize;
        while start + run < Self::SECTOR_SIZE {
            if row_mask & (1u32 << (start + run)) == 0 {
                break;
            }
            let other = sector.palette_index_at(face, layer, row, start + run);
            if !registry.same_merge_value(base, other) {
                break;
            }
            run += 1;
        }
        run
    }
}

impl PackedQuad {
    #[inline]
    pub fn new(
        x: u32,
        y: u32,
        z: u32,
        width: u32,
        height: u32,
        face: BlockFace,
        ao_packed: u8,
        light: u8,
    ) -> Self {
        let face_dir = face.index() as u32;
        let packed_geometry =
            (x & 0x3F)
                | ((y & 0x3F) << 6)
                | ((z & 0x3F) << 12)
                | ((width & 0x3F) << 18)
                | ((height & 0x3F) << 24)
                | ((face_dir & 0x03) << 30);
        Self {
            packed_geometry,
            block_type: 0, // merge run'dan dominant palette index (implementasyonda doldurulur)
            ao_packed,
            light,
            flags: 0,
        }
    }
}

/// Sector lightmap'ten quad merkezi örnekleme (`06` §Sector LightMap).
#[inline]
fn pack_quad_light(sample: IVec3, sector: &Sector) -> u8 {
    let (sky, block) = sector.get_light(sample);
    (sky << 4) | block
}

impl Mesher for BinaryGreedyMesher {
    fn mesh_sector(&self, sector: &Sector, registry: &BlockRegistry) -> MeshData {
        // Reusable scratch buffer — IncrementalMesher'dan alınır veya yerel oluşturulur.
        // memory: occupancy 4KB + face_masks 24KB + quads ~6.4KB
        let mut scratch = OccupancyScratch::default();

        let occupancy = self.build_occupancy_mask(sector, &mut scratch);
        self.build_face_masks(sector, registry, occupancy, &mut scratch);

        for face_idx in 0..6 {
            self.greedy_merge_binary(face_idx, sector, registry, &mut scratch);
        }

        MeshData {
            vertex_data: bytemuck::cast_slice(&scratch.quads).to_vec(),
            indices: None, // Vertex pulling — shared quad index buffer
            aabb: compute_sector_aabb(sector),
            element_count: scratch.quads.len() as u32,
            index_count: scratch.quads.len() as u32 * 6,
            format: VertexFormat::PackedQuad,
        }
    }

    fn mesh_face(&self, sector: &Sector, face: BlockFace, pos: IVec3, registry: &BlockRegistry) -> Option<FaceMeshData> {
        // Incremental: sadece etkilenen face mask'i yeniden hesapla
        // ...
        None
    }

    fn mesh_type(&self) -> MeshType { self.mesh_type }
    fn vertex_format(&self) -> VertexFormat { VertexFormat::PackedQuad }
}
```

### 4.3 Naive vs Binary Karşılaştırma

| Metrik | Naive Greedy (eski §3) | Binary Greedy (bu §) |
|---|---|---|
| Chunk mesh süresi | ~300-500µs | **55-65µs** (32³) |
| Yöntem | Loop + Vec push | Bitwise ops |
| Memory alloc | `Vec<MergedQuad>` her iterasyon | Pre-allocated bitmask |
| Cache behavior | Random access | Sequential row scan |
| Output | 32B vertex × 4 | **8B packed quad** |

> **Benchmark notu:** `binary-greedy-meshing` crate v0.5, 64³ padded chunk üzerinde
> 65µs (opaque) / 90µs (transparent) rapor eder. Strata 32³ sektör kullanır
> (1/8 hacim) — hedef <65µs (LUT dahil) geréeklidir.

> **MIGRASYON NOTU:** Eski §3'teki naive `GreedyMesher` KALDIRILMIŞTIR.
> Tüm CPU meshing `BinaryGreedyMesher` üzerinden yapılır.

### 4.4 Visibility LUT Entegrasyonu (`05` §18)

`05-block-registry.md` §18'deki `VisibilityTable` (8KB precomputed bit matrix), greedy
meshing'in en sıcak döngüsü olan face culling'i **%40** hızlandırır:

| Yaklaşım | Maliyet/yüz | 98K yüz (1 sektör) |
|---|---|---|
| Flags check (runtime) | ~0.5ns | ~49µs |
| **Visibility LUT** | **~0.3ns** | **~29µs** |
| **Kazanç** | **%40 hızlanma** | **~20µs tasarruf** |

`BinaryGreedyMesher`, `VisibilityTable`'ı `build_face_masks` içinde iki katmanlı kullanır:
1. **Occupancy fast path:** `current & !neighbor` → hava bloklarını bedavaya cull et.
2. **LUT slow path:** `both = current & neighbor` → dolu-dolu komşular için block type
   görünürlük sorgusu (`visibility.refine_face`).

### 4.5 T-Junction Artifact Yönetimi

Greedy meshing farklı boyutlarda komşu quad'lar ürettiğinde, quad kenarlarının
orta noktasında T-junction oluşur. Bu, sub-pixel gap artifact'larına yol açabilir.

**Çözüm** (Referans: `binary-greedy-meshing` v2):
1. **Slight expansion:** Quad'lar ~1px büyük render edilir (gap'leri örter).
2. **Eye-space pozisyon:** Vertex pozisyonları eye-space'te hesaplanır (float precision artırır).
3. **Grid snapping:** Vertex pozisyonları integer grid'e snap edilir (seamless tiling).

> **NOT:** PackedQuad'un 6-bit pozisyon precision'ı (0-63) 32³ sektör için yeterlidir
> (max 32 pozisyon + 32 genişlik). T-junction, precision hatasından değil, komşu
> quad'ların farklı boyutlarından kaynaklanır — çözüm geometric, numeric değil.

### 4.6 Bellek ve Scratch Buffer (Heap-Free Hot Path)

| Buffer | Boyut | Yaşam döngüsü | Kaynak |
|--------|-------|---------------|--------|
| `occupancy` | 4 KB (`[u32; 1024]`) | `OccupancyScratch` | `binary-greedy-meshing` precomputed mask |
| `face_masks[6]` | 24 KB (fixed) | aynı scratch | `block-mesh-bgm` `visible_rows` |
| `quads` | ~6.4 KB (`Vec`, capacity reuse) | `clear()` ile reset | `MeshData::clear()` pattern |

**Kural:** `mesh_sector` içinde `Vec::new()` yalnızca ilk `OccupancyScratch` oluşturulurken;
`IncrementalMesher` tek bir `OccupancyScratch` + `GlobalMeshScratch` resource tutar (thread-local veya
`AsyncComputeTaskPool` worker başına bir scratch — data race yok).

**AO-safe merge (opsiyonel, Faz 1b):** Production default = maximum merge + post-merge AO
bi-linear shader. Kalite kritikse `block-mesh-bgm::binary_greedy_quads_ao_safe` politikası
port edilir (`ao.rs` exterior-plane bitmask).

---

## 5. Culled Meshing (Debug/Fallback)

Sadece hidden face culling — greedy merge YOK. Default tier stratejisi **değildir**
(bkz. §8.1: WARM = CachedGreedy). Sadece low-end GPU fallback veya debug amaçlı kullanılır.
`MesherRegistry` override ile aktif edilebilir (§9).

```rust
/// Culled mesher — sadece hidden face culling, merge yok.
/// Debug/fallback amaçlı; default tier stratejisi değildir (bkz. §8.1).
///
/// BinaryGreedy ile aynı occupancy + Visibility LUT pipeline; yalnızca merge atlanır.
pub struct CulledMesher {
    mesh_type: MeshType,
    visibility: VisibilityTable, // `05` §18 — dolu-dolu komşu yüzleri doğru cull eder
}

impl CulledMesher {
    pub fn new(mesh_type: MeshType, registry: &BlockRegistry) -> Self {
        Self {
            mesh_type,
            visibility: VisibilityTable::build(registry),
        }
    }
}

impl Mesher for CulledMesher {
    fn mesh_sector(&self, sector: &Sector, registry: &BlockRegistry) -> MeshData {
        let mut scratch = OccupancyScratch::default();
        let mesher = BinaryGreedyMesher {
            mesh_type: self.mesh_type,
            visibility: self.visibility.clone(), // veya shared Arc
        };
        let occupancy = mesher.build_occupancy_mask(sector, &mut scratch);
        mesher.build_face_masks(sector, registry, occupancy, &mut scratch);

        let mut vertices = Vec::with_capacity(4096);
        for face_idx in 0..6 {
            let mask = &scratch.face_masks[face_idx];
            for layer in 0..32 {
                for row in 0..32 {
                    let mut visible = mask.get_row(layer, row);
                    while visible != 0 {
                        let bit = visible.trailing_zeros() as usize;
                        vertices.extend(CompactVertex::quad_from_voxel(
                            mask.face, layer, row, bit, sector, registry,
                        ));
                        visible &= visible - 1; // clear lowest set bit
                    }
                }
            }
        }
        MeshData::from_compact_vertices(vertices, VertexFormat::Compact)
    }

    fn mesh_face(&self, _s: &Sector, _f: BlockFace, _p: IVec3, _r: &BlockRegistry) -> Option<FaceMeshData> { None }
    fn mesh_type(&self) -> MeshType { self.mesh_type }
    fn vertex_format(&self) -> VertexFormat { VertexFormat::Compact }
}
```

### Tradeoff: Greedy vs Culled

| | Binary Greedy | Culled |
|---|---|---|
| Mesh süresi | ~55-65µs | **~30µs** |
| Vertex sayısı | %40-60 azaltma | %0 (ham) |
| GPU draw cost | Düşük | Yüksek |
| Use case | ACTIVE tier (+ WARM cache) | Debug/low-end fallback |

---

## 6. GPU Compute Meshing (Faz 2) — Branchless

### 6.1 Branchless GPU Meshing

Eski WGSL shader'daki `while` loop'ları **GPU wavefront'u böler**. Doğru yaklaşım:
`select`, `firstTrailingBit`, `countOneBits` gibi branchless intrinsics kullan.

```rust
/// GPU compute mesher — branchless, 3-pass pipeline.
pub struct GpuMesher {
    /// Pass 1: Face visibility mask (compute).
    face_mask_pipeline: wgpu::ComputePipeline,
    /// Pass 2: Prefix-sum (quad count + offset).
    prefix_sum_pipeline: wgpu::ComputePipeline,
    /// Pass 3: Vertex generation (compute).
    vertex_gen_pipeline: wgpu::ComputePipeline,

    bind_group_layout: wgpu::BindGroupLayout,
    input_buffer: wgpu::Buffer,
    face_mask_buffer: wgpu::Buffer,
    prefix_sum_buffer: wgpu::Buffer,
    output_buffer: wgpu::Buffer,
    indirect_buffer: wgpu::Buffer,
    mesh_type: MeshType,
}

impl GpuMesher {
    /// 3-pass GPU meshing pipeline.
    pub fn mesh_sector_gpu(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sector: &Sector,
    ) {
        queue.write_buffer(&self.input_buffer, 0, &sector.to_gpu_data());

        let mut encoder = device.create_command_encoder(&Default::default());

        // Pass 1: Face visibility mask (branchless)
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.face_mask_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(4, 4, 6); // 32×32×6 faces
        }

        // Pass 2: Prefix sum (quad offset hesabı)
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.prefix_sum_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(32, 1, 1);
        }

        // Pass 3: Vertex generation (branchless quad emit)
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.vertex_gen_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(4, 4, 6);
        }

        queue.submit(Some(encoder.finish()));
    }
}
```

### 6.2 Branchless WGSL Shader

```wgsl
// Pass 1: Face visibility — branchless hidden face culling
@group(0) @binding(0) var<storage, read> sector_data: array<u32>;
@group(0) @binding(1) var<storage, read_write> face_masks: array<u32>;

@compute @workgroup_size(32, 1, 1)
fn compute_face_mask(@builtin(global_invocation_id) id: vec3<u32>) {
    let face = id.z;
    let row = id.y;
    let col = id.x;

    // Bitwise: komşu voxel ile XOR → 1 = görünür yüz
    let current_block = sector_load(face, col, row, 0u);
    let neighbor_block = sector_load(face, col, row, 1u);

    // Branchless visibility: current exists AND (neighbor is air OR neighbor is transparent)
    let is_visible = select(0u, 1u,
        current_block != 0u && (neighbor_block == 0u || is_transparent(neighbor_block))
    );

    // Atomic OR ile face mask'e yaz (32 thread = 1 u32 row)
    atomicOr(&face_masks[face * 32u * 32u + row * 32u + col / 32u], is_visible << (col % 32u));
}

// Pass 3: Vertex generation — branchless, firstTrailingBit ile quad bulma
@group(0) @binding(0) var<storage, read> face_masks: array<u32>;
@group(0) @binding(1) var<storage, read> prefix_sum: array<u32>;
@group(0) @binding(2) var<storage, read_write> quads: array<PackedQuad>;

@compute @workgroup_size(8, 8, 1)
fn generate_quads(@builtin(global_invocation_id) id: vec3<u32>) {
    let row = id.x;
    let layer = id.y;
    let face = id.z;

    let mask = face_masks[face * 1024u + layer * 32u + row];
    if (mask == 0u) { return; }

    // Branchless: firstTrailingBit ile ilk visible face'i bul
    let first = firstTrailingBit(mask);
    // Branchless: trailing ones count = width
    let shifted = mask >> first;
    let width = firstTrailingBit(~shifted); // İlk 0-bit → run uzunluğu

    // Prefix sum'dan quad offset al
    let offset = prefix_sum[face * 1024u + layer * 32u + row];

    // PackedQuad yaz
    quads[offset] = pack_quad(first, layer, row, width, 1u, face);
}
```

### 6.3 Neden Branchless?

| Eski (branching) | Yeni (branchless) |
|---|---|
| `while` loop → wavefront divergence | `firstTrailingBit` → 1 cycle |
| `if/else` → thread stalling | `select` → predicated move |
| ~200µs/sector | **~50µs/sector** |
| GPU occupancy %40-60 | GPU occupancy **%90+** |

---

## 7. GPU-Driven Rendering: Indirect Draw + Culling

Tüm sektörler **tek bir draw call** ile render edilir. GPU compute shader görünür
sektörleri seçer, sadece onlar çizilir.

Referans: Ascendant Engine (Vulkan Guide), `06-xbrickmap.md` GPU feedback loop.

### 7.1 Architecture

```
┌──────────────┐     ┌───────────────┐     ┌────────────────┐
│ Sector Mesh  │────▶│  Gigabuffer   │────▶│ Indirect Draw  │
│ Data (CPU)   │     │  (GPU VRAM)   │     │ (GPU Cull)     │
└──────────────┘     └───────────────┘     └────────────────┘
     upload              sub-alloc          compute cull pass
                                               │
                                               ▼
                                         ┌──────────────┐
                                         │  Multi-Draw  │
                                         │  Indirect    │
                                         └──────────────┘
```

### 7.2 Gigabuffer

Tek büyük GPU buffer (~256–512 MB). Tüm sektör mesh verileri **sub-allocate** edilir.
32-bit byte offset (Ascendant Engine / Bevy mesh packing ile uyumlu); metadata CPU'da,
VRAM'da yalnızca ham quad/vertex bytes.

**Allocator seçimi:** Buddy/slab değil — **OffsetAllocator (TLSF ailesi)**.
Değişken boyutlu sektör mesh'leri (1.6–64 KB) için en düşük fragmentasyon ve O(1) alloc/free.

| Allocator | Fragmentasyon | Değişken boyut | GPU sub-alloc |
|-----------|---------------|----------------|---------------|
| Buddy | Yüksek (power-of-2 waste) | Kötü | Orta |
| Slab (sabit sınıf) | Düşük | Kötü (sınıf seçimi) | İyi (tek boyut) |
| **OffsetAllocator (TLSF)** | **≤%12.5 overhead** | **İyi** | **İyi (Bevy hedefi)** |

Referanslar: [sebbbi/OffsetAllocator](https://github.com/sebbbi/OffsetAllocator),
[pcwalton/offset-allocator](https://github.com/pcwalton/offset-allocator) (Rust, Bevy PR #13218),
[Vulkan Guide — Ascendant gigabuffer](https://www.vkguide.dev/docs/ascendant/ascendant_geometry/).

```rust
use offset_allocator::{Allocator, Allocation};

/// Gigabuffer — tek VRAM allocation, byte-offset sub-allocation.
pub struct GigaBuffer {
    buffer: wgpu::Buffer,
    /// O(1) alloc/free, hard real-time (TLSF-benzeri, 256 float bin).
    allocator: Allocator,
    capacity: u64,
}

#[derive(Clone, Copy)]
pub struct GigaBufferHandle {
    pub offset: u32, // byte offset into `buffer`
    pub size: u32,
}

impl GigaBuffer {
    pub const DEFAULT_BYTES: u64 = 512 * 1024 * 1024;

    pub fn new(device: &wgpu::Device, capacity_bytes: u64) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gigabuffer"),
            size: capacity_bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            allocator: Allocator::new(capacity_bytes as u32),
            capacity: capacity_bytes,
        }
    }

    pub fn upload_sector(
        &mut self,
        queue: &wgpu::Queue,
        mesh_data: &MeshData,
    ) -> Option<GigaBufferHandle> {
        let size = mesh_data.vertex_data.len() as u32;
        let alloc = self.allocator.allocate(size)?;
        queue.write_buffer(
            &self.buffer,
            alloc.offset as u64,
            &mesh_data.vertex_data,
        );
        Some(GigaBufferHandle {
            offset: alloc.offset,
            size,
        })
    }

    pub fn free_sector(&mut self, handle: GigaBufferHandle) {
        self.allocator.free(Allocation {
            offset: handle.offset,
            size: handle.size,
        });
    }

    /// Utilization raporu (debug HUD — `33-diagnostics`).
    pub fn utilization(&self) -> f32 {
        self.allocator.storage_report().total_allocated as f32
            / self.capacity as f32
    }
}
```

> **Neden slab değil?** Sektör başına quad sayısı 200–800 arası değişir → 1.6–6.4 KB
> tipik, boş sektör ~0 B. Slab sınıfları ya waste ya da OOM üretir. OffsetAllocator
> float-bin dağılımı ile her boyuta ≤%12.5 internal fragmentation garantisi verir.

### 7.3 Indirect Draw + GPU Culling

```rust
/// Her sektör için 1 indirect draw command.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkDrawIndirect {
    // Draw indirect params
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub vertex_offset: i32,
    pub first_instance: u32,
    // Chunk position (GPU culling için)
    pub chunk_x: i32,
    pub chunk_y: i32,
    pub chunk_z: i32,
}

/// GPU-side chunk info (culling için).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ChunkDrawInfo {
    pub position: [i32; 3],
    pub draw_type: i16,       // opaque/transparent/cutout
    pub quad_count: i16,      // -1 = data refresh gerekli
    pub gigabuffer_index: i32, // -1 = allocate edilmemiş
}
```

```wgsl
// GPU Cull Shader — frustum + distance culling
@group(0) @binding(0) var<storage, read> scene: SceneData;
@group(0) @binding(1) var<storage, read_write> draw_cmds: array<ChunkDrawIndirect>;
@group(0) @binding(2) var<storage, read> chunk_info: array<ChunkDrawInfo>;
@group(0) @binding(3) var<storage, read_write> draw_count: array<atomic<u32>>;

@compute @workgroup_size(256, 1, 1)
fn cull_and_draw(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= scene.chunk_count) { return; }

    let chunk = chunk_info[idx];
    if (chunk.quad_count <= 0) { return; }

    // Frustum cull (branchless sphere test)
    let center = vec3<f32>(
        f32(chunk.position[0]) + 16.0,
        f32(chunk.position[1]) + 16.0,
        f32(chunk.position[2]) + 16.0,
    );
    let radius = 27.7; // 32 * sqrt(3) / 2 ≈ sector bounding sphere

    if (!is_visible_sphere(center, radius, scene.frustum_planes)) { return; }

    // Visible — indirect draw command yaz
    let draw_idx = atomicAdd(&draw_count[0], 1u);
    draw_cmds[draw_idx].index_count = u32(chunk.quad_count) * 6u;
    draw_cmds[draw_idx].instance_count = 1u;
    draw_cmds[draw_idx].first_index = 0u;
    draw_cmds[draw_idx].vertex_offset = chunk.gigabuffer_index * 4; // quad başına 4 vertex
    draw_cmds[draw_idx].first_instance = draw_idx;
    draw_cmds[draw_idx].chunk_x = chunk.position[0];
    draw_cmds[draw_idx].chunk_y = chunk.position[1];
    draw_cmds[draw_idx].chunk_z = chunk.position[2];
}
```

> **NOT:** Bu sistem `06-xbrickmap.md` §GPU Feedback Loop ile entegre çalışır.
> GPU'nun SSBO'ya yazdığı görünürlük bilgisi, streaming öncelik sırasını belirler.
>
> **Cross-ref — Hi-Z + Visibility Buffer (`07` §1.7):**
> `07-svdag.md` §1.7'deki GPU-driven pipeline (tile selection → DAG ray marching →
> Hi-Z re-execution → color resolve) bu indirect draw sistemiyle **entegre** çalışır:
> - **Cull pass** (bu §): sector-level frustum + distance culling.
> - **Hi-Z pass** (`07` §1.7): önceki frame'in depth texture'ından oluşturulan mip
>   piramidi ile occlusion test — görünür sector'lerin gereksiz işlenmesini önler.
> - **Visibility buffer** (`07` §1.7): 64-bit packed (depth + normal + sector_id +
>   voxel_pos) — SVDAG ray march sonucu bu buffer'a yazılır, color resolve pass'te okunur.
>
> Meshing pipeline (bu §) sector mesh data'yı GigaBuffer'a upload eder; render pipeline
> (`10`, taslak) cull + Hi-Z + ray march + resolve pass'lerini orkestre eder.

---

## 8. Hybrid Meshing Strategy (Tier-Based)

`08-streaming.md` tier sistemi ile uyumlu: farklı mesafelerde farklı meshing stratejileri.

### 8.1 WARM Tier Kararı: Cached BinaryGreedy (CulledMesher DEĞİL)

**Eski tasarım (reddedildi):** WARM tier'da `CulledMesher` (~30µs, merge yok, ~1000-4000 vertex).

**Sorun:** `08-streaming.md` §3.3, WARM tier'da **XBrickMap mesh'in öncelikli** render
kaynağı olduğunu belirtir. CulledMesher kullanmak:
1. BinaryGreedy'den **daha fazla vertex** üretir (%40-60 merge kaybı → GPU draw cost artar).
2. WARM'da her geçişte re-mesh gerekir (BinaryGreedy ~55-65µs, Culled ~30µs — ama cache ile
   BinaryGreedy **0µs** re-mesh).
3. CulledMesh'in 16B/vertex formatı (CompactVertex), BinaryGreedy'nin 8B/quad'ından
   **8x daha fazla VRAM** kullanır (4000×16B = 64KB vs 800×8B = 6.4KB).

**Yeni tasarım:** ACTIVE'ten WARM'a geçişte **BinaryGreedy mesh cache'lenir**.
- Mesh, sector ACTIVE iken zaten hesaplanmış ve GigaBuffer'a upload edilmiştir.
- WARM'da mesh aynen kullanılır (0µs re-mesh, ~1.6-6.4KB VRAM).
- SVDAG fallback: `07` ghost page table ile — brick miss durumunda SVDAG ray cast.
- Dirty olursa (block edit): WARM'da da `NeedsRemesh` tetiklenir, BinaryGreedy re-mesh.

```rust
/// Tier bazlı meshing stratejisi.
#[derive(Clone, Copy)]
pub enum MeshingStrategy {
    /// ACTIVE tier (<96m): Full binary greedy + vertex pulling.
    /// En yüksek kalite, en pahalı (~55-65µs/sector, LUT dahil).
    BinaryGreedy,

    /// WARM tier (96-384m): ACTIVE'ten cache'lenmiş BinaryGreedy mesh.
    /// Re-mesh YOK (0µs). SVDAG fallback: `07` ghost page table.
    /// `08-streaming.md` §3.3 ile uyumlu: XBrickMap öncelikli, SVDAG fallback.
    CachedGreedy,

    /// DISTANT tier (384-1536m): SVDAG raycast (07-svdag.md §1.7).
    /// Meshing YOK — doğrudan GPU voxel raycast (visibility buffer + Hi-Z).
    SvdagRaycast,

    /// ARCHIVE tier (≥1536m): Impostor / billboard.
    /// En düşük kalite, en ucuz.
    Impostor,
}

/// Streaming tier'dan meshing stratejisi seç.
pub fn strategy_for_tier(tier: StreamingTier) -> MeshingStrategy {
    match tier {
        StreamingTier::Active => MeshingStrategy::BinaryGreedy,
        StreamingTier::Warm   => MeshingStrategy::CachedGreedy,
        StreamingTier::Distant => MeshingStrategy::SvdagRaycast,
        StreamingTier::Archive => MeshingStrategy::Impostor,
    }
}
```

### 8.2 Tier Transition Stratejisi

```rust
/// Tier geçişi yöneticisi — popping artifact'larını minimize eder.
pub struct TierTransitionManager {
    /// Hysteresis buffer (08-streaming.md §2.3 ile uyumlu).
    enter_extra_m: f32, // 16m
    /// Transition sırasında crossfade süresi.
    crossfade_frames: u32, // 3-5 frame
}
```

### 8.3 Tier Performans Tablosu

| Tier | Strateji | Vertex/sector | Mesh süresi | VRAM | Cache |
|---|---|---|---|---|---|
| ACTIVE | Binary Greedy + PackedQuad | ~200-800 quad | ~55-65µs | ~1.6-6.4KB | GigaBuffer'da |
| WARM | Cached Greedy (re-mesh yok) | ~200-800 quad | **0µs** (cache) | ~1.6-6.4KB | ACTIVE'ten kalma |
| DISTANT | SVDAG raycast (`07`) | 0 (no mesh) | 0µs | SVDAG node pool | — |
| ARCHIVE | Impostor | 1 quad | ~1µs | ~64B | — |

> **CulledMesher durumu:** `CulledMesher` (§5) WARM tier'da **kullanılmaz**.
> Sadece özel durumlarda (memory pressure, debug, low-end GPU fallback) `MesherRegistry`
> override ile aktif edilebilir (§9). Default strateji `CachedGreedy`'dir.

---

## 9. Mesher Registry

```rust
/// Mesher registry — farklı mesher'ları ve stratejileri yönetir.
pub struct MesherRegistry {
    /// Kayıtlı mesher'lar.
    meshers: HashMap<String, Box<dyn Mesher>>,
    /// Tier → mesher mapping.
    tier_strategy: HashMap<StreamingTier, String>,
    /// Aktif mesher (override, None = tier-based).
    active_override: Option<String>,
}

impl MesherRegistry {
    /// Mesher kaydet.
    pub fn register(&mut self, name: String, mesher: Box<dyn Mesher>) {
        self.meshers.insert(name, mesher);
    }

    /// Tier için strateji ata.
    pub fn set_tier_strategy(&mut self, tier: StreamingTier, mesher_name: &str) {
        self.tier_strategy.insert(tier, mesher_name.to_string());
    }

    /// Tier'a uygun mesher'ı al.
    pub fn mesher_for_tier(&self, tier: StreamingTier) -> &dyn Mesher {
        let name = self.active_override.as_ref()
            .or_else(|| self.tier_strategy.get(&tier))
            .expect("No mesher for tier");
        self.meshers.get(name).unwrap().as_ref()
    }

    /// Override: tüm tier'lar için tek mesher kullan.
    pub fn set_override(&mut self, name: &str) -> Result<(), MesherError> {
        if self.meshers.contains_key(name) {
            self.active_override = Some(name.to_string());
            Ok(())
        } else {
            Err(MesherError::UnknownMesher(name.to_string()))
        }
    }

    /// Override'ı kaldır (tier-based'e dön).
    pub fn clear_override(&mut self) {
        self.active_override = None;
    }
}

#[derive(Debug)]
pub enum MesherError {
    UnknownMesher(String),
}
```

---

## 10. Incremental Meshing (ECS + Async)

`03-ecs-architecture.md` **Filter-First** kuralı: `World::get_sector` ile ham erişim **yasak**.
Dirty işaretleme `NeedsRemesh` ZST component; mesh üretimi `AsyncComputeTaskPool` ile
arka planda, sonuç ana thread'de GigaBuffer'a yazılır.

Referans: Bevy `Changed<T>` + `AsyncComputeTaskPool` chunk mesh pattern (Stack Overflow / Bevy discussions).

### 10.1 ECS Bileşenleri

```rust
/// Sektör mesh'i yeniden üretilmeli (`03` filter-first).
#[derive(Component)]
pub struct NeedsRemesh;

/// Arka plan mesh task'i — poll ile tamamlanır.
#[derive(Component)]
pub struct SectorMeshTask {
    pub coord: SectorCoord,
    pub task: Task<MeshData>,
}

/// GigaBuffer'daki upload konumu (`07` / render extract ile paylaşılır).
#[derive(Component)]
pub struct SectorMeshGpu {
    pub handle: GigaBufferHandle,
    pub element_count: u32,
    pub aabb: Aabb,
}
```

### 10.2 Sistem Seti

```rust
/// Blok edit / palette değişince dirty işaretle.
/// `Changed<SectorPalette>` veya `ChunkDirty` event consumer.
pub fn mark_needs_remesh(
    mut commands: Commands,
    edited: Query<
        Entity,
        (
            With<Sector>,
            Or<(Changed<SectorPalette>, Added<NeedsRemesh>)>,
        ),
    >,
    neighbors: Query<&SectorCoord>,
) {
    for entity in edited.iter() {
        commands.entity(entity).insert(NeedsRemesh);
        // AO/lighting bleed: 6 komşu sektör entity'leri de `NeedsRemesh` (coord lookup).
    }
}

/// Dirty sektörler için async mesh task spawn.
pub fn spawn_sector_mesh_tasks(
    mut commands: Commands,
    query: Query<
        (Entity, &SectorCoord, &Sector),
        (With<NeedsRemesh>, Without<SectorMeshTask>),
    >,
    pool: Res<AsyncComputeTaskPool>,
    registry: Res<Arc<BlockRegistry>>,
    mesher: Res<Arc<BinaryGreedyMesher>>,
) {
    for (entity, coord, sector) in query.iter() {
        let sector = sector.clone_arc(); // `Arc<Sector>` snapshot — edit sırasında tutarlı
        let registry = Arc::clone(&registry);
        let mesher = Arc::clone(&mesher);

        let task = pool.spawn(async move {
            // Worker thread: kendi `OccupancyScratch` (thread_local veya stack).
            mesher.mesh_sector(&sector, &registry)
        });

        commands.entity(entity)
            .remove::<NeedsRemesh>()
            .insert(SectorMeshTask {
                coord: *coord,
                task,
            });
    }
}

/// Tamamlanan task'leri GigaBuffer'a uygula.
pub fn apply_sector_mesh_tasks(
    mut commands: Commands,
    mut query: Query<(Entity, &SectorCoord, &mut SectorMeshTask)>,
    mut gigabuffer: ResMut<GigaBuffer>,
    queue: Res<RenderQueue>,
    mut cache: ResMut<SectorMeshCache>, // coord → handle, free on replace
) {
    for (entity, coord, mut mesh_task) in query.iter_mut() {
        let Some(mesh_data) = block_on(poll_once(&mut mesh_task.task)) else {
            continue;
        };

        if let Some(old) = cache.remove(coord) {
            gigabuffer.free_sector(old);
        }
        let Some(handle) = gigabuffer.upload_sector(&queue, &mesh_data) else {
            warn!("GigaBuffer OOM for sector {:?}", coord);
            commands.entity(entity).remove::<SectorMeshTask>();
            continue;
        }

        cache.insert(*coord, handle);
        commands.entity(entity)
            .remove::<SectorMeshTask>()
            .insert(SectorMeshGpu {
                handle,
                element_count: mesh_data.element_count,
                aabb: mesh_data.aabb,
            });
    }
}
```

### 10.3 Sistem Sırası (`03` ile hizalı)

```
BlockEdit / NetworkApply
  → mark_needs_remesh
  → spawn_sector_mesh_tasks   (AsyncCompute)
  → apply_sector_mesh_tasks   (Main, RenderSet::Prepare)
  → extract_meshes_for_gpu    (Bevy render world)
```

> **Debouncing:** Oyuncu sürekli kazarken her voxel için remesh yerine `NeedsRemesh`
> biriktirilir; `apply` frame başına N sektör bütçesi ile sınırlanır (`08` IO bütçesi ile uyumlu).

---

## 11. Ambient Occlusion

### 11.1 AO Hesabı (0fps.net / Exile Engine referans)

**AO değer semantiği: 0 = en karanlık (tam occluded), 3 = en aydınlık (açık).**
Bu, 0fps.net ve Exile Engine ile uyumludur. Fragment shader'da `AO_CURVE[ao]` ile çarpılır.

Her quad için 4 AO değeri `PackedQuad.ao_packed`'e packed edilir (4 × 2-bit = 8-bit).
Fragment shader UV koordinatına göre bi-linear interpolate eder.

**Referanslar:**
- 0fps.net "Ambient occlusion for Minecraft-like worlds" (2013): `vertexAO(side1, side2, corner)`
- Exile Engine: AO curve `[0.75, 0.825, 0.9, 1.0]`, bi-linear interpolation
- `block-mesh-bgm` crate: `binary_greedy_quads_ao_safe()` — exterior occupancy mask ile AO-safe merge
- Andre Blunt "Vertex Ambient Occlusion for Voxel Games": quad flipping (diagonal comparison)

```rust
/// AO değeri — 0 (tam occluded) → 3 (tam açık).
/// vertexAO(side1, side2, corner):
///   side1 && side2 → 0 (en karanlık, iki komşu da dolu)
///   else → 3 - (side1 + side2 + corner)
///
/// Referans: 0fps.net, Exile Engine.
fn compute_vertex_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0u8 // Her iki komşu dolu → tam occluded
    } else {
        3u8 - (side1 as u8) - (side2 as u8) - (corner as u8)
    }
}

/// AO packed — quad'ın 4 köşesi için (4 × 2-bit = 8-bit).
fn compute_ao_packed(pos: IVec3, face: BlockFace, sector: &Sector) -> u8 {
    let mut ao_packed: u8 = 0;

    for corner in 0..4u8 {
        let (side1, side2, corner_block) = get_ao_neighbors(pos, face, corner, sector);
        let ao = compute_vertex_ao(side1, side2, corner_block);
        ao_packed |= ao << (corner * 2);
    }

    ao_packed
}
```

**AO-safe greedy merge:** greedy merge sırasında farklı AO değerine sahip hücrelerin
birleştirilmesi shading artifact üretir. `block-mesh-bgm` crate'in `ao_safe` modu,
her hücre için AO signature hesaplamak yerine **exterior occupancy mask**'ten
türetilmiş binary constraint mask'ları kullanır:

```rust
/// AO-safe merge constraint mask'ları — exterior plane occupancy'den türetilir.
/// `block-mesh-bgm` crate `ao::build_slice_ao_masks()` referans.
///
/// Her visible opaque cell şu kategorilerden birine girer:
/// - unit:  hiçbir yönde merge edilemez (1×1 quad).
/// - horizontal: sadece kendi row'u içinde merge (width > 1, height = 1).
/// - vertical: sadece row'lar arası merge (width = 1, height > 1).
/// - bidir: her iki yönde merge serbest (full greedy).
fn classify_ao_cell(
    opaque_visible: u64,
    prev_row: u64,
    current_row: u64,
    next_row: u64,
) -> (u64, u64, u64) {
    // ... block-mesh-bgm ao.rs §classify_ao_opaque_row logic ...
    // Pratikte: exterior plane'de komşu occupancy'e bakarak hangi yönlerde
    // AO değerinin değişmeyeceği kanıtlanır.
}
```

**Quad flipping (0fps.net, Andre Blunt):** Greedy quad'ın iki üçgene bölünme
yönü AO artifact'larını etkiler. Doğru kural: çapraz AO toplamı karşılaştırması.

```rust
/// Quad orientation — AO artifact'larını minimize etmek için
/// 0fps.net diagonal comparison kuralı:
///   quad'ı, karanlık köşeleri birleştirecek şekilde iki üçgene böl.
fn quad_orientation(ao: [u8; 4]) -> bool {
    // ao[0]=top-left, ao[1]=top-right, ao[2]=bottom-left, ao[3]=bottom-right
    // false = normal quad (üçgenler: [0,1,2] ve [1,2,3])
    // true  = flipped quad (üçgenler: [0,1,3] ve [0,3,2])
    (ao[0] as u32 + ao[3] as u32) > (ao[1] as u32 + ao[2] as u32)
}
```

```wgsl
// Fragment shader — bi-linear AO interpolation + AO curve lookup
fn sample_ao(uv: vec2<f32>, ao_packed: u32) -> f32 {
    let ao0 = (ao_packed      ) & 3u;  // 0-3
    let ao1 = (ao_packed >> 2u) & 3u;
    let ao2 = (ao_packed >> 4u) & 3u;
    let ao3 = (ao_packed >> 6u) & 3u;

    // Bi-linear interpolation (UV bazlı)
    let top = mix(f32(ao0), f32(ao1), uv.x);
    let bot = mix(f32(ao2), f32(ao3), uv.x);
    let ao_index = u32(mix(top, bot, uv.y));

    // AO curve lookup — AO_CURVE uniform buffer'dan okunur
    return ao_curve[ao_index];
}
```

### 11.2 AO Curve (Tunable — Sanatçı Tarafından Ayarlanabilir)

```rust
/// AO eğrisi — sanatçılar tarafından ayarlanabilir uniform buffer.
/// Exile Engine default değerleri: [0.75, 0.825, 0.9, 1.0]
/// 0 = en karanlık → index 0 = 0.75 (occluded)
/// 3 = en aydınlık → index 3 = 1.0 (open)
pub const AO_CURVE_DEFAULT: [f32; 4] = [0.75, 0.825, 0.9, 1.0];

/// WGSL uniform'da AO curve:
/// ```wgsl
/// struct AoUniform {
///     curve: vec4<f32>,  // default: (0.75, 0.825, 0.9, 1.0)
/// };
/// @group(1) @binding(0) var<uniform> ao: AoUniform;
/// ```
///
/// Performans notu: AO curve lookup texture sampler'dan ~2-4x daha hızlıdır
/// (4×f32 = 16B uniform, L1 cache hit).`select` ile de branchless yapılabilir.
```

---

## 11b. Transparent ve Cutout Meshing

`MeshType::Opaque` bloklar `BinaryGreedyMesher` ile greedy merge edilir.
`MeshType::Transparent` ve `MeshType::Cutout` bloklar **greedy merge edilemez**
(şeffaflık sınırları belirsiz quad'lar oluşturur), bu yüzden `CompactVertex` ile
per-vertex meshing yapılır.

### Transparent Meshing Stratejisi

- **Greedy merge YOK:** Her visible face için 4 × CompactVertex (16B/vertex, 64B/quad).
- **Back-face rendering:** Transparent bloklar her iki yönden de görünür olmalı
  (içeriden bakıldığında da görülebilir). Bu nedenle `CullMode::None` kullanılır.
- **Sıralama:** Back-to-front (painter's algorithm) — `10-render-pipeline.md` (taslak)
  transparent pass'te yapılır.
- **Depth write:** `OFF` (depth test açık ama yazma kapalı — blending artifact önler).

### Cutout Meshing Stratejisi

- **Greedy merge YOK:** Benzer şekilde per-vertex.
- **Alpha test:** Fragment shader'da `alpha < threshold` → discard.
- **Sıralama GEREKSİZ:** Alpha test deterministik (ya opaque ya discard).
- **Depth write:** `ON` (opaque gibi davranır, sadece alpha-tested kısımlar discard).

```rust
/// Transparent/Cutout mesher — greedy merge yok, CompactVertex.
///
/// **Dual-mask face culling** (EngineersBox / `binary-greedy-meshing` trans_mask):
/// - `solid_cols`: tüm non-air voxels
/// - `opaque_cols`: yalnızca opaque (transparent hariç)
/// - visible = `(solid & !solid_shift) | (opaque & !opaque_shift)` → solid→trans yüzleri korunur
///
/// Referans: https://engineersbox.github.io/website/2024/09/19/transparency-with-binary-greedy-meshing.html
pub struct NonGreedyMesher {
    mesh_type: MeshType,
    visibility: VisibilityTable,
    transparent_ids: Arc<[u16]>, // registry'den init'te toplanır
}

impl NonGreedyMesher {
    fn build_transparent_face_masks(
        &self,
        sector: &Sector,
        registry: &BlockRegistry,
    ) -> [FaceMaskBinary; 6] {
        let mut solid = [[[0u64; 32]; 32]; 6];
        let mut opaque = [[[0u64; 32]; 32]; 6];

        // Kolon maskeleri (binary-greedy-meshing `compute_opaque_mask` / `trans_mask`)
        for z in 0..32 {
            for x in 0..32 {
                for y in 0..32 {
                    let idx = sector.palette_index(IVec3::new(x, y, z));
                    if idx == 0 { continue; }
                    let axis_masks = pack_column_bits(x, y, z);
                    for (face, bit) in axis_masks {
                        solid[face][/*row*/][/*col*/] |= 1u64 << bit;
                        if !registry.is_transparent(idx) {
                            opaque[face][/*row*/][/*col*/] |= 1u64 << bit;
                        }
                    }
                }
            }
        }

        let mut out = [FaceMaskBinary::zero(BlockFace::Up); 6];
        for face in 0..6 {
            for z in 0..32 {
                for x in 0..32 {
                    let s = solid[face][z][x];
                    let o = opaque[face][z][x];
                    // Solid boundary OR opaque boundary (EngineersBox NE | NETC)
                    let vis_pos = (s & !(s << 1)) | (o & !(o << 1));
                    let vis_neg = (s & !(s >> 1)) | (o & !(o >> 1));
                    out[face].set_row(z, x, vis_pos); // axis'e göre pos/neg ayrı face index
                }
            }
        }
        out
    }
}

impl Mesher for NonGreedyMesher {
    fn mesh_sector(&self, sector: &Sector, registry: &BlockRegistry) -> MeshData {
        let masks = self.build_transparent_face_masks(sector, registry);
        let mut vertices = Vec::with_capacity(8192);

        for face_idx in 0..6 {
            let mask = &masks[face_idx];
            let double_sided = self.mesh_type == MeshType::Transparent;

            for layer in 0..32 {
                for row in 0..32 {
                    let mut visible = mask.get_row(layer, row);
                    while visible != 0 {
                        let bit = visible.trailing_zeros() as usize;
                        let pos = mask.face.voxel_at(layer, row, bit);

                        if self.visibility.is_face_visible(
                            sector, registry, mask.face, pos,
                        ) {
                            vertices.extend(CompactVertex::quad_from_voxel(
                                mask.face, layer, row, bit, sector, registry,
                            ));
                            if double_sided {
                                vertices.extend(CompactVertex::quad_from_voxel_flipped(
                                    mask.face, layer, row, bit, sector, registry,
                                ));
                            }
                        }
                        visible &= visible - 1;
                    }
                }
            }
        }

        MeshData::from_compact_vertices(vertices, VertexFormat::Compact)
    }

    fn mesh_type(&self) -> MeshType { self.mesh_type }
    fn vertex_format(&self) -> VertexFormat { VertexFormat::Compact }
}
```

> **Render ayrımı:** Transparent mesh'ler **ayrı GigaBuffer layer** veya ayrı draw batch
> (`ChunkDrawInfo.draw_type = Transparent`). Sıralama `10-render-pipeline` transparent pass'te
> kamera mesafesine göre (`Ascendant`: 3 layer — opaque / transparent / clutter).

| MeshType | Mesher | Vertex format | Greedy merge | Sıralama | Depth write |
|---|---|---|---|---|---|
| Opaque | `BinaryGreedyMesher` | PackedQuad (8B/quad) | Evet | Gerekmez | ON |
| Transparent | `NonGreedyMesher` | CompactVertex (16B) | Hayır | Back-to-front | OFF |
| Cutout | `NonGreedyMesher` | CompactVertex (16B) | Hayır | Gerekmez | ON |

---

## 12. Performans Hedefleri

| Metrik | Hedef | Not |
|---|---|---|
| Binary greedy mesh (CPU, sector) | **<80µs** | 32×32×32, %50 doluluk |
| Binary greedy + Visibility LUT | **<65µs** | LUT ile ~%20 face culling tasarrufu (`05` §18) |
| Culled mesh (CPU, sector) | **<35µs** | Hidden face culling only (fallback) |
| GPU mesh (branchless, 3-pass) | **<50µs** | Compute dispatch, no readback |
| Vertex azalması (greedy) | %40-60 | Naive mesh'e kıyasla |
| VRAM per sector (packed quad) | **<6.4KB** | 800 quad × 8B |
| VRAM per sector (compact) | <64KB | 4000 vertex × 16B |
| Incremental rebuild (tek sektör) | **<80µs** | Mesh + gigabuffer upload |
| Indirect draw cull (GPU, 100K chunks) | **<2ms** | Compute shader |
| Mesh cache hit rate | >80% | Dirty olmayan sektör'lar |
| WARM tier re-mesh | **0µs** | ACTIVE'ten cache'lenmiş BinaryGreedy (§8.1) |
| Gigabuffer utilization | >85% | Sub-allocation efficiency |

### Eski vs Yeni Performans Karşılaştırması

| Metrik | Eski (§3 naive) | Yeni (binary + packed + LUT) | İyileşme |
|---|---|---|---|
| CPU mesh süresi | ~300-500µs | **55-65µs** | **~5-8x** |
| Vertex format | 32B | **8B/quad** | **4x** |
| VRAM/sector | ~96KB | **~6.4KB** | **~15x** |
| GPU draw calls | Per-sector | **1 (indirect)** | **∞** |
| WARM tier re-mesh | N/A | **0µs** (cache) | **∞** |

---

## 13. Crate Organizasyonu

```
crates/
  meshing/
    ├── mod.rs                ← Meshing plugin entry point
    ├── trait_def.rs          ← Mesher trait, MeshData, VertexFormat
    ├── vertex/
    │   ├── mod.rs            ← Vertex types export
    │   ├── compact.rs        ← CompactVertex (16B)
    │   ├── packed_quad.rs    ← PackedQuad (8B) + vertex pulling
    │   └── legacy.rs         ← LegacyVertex (32B, debug only)
    ├── binary_greedy/
    │   ├── mod.rs            ← BinaryGreedyMesher (CPU, primary)
    │   ├── occupancy.rs      ← Occupancy mask (bitwise)
    │   ├── face_mask.rs      ← FaceMaskBinary (bitwise + LUT culling)
    │   ├── merge.rs          ← Greedy merge (bitwise)
    │   └── ao.rs             ← Ambient Occlusion (packed)
    ├── non_greedy/
    │   └── mod.rs            ← NonGreedyMesher (Transparent/Cutout)
    ├── culled/
    │   └── mod.rs            ← CulledMesher (lightweight, debug/fallback only)
    ├── gpu/
    │   ├── mod.rs            ← GpuMesher (branchless, 3-pass)
    │   ├── face_mask.wgsl    ← Pass 1: face visibility (branchless)
    │   ├── prefix_sum.wgsl   ← Pass 2: prefix-sum offsets
    │   ├── vertex_gen.wgsl   ← Pass 3: quad generation
    │   └── pipeline.rs       ← Pipeline setup + bind groups
    ├── indirect/
    │   ├── mod.rs            ← Indirect draw manager
    │   ├── gigabuffer.rs     ← GigaBuffer (`offset-allocator` crate)
    │   ├── cull.wgsl         ← GPU cull shader
    │   └── types.rs          ← ChunkDrawIndirect, ChunkDrawInfo
    ├── ecs/
    │   ├── components.rs     ← NeedsRemesh, SectorMeshTask, SectorMeshGpu
    │   ├── mark_remesh.rs    ← mark_needs_remesh
    │   ├── spawn_tasks.rs    ← spawn_sector_mesh_tasks
    │   └── apply_tasks.rs    ← apply_sector_mesh_tasks
    ├── scratch.rs            ← OccupancyScratch (thread_local factory)
    ├── strategy.rs           ← MeshingStrategy (tier-based)
    ├── registry.rs           ← MesherRegistry
    └── types.rs              ← MeshType, BlockFace, Aabb

# Cargo.toml (meshing crate excerpt)
# binary-greedy-meshing = { version = "0.5", optional }  # referans / test parity
# offset-allocator = "0.2"                               # GigaBuffer sub-alloc
# block-mesh-bgm = { version = "0.1", optional }         # AO-safe merge port (Faz 1b)
```

---

## 14. Alternatif Algoritmalar ve Tradeoff'lar

### 14.1 Dual Contouring

**Nedir:** Voxel grid'den smooth mesh çıkarma (isosurface extraction).

| | Avantaj | Dezavantaj |
|---|---|---|
| Görünüm | Organik, smooth terrain | Kübik estetik bozulur |
| LOD | Octree ile doğal LOD | Sharp edges kaybolur |
| Performans | — | Greedy'den ~2-3x yavaş |
| Karmaşıklık | — | Hermite data + QEF solver |

**Karar:** Strata'nın kübik voxel estetiği (`06-xbrickmap.md`) ile çelişir. **KULLANILMAZ.**

### 14.2 Marching Cubes

**Nedir:** 256 lookup table ile isosurface triangulation.

| | Avantaj | Dezavantaj |
|---|---|---|
| Basitlik | İyi dokümante, kolay implement | Sadece smooth terrain |
| Ambiguity | — | Topology hataları (dual contouring çözer) |

**Karar:** Voxel blok oyunu için uygun değil. **KULLANILMAZ.**

### 14.3 Mesh Shaders (Task + Mesh)

**Nedir:** Next-gen GPU geometry pipeline (Vulkan/DX12 Ultimate).

| | Avantaj | Dezavantaj |
|---|---|---|
| Culling | Fine-grained meshlet culling | WebGPU/WGSL'de YOK |
| Performance | Nanite-benzeri | Vulkan-only, complexity yüksek |
| Amplification | Task shader ile dinamik | Debugging çok zor |

**Karar:** **Faz 3** olarak planlanır. WebGPU mesh shader desteği geldiğinde veya
Vulkan backend (`10-render-pipeline.md`) hazır olduğunda eklenir.

### 14.4 Raycasting (Far-Field SVDAG)

**Nedir:** Mesh YOK — her pixel için voxel raycast (`07-svdag.md`).

| | Avantaj | Dezavantaj |
|---|---|---|
| Memory | 0 mesh VRAM | GPU compute heavy |
| Quality | Pixel-perfect | Overdraw maliyeti yüksek |
| LOD | SVDAG doğal LOD | Yakın mesafede pahalı |

**Karar:** DISTANT+ tier'lar için **KULLANILIR** (zaten `07-svdag.md` kapsamında).

---

## 15. Optimizasyon Kararları (Araştırma Özeti)

Önceki incelemede tespit edilen sorunlar ve uygulanan çözümler:

| Sorun | Eski tasarım | Optimize çözüm | Kaynak |
|-------|--------------|----------------|--------|
| Heap alloc / mesh | `Vec` her `mesh_sector` | `OccupancyScratch` + `clear()` | `binary-greedy-meshing` `MeshData::clear` |
| ECS bypass | `world.get_sector()` | `Query` + `NeedsRemesh` + async task | Bevy `Changed<T>`, `AsyncComputeTaskPool` |
| Light packing | `PackedQuad.light` boş | `SectorLightMap` 4+4 bit pack | `06` §LightMap, Exile 8B face |
| AO yorum / shader | Tutarsız semantik | 0=occluded, 3=open + `AO_CURVE` uniform | 0fps.net, Exile |
| AO + greedy | Merge artifact riski | Opsiyonel `block-mesh-bgm` ao_safe | `ao.rs` exterior masks |
| NonGreedy placeholder | `// ...` | Dual-mask NE\|NETC culling | EngineersBox BGM transparency |
| GigaBuffer allocator | Belirsiz `OffsetAllocator` | `offset-allocator` crate (TLSF) | sebbbi, Bevy #13218, Ascendant |
| FaceMask / merge run | Tanımsız | `FaceMaskBinary`, `find_same_type_run` | cgerikj BGM v2, `block-mesh-bgm` |
| CulledMesher LUT | Yok | `VisibilityTable` paylaşımlı pipeline | `05` §18 |

---

## 16. Faz Planı

| Faz | İçerik | Bağımlılık |
|---|---|---|
| **Faz 1** | Binary Greedy + scratch buffer + Visibility LUT + PackedQuad (light/AO) + Vertex Pulling + NonGreedy dual-mask + ECS incremental | `03`, `05`, `06` |
| **Faz 1b** | CachedGreedy (WARM) + optional `binary_greedy_quads_ao_safe` port | `08`, `block-mesh-bgm` |
| **Faz 2** | Branchless GPU meshing + GigaBuffer (`offset-allocator`) + Indirect Draw + Hi-Z | `07` §1.7, `10` |
| **Faz 3** | Mesh Shaders (Vulkan backend) | `10`, Vulkan |
