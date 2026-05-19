# Phase 1 Implementation Plan — Strata (Faz 1: Temel Altyapı)

**Süre:** 4 Hafta (Hafta 1-4)
**Hedef:** Çalışan, oynanabilir prototip — prosedürel dünya, blok kırma/yerleştirme, temel fizik

---

## Hafta 1: Workspace + Core + ECS

### Gün 1-2: Workspace Kurulumu

#### 1.1. Root Cargo.toml
```
strata/
├── Cargo.toml          # [workspace] members
├── .gitignore
├── rust-toolchain.toml # stable-x86_64-pc-windows-msvc
└── rustfmt.toml
```

**Root `Cargo.toml`:**
```toml
[workspace]
resolver = "2"
members = [
    "crates/core",
    "crates/ecs",
    "crates/world-gen",
    "crates/meshing",
    "crates/storage",
    "crates/physics",
    "bin/client",
]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"

[workspace.dependencies]
# ECS
bevy_ecs = "0.18"
bevy_app = "0.18"
bevy_math = "0.18"
bevy_hierarchy = "0.18"
bevy_transform = "0.18"

# Math
glam = "0.29"

# Async
tokio = { version = "1", features = ["full"] }

# Serialization
rkyv = { version = "0.8", features = ["validation"] }
postcard = { version = "1.1", features = ["alloc"] }

# Noise
fastnoise2 = "0.4"

# Compression
zstd = "0.13"

# Physics
bevy_rapier3d = { version = "0.33", features = ["enhanced-determinism"] }

# Window / Render (Faz 1 minimal)
winit = "0.30"
wgpu = "29"

# Logging
tracing = "0.1"
tracing-subscriber = "0.3"

# Error handling
thiserror = "2"
anyhow = "1"

[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3

[profile.release]
lto = "thin"
codegen-units = 1
opt-level = 3
```

**`rust-toolchain.toml`:**
```toml
[toolchain]
channel = "stable-x86_64-pc-windows-msvc"
```

**`rustfmt.toml`:**
```toml
max_width = 100
hard_tabs = false
tab_spaces = 4
newline_style = "Crlf"
```

**`.gitignore`:**
```
/target
/Cargo.lock
*.pdb
```

#### 1.2. Dizin Yapısı
```bash
mkdir -p crates/{core,ecs,world-gen,meshing,storage,physics}/src
mkdir -p bin/client/src
mkdir -p assets/textures
```

---

### Gün 2-3: `core` Crate

**`crates/core/Cargo.toml`:**
```toml
[package]
name = "strata-core"
version.workspace = true
edition.workspace = true

[dependencies]
glam.workspace = true
rkyv.workspace = true
thiserror.workspace = true
tracing.workspace = true
bevy_ecs.workspace = true
bevy_math.workspace = true

[dev-dependencies]
criterion = "0.5"

[[bench]]
name = "chunk_bench"
harness = false
```

#### 1.2.1. `block.rs` — Block Registry

```rust
/// Unique block identifier (0 = Air)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct BlockId(pub u16);

impl BlockId {
    pub const AIR: Self = Self(0);
    pub const STONE: Self = Self(1);
    pub const DIRT: Self = Self(2);
    pub const GRASS: Self = Self(3);
    pub const BEDROCK: Self = Self(4);

    #[inline]
    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

/// Block properties (registry entry)
#[derive(Debug, Clone)]
pub struct BlockProperties {
    pub id: BlockId,
    pub name: &'static str,
    pub transparent: bool,
    pub solid: bool,
    pub hardness: f32,
    pub light_emission: u8, // 0-15
    pub texture_index: u16, // Texture2DArray index
}

/// Block registry — dense Vec lookup, O(1) access
pub struct BlockRegistry {
    blocks: Vec<BlockProperties>,
    name_map: hashbrown::HashMap<&'static str, BlockId>,
}

impl BlockRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            blocks: Vec::with_capacity(256),
            name_map: hashbrown::HashMap::new(),
        };
        // Register defaults
        registry.register(BlockProperties {
            id: BlockId::AIR,
            name: "air",
            transparent: true,
            solid: false,
            hardness: 0.0,
            light_emission: 0,
            texture_index: 0,
        });
        registry
    }

    pub fn register(&mut self, props: BlockProperties) -> BlockId {
        let id = props.id;
        self.name_map.insert(props.name, id);
        if id.0 as usize >= self.blocks.len() {
            self.blocks.resize(id.0 as usize + 1, self.blocks[0].clone());
        }
        self.blocks[id.0 as usize] = props;
        id
    }

    #[inline]
    pub fn get(&self, id: BlockId) -> &BlockProperties {
        &self.blocks[id.0 as usize]
    }

    #[inline]
    pub fn by_name(&self, name: &str) -> Option<BlockId> {
        self.name_map.get(name).copied()
    }
}
```

#### 1.2.2. `chunk.rs` — Chunk Data Structure

