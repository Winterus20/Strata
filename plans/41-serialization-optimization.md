# 41 — postcard + bytemuck Optimizasyon Stratejisi

> **Durum:** Araştırma tamamlandı
> **Tarih:** 2026-05-22
> **Temel:** `plans/40-serialization-format.md` kararı üzerine derin optimizasyon
> **Kaynaklar:** Aşağıda her bölümde listelenmiştir

---

## 0. Optimizasyon Özeti

| Katman | Optimizasyon | Tahmini Kazanım | Karmaşıklık |
|--------|-------------|-----------------|-------------|
| **Network** | SDEC delta codec (bit-packed) | **4.3× boyut azalma** | Orta |
| **Network** | postcard COBS + CRC flavor | Güvenilirlik + framing | Düşük |
| **Network** | Arena-based deserialization | %30-50 daha az alloc | Orta |
| **Save/Load** | postcard + zstd dictionary | %15-25 ek sıkıştırma | Düşük |
| **Save/Load** | Region file + mmap | O(1) chunk erişimi | Yüksek |
| **GPU** | bytemuck struct optimization | %20-33 memory azalma | Düşük |
| **GPU** | encase fallback (dynamic layout) | Esneklik | Düşük |
| **GPU** | wgsl_bindgen (type sharing) | Derleme zamanı safety | Orta |
| **Genel** | Bumpalo arena allocator | Frame-based alloc reset | Düşük |
| **Genel** | Cow<str> for rare mutations | Alloc avoidance | Düşük |

**Potansiyel Toplam Kazanım:** Network bandwidth ~5× daha küçük, GPU memory ~30% daha verimli, alloc overhead ~%40 azalma

---

## 1. Network Optimizasyonları

### 1.1 SDEC (Snapshot Delta Encoding Codec)

**Ne:** Transport-agnostic, bit-packed snapshot + delta codec

**Neden kritik?**
bevy_replicon'un kendi delta encoding'i var, ama SDEC daha agresif optimizasyon sunar:

| Metrik | SDEC Delta | Bincode Delta | Fark |
|--------|-----------|---------------|------|
| Ortalama packet boyutu | **259 byte** | 1,114 byte | **4.3× daha küçük** |
| P95 packet boyutu | 266 byte | 1,159 byte | 4.3× daha küçük |
| Encode süresi | ~10 µs | ~2 µs | 5× daha yavaş |
| Decode süresi | ~5 µs | ~1 µs | 5× daha yavaş |

**Trade-off:** CPU'dan feragat, bandwidth kazanımı. Network-bound oyunlarda bu doğru takas.

**Nasıl çalışır:**
1. Bit-level precision (byte boundary yok)
2. Delta encoding: sadece değişen field'lar
3. Quantization: position/rotation için configurable precision
4. Schema-driven: hangi field'ın nasıl encode edildiği tanımlı

**Entegrasyon stratejisi:**
```
bevy_replicon (ECS replication)
    ↓ dirty components
sdec-bevy (extract → schema → codec)
    ↓ bit-packed delta
bevy_quinnet (QUIC transport)
```

