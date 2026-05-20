# 02 — XBrickMap Veri Yapısı

## 1. XBrickMap — Aktif Alan Veri Yapısı

### 1.1 Hiyerarşik Yapı

XBrickMap, VoxelRT benchmark'da **en iyi** performansı gösteren brickmap varyantıdır. 4 seviyeli hiyerarşik bitmask + left-packed materyal dizisi kullanır.

Sector boyutu **32×128×32** (Y ekseni 128 voxel — mağara/tavan desteği için). Bu boyut, tek bir u64 bitmask ile 4³=64 brick'i adreslemenin ötesinde olduğu için **4 slab** yapısı kullanılır:

```
Sector (32×128×32 = 131.072 voxel)
  ├── 4 Slab (her biri 32×32×32 = 32.768 voxel)
  │   │   Y=0..32, Y=32..64, Y=64..96, Y=96..128
  │   │
  │   ├── Slab Bitmask: u64
  │   │   └── 4³ = 64 brick'in doluluk bilgisi (her bit 1 brick)
  │   │       └── Left-packed: boş brick'ler dizide yer kaplamaz
  │   │
  │   ├── Brick[0..N] (N ≤ 64, sadece dolu olanlar)
  │   │   ├── Brick Bitmask: u64
  │   │   │   └── 4³ = 64 sub-brick'in doluluk bilgisi
  │   │   │       └── Her sub-brick = 2³ = 8 voxel
  │   │   │
  │   │   ├── Sub-brick[0..M] (M ≤ 64, sadece dolu olanlar)
  │   │   │   ├── Sub-brick Bitmask: 8-bit
  │   │   │   │   └── 2³ = 8 voxel'in doluluk bilgisi
  │   │   │   └── Materials: [u16; K] (K ≤ 8, left-packed)
  │   │   │
  │   │   └── Mip Levels (space skipping için)
  │   │       ├── mip_half: 64-bit (4³ = 64 voxel'in ortalama doluluğu)
  │   │       └── mip_quarter: 64-bit (2³ = 8 voxel'in ortalama doluluğu)
  │   │
  │   └── SVDAG Root Index (Tier 2'den itibaren, Option<u32>)
  │
  └── Slab metadata: [SlabMeta; 4]
```

**Neden 4 slab?**
- 32×128×32 = 131.072 voxel → 256 brick (8³ boyutunda)
- 256 brick > 64 bit → tek u64 bitmask yetmez
- 4 slab × 64 brick = 256 brick, her slab kendi u64'üne sahip
- Ray tracing: Y koordinatından slab index'i O(1) (`y / 32`)

### 1.2 Bellek Hesabı

| Bileşen | Boyut | Not |
|---|---|---|
| Slab metadata (4 slab) | 4 × 8 byte = 32B | Her slab'ın u64 bitmask'i |
| Brick dizisi (per slab) | 0-64 × ~1.2KB | Left-packed, boş brick yok |
| — Brick bitmask | 8 byte | Her brick için |
| — Sub-brick dizisi | 0-64 × ~24B | Left-packed |
| — Materials | 0-512 × 2B | Sadece dolu voxel'ler |
| — Mip levels | 16 byte | 2 × 64-bit per brick |
| **Tam dolu sector** | ~312KB | 131.072 voxel (4 × ~78KB) |
| **Ortalama arazi** | ~120-160KB | ~50% boşluk varsayımı |
| **Boş sector** | 32 byte | Sadece 4 slab bitmask |

**Karşılaştırma:** Eski 16×256×16 `Vec<u16>` chunk = 128KB. Yeni XBrickMap boş sector = 32B, dolu sector = ~312KB. Sparse arazi için ortalama ~120-160KB, `Vec<u16>`'dan daha verimli.

### 1.3 Veri Yapısı (Rust)

```rust
/// 32×128×32 voksellik bir sektörün XBrickMap temsili.
pub struct Sector {
    pub slabs: [Slab; 4],
    pub svdag_root: Option<u32>,
    pub dirty: bool,
    pub last_bake_time: Instant,
}

pub struct Slab {
    pub slab_mask: u64,
    pub bricks: Vec<Brick>,
}

pub struct Brick {
    pub brick_mask: u64,
    pub sub_bricks: Vec<SubBrick>,
    pub mip_half: u64,
    pub mip_quarter: u64,
}

pub struct SubBrick {
    pub voxel_mask: u8,
    pub materials: Vec<u16>,
}
```

### 1.4 Random Access (Popcnt ile O(1))

```rust
impl Sector {
    #[inline]
    fn slab_index(y: i32) -> usize {
        (y >> 5) as usize
    }

    pub fn get_block(&self, pos: IVec3) -> Option<u16> {
        let slab_idx = Self::slab_index(pos.y);
        let slab = &self.slabs[slab_idx];
        let local_y = pos.y & 31;

        let bx = pos.x / 8;
        let by = local_y / 8;
        let bz = pos.z / 8;
        let brick_index = bx + bz * 4 + by * 16;

        if slab.slab_mask & (1 << brick_index) == 0 {
            return None;
        }

        let brick_offset = (slab.slab_mask & ((1u64 << brick_index) - 1)).count_ones() as usize;
        let brick = &slab.bricks[brick_offset];

        let sx = pos.x % 8 / 2;
        let sy = local_y % 8 / 2;
        let sz = pos.z % 8 / 2;
        let sub_index = sx + sz * 4 + sy * 16;

        if brick.brick_mask & (1 << sub_index) == 0 {
            return None;
        }

        let sub_offset = (brick.brick_mask & ((1u64 << sub_index) - 1)).count_ones() as usize;
        let sub = &brick.sub_bricks[sub_offset];

        let vx = pos.x % 2;
        let vy = local_y % 2;
        let vz = pos.z % 2;
        let v_index = (vx + vz * 2 + vy * 4) as u8;

        if sub.voxel_mask & (1 << v_index) == 0 {
            return None;
        }

        let v_offset = (sub.voxel_mask & ((1u8 << v_index) - 1)).count_ones() as usize;
        Some(sub.materials[v_offset])
    }

    pub fn set_block(&mut self, pos: IVec3, block_id: Option<u16>) {
        let slab_idx = Self::slab_index(pos.y);
        self.dirty = true;
    }
}
```