```rust
use crate::block::BlockId;
use glam::IVec2;

pub const CHUNK_WIDTH: usize = 16;
pub const CHUNK_HEIGHT: usize = 256;
pub const CHUNK_DEPTH: usize = 16;
pub const CHUNK_VOLUME: usize = CHUNK_WIDTH * CHUNK_HEIGHT * CHUNK_DEPTH; // 65,536

/// Chunk world position (X, Z)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos(pub IVec2);

impl ChunkPos {
    #[inline]
    pub fn from_world(x: i32, z: i32) -> Self {
        Self(IVec2::new(
            x.div_euclid(CHUNK_WIDTH as i32),
            z.div_euclid(CHUNK_DEPTH as i32),
        ))
    }

    #[inline]
    pub fn world_x(&self) -> i32 {
        self.0.x * CHUNK_WIDTH as i32
    }

    #[inline]
    pub fn world_z(&self) -> i32 {
        self.0.y * CHUNK_DEPTH as i32
    }
}

/// 16x256x16 voxel chunk — flat Vec<u16>, cache-optimal layout
pub struct Chunk {
    pub position: ChunkPos,
    pub blocks: Vec<u16>,              // index = x + z*16 + y*256
    pub heightmap_top: [u16; 256],     // column top
    pub heightmap_bottom: [u16; 256],  // column bottom
    pub dirty: bool,
}

impl Chunk {
    pub fn new(position: ChunkPos) -> Self {
        Self {
            position,
            blocks: vec![0u16; CHUNK_VOLUME], // all air
            heightmap_top: [0u16; 256],
            heightmap_bottom: [0u16; 256],
            dirty: false,
        }
    }

    /// Flat array index — inlined, zero-cost
    #[inline(always)]
    pub fn index(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_WIDTH && y < CHUNK_HEIGHT && z < CHUNK_DEPTH);
        x + z * CHUNK_WIDTH + y * CHUNK_WIDTH * CHUNK_DEPTH
    }

    /// Column index for heightmap
    #[inline(always)]
    pub fn column_index(x: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_WIDTH && z < CHUNK_DEPTH);
        x + z * CHUNK_WIDTH
    }

    #[inline]
    pub fn get_block(&self, x: usize, y: usize, z: usize) -> BlockId {
        BlockId(self.blocks[Self::index(x, y, z)])
    }

    #[inline]
    pub fn set_block(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        let idx = Self::index(x, y, z);
        self.blocks[idx] = id.0;
        self.update_heightmap(x, z, y);
        self.dirty = true;
    }

    /// Update heightmap for a single column
    fn update_heightmap(&mut self, x: usize, z: usize, modified_y: usize) {
        let col = Self::column_index(x, z);

        // Update top
        if modified_y >= self.heightmap_top[col] as usize {
            let mut top = CHUNK_HEIGHT as u16;
            for y in (0..CHUNK_HEIGHT).rev() {
                if !BlockId(self.blocks[Self::index(x, y, z)]).is_air() {
                    top = y as u16;
                    break;
                }
            }
            self.heightmap_top[col] = top;
        }

        // Update bottom
        if modified_y <= self.heightmap_bottom[col] as usize || self.heightmap_bottom[col] == 0 {
            let mut bottom = 0u16;
            for y in 0..CHUNK_HEIGHT {
                if !BlockId(self.blocks[Self::index(x, y, z)]).is_air() {
                    bottom = y as u16;
                    break;
                }
            }
            self.heightmap_bottom[col] = bottom;
        }
    }

    /// Check if chunk is entirely air
    pub fn is_empty(&self) -> bool {
        self.blocks.iter().all(|&b| b == 0)
    }

    /// Raw slice for serialization (zero-copy friendly)
    #[inline]
    pub fn as_slice(&self) -> &[u16] {
        &self.blocks
    }

    /// Mutable raw slice
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u16] {
        &mut self.blocks
    }
}
```

#### 1.2.3. `world.rs` — World Coordinate System

```rust
use crate::chunk::{ChunkPos, CHUNK_WIDTH, CHUNK_DEPTH};
use glam::IVec3;

/// Global block position
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos(pub IVec3);

impl BlockPos {
    /// Convert world position to (chunk_pos, local_x, local_y, local_z)
    #[inline]
    pub fn to_chunk_local(self) -> (ChunkPos, usize, usize, usize) {
        let chunk_x = self.0.x.div_euclid(CHUNK_WIDTH as i32);
        let chunk_z = self.0.z.div_euclid(CHUNK_DEPTH as i32);
        let local_x = self.0.x.rem_euclid(CHUNK_WIDTH as i32) as usize;
        let local_y = self.0.y as usize;
        let local_z = self.0.z.rem_euclid(CHUNK_DEPTH as i32) as usize;
        (ChunkPos(glam::IVec2::new(chunk_x, chunk_z)), local_x, local_y, local_z)
    }
}
```

#### 1.2.4. `lib.rs`

```rust
pub mod block;
pub mod chunk;
pub mod world;

pub use block::{BlockId, BlockProperties, BlockRegistry};
pub use chunk::{Chunk, ChunkPos, CHUNK_HEIGHT, CHUNK_VOLUME, CHUNK_WIDTH};
pub use world::BlockPos;
```

---

### Gün 3-4: `ecs` Crate

**`crates/ecs/Cargo.toml`:**
```toml
[package]
name = "strata-ecs"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
bevy_ecs.workspace = true
bevy_app.workspace = true
bevy_math.workspace = true
bevy_transform.workspace = true
bevy_hierarchy.workspace = true
glam.workspace = true
```

#### 1.3.1. Components

```rust
// crates/ecs/src/components/mod.rs
pub mod position;
pub mod chunk;
pub mod player;
pub mod interaction;

pub use position::*;
pub use chunk::*;
pub use player::*;
pub use interaction::*;
```

```rust
// crates/ecs/src/components/position.rs
use bevy_ecs::prelude::*;
use bevy_math::Vec3;

#[derive(Component, Debug, Clone, Copy)]
pub struct Position(pub Vec3);

#[derive(Component, Debug, Clone, Copy)]
pub struct Velocity(pub Vec3);
```

```rust
// crates/ecs/src/components/chunk.rs
use bevy_ecs::prelude::*;
use bevy_math::IVec2;
use strata_core::ChunkPos;

#[derive(Component, Debug, Clone, Copy)]
pub struct ChunkPosition(pub ChunkPos);

#[derive(Component, Debug)]
pub struct ChunkDirty {
    pub needs_mesh: bool,
    pub needs_light: bool,
}
```

```rust
// crates/ecs/src/components/player.rs
use bevy_ecs::prelude::*;

#[derive(Component, Debug)]
pub struct Player {
    pub selected_slot: u8,
}
```

```rust
// crates/ecs/src/components/interaction.rs
use bevy_ecs::prelude::*;
use strata_core::BlockPos;

#[derive(Event, Debug)]
pub struct BlockBreakEvent(pub BlockPos);

#[derive(Event, Debug)]
pub struct BlockPlaceEvent {
    pub position: BlockPos,
    pub block_id: u16,
}
```

#### 1.3.2. Plugin

```rust
// crates/ecs/src/lib.rs
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;

pub mod components;
pub mod systems;

pub use components::*;

pub struct EcsPlugin;

impl Plugin for EcsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<systems::WorldState>();
    }
}
```

```rust
// crates/ecs/src/systems/mod.rs
pub mod world_state;

pub use world_state::*;
```

```rust
// crates/ecs/src/systems/world_state.rs
use bevy_ecs::prelude::*;

/// Tracks loaded chunks, player position, etc.
#[derive(Resource, Default)]
pub struct WorldState {
    pub loaded_chunks: hashbrown::HashSet<strata_core::ChunkPos>,
    pub render_distance: u32,
}
```

---

### Gün 4-5: Doğrulama + Test

```bash
cargo test -p strata-core
cargo test -p strata-ecs
cargo clippy --workspace -- -D warnings
cargo fmt
```

