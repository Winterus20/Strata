# 23 — Meshing Sistemi

## 1. Genel Bakış

Strata'nın meshing sistemi **trait-based** ve **algorithm-agnostic**'tır. Render crate hangi mesher'in çalıştığını bilmez. Sadece `Mesher` trait'ini uygular ve `MeshData` alır.

### Temel Prensipler

- **Trait-based:** `Mesher` trait — algoritma değiştirilebilir
- **Algorithm-agnostic:** Render crate mesher tipini bilmez
- **GPU-ready:** CPU ve GPU mesher'lar aynı trait'i uygular
- **Incremental:** Sadece dirty bölgeler re-mesh edilir

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

/// Mesh verisi — GPU'ya upload-ready.
pub struct MeshData {
    /// Vertex buffer verisi.
    pub vertices: Vec<Vertex>,

    /// Index buffer verisi.
    pub indices: Vec<u32>,

    /// AABB (frustum culling için).
    pub aabb: Aabb,

    /// Vertex sayısı.
    pub vertex_count: u32,

    /// Index sayısı.
    pub index_count: u32,
}

/// Vertex — GPU layout.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vertex {
    /// Pozisyon.
    pub position: [f32; 3],

    /// Normal (axis-aligned, 0-5).
    pub normal: u8,

    /// UV koordinatı (doku).
    pub uv: [f32; 2],

    /// Vertex color (lighting).
    pub color: [u8; 4],

    /// Texture index (texture array).
    pub tex_index: u8,

    /// AO değeri.
    pub ao: u8,

    /// Padding.
    pub _padding: [u8; 2],
}

/// Face mesh data (incremental update).
pub struct FaceMeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// AABB.
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}
```

---

## 3. Greedy Meshing (CPU)

Greedy meshing, bitişik aynı tipteki yüzleri birleştirerek vertex sayısını **%40-60** azaltır.

```rust
/// Greedy meshing implementasyonu.
pub struct GreedyMesher {
    mesh_type: MeshType,
}

impl GreedyMesher {
    pub fn new(mesh_type: MeshType) -> Self {
        Self { mesh_type }
    }
}

impl Mesher for GreedyMesher {
    fn mesh_sector(&self, sector: &Sector, registry: &BlockRegistry) -> MeshData {
        let mut mesh_data = MeshData::empty();

        // 6 yüz için greedy mesh
        for face in BlockFace::ALL {
            self.greedy_mesh_face(sector, face, registry, &mut mesh_data);
        }

        mesh_data
    }

    fn mesh_face(
        &self,
        sector: &Sector,
        face: BlockFace,
        pos: IVec3,
        registry: &BlockRegistry,
    ) -> Option<FaceMeshData> {
        // Tek yüz mesh'le (incremental)
        // ...
        None
    }

    fn mesh_type(&self) -> MeshType {
        self.mesh_type
    }
}

impl GreedyMesher {
    /// Bir yüz için greedy mesh uygula.
    fn greedy_mesh_face(
        &self,
        sector: &Sector,
        face: BlockFace,
        registry: &BlockRegistry,
        mesh_data: &mut MeshData,
    ) {
        // 1. Yüz maskesi oluştur (hangi pozisyonlarda yüz var?)
        let mask = self.build_face_mask(sector, face, registry);

        // 2. Greedy merge — bitişik aynı tip yüzleri birleştir
        let merged_quads = self.greedy_merge(&mask, face);

        // 3. Quad'ları vertex/index'e dönüştür
        for quad in merged_quads {
            self.quad_to_vertices(quad, face, sector, registry, mesh_data);
        }
    }