**Kaynaklar:**
- [sdec-repgraph](https://lib.rs/crates/sdec-repgraph)
- [sdec-bevy](https://crates.io/crates/sdec-bevy)

### 1.2 postcard COBS + CRC Flavor

**Ne:** postcard'ın flavor sistemi ile framing + checksum

**COBS (Consistent Overhead Byte Stuffing):**
- Mesajları `0x00` delimiter ile çerçeveler
- QUIC zaten kendi framing'ini yapıyor, ama unreliable datagram'larda faydalı
- Overhead: ~1 byte per 254 byte data

**CRC (Cyclic Redundancy Check):**
- Data integrity verification
- `postcard::ser_flavors::crc` modülü
- CRC-32 veya CRC-16 seçenekleri

**Kullanım:**
```rust
use postcard::{
    serialize_with_flavor,
    ser_flavors::{Cobs, Slice, crc::Crc32},
};

// COBS + CRC + Slice flavor stack
let res = serialize_with_flavor::<_, Cobs<Crc32<Slice>>, _>(
    &data,
    Cobs::try_new(Crc32::new(Slice::new(buffer))).unwrap(),
).unwrap();
```

**Ne zaman kullan:**
- Unreliable datagram (QUIC Datagram) → COBS framing gerekli
- Reliable stream → QUIC zaten CRC/checksum yapıyor, gereksiz

**Kaynaklar:**
- [postcard ser_flavors](https://docs.rs/postcard/latest/postcard/ser_flavors/index.html)
- [COBS + CRC usage](https://github.com/jamesmunns/postcard/issues/117)

### 1.3 Arena-Based Deserialization

**Ne:** Deserialization sırasında heap allocation yerine arena kullanma

**Neden?**
Her network packet'te `Vec`, `String` gibi tipler deserialize edilirken heap allocation olur. 600 oyuncu × 20Hz = 12,000 alloc/saniye.

**Çözüm: bumpalo arena**
```rust
use bumpalo::Bump;

let arena = Bump::new();
// Deserialize into arena-allocated types
let data: &MyPacket = postcard::from_bytes(&arena, &bytes)?;
// Arena dropped → tüm allocation'lar tek seferde free
```

**Sorun:** postcard serde ile çalışır, bumpalo serde integration gerektirir.

**Alternatif: bumpalo_serde**
- `bumpalo-serde` crate'i serde deserialization'ı arena'ya yönlendirir
- Ama `Vec`, `String` gibi tipler `bumpalo::collections::Vec`, `bumpalo::collections::String` olmalı

**Trade-off:**
- Tip sistemi değişikliği gerektirir (Vec → bumpalo::collections::Vec)
- Lifetime management karmaşıklaşır
- Ama alloc overhead dramatik şekilde azalır

**Kaynaklar:**
- [serde optimization gauntlet](https://nickb.dev/blog/the-serde-optimization-gauntlet-wasm-and-arenas)
- [Arena allocation in Rust](https://medium.com/@syntaxSavage/arena-allocation-in-rust-fast-memory-for-short-lived-objects-2e55a89257d6)

---

## 2. Save/Load Optimizasyonları

### 2.1 postcard + zstd Dictionary

**Ne:** zstd dictionary mode ile tekrarlayan pattern'ler için daha iyi sıkıştırma

**Nasıl çalışır:**
zstd dictionary, veri setindeki tekrarlayan pattern'leri öğrenir ve daha iyi sıkıştırma sağlar.

```rust
// Dictionary eğitimi (bir kez)
let dictionary = zstd::dict::from_samples(&training_data, 32768)?;

// Sıkıştırma (dictionary ile)
let compressed = zstd::encode_all_with_dictionary(&data[..], &dictionary)?;

// Açma (dictionary ile)
let decompressed = zstd::decode_all_with_dictionary(&compressed[..], &dictionary)?;
```

**Kazanım:** Standart zstd'ye göre %15-25 ek sıkıştırma (tekrarlayan chunk data için)

**Ne zaman eğitim:**
- İlk world generation'dan sonra
- Sample data: farklı chunk tiplerinden 100-1000 chunk
- Dictionary boyutu: 32KB (default)

**Kaynaklar:**
- [Better Compression with Zstandard](https://gregoryszorc.com/blog/2017/03/07/better-compression-with-zstandard)
- [zstd dictionary](https://facebook.github.io/zstd/zstd_manual.html)

### 2.2 Region File Format + mmap

**Ne:** Minecraft tarzı region file formatı

**Nasıl çalışır:**
```
region_file.bin:
┌─────────────────────────────────┐
│ Header (1024 entries)           │
│ ├── offset (3 byte)             │
│ ├── sector count (1 byte)       │
│ └── timestamp (4 byte)          │
├─────────────────────────────────┤
│ Chunk Data Sectors              │
│ ├── chunk_0_0 (postcard + zstd) │
│ ├── chunk_1_0 (postcard + zstd) │
│ └── ...                         │
└─────────────────────────────────┘
```

**mmap ile erişim:**
```rust
use memmap2::Mmap;

let file = File::open("region_0_0.bin")?;
let mmap = unsafe { Mmap::map(&file)? };

// Header'dan chunk offset'ini oku
let offset = read_offset(&mmap, chunk_x, chunk_z);

// Chunk data'yı doğrudan mmap'den oku (zero-copy)
let chunk_data = &mmap[offset..offset + size];
let chunk: ChunkData = postcard::from_bytes(chunk_data)?;
```

**Kazanım:**
- O(1) chunk erişimi (seek yok)
- OS page cache yönetimi
- Büyük world'ler için kritik

**Kaynaklar:**
- [memmap2 crate](https://docs.rs/memmap2)
- [Minecraft region file format](https://minecraft.wiki/w/Java_Edition_region_file_format)

---

## 3. GPU Optimizasyonları

### 3.1 bytemuck Struct Optimization

**Ne:** `#[repr(C)]` struct'larda field ordering ile padding azaltma

**Sorun:**
```rust
// KÖTÜ: 24 byte (8 byte padding)
#[repr(C)]
#[derive(Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],  // 12 byte, alignment 4
    normal: [f32; 3],    // 12 byte, alignment 4
    uv: [f32; 2],        // 8 byte, alignment 4
}
// Toplam: 32 byte, ama alignment 4 → 32 byte ✓
// Bu durumda sorun yok, ama daha karmaşık struct'larda sorun olabilir
```

**Daha karmaşık örnek:**
```rust
// KÖTÜ: 32 byte (12 byte padding)
#[repr(C)]
struct BadUniform {
    a: f32,           // 4 byte
    _pad1: [u8; 12],  // 12 byte padding (alignment 16 için)
    b: [f32; 4],      // 16 byte, alignment 16
}
// Toplam: 32 byte

// İYİ: 20 byte (0 padding)
#[repr(C)]
struct GoodUniform {
    b: [f32; 4],  // 16 byte, alignment 16
    a: f32,       // 4 byte
}
// Toplam: 20 byte, alignment 16 → 32 byte (round up)
// Ama field'lar arası padding yok!
```

**Kural:** Büyük alignment'lı field'ları önce koy.

**WGSL alignment kuralları:**
- `f32`, `u32`, `i32`: 4 byte alignment
- `vec2<f32>`: 8 byte alignment
- `vec3<f32>`: 16 byte alignment (!)
- `vec4<f32>`: 16 byte alignment
- `mat4x4<f32>`: 16 byte alignment

**Kaynaklar:**
- [WGSL Memory Layout](https://webgpufundamentals.org/webgpu/lessons/webgpu-memory-layout.html)
- [Learn Wgpu - Alignment](https://sotrh.github.io/learn-wgpu/showcase/alignment)
- [StackOverflow GPU alignment](https://stackoverflow.com/questions/75522842/problem-with-aligning-rust-structs-to-send-to-the-gpu-using-bytemuck-and-wgpu)

### 3.2 encase Fallback

**Ne:** bytemuck'un Pod derive'ı yapamadığı durumlar için encase kullanımı

**Ne zaman gerekli:**
- Dynamic array length'li struct'lar
- Nested struct'lar (iç içe struct'lar)
- Runtime'da boyut değişen veri

```rust
use encase::ShaderType;

#[derive(ShaderType)]
struct DynamicData {
    #[size(runtime)]
    data: Vec<u32>,
}

// encase otomatik padding ekler
let buffer = encase::StorageBuffer::write(&data);
```

**Trade-off:**
- bytemuck: compile-time, zero-copy, en hızlı
- encase: runtime, biraz overhead, daha esnek

**Kural:** Mümkünse bytemuck, mümkün değilse encase.

**Kaynaklar:**
- [encase crate](https://crates.io/crates/encase)
- [wgmath buffers initialization](https://wgmath.rs/docs/user_guides/wgcore/buffers_initialization)

### 3.3 wgsl_bindgen (Type Sharing)

**Ne:** WGSL shader ve Rust struct'ları arasında tip paylaşımı

**Nasıl çalışır:**
```rust
// Shader'dan Rust tipi generate et
// WGSL:
// struct Uniforms {
//     view_proj: mat4x4<f32>,
//     camera_pos: vec3<f32>,
// }

// Generate edilen Rust:
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 3],
    _padding: f32,  // otomatik padding
}
```

**Kazanım:**
- Shader ve Rust arasındaki tip uyumsuzluğu compile-time'da yakalanır
- Manuel padding hesaplama gereksiz
- Refactor güvenliği

**Kaynaklar:**
- [wgsl_bindgen](https://lib.rs/crates/wgsl_bindgen)
- [Sharing types between WGPU and rust-gpu](https://dev.to/bardt/sharing-types-between-wgpu-code-and-rust-gpu-shaders-17c4)

---

## 4. Genel Optimizasyonlar

### 4.1 Bumpalo Arena Allocator

**Ne:** Frame-based allocation strategy

**Nasıl çalışır:**
```rust
use bumpalo::Bump;

// Her frame için yeni arena
let frame_arena = Bump::new();

// Frame içinde allocation
let mesh_data = frame_arena.alloc_slice_copy(&vertices);
let indices = frame_arena.alloc_slice_copy(&index_buffer);

// Frame sonunda → arena dropped, tüm allocation'lar free
// O(1) bulk free!
```

**Nerede kullan:**
- Mesh generation (her chunk için)
- Temporary computation buffers
- Network packet processing

**Entegrasyon:**
```toml
[dependencies]
bumpalo = { version = "3.16", features = ["allocator_api"] }
```

**Kaynaklar:**
- [bumpalo crate](https://docs.rs/bumpalo)
- [Arena allocator tips](https://nullprogram.com/blog/2023/09/27)

### 4.2 Cow<str> for Rare Mutations

**Ne:** Nadir değişen string'ler için Copy-On-Write

**Nasıl çalışır:**
```rust
use std::borrow::Cow;

// Çoğu durumda owned String gereksiz
struct BlockDef {
    name: Cow<'static, str>,  // &str veya String
    id: u16,
}

// Statik string → allocation yok
let stone = BlockDef {
    name: Cow::Borrowed("stone"),
    id: 1,
};

// Nadir durumda owned
let custom = BlockDef {
    name: Cow::Owned("mod:block_name".to_string()),
    id: 1000,
};
```

**Kazanım:** Nadir değişen string'ler için heap allocation avoidance.

---

## 5. Compression Karşılaştırması

### 5.1 Genel Amaçlı Compression

| Algoritma | Compress Speed | Decompress Speed | Ratio | Kullanım |
|-----------|---------------|------------------|-------|----------|
| **LZ4** | ~800 MB/s | ~4 GB/s | ~25% | Realtime network |
| **zstd -1** | ~400 MB/s | ~1 GB/s | ~22% | Genel amaçlı |
| **zstd -3** | ~200 MB/s | ~1 GB/s | ~20% | Save/load (dengeli) |
| **zstd -19** | ~5 MB/s | ~1 GB/s | ~15% | Arşivleme |
| **Brotli -5** | ~50 MB/s | ~400 MB/s | ~18% | Web assets |
| **Deflate** | ~100 MB/s | ~300 MB/s | ~22% | Legacy uyumluluk |

**Strata için önerilen:**
- **Network:** LZ4 (hız kritik) veya zstd -1 (daha iyi ratio)
- **Save/Load:** zstd -3 (dengeli) veya zstd dictionary (en iyi ratio)
- **Asset cache:** zstd -19 (bir kez sıkıştır, çok kez aç)

Kaynak: [Compression Algorithms Deep Dive](https://www.youngju.dev/blog/culture/2026-04-15-compression-algorithms-lz77-zstd-brotli-ans-huffman-deep-dive-guide-2025.en), [Rust compression comparison](https://github.com/hwjsnc/rust-compression-comparison)

### 5.2 Voxel-Specific Compression (Veloren Bulguları)

| Yaklaşım | Ratio | Not |
|----------|-------|-----|
| LZ4 | ~25% | Genel network |
| Deflate | ~17% | Daha iyi, benzer hız |
| Greyscale PNG | ~1-2% | Gameplay data (block kind) |
| Palette + RLE | ~5-10% | Block ID'leri için ideal |
| Quarter-res PNG | ~3-5% | Lossy color, iyi kalite |

**Veloren'in keşfi:** "Flipping every odd z-level across the y-axis improves spatial locality, which translates to improved compression ratio and encoding speed."

Kaynak: [Veloren Devblog 117](https://veloren.net/blog/devblog-117)

---

## 6. Uygulama Planı

### Faz 1: Temel Optimizasyonlar (Hemen)
1. bytemuck struct field reordering (GPU)
2. postcard flavor sistemi kurulumu (COBS, CRC)
3. zstd compression entegrasyonu (save/load)

### Faz 2: Orta Seviye (Hafta 2-3)
4. Arena allocator entegrasyonu (bumpalo)
5. zstd dictionary eğitimi (chunk data)
6. Cow<str> refactor (block registry)

### Faz 3: İleri Seviye (Hafta 4+)
7. SDEC delta codec entegrasyonu (network)
8. Region file + mmap (save/load)
9. wgsl_bindgen (GPU type sharing)

---

## 7. Güncellenmiş Crate Listesi

```toml
[dependencies]
# Serialization
postcard = { version = "1.1", features = ["alloc", "use-crc"] }
serde = { version = "1.0", features = ["derive"] }

# GPU
bytemuck = { version = "1.19", features = ["derive"] }
encase = { version = "0.7" }  # fallback
# wgsl_bindgen = "0.3"  # opsiyonel, ileri seviye

# Compression
zstd = { version = "0.13", features = ["dict"] }
lz4 = "1.24"  # network için

# Memory
bumpalo = { version = "3.16", features = ["allocator_api"] }

# Config
toml = "0.8"

# Disk I/O
memmap2 = "0.9"  # region file mmap

# Network delta encoding (opsiyonel, ileri seviye)
# sdec = { version = "0.1", optional = true }
# sdec-bevy = { version = "0.1", optional = true }
```

---

## 8. Kaynaklar

### postcard Optimizasyon
- [postcard ser_flavors](https://docs.rs/postcard/latest/postcard/ser_flavors/index.html)
- [postcard-rpc](https://lib.rs/crates/postcard-rpc)
- [Postcard Wire Format](https://postcard.jamesmunns.com/wire-format)
- [COBS + CRC usage](https://github.com/jamesmunns/postcard/issues/117)

### bytemuck & GPU
- [bytemuck Pod](https://docs.rs/bytemuck/latest/bytemuck/trait.Pod.html)
- [encase](https://crates.io/crates/encase)
- [wgsl_bindgen](https://lib.rs/crates/wgsl_bindgen)
- [WGSL Memory Layout](https://webgpufundamentals.org/webgpu/lessons/webgpu-memory-layout.html)
- [GPU alignment](https://stackoverflow.com/questions/75522842/problem-with-aligning-rust-structs-to-send-to-the-gpu-using-bytemuck-and-wgpu)

### SDEC Delta Codec
- [sdec-repgraph](https://lib.rs/crates/sdec-repgraph)
- [sdec-bevy](https://crates.io/crates/sdec-bevy)

### Compression
- [Compression Algorithms Deep Dive](https://www.youngju.dev/blog/culture/2026-04-15-compression-algorithms-lz77-zstd-brotli-ans-huffman-deep-dive-guide-2025.en)
- [Rust compression comparison](https://github.com/hwjsnc/rust-compression-comparison)
- [zstd dictionary](https://facebook.github.io/zstd/zstd_manual.html)
- [Veloren Devblog 117](https://veloren.net/blog/devblog-117)

### Memory Management
- [bumpalo](https://docs.rs/bumpalo)
- [Arena allocator tips](https://nullprogram.com/blog/2023/09/27)
- [serde optimization with arenas](https://nickb.dev/blog/the-serde-optimization-gauntlet-wasm-and-arenas)
- [memmap2](https://docs.rs/memmap2)

### Struct Optimization
- [Rust struct field reordering](https://camlorn.net/posts/April%202017/rust-struct-field-reordering)
- [Cache line optimization](https://users.rust-lang.org/t/cache-line-optimization-methodology-for-structs/31118)