**Milestone 1:** `cargo build --workspace` başarılı, core + ecs derleniyor.

---

## Hafta 2: World-Gen + Meshing

### Gün 6-7: `world-gen` Crate

**`crates/world-gen/Cargo.toml`:**
```toml
[package]
name = "strata-world-gen"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
fastnoise2 = "0.4"
glam.workspace = true
rand = "0.8"
```

#### 2.1.1. `noise.rs`

```rust
use fastnoise2::prelude::*;

pub struct TerrainNoise {
    noise: FastNoise,
    seed: u32,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        let noise = FastNoise::builder()
            .set_noise_type(NoiseType::OpenSimplex2)
            .set_fractal_type(FractalType::FBm)
            .set_fractal_octaves(4)
            .set_fractal_lacunarity(2.0)
            .set_fractal_gain(0.5)
            .build();
        Self { noise, seed }
    }

    /// Returns height (0-255) for given world x, z
    pub fn height(&self, x: i32, z: i32) -> u16 {
        let scale = 0.005;
        let nx = x as f32 * scale;
        let nz = z as f32 * scale;
        let value = self.noise.get_noise2d(nx, nz);
        // Map [-1, 1] -> [20, 180]
        ((value * 0.5 + 0.5) * 160.0 + 20.0) as u16
    }

    /// Biome value for given position
    pub fn biome(&self, x: i32, z: i32) -> f32 {
        let scale = 0.002;
        (self.noise.get_noise2d(x as f32 * scale + 1000.0, z as f32 * scale + 1000.0) * 0.5 + 0.5)
    }
}
```

#### 2.1.2. `terrain.rs`

```rust
use strata_core::{BlockId, Chunk, ChunkPos, CHUNK_HEIGHT, CHUNK_WIDTH};
use crate::noise::TerrainNoise;

pub struct TerrainGenerator {
    noise: TerrainNoise,
}

impl TerrainGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            noise: TerrainNoise::new(seed),
        }
    }

    /// Generate terrain for a chunk — fills blocks bottom-to-top
    pub fn generate(&self, chunk: &mut Chunk) {
        let world_x = chunk.position.world_x();
        let world_z = chunk.position.world_z();

        for x in 0..CHUNK_WIDTH {
            for z in 0..CHUNK_WIDTH {
                let wx = world_x + x as i32;
                let wz = world_z + z as i32;
                let height = self.noise.height(wx, wz) as usize;

                for y in 0..CHUNK_HEIGHT {
                    let block = if y == 0 {
                        BlockId::BEDROCK
                    } else if y < height - 4 {
                        BlockId::STONE
                    } else if y < height {
                        BlockId::DIRT
                    } else if y == height {
                        BlockId::GRASS
                    } else {
                        BlockId::AIR
                    };
                    chunk.set_block(x, y, z, block);
                }
            }
        }
        chunk.dirty = false; // Fresh chunk, mesh will be built
    }
}
```

#### 2.1.3. `generator.rs` — Async Chunk Pipeline

```rust
use std::collections::VecDeque;
use strata_core::ChunkPos;
use crate::terrain::TerrainGenerator;

pub struct ChunkGenerator {
    generator: TerrainGenerator,
    queue: VecDeque<ChunkPos>,
    chunks_per_frame: u8,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            generator: TerrainGenerator::new(seed),
            queue: VecDeque::new(),
            chunks_per_frame: 2,
        }
    }

    pub fn request_chunk(&mut self, pos: ChunkPos) {
        if !self.queue.contains(&pos) {
            self.queue.push_back(pos);
        }
    }

    /// Process up to N chunks — call every frame
    pub fn process(&mut self) -> Vec<strata_core::Chunk> {
        let mut results = Vec::new();
        let limit = self.chunks_per_frame.min(self.queue.len() as u8);

        for _ in 0..limit {
            if let Some(pos) = self.queue.pop_front() {
                let mut chunk = strata_core::Chunk::new(pos);
                self.generator.generate(&mut chunk);
                results.push(chunk);
            }
        }

        results
    }
}
```

#### 2.1.4. `lib.rs`

```rust
pub mod noise;
pub mod terrain;
pub mod generator;

pub use generator::ChunkGenerator;
pub use terrain::TerrainGenerator;
```

---

### Gün 8-10: `meshing` Crate

**`crates/meshing/Cargo.toml`:**
```toml
[package]
name = "strata-meshing"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
glam.workspace = true
```

#### 2.2.1. `mesher.rs` — Trait + MeshData

```rust
use glam::Vec3;

/// Vertex format — matches WGSL shader input
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub ao: f32,
    pub texture_id: u16,
}

/// Bounding box for mesh
#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
    pub min: Vec3,
    pub max: Vec3,
}

/// Algorithm-agnostic mesh output
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub vertex_count: usize,
    pub index_count: usize,
    pub bounds: BoundingBox,
}

impl MeshData {
    pub fn empty() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            vertex_count: 0,
            index_count: 0,
            bounds: BoundingBox {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertex_count == 0
    }
}

/// Mesher trait — swap algorithms without changing render pipeline
pub trait Mesher: Send + Sync {
    fn generate_mesh(&self, chunk: &strata_core::Chunk) -> MeshData;
    fn name(&self) -> &str;
}
```

#### 2.2.2. `classic_greedy.rs` — Greedy Meshing (Faz 1)