### 1.5 Ray Tracing (4-Level Space Skipping)

```wgsl
@compute @workgroup_size(64)
fn xbrickmap_ray_trace(
    @builtin(global_invocation_id) id: vec3<u32>,
) {
    let pixel = id.xy;
    let ray = camera_get_ray(pixel);

    var t: f32 = 0.0;
    var step: f32 = 1.0;

    for (var i = 0u; i < 512u; i++) {
        let pos = ray.origin + ray.direction * t;
        let sector_coord = floor(pos / vec3f(32.0, 128.0, 32.0));

        let sector_bitmask = sector_query_bitmask(sector_coord);
        if (sector_bitmask == 0u) {
            step = 128.0;
            t += step;
            continue;
        }

        let slab_idx = u32(floor(pos.y / 32.0)) & 3u;
        let slab_bitmask = slab_load(sector_coord, slab_idx);
        if (slab_bitmask == 0u) {
            step = 32.0;
            t += step;
            continue;
        }

        let local_y = fract(pos.y / 32.0) * 32.0;
        let brick_pos = vec3f(fract(pos.x / 8.0) * 8.0, fract(local_y / 8.0) * 8.0, fract(pos.z / 8.0) * 8.0);
        let brick_index = compute_brick_index(brick_pos);

        if ((slab_bitmask & (1u << brick_index)) == 0u) {
            step = 8.0;
            t += step;
            continue;
        }

        let sub_pos = fract(pos / 2.0) * 2.0;
        let sub_index = compute_sub_index(sub_pos);

        let brick_data = brick_load(sector_coord, slab_idx, brick_index);
        if ((brick_data.brick_mask & (1u << sub_index)) == 0u) {
            step = 2.0;
            t += step;
            continue;
        }

        let voxel_pos = floor(pos);
        let voxel_id = voxel_load(sector_coord, slab_idx, brick_index, sub_index, voxel_pos);
        if (voxel_id != AIR) {
            visibility_buffer_write(pixel, t, pos, voxel_id);
            return;
        }

        step = 1.0;
        t += step;
    }
}
```

### 1.6 SOA Layout + SIMD Optimizasyonu

Mevcut AOS layout **pointer chasing** yaratır. **SOA (Structure of Arrays)** ile bu sorun çözülür.

#### 1.6.1 AOS → SOA Dönüşümü

```rust
// SOA (iyi) — tüm veriler bitişik, SIMD ile işlenebilir
pub struct Slab {
    pub slab_mask: u64,
    pub brick_masks: Vec<u64>,
    pub sub_brick_offsets: Vec<u32>,
    pub sub_bricks: Vec<SubBrick>,
    pub materials: Vec<u16>,
    pub mip_half: Vec<u64>,
    pub mip_quarter: Vec<u64>,
}
```

#### 1.6.2 SIMD Popcnt ile Paralel Bitmask İşleme

```rust
use wide::u64x4;

impl Slab {
    #[inline]
    pub fn popcnt_4_bricks(&self, indices: [usize; 4]) -> [u32; 4] {
        let masks = u64x4::new([
            self.brick_masks[indices[0]],
            self.brick_masks[indices[1]],
            self.brick_masks[indices[2]],
            self.brick_masks[indices[3]],
        ]);
        let result = masks.count_ones();
        [result[0], result[1], result[2], result[3]]
    }

    #[inline]
    pub fn check_4_bricks(&self, indices: [usize; 4]) -> u64x4 {
        let masks = u64x4::new([
            self.brick_masks[indices[0]],
            self.brick_masks[indices[1]],
            self.brick_masks[indices[2]],
            self.brick_masks[indices[3]],
        ]);
        masks
    }
}
```

#### 1.6.3 SOA Bellek Hesabı

| Bileşen | AOS (bytes) | SOA (bytes) | Fark |
|---|---|---|---|
| Slab header | 16 | 56 | +40 |
| Brick dizisi (64 brick) | 3072 | 512 | **-2560** |
| Sub-brick dizisi | 1536 | 512 | **-1024** |
| Materials | 1024 | 1024 | 0 |
| Mip levels | 1024 | 1024 | 0 |
| **Toplam (full slab)** | **~6672B** | **~3128B** | **-53%** |

#### 1.6.4 Object Pooling

```rust
use slotmap::SlotMap;

pub struct BrickPool {
    bricks: SlotMap<BrickKey, BrickData>,
    free_list: Vec<BrickKey>,
}

pub struct BrickData {
    pub brick_mask: u64,
    pub sub_brick_start: u32,
    pub sub_brick_count: u32,
    pub mip_half: u64,
    pub mip_quarter: u64,
}
```