    /// Yüz maskesi oluştur.
    fn build_face_mask(
        &self,
        sector: &Sector,
        face: BlockFace,
        registry: &BlockRegistry,
    ) -> FaceMask {
        let mut mask = FaceMask::new(face);

        // Yüz yönüne göre 2D grid oluştur
        for u in 0..32 {
            for v in 0..128 {
                let pos = self.uv_to_world(u, v, face);
                let neighbor = pos + face.direction();

                let block_id = sector.get_block(pos);
                let neighbor_id = sector.get_block(neighbor);

                if let Some(block_id) = block_id {
                    let block_def = registry.get(block_id);

                    // Yüz görünür mü?
                    let visible = match (block_def.appearance.transparency, neighbor_id) {
                        (TransparencyType::Opaque, None) => true,
                        (TransparencyType::Opaque, Some(n_id)) => {
                            let n_def = registry.get(n_id);
                            n_def.appearance.transparency != TransparencyType::Opaque
                        }
                        (TransparencyType::Transparent, _) => true,
                        (TransparencyType::Translucent, _) => true,
                    };

                    if visible {
                        mask.set(u, v, block_id);
                    }
                }
            }
        }

        mask
    }

    /// Greedy merge — en büyük dikdörtgenleri bul.
    fn greedy_merge(&self, mask: &FaceMask, face: BlockFace) -> Vec<MergedQuad> {
        let mut quads = Vec::new();
        let mut visited = VisitedMask::new(mask.size());

        for u in 0..mask.width {
            for v in 0..mask.height {
                if visited.is_set(u, v) || mask.get(u, v).is_none() {
                    continue;
                }

                let block_id = mask.get(u, v).unwrap();

                // Genişliği bul (sağa doğru)
                let mut width = 1;
                while u + width < mask.width
                    && !visited.is_set(u + width, v)
                    && mask.get(u + width, v) == Some(block_id)
                {
                    width += 1;
                }

                // Yüksekliği bul (aşağı doğru, tüm genişlik için)
                let mut height = 1;
                'outer: while v + height < mask.height {
                    for w in 0..width {
                        if visited.is_set(u + w, v + height)
                            || mask.get(u + w, v + height) != Some(block_id)
                        {
                            break 'outer;
                        }
                    }
                    height += 1;
                }

                // Quad oluştur
                quads.push(MergedQuad {
                    u,
                    v,
                    width,
                    height,
                    block_id,
                });

                // Visited olarak işaretle
                for du in 0..width {
                    for dv in 0..height {
                        visited.set(u + du, v + dv);
                    }
                }
            }
        }

        quads
    }

    /// Merged quad'ı vertex'lere dönüştür.
    fn quad_to_vertices(
        &self,
        quad: MergedQuad,
        face: BlockFace,
        sector: &Sector,
        registry: &BlockRegistry,
        mesh_data: &mut MeshData,
    ) {
        let start_idx = mesh_data.vertices.len() as u32;

        // 4 köşe pozisyonu
        let corners = self.compute_quad_corners(quad, face);

        // Her köşe için vertex oluştur
        for (i, corner) in corners.iter().enumerate() {
            let vertex = Vertex {
                position: corner.position,
                normal: face.normal_index(),
                uv: corner.uv,
                color: self.compute_vertex_color(corner.pos, sector, registry),
                tex_index: registry.get(quad.block_id).appearance.textures[face.index()] as u8,
                ao: self.compute_ao(corner.pos, face, sector),
                _padding: [0; 2],
            };
            mesh_data.vertices.push(vertex);
        }

        // 2 üçgen (6 index)
        let indices = [
            start_idx, start_idx + 1, start_idx + 2,
            start_idx + 2, start_idx + 1, start_idx + 3,
        ];
        mesh_data.indices.extend_from_slice(&indices);
    }

    /// Ambient Occlusion hesapla.
    fn compute_ao(&self, pos: IVec3, face: BlockFace, sector: &Sector) -> u8 {
        // AO: 4 komşu blok kontrolü
        // 0 = tam aydınlık, 3 = tam karanlık
        let ao_sides = self.get_ao_neighbors(pos, face, sector);

        // AO formülü: (side1 + side2) - corner
        let ao = ao_sides.0 + ao_sides.1 - ao_sides.2;
        ao.min(3) as u8
    }

    /// Vertex color (lighting).
    fn compute_vertex_color(
        &self,
        pos: IVec3,
        sector: &Sector,
        registry: &BlockRegistry,
    ) -> [u8; 4] {
        // 4 komşu light ortalaması (smooth lighting)
        let samples = self.get_light_samples(pos, sector);

        let avg_sky = samples.iter().map(|s| s.sky()).sum::<u8>() / 4;
        let avg_r = samples.iter().map(|s| s.block_r()).sum::<u8>() / 4;
        let avg_g = samples.iter().map(|s| s.block_g()).sum::<u8>() / 4;
        let avg_b = samples.iter().map(|s| s.block_b()).sum::<u8>() / 4;

        [avg_r, avg_g, avg_b, avg_sky]
    }
}
```

---

## 4. GPU Compute Meshing (Faz 2)

```rust
/// GPU compute meshing pipeline.
pub struct GpuMesher {
    /// Compute pipeline.
    compute_pipeline: wgpu::ComputePipeline,