```rust
use strata_core::{BlockId, Chunk, CHUNK_HEIGHT, CHUNK_WIDTH};
use crate::mesher::{BoundingBox, MeshData, Mesher, Vertex};
use glam::Vec3;

/// Face direction
#[derive(Clone, Copy)]
struct Face {
    axis: usize,       // 0=X, 1=Y, 2=Z
    direction: i32,    // -1 or +1
    x: usize,
    y: usize,
    z: usize,
    block_id: u16,
}

pub struct ClassicGreedyMesher;

impl ClassicGreedyMesher {
    /// Check if face at position should be generated (neighbor is air or transparent)
    #[inline]
    fn should_render_face(chunk: &Chunk, x: usize, y: usize, z: usize, axis: usize, dir: i32) -> bool {
        let (nx, ny, nz) = match (axis, dir) {
            (0, -1) => (x.wrapping_sub(1), y, z),
            (0, 1) => (x + 1, y, z),
            (1, -1) => (x, y.wrapping_sub(1), z),
            (1, 1) => (x, y + 1, z),
            (2, -1) => (x, y, z.wrapping_sub(1)),
            (2, 1) => (x, y, z + 1),
            _ => unreachable!(),
        };

        if nx >= CHUNK_WIDTH || ny >= CHUNK_HEIGHT || nz >= CHUNK_WIDTH {
            return true; // Out of chunk = render face
        }

        let neighbor = chunk.get_block(nx, ny, nz);
        neighbor.is_air()
    }

    /// Greedy merge on 2D mask — finds largest rectangles
    fn greedy_merge(
        mask: &[Vec<bool>],
        width: usize,
        height: usize,
        block_id: u16,
    ) -> Vec<(usize, usize, usize, usize)> {
        // (x, y, w, h) rectangles
        let mut rects = Vec::new();
        let mut visited = vec![vec![false; height]; width];

        for y in 0..height {
            for x in 0..width {
                if visited[x][y] || !mask[x][y] {
                    continue;
                }

                // Find width
                let mut w = 1;
                while x + w < width && mask[x + w][y] && !visited[x + w][y] {
                    w += 1;
                }

                // Find height
                let mut h = 1;
                let mut can_extend = true;
                while y + h < height && can_extend {
                    for dx in 0..w {
                        if !mask[x + dx][y + h] || visited[x + dx][y + h] {
                            can_extend = false;
                            break;
                        }
                    }
                    if can_extend {
                        h += 1;
                    }
                }

                // Mark visited
                for dy in 0..h {
                    for dx in 0..w {
                        visited[x + dx][y + dy] = true;
                    }
                }

                rects.push((x, y, w, h));
            }
        }

        rects
    }
}

impl Mesher for ClassicGreedyMesher {
    fn generate_mesh(&self, chunk: &Chunk) -> MeshData {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut vertex_offset = 0u32;

        let world_x = chunk.position.world_x() as f32;
        let world_z = chunk.position.world_z() as f32;

        // 6 face directions: ±X, ±Y, ±Z
        for axis in 0..3 {
            for dir in [-1, 1] {
                // Build 2D mask per block type
                let (dim_u, dim_v) = match axis {
                    0 => (CHUNK_HEIGHT, CHUNK_WIDTH), // YZ plane
                    1 => (CHUNK_WIDTH, CHUNK_WIDTH),   // XZ plane
                    2 => (CHUNK_WIDTH, CHUNK_HEIGHT),  // XY plane
                    _ => unreachable!(),
                };

                // Group by block_id for texture correctness
                let mut masks: hashbrown::HashMap<u16, Vec<Vec<bool>>> = hashbrown::HashMap::new();

                for u in 0..dim_u {
                    for v in 0..dim_v {
                        let (x, y, z) = match axis {
                            0 => (if dir == 1 { CHUNK_WIDTH - 1 } else { 0 }, u, v),
                            1 => (v, if dir == 1 { CHUNK_HEIGHT - 1 } else { 0 }, u),
                            2 => (u, v, if dir == 1 { CHUNK_WIDTH - 1 } else { 0 }),
                            _ => unreachable!(),
                        };

                        if chunk.get_block(x, y, z).is_air() {
                            continue;
                        }

                        if !Self::should_render_face(chunk, x, y, z, axis, dir) {
                            continue;
                        }

                        let block_id = chunk.get_block(x, y, z).0;
                        masks.entry(block_id).or_insert_with(|| {
                            vec![vec![false; dim_v]; dim_u]
                        })[u][v] = true;
                    }
                }

                // Greedy merge each block type's mask
                for (block_id, mask) in &masks {
                    let rects = Self::greedy_merge(mask, dim_u, dim_v, *block_id);

                    for (u, v, w, h) in rects {
                        // Convert rect to quad vertices
                        let (p0, p1, p2, p3) = match axis {
                            0 => {
                                let x = if dir == 1 { world_x + CHUNK_WIDTH as f32 } else { world_x };
                                let y0 = world_x + u as f32; // Y axis
                                let z0 = world_z + v as f32; // Z axis
                                (
                                    [x, y0, z0],
                                    [x, y0 + h as f32, z0],
                                    [x, y0 + h as f32, z0 + w as f32],
                                    [x, y0, z0 + w as f32],
                                )
                            }
                            1 => {
                                let y = if dir == 1 { CHUNK_HEIGHT as f32 } else { 0.0 };
                                let x0 = world_x + v as f32;
                                let z0 = world_z + u as f32;
                                (
                                    [x0, y, z0],
                                    [x0 + w as f32, y, z0],
                                    [x0 + w as f32, y, z0 + h as f32],
                                    [x0, y, z0 + h as f32],
                                )
                            }
                            2 => {
                                let z = if dir == 1 { world_z + CHUNK_WIDTH as f32 } else { world_z };
                                let x0 = world_x + u as f32;
                                let y0 = v as f32;
                                (
                                    [x0, y0, z],
                                    [x0, y0 + h as f32, z],
                                    [x0 + w as f32, y0 + h as f32, z],
                                    [x0 + w as f32, y0, z],
                                )
                            }
                            _ => unreachable!(),
                        };

                        let normal = match axis {
                            0 => [dir as f32, 0.0, 0.0],
                            1 => [0.0, dir as f32, 0.0],
                            2 => [0.0, 0.0, dir as f32],
                            _ => unreachable!(),
                        };

                        let uv0 = [0.0, 0.0];
                        let uv1 = [0.0, 1.0];
                        let uv2 = [1.0, 1.0];
                        let uv3 = [1.0, 0.0];

                        // AO = 1.0 (placeholder for Faz 2)
                        let ao = 1.0;
                        let tex = *block_id;

                        vertices.push(Vertex { position: p0, normal, uv: uv0, ao, texture_id: tex });
                        vertices.push(Vertex { position: p1, normal, uv: uv1, ao, texture_id: tex });
                        vertices.push(Vertex { position: p2, normal, uv: uv2, ao, texture_id: tex });
                        vertices.push(Vertex { position: p3, normal, uv: uv3, ao, texture_id: tex });

                        indices.push(vertex_offset);
                        indices.push(vertex_offset + 1);
                        indices.push(vertex_offset + 2);
                        indices.push(vertex_offset);
                        indices.push(vertex_offset + 2);
                        indices.push(vertex_offset + 3);

                        vertex_offset += 4;
                    }
                }
            }
        }

        let bounds = BoundingBox {
            min: Vec3::new(world_x, 0.0, world_z),
            max: Vec3::new(world_x + CHUNK_WIDTH as f32, CHUNK_HEIGHT as f32, world_z + CHUNK_WIDTH as f32),
        };

        MeshData {
            vertex_count: vertices.len(),
            index_count: indices.len(),
            vertices,
            indices,
            bounds,
        }
    }

    fn name(&self) -> &str {
        "classic_greedy"
    }
}
```

#### 2.2.3. `chunk_mesh.rs`

```rust
use crate::mesher::{MeshData, Mesher};
use strata_core::Chunk;

/// High-level chunk mesh builder
pub struct ChunkMeshBuilder {
    mesher: Box<dyn Mesher>,
}

impl ChunkMeshBuilder {
    pub fn new(mesher: impl Mesher + 'static) -> Self {
        Self {
            mesher: Box::new(mesher),
        }
    }

    pub fn build(&self, chunk: &Chunk) -> MeshData {
        self.mesher.generate_mesh(chunk)
    }

    pub fn mesher_name(&self) -> &str {
        self.mesher.name()
    }
}
```

#### 2.2.4. `lib.rs`

```rust
pub mod mesher;
pub mod classic_greedy;
pub mod chunk_mesh;

pub use classic_greedy::ClassicGreedyMesher;
pub use chunk_mesh::ChunkMeshBuilder;
pub use mesher::{MeshData, Mesher, Vertex};
```

---

### Gün 10: Doğrulama

```bash
cargo test -p strata-world-gen
cargo test -p strata-meshing
cargo clippy --workspace -- -D warnings
```

**Milestone 2:** World-gen chunk üretiyor, mesher MeshData döndürüyor.

---

## Hafta 3: Storage + Physics + Client Window

### Gün 11-12: `storage` Crate

**`crates/storage/Cargo.toml`:**
```toml
[package]
name = "strata-storage"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
rkyv.workspace = true
zstd.workspace = true
tokio.workspace = true
thiserror.workspace = true
tracing.workspace = true
anyhow.workspace = true
```

#### 3.1.1. `format.rs` — Binary Chunk Format

```rust
use strata_core::{Chunk, ChunkPos, CHUNK_VOLUME};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Invalid magic bytes")]
    InvalidMagic,
    #[error("Version mismatch: expected {0}, got {1}")]
    VersionMismatch(u16, u16),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Compression error: {0}")]
    Compression(#[from] zstd::Error),
}

const MAGIC: [u8; 4] = *b"VXCL";
const VERSION: u16 = 1;

/// Serialize chunk to binary + zstd
pub fn serialize_chunk(chunk: &Chunk) -> Result<Vec<u8>, StorageError> {
    // Header: 16 bytes
    let mut header = Vec::with_capacity(16);
    header.extend_from_slice(&MAGIC);
    header.extend_from_slice(&VERSION.to_le_bytes());
    header.extend_from_slice(&chunk.position.0.x.to_le_bytes());
    header.extend_from_slice(&chunk.position.0.y.to_le_bytes());

    // Raw data: 65,536 × u16 = 131,072 bytes
    let raw_data: Vec<u8> = chunk
        .as_slice()
        .iter()
        .flat_map(|b| b.to_le_bytes())
        .collect();

    // Compress
    let compressed = zstd::encode_all(raw_data.as_slice(), 3)?;
    let data_len = compressed.len() as u16;

    header.extend_from_slice(&data_len.to_le_bytes());

    let mut output = header;
    output.extend(compressed);

    Ok(output)
}

/// Deserialize binary to chunk
pub fn deserialize_chunk(data: &[u8]) -> Result<Chunk, StorageError> {
    if data.len() < 16 {
        return Err(StorageError::InvalidMagic);
    }

    if &data[0..4] != MAGIC {
        return Err(StorageError::InvalidMagic);
    }

    let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
    if version != VERSION {
        return Err(StorageError::VersionMismatch(VERSION, version));
    }

    let chunk_x = i32::from_le_bytes(data[6..10].try_into().unwrap());
    let chunk_z = i32::from_le_bytes(data[10..14].try_into().unwrap());
    let data_len = u16::from_le_bytes(data[14..16].try_into().unwrap()) as usize;

    let compressed = &data[16..16 + data_len];
    let raw_data = zstd::decode_all(compressed)?;

    let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
    for chunk in raw_data.chunks_exact(2) {
        blocks.push(u16::from_le_bytes(chunk.try_into().unwrap()));
    }

    let mut chunk = Chunk::new(ChunkPos(glam::IVec2::new(chunk_x, chunk_z)));
    chunk.blocks = blocks;
    chunk.dirty = false;

    Ok(chunk)
}
```

#### 3.1.2. `region.rs` — Region File Management

```rust
use std::path::{Path, PathBuf};
use strata_core::ChunkPos;
use crate::format::{deserialize_chunk, serialize_chunk, StorageError};

/// Region = 32x32 chunks in a single directory
pub struct RegionManager {
    base_path: PathBuf,
}

impl RegionManager {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        let base = base_path.into();
        std::fs::create_dir_all(&base).ok();
        Self { base_path: base }
    }

    fn chunk_path(&self, pos: ChunkPos) -> PathBuf {
        // region_x/region_z/chunk_x.chunk_z.dat
        let region_x = pos.0.x.div_euclid(32);
        let region_z = pos.0.y.div_euclid(32);
        let region_dir = self.base_path.join(format!("r{}_r{}", region_x, region_z));
        region_dir.join(format!("c{}_c{}.dat", pos.0.x, pos.0.y))
    }

    pub fn save_chunk(&self, chunk: &strata_core::Chunk) -> Result<(), StorageError> {
        let path = self.chunk_path(chunk.position);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serialize_chunk(chunk)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn load_chunk(&self, pos: ChunkPos) -> Result<Option<strata_core::Chunk>, StorageError> {
        let path = self.chunk_path(pos);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read(path)?;
        let chunk = deserialize_chunk(&data)?;
        Ok(Some(chunk))
    }

    pub fn chunk_exists(&self, pos: ChunkPos) -> bool {
        self.chunk_path(pos).exists()
    }
}
```

#### 3.1.3. `cache.rs` — LRU Chunk Cache