    /// Input buffer (sector verisi).
    input_buffer: wgpu::Buffer,

    /// Output buffer (vertex/index).
    output_buffer: wgpu::Buffer,

    /// Indirect draw buffer.
    indirect_buffer: wgpu::Buffer,

    /// Mesh tipi.
    mesh_type: MeshType,
}

impl GpuMesher {
    /// GPU'da sector mesh'le.
    pub fn mesh_sector_gpu(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sector: &Sector,
    ) -> MeshData {
        // 1. Sector verisini GPU'ya upload
        queue.write_buffer(&self.input_buffer, 0, &sector.to_gpu_data());

        // 2. Compute dispatch
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.compute_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(32, 128, 1);
        }

        // 3. Output buffer'dan oku
        // ...
        MeshData::empty()
    }
}
```

### GPU Meshing Shader (WGSL)

```wgsl
// GPU greedy meshing compute shader
@group(0) @binding(0)
var<storage, read> sector_data: array<u16, 131072>;

@group(0) @binding(1)
var<storage, read_write> vertices: array<Vertex>;

@group(0) @binding(2)
var<storage, read_write> indices: array<u32>;

@group(0) @binding(3)
var<storage, read_write> vertex_count: atomic<u32>;

@compute @workgroup_size(8, 8)
fn greedy_mesh_face(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let u = id.x;
    let v = id.y;

    if (u >= 32u || v >= 128u) { return; }

    // Yüz kontrolü
    let pos = compute_world_pos(u, v, FACE_DIRECTION);
    let neighbor = pos + FACE_DIRECTION;

    let block_id = sector_load(pos);
    let neighbor_id = sector_load(neighbor);

    if (block_id == 0u) { return; } // AIR
    if (!is_face_visible(block_id, neighbor_id)) { return; }

    // Greedy: sadece sol-üst köşe olan thread mesh'ler
    let left_same = is_same_face(u - 1u, v, block_id);
    let up_same = is_same_face(u, v - 1u, block_id);

    if (left_same || up_same) { return; }

    // Genişlik ve yükseklik bul
    var width: u32 = 1u;
    while (u + width < 32u && is_same_face(u + width, v, block_id)) {
        width++;
    }

    var height: u32 = 1u;
    while (v + height < 128u) {
        var row_match = true;
        for (var w: u32 = 0u; w < width; w = w + 1u) {
            if (!is_same_face(u + w, v + height, block_id)) {
                row_match = false;
                break;
            }
        }
        if (!row_match) { break; }
        height++;
    }

    // Vertex oluştur (atomic counter ile)
    let idx = atomicAdd(&vertex_count, 4u);

    // 4 köşe
    vertices[idx + 0u] = create_vertex(u, v, width, height, 0u);
    vertices[idx + 1u] = create_vertex(u + width, v, width, height, 1u);
    vertices[idx + 2u] = create_vertex(u, v + height, width, height, 2u);
    vertices[idx + 3u] = create_vertex(u + width, v + height, width, height, 3u);

    // Indices
    let base_idx = idx * 3u / 2u; // Her 4 vertex = 6 index = 2 üçgen
    indices[base_idx + 0u] = idx + 0u;
    indices[base_idx + 1u] = idx + 1u;
    indices[base_idx + 2u] = idx + 2u;
    indices[base_idx + 3u] = idx + 2u;
    indices[base_idx + 4u] = idx + 1u;
    indices[base_idx + 5u] = idx + 3u;
}
```

---

## 5. Mesher Registry

```rust
/// Mesher registry — farklı mesher'ları yönetir.
pub struct MesherRegistry {
    /// Kayıtlı mesher'lar.
    meshers: HashMap<String, Box<dyn Mesher>>,