```rust
use std::collections::HashMap;
use strata_core::{Chunk, ChunkPos};

/// Simple LRU cache for chunks in memory
pub struct ChunkCache {
    cache: HashMap<ChunkPos, Chunk>,
    order: Vec<ChunkPos>,
    max_size: usize,
}

impl ChunkCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: HashMap::with_capacity(max_size),
            order: Vec::with_capacity(max_size),
            max_size,
        }
    }

    pub fn get(&self, pos: &ChunkPos) -> Option<&Chunk> {
        self.cache.get(pos)
    }

    pub fn insert(&mut self, pos: ChunkPos, chunk: Chunk) {
        if self.cache.len() >= self.max_size {
            if let Some(oldest) = self.order.first().copied() {
                self.cache.remove(&oldest);
                self.order.remove(0);
            }
        }
        self.cache.insert(pos, chunk);
        self.order.push(pos);
    }

    pub fn remove(&mut self, pos: &ChunkPos) -> Option<Chunk> {
        self.order.retain(|p| p != pos);
        self.cache.remove(pos)
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn contains(&self, pos: &ChunkPos) -> bool {
        self.cache.contains_key(pos)
    }
}
```

#### 3.1.4. `loader.rs` — Async Chunk Loader

```rust
use tokio::sync::mpsc;
use strata_core::{Chunk, ChunkPos};
use crate::cache::ChunkCache;
use crate::region::RegionManager;

pub struct AsyncChunkLoader {
    cache: ChunkCache,
    region: RegionManager,
    rx: mpsc::Receiver<Chunk>,
    tx: mpsc::Sender<Chunk>,
}

impl AsyncChunkLoader {
    pub fn new(region: RegionManager, cache_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(64);
        Self {
            cache: ChunkCache::new(cache_size),
            region,
            rx,
            tx,
        }
    }

    /// Request chunk load (async, non-blocking)
    pub fn request_load(&self, pos: ChunkPos) {
        let tx = self.tx.clone();
        let region = self.region.clone(); // RegionManager needs Clone or Arc

        tokio::spawn(async move {
            if let Ok(Some(chunk)) = region.load_chunk(pos) {
                let _ = tx.send(chunk).await;
            }
        });
    }

    /// Drain loaded chunks — call every frame
    pub fn drain_loaded(&mut self) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        while let Ok(chunk) = self.rx.try_recv() {
            chunks.push(chunk);
        }
        chunks
    }

    pub fn get_cached(&self, pos: &ChunkPos) -> Option<&Chunk> {
        self.cache.get(pos)
    }

    pub fn cache_chunk(&mut self, pos: ChunkPos, chunk: Chunk) {
        self.cache.insert(pos, chunk);
    }
}
```

#### 3.1.5. `lib.rs`

```rust
pub mod format;
pub mod region;
pub mod cache;
pub mod loader;

pub use cache::ChunkCache;
pub use format::{deserialize_chunk, serialize_chunk};
pub use loader::AsyncChunkLoader;
pub use region::RegionManager;
```

---

### Gün 13-14: `physics` Crate

**`crates/physics/Cargo.toml`:**
```toml
[package]
name = "strata-physics"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../core" }
strata-ecs = { path = "../ecs" }
bevy_rapier3d = { version = "0.33", features = ["enhanced-determinism"] }
bevy_ecs.workspace = true
bevy_app.workspace = true
bevy_math.workspace = true
glam.workspace = true
```

#### 3.2.1. `rapier_plugin.rs`

```rust
use bevy_app::prelude::*;
use bevy_rapier3d::prelude::*;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());
        app.init_resource::<PhysicsConfig>();
    }
}

#[derive(Resource)]
pub struct PhysicsConfig {
    pub gravity: f32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self { gravity: -20.0 }
    }
}
```

#### 3.2.2. `collision.rs`

```rust
use strata_core::{BlockPos, Chunk, BlockId};
use glam::IVec3;

/// Simple AABB collision check against voxel data
pub fn is_block_solid(chunk: &Chunk, x: usize, y: usize, z: usize) -> bool {
    let block = chunk.get_block(x, y, z);
    !block.is_air() && !strata_core::BlockRegistry::new().get(block).transparent
}

/// Raycast through voxel grid — returns first solid block hit
pub fn voxel_raycast(
    chunk: &Chunk,
    origin: glam::Vec3,
    direction: glam::Vec3,
    max_dist: f32,
) -> Option<BlockPos> {
    let dir = direction.normalize();
    let mut pos = origin;
    let step = 0.1;
    let steps = (max_dist / step) as usize;

    for _ in 0..steps {
        pos += dir * step;
        let bx = pos.x.floor() as i32;
        let by = pos.y.floor() as i32;
        let bz = pos.z.floor() as i32;

        let (chunk_pos, lx, ly, lz) = BlockPos(IVec3::new(bx, by, bz)).to_chunk_local();
        if chunk_pos == chunk.position {
            if !chunk.get_block(lx, ly, lz).is_air() {
                return Some(BlockPos(IVec3::new(bx, by, bz)));
            }
        }
    }

    None
}
```

#### 3.2.3. `aabb.rs`

```rust
use glam::Vec3;

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(center: Vec3, half_size: Vec3) -> Self {
        Self {
            min: center - half_size,
            max: center + half_size,
        }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
            && self.min.z < other.max.z
            && self.max.z > other.min.z
    }

    pub fn contains_point(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }
}
```

#### 3.2.4. `lib.rs`

```rust
pub mod rapier_plugin;
pub mod collision;
pub mod aabb;
pub mod raycast;

pub use aabb::Aabb;
pub use collision::{is_block_solid, voxel_raycast};
pub use rapier_plugin::PhysicsPlugin;
```

---

### Gün 14-15: `bin/client` — Window + WGPU Init

**`bin/client/Cargo.toml`:**
```toml
[package]
name = "strata-client"
version.workspace = true
edition.workspace = true

[dependencies]
strata-core = { path = "../../crates/core" }
strata-ecs = { path = "../../crates/ecs" }
strata-world-gen = { path = "../../crates/world-gen" }
strata-meshing = { path = "../../crates/meshing" }
strata-storage = { path = "../../crates/storage" }
strata-physics = { path = "../../crates/physics" }

winit.workspace = true
wgpu.workspace = true
glam.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
anyhow.workspace = true
```

#### 3.3.1. `main.rs` — Minimal Window + Event Loop

```rust
use anyhow::Result;
use strata_world_gen::ChunkGenerator;
use strata_meshing::{ClassicGreedyMesher, ChunkMeshBuilder};
use strata_storage::RegionManager;
use tracing::info;

fn main() -> Result<()> {
    // Init logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Strata client starting...");

    // Winit window
    let event_loop = winit::event_loop::EventLoop::new()?;
    let window = winit::window::Window::new(&event_loop)?;
    window.set_title("Strata — Phase 1");
    window.set_inner_size(winit::dpi::PhysicalSize::new(1280, 720));

    info!("Window created");

    // Chunk generator
    let mut chunk_gen = ChunkGenerator::new(42);

    // Request initial chunks around origin
    for x in -2..=2 {
        for z in -2..=2 {
            chunk_gen.request_chunk(strata_core::ChunkPos(glam::IVec2::new(x, z)));
        }
    }

    // Process initial generation
    let chunks = chunk_gen.process();
    info!("Generated {} chunks", chunks.len());

    // Build meshes
    let mesher = ChunkMeshBuilder::new(ClassicGreedyMesher);
    for chunk in &chunks {
        let mesh = mesher.build(chunk);
        info!(
            "Chunk {:?}: {} vertices, {} indices",
            chunk.position, mesh.vertex_count, mesh.index_count
        );
    }

    // Storage
    let region = RegionManager::new("world_data");
    for chunk in &chunks {
        region.save_chunk(chunk)?;
    }
    info!("Chunks saved to disk");

    // Main loop (minimal — just keep window alive for now)
    event_loop.run(move |event, elwt| {
        match event {
            winit::event::Event::WindowEvent { event, .. } => match event {
                winit::event::WindowEvent::CloseRequested => {
                    elwt.exit();
                }
                winit::event::WindowEvent::KeyboardInput {
                    event:
                        winit::event::KeyEvent {
                            state: winit::event::ElementState::Pressed,
                            logical_key: winit::keyboard::Key::Character(ch),
                            ..
                        },
                    ..
                } => {
                    if ch.as_str() == "q" {
                        elwt.exit();
                    }
                }
                _ => {}
            },
            winit::event::Event::AboutToWait => {
                window.request_redraw();
            }
            _ => {}
        }
    })?;

    Ok(())
}
```

---

### Gün 15: Doğrulama

```bash
cargo run -p strata-client
cargo clippy --workspace -- -D warnings
cargo fmt
```

**Milestone 3:** Pencere açılıyor, chunk'lar üretiliyor, mesh'leniyor, diske kaydediliyor.

---

## Hafta 4: Blok Kırma/Yerleştirme + İlk Oynanabilir

### Gün 16-17: Input Sistemi + Camera (winit)

#### 4.1.1. Input State

```rust
// bin/client/src/input.rs
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct InputState {
    pub move_forward: bool,
    pub move_backward: bool,
    pub move_left: bool,
    pub move_right: bool,
    pub jump: bool,
    pub sprint: bool,
    pub break_block: bool,
    pub place_block: bool,
    pub mouse_dx: f64,
    pub mouse_dy: f64,
}

impl InputState {
    pub fn update(&mut self) {
        self.mouse_dx = 0.0;
        self.mouse_dy = 0.0;
    }
}
```

#### 4.1.2. Camera

```rust
// bin/client/src/camera.rs
use glam::{Vec3, Mat4};

pub struct Camera {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new() -> Self {
        Self {
            position: Vec3::new(0.0, 100.0, 0.0),
            yaw: 0.0,
            pitch: -0.3,
            fov: 70.0_f32.to_radians(),
            aspect: 1280.0 / 720.0,
            near: 0.1,
            far: 1000.0,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        let front = Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.sin() * self.pitch.cos(),
        ).normalize();
        Mat4::look_at_rh(self.position, self.position + front, Vec3::Y)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        Mat4::perspective_rh(self.fov, self.aspect, self.near, self.max)
    }

    pub fn move_direction(&self) -> (Vec3, Vec3) {
        let forward = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin()).normalize();
        let right = forward.cross(Vec3::Y).normalize();
        (forward, right)
    }
}
```

### Gün 18-19: World Manager + Block Interaction

#### 4.2.1. World Manager

```rust
// bin/client/src/world.rs
use hashbrown::HashMap;
use strata_core::{Chunk, ChunkPos, BlockPos, BlockId};
use strata_world_gen::ChunkGenerator;
use strata_meshing::{ChunkMeshBuilder, MeshData};
use strata_storage::RegionManager;

pub struct WorldManager {
    chunks: HashMap<ChunkPos, Chunk>,
    meshes: HashMap<ChunkPos, MeshData>,
    chunk_gen: ChunkGenerator,
    mesh_builder: ChunkMeshBuilder,
    region: RegionManager,
}

impl WorldManager {
    pub fn new(seed: u32) -> Self {
        Self {
            chunks: HashMap::new(),
            meshes: HashMap::new(),
            chunk_gen: ChunkGenerator::new(seed),
            mesh_builder: ChunkMeshBuilder::new(strata_meshing::ClassicGreedyMesher),
            region: RegionManager::new("world_data"),
        }
    }

    pub fn get_chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn get_or_generate(&mut self, pos: ChunkPos) -> &Chunk {
        if !self.chunks.contains_key(&pos) {
            // Try disk first
            if let Ok(Some(chunk)) = self.region.load_chunk(pos) {
                let mesh = self.mesh_builder.build(&chunk);
                self.meshes.insert(pos, mesh);
                self.chunks.insert(pos, chunk);
            } else {
                // Generate
                let mut chunk = Chunk::new(pos);
                // Use terrain generator directly
                let gen = strata_world_gen::TerrainGenerator::new(42);
                gen.generate(&mut chunk);
                let mesh = self.mesh_builder.build(&chunk);
                self.meshes.insert(pos, mesh);
                self.chunks.insert(pos, chunk);
                // Save to disk
                let _ = self.region.save_chunk(&self.chunks[&pos]);
            }
        }
        self.chunks.get(&pos).unwrap()
    }

    pub fn break_block(&mut self, pos: BlockPos) -> Option<BlockId> {
        let (chunk_pos, lx, ly, lz) = pos.to_chunk_local();
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            let old = chunk.get_block(lx, ly, lz);
            chunk.set_block(lx, ly, lz, BlockId::AIR);
            // Rebuild mesh
            let mesh = self.mesh_builder.build(chunk);
            self.meshes.insert(chunk_pos, mesh);
            return Some(old);
        }
        None
    }

    pub fn place_block(&mut self, pos: BlockPos, block: BlockId) {
        let (chunk_pos, lx, ly, lz) = pos.to_chunk_local();
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            chunk.set_block(lx, ly, lz, block);
            let mesh = self.mesh_builder.build(chunk);
            self.meshes.insert(chunk_pos, mesh);
        }
    }
}
```