    /// Aktif mesher.
    active_mesher: String,
}

impl MesherRegistry {
    /// Mesher kaydet.
    pub fn register(&mut self, name: String, mesher: Box<dyn Mesher>) {
        self.meshers.insert(name, mesher);
    }

    /// Aktif mesher'i değiştir.
    pub fn set_active(&mut self, name: &str) -> Result<(), MesherError> {
        if self.meshers.contains_key(name) {
            self.active_mesher = name.to_string();
            Ok(())
        } else {
            Err(MesherError::UnknownMesher(name.to_string()))
        }
    }

    /// Aktif mesher'ı al.
    pub fn active(&self) -> &dyn Mesher {
        self.meshers.get(&self.active_mesher).unwrap().as_ref()
    }
}

#[derive(Debug)]
pub enum MesherError {
    UnknownMesher(String),
}
```

---

## 6. Incremental Meshing

```rust
/// Incremental meshing — sadece dirty bölgeler re-mesh edilir.
pub struct IncrementalMesher {
    base_mesher: Box<dyn Mesher>,

    /// Dirty yüz takibi.
    dirty_faces: HashSet<(IVec3, BlockFace)>,

    /// Mesh cache.
    mesh_cache: HashMap<SectorCoord, MeshData>,
}

impl IncrementalMesher {
    /// Blok değişikliğini kaydet.
    pub fn mark_dirty(&mut self, pos: IVec3, face: BlockFace) {
        self.dirty_faces.insert((pos, face));

        // Komşu yüzleri de dirty yap (bağlantı için)
        for neighbor_face in face.adjacent_faces() {
            let neighbor_pos = pos + face.direction();
            self.dirty_faces.insert((neighbor_pos, neighbor_face));
        }
    }

    /// Dirty yüzleri re-mesh'le.
    pub fn rebuild_dirty(
        &mut self,
        sector: &Sector,
        registry: &BlockRegistry,
    ) -> Vec<MeshUpdate> {
        let mut updates = Vec::new();

        // Dirty yüzleri grupla (aynı sector'dakileri birleştir)
        let by_sector = self.group_by_sector(&self.dirty_faces);

        for (coord, faces) in by_sector {
            // Sector'ü re-mesh'le
            let new_mesh = self.base_mesher.mesh_sector(sector, registry);

            updates.push(MeshUpdate {
                sector: coord,
                old_mesh: self.mesh_cache.remove(&coord),
                new_mesh,
            });
        }

        self.dirty_faces.clear();
        updates
    }
}
```

---

## 7. Performans Hedefleri

| Metrik | Hedef | Not |
|---|---|---|
| Greedy mesh (CPU, sector) | <500µs | 32×128×32, %50 doluluk |
| GPU mesh (compute, sector) | <50µs | Compute dispatch + readback |
| Vertex azalması (greedy) | %40-60 | Naive mesh'e kıyasla |
| Incremental rebuild (tek yüz) | <10µs | Sadece etkilenen alan |
| Mesh cache hit rate | >80% | Dirty olmayan sector'lar |

---

## 8. Crate Organizasyonu

```
crates/
  meshing/
    ├── mod.rs              ← Meshing plugin entry point
    ├── trait.rs            ← Mesher trait, MeshData, Vertex
    ├── greedy/
    │   ├── mod.rs          ← GreedyMesher (CPU)
    │   ├── mask.rs         ← FaceMask
    │   ├── merge.rs        ← Greedy merge algoritması
    │   ├── quad.rs         ← MergedQuad
    │   └── ao.rs           ← Ambient Occlusion
    ├── gpu/
    │   ├── mod.rs          ← GpuMesher
    │   ├── pipeline.rs     ← Compute pipeline
    │   └── shader.wgsl     ← GPU meshing shader
    ├── registry.rs         ← MesherRegistry
    ├── incremental.rs      ← IncrementalMesher
    └── types.rs            ← MeshType, BlockFace, Aabb
```