### Gün 20-21: Minimal WGPU Render (Debug)

#### 4.3.1. Render State

```rust
// bin/client/src/render.rs
use wgpu::util::DeviceExt;
use glam::Mat4;

pub struct RenderState {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub render_pipeline: wgpu::RenderPipeline,
}

impl RenderState {
    pub async fn new(window: &winit::window::Window) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).unwrap();

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .unwrap();

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .await
            .unwrap();

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats[0];
        let size = window.inner_size();

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // Shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Chunk Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/chunk.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Chunk Pipeline Layout"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Chunk Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<strata_meshing::Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, // position
                        1 => Float32x3, // normal
                        2 => Float32x2, // uv
                        3 => Float32,   // ao
                        4 => Uint16,    // texture_id
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            surface,
            device,
            queue,
            config,
            render_pipeline,
        }
    }
}
```

#### 4.3.2. Shader

```wgsl
// bin/client/src/shaders/chunk.wgsl
struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) ao: f32,
    @location(4) texture_id: u16,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) ao: f32,
};

@group(0) @binding(0)
var<uniform> view_proj: mat4x4<f32>;

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_pos = view_proj * vec4<f32>(input.position, 1.0);
    output.normal = input.normal;
    output.uv = input.uv;
    output.ao = input.ao;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light = max(dot(normalize(input.normal), vec3<f32>(0.3, 1.0, 0.5).normalize()), 0.0);
    let ambient = 0.3;
    let color = vec3<f32>(0.5, 0.8, 0.3) * (ambient + light * 0.7) * input.ao;
    return vec4<f32>(color, 1.0);
}
```

### Gün 21-22: Main Loop Integration

```rust
// bin/client/src/main.rs — Full loop
use anyhow::Result;
use strata_core::{BlockId, BlockPos, ChunkPos};
use strata_world_gen::ChunkGenerator;
use strata_meshing::{ClassicGreedyMesher, ChunkMeshBuilder};
use strata_storage::RegionManager;
use tracing::info;

mod input;
mod camera;
mod world;
mod render;

use input::InputState;
use camera::Camera;
use world::WorldManager;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Strata client starting...");

    let event_loop = winit::event_loop::EventLoop::new()?;
    let window = winit::window::Window::new(&event_loop)?;
    window.set_title("Strata — Phase 1");
    window.set_inner_size(winit::dpi::PhysicalSize::new(1280, 720));

    let render = render::RenderState::new(&window).await;
    let mut camera = Camera::new();
    let mut world = WorldManager::new(42);
    let mut input = InputState::default();

    // Load initial chunks
    for x in -2..=2 {
        for z in -2..=2 {
            world.get_or_generate(ChunkPos(glam::IVec2::new(x, z)));
        }
    }
    info!("Initial chunks loaded");

    event_loop.run(move |event, elwt| {
        match event {
            winit::event::Event::WindowEvent { event, .. } => match event {
                winit::event::WindowEvent::CloseRequested => elwt.exit(),
                winit::event::WindowEvent::KeyboardInput { event, .. } => {
                    // Handle input
                }
                winit::event::WindowEvent::MouseInput { state, button, .. } => {
                    if state == winit::event::ElementState::Pressed {
                        match button {
                            winit::event::MouseButton::Left => {
                                // Break block (raycast)
                            }
                            winit::event::MouseButton::Right => {
                                // Place block
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            winit::event::Event::AboutToWait => {
                // Update camera from input
                // Update world from input
                // Render frame
                window.request_redraw();
            }
            _ => {}
        }
    })?;

    Ok(())
}
```

---

### Gün 22-23: Final Polish + Testing

```bash
# Full workspace build
cargo build --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format
cargo fmt

# Tests
cargo test --workspace

# Run client
cargo run -p strata-client
```

---

## Faz 1 Teslim Kriterleri

| # | Kriter | Durum |
|---|--------|-------|
| 1 | `cargo build --workspace` başarılı | |
| 2 | `cargo clippy --workspace -- -D warnings` temiz | |
| 3 | `cargo test --workspace` geçer | |
| 4 | Pencere açılır, wgpu initialize olur | |
| 5 | Prosedürel chunk üretimi (fastnoise2 FBM) | |
| 6 | Classic greedy meshing çalışır (<500µs/chunk) | |
| 7 | Chunk disk'e kaydedilir ve geri yüklenir (binary+zstd) | |
| 8 | Blok kırma çalışır (sol tık) | |
| 9 | Blok yerleştirme çalışır (sağ tık) | |
| 10 | Temel kamera hareketi (WASD + mouse) | |
| 11 | Mesh rebuild after block change | |
| 12 | FPS sayacı (debug overlay) | |

---

## Performans Doğrulama (Faz 1)

```bash
# Benchmark meshing
cargo bench -p strata-meshing

# Expected: <500µs/chunk for classic greedy
```

---

## Riskler ve Yedek Planlar

| Risk | Olasılık | Yedek Plan |
|------|----------|------------|
| fastnoise2 C++ build hatası (MSVC) | Düşük | `cc` crate MSVC toolchain'i otomatik bulur; gerekirse `vcpkg` ile önceden kur |
| wgpu 29 + winit 0.30 API breaking | Orta | Versiyonları `Cargo.lock` ile sabitle, migration guide takip et |
| Greedy meshing T-junction artifact'ları | Orta | Faz 1'de kabul et, Faz 2'de vertex snapping ile düzelt |
| bevy_rapier Windows MSVC build | Düşük | `enhanced-determinism` feature'ı dene; gerekirse Faz 1'de custom AABB kullan |
| Zstd compression ratio düşük | Düşük | Level 3-5 arası test et; RLE ön-işleme ekle |

---

## Sonraki Faz Hazırlığı (Faz 2)

Faz 1 tamamlandığında şu altyapılar Faz 2 için hazır olmalı:

1. **`Mesher` trait** → GPU compute shader implementasyonu takılabilir
2. **`MeshData` formatı** → Texture2DArray + lighting vertex attrib'leri eklenebilir
3. **Plugin trait** → Render/Lighting/WorldGen plugin'leri refactor edilebilir
4. **Chunk dirty flag** → Incremental light propagation için kullanılabilir
5. **Heightmap** → Frustum culling + LOD için kullanılabilir
