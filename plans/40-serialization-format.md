# 40 — Serialization Format Kararı

> **Durum:** Araştırma tamamlandı, karar bekliyor
> **Tarih:** 2026-05-22
> **Kaynaklar:** Aşağıda her bölümde listelenmiştir

---

## 0. Karar Özeti

| Seçenek | Puan | En İyi Kullanım | Risk |
|---------|------|-----------------|------|
| **postcard** (network + save) + **bytemuck** (GPU) | **8.5/10** | Network, save/load, GPU upload | Düşük |
| rkyv (her şey için) | 7.0/10 | Read-heavy, zero-copy | Orta (alignment, schema evolution) |
| bincode 2.0 | 7.5/10 | Genel amaçlı binary | Düşük |
| flatbuffers | 6.0/10 | Cross-language, large data | Yüksek (karmaşıklık) |
| bitcode | 7.5/10 | En hızlı serialize | Düşük (yeni, küçük ekosistem) |

**Önerilen Strateji:** Hybrid — postcard (network + save/load) + bytemuck (GPU upload)

---

## 1. Kullanım Alanları ve Gereksinimler

Strata'da serialization 4 farklı alanda kullanılacak:

### 1.1 Network Replication (bevy_replicon)
- **Gereksinim:** Serde uyumlu, küçük boyut, hızlı serialize/deserialize
- **bevy_replicon:** `Serialize + Deserialize` trait'leri (serde) gerektirir
- **Veri tipi:** Entity state, component updates, events
- **Hedef:** 20-30Hz tick, 600 oyuncu, ~17 KB/s/oyuncu

### 1.2 Save/Load (Disk)
- **Gereksinim:** Hızlı, compact, güvenilir
- **Veri tipi:** Chunk data, world state, player data
- **Hedef:** Büyük world'ler (GB seviyesi), hızlı yükleme

### 1.3 GPU Upload (wgpu)
- **Gereksinim:** `#[repr(C)]`, alignment, zero-copy `&[u8]` dönüşümü
- **Veri tipi:** Vertex buffers, instance data, uniform buffers
- **Hedef:** Sıfır kopyalama, doğrudan `bytemuck::cast_slice`

### 1.4 Asset Loading (Block Registry, Config)
- **Gereksinim:** İnsan-okunabilir, mod desteği
- **Veri tipi:** Block tanımları, yapılandırma
- **Hedef:** Geliştirme kolaylığı, hot-reload

---

## 2. Format Karşılaştırması (Benchmark Verileri)

Kaynak: [rust_serialization_benchmark](https://github.com/djkoloski/rust_serialization_benchmark) (2026-04-24, AMD EPYC 7763)

### 2.1 Serialize / Deserialize Hızı (Log dataset)

| Format | Serialize | Deserialize | Boyut (raw) | Boyut (zstd) |
|--------|-----------|-------------|-------------|--------------|
| **bitcode** | **138.84 µs** | **1.4670 ms** | 703,710 | **227,322** |
| **wincode** | 174.76 µs | 1.7708 ms | 1,045,784 | 311,553 |
| **speedy** | 200.26 µs | 1.7527 ms | 885,780 | 286,248 |
| **rkyv** | 247.23 µs | 1.5516 ms (unvalidated) | 1,011,488 | 325,965 |
| **postcard** | 428.61 µs | 2.2860 ms | 724,953 | 252,968 |
| **bincode 2.0** | 336.98 µs | 2.1362 ms | 741,295 | 256,422 |
| **flatbuffers** | 1.0315 ms | † | 1,276,368 | 388,381 |
| **serde_json** | 3.8240 ms | 5.9020 ms | 1,827,461 | 360,727 |

### 2.2 Zero-Copy Erişim Hızı

| Format | Access | Read | Update |
|--------|--------|------|--------|
| **rkyv** | **1.25 ns** (unvalidated) | 10.43 µs | 7.63 µs |
| **flatbuffers** | 2.49 ns (unvalidated) | 51.87 µs | ‡ |
| **capnp** | 78.82 ns | 135.62 µs | ‡ |

### 2.3 Postcard vs Bincode Farkları

| Özellik | postcard | bincode 2.0 |
|---------|----------|-------------|
| Integer encoding | Varint (LEB128) | Configurable (fixed/varint) |
| 251-16383 arası | 2 byte | 3 byte |
| Array length | Varint | Varint (default) |
| no-std desteği | ✅ Tam | ⚠️ Kısmî |
| Wire format spec | ✅ Dokümante | ⚠️ Bazı boşluklar |
| Serde uyumluluğu | ✅ | ✅ |

Kaynak: [Postcard Wire Format](https://postcard.jamesmunns.com/wire-format), [Bincode vs Postcard](https://users.rust-lang.org/t/is-it-better-to-use-bincode-or-postcard/88740)

---

## 3. Ayrıntılı Format Analizi

### 3.1 postcard

**Artılar:**
- En compact serde format'ı (varint encoding sayesinde)
- no-std desteği (embedded uyumlu)
- Stabil wire format spec (v1.0+)
- Küçük integer'lar için çok verimli (1 byte < 128)
- Serde ekosistemi ile tam uyumlu
- bevy_replicon ile doğrudan çalışır
- Aktif geliştirme (James Munns, RustNL talks)

**Eksiler:**
- Zero-copy desteği yok (deserialization gerekli)
- Schema evolution yok (non-self-describing)
- Büyük data için rkyv kadar hızlı değil

**Kullanım:** Network replication, save/load, config dosyaları

### 3.2 rkyv

**Artılar:**
- Zero-copy deserialization (1.25 ns access!)
- Safe mutation (data'yı deserialize etmeden değiştirme)
- Hash map ve B-tree desteği
- Shared pointer desteği
- En hızlı read performansı

**Eksiler:**
- **Schema evolution YOK** — format değişirse eski kayıtlar okunamaz
- **Alignment sorunları** — `#[repr(Rust)]` padding'i farklı olabilir
- GPU upload için uyumsuz (bytemuck ile doğrudan kullanılamaz)
- Cross-language desteği yok
- Validation yavaş (354 µs vs 1.25 ns unvalidated)
- Serde'den ayrı bir trait sistemi (`Archive`, `Serialize`, `Deserialize`)

**Kritik Sorun — Alignment:**
rkyv kendi alignment kurallarını kullanır. `#[repr(C)]` struct'lar ile uyumsuzluk olabilir:
- GPU'ya veri göndermek için `bytemuck::Pod` trait'i gerekir
- `bytemuck::Pod` → `#[repr(C)]` + padding yok gerektirir
- rkyv → kendi `#[repr(C)]` benzeri formatı, ama alignment farklı olabilir
- **Çözüm:** rkyv archive'ı doğrudan GPU'ya gönderilemez, transform gerekir

Kaynak: [rkyv Alignment](https://rkyv.org/format/alignment.html), [StackOverflow GPU alignment](https://stackoverflow.com/questions/75522842/problem-with-aligning-rust-structs-to-send-to-the-gpu-using-bytemuck-and-wgpu)

**Kullanım:** Read-heavy cache layer (opsiyonel)

### 3.3 bytemuck (GPU için)

**Artılar:**
- Doğrudan `&[u8]` dönüşümü (`cast_slice`)
- `#[repr(C)]` ile tam uyumlu
- wgpu ile endüstri standardı
- Sıfır kopyalama
- Compile-time safety (`Pod` + `Zeroable`)

**Eksiler:**
- Sadece `#[repr(C)]` struct'lar
- Enum desteği yok
- String desteği yok
- Sadece fixed-size types

**Kullanım:** GPU buffer upload (vertex, instance, uniform)

### 3.4 bincode 2.0

**Artılar:**
- Olgun ve yaygın
- Serde uyumlu
- Configurable encoding
- İyi performans

**Eksiler:**
- postcard'dan biraz daha büyük
- Wire format spec'de bazı boşluklar
- no-std desteği kısmî

**Kullanım:** postcard alternatifi (eğer postcard sorun çıkarırsa)

### 3.5 flatbuffers

**Artılar:**
- Schema evolution desteği
- Cross-language
- Zero-copy erişim

**Eksiler:**
- En yavaş serialize (1.03 ms vs 138 µs)
- En büyük boyut (1.27 MB)
- Karmaşık API
- Rust desteği diğer diller kadar olgun değil
- bevy_replicon ile uyumsuz (serde gerektirir)

**Kullanım:** Cross-language senaryoları (gerektiğinde)

### 3.6 bitcode

**Artılar:**
- **En hızlı serialize** (138.84 µs)
- **En küçük zstd boyutu** (227,322 byte)
- İyi deserialize hızı

**Eksiler:**
- Yeni format (küçük ekosistem)
- Topluluk desteği az
- Uzun vadeli belirsizlik

**Kullanım:** Gelecek alternatif (izlenmeli)

---

## 4. Voxel-Specific Stratejiler

### 4.1 Palette-Based Chunk Compression

Minecraft ve Veloren'in kullandığı strateji:

| Yaklaşım | Oran | Açıklama |
|----------|------|----------|
| LZ4 | ~25% | Genel amaçlı, hızlı |
| Deflate | ~17% | Daha iyi sıkıştırma |
| Greyscale PNG | ~1-2% | Gameplay data için en iyi |
| Palette + RLE | ~5-10% | Block ID'leri için ideal |

**Veloren bulguları:**
- Subchunk'lar 32×32×16, adjacent block'lar deduplique
- PNG delta-encoding 2D spatial locality yakalıyor
- "Flipping every odd z-level across the y-axis improves spatial locality"

Kaynak: [Veloren Devblog 117](https://veloren.net/blog/devblog-117), [Voxel.Wiki Palette Compression](https://voxel.wiki/wiki/palette-compression)

### 4.2 XBrickMap Verisi için Önerilen Strateji

```
┌─────────────────────────────────────────────────────┐
│  Layer 1: In-Memory (ECS Component)                 │
│  ├── XBrickMap<Sector> → Bevy component             │
│  ├── Palette-based encoding (block ID → index)      │
│  ├── Bit-packed indices (4/8/16 bit per voxel)      │
│  └── Hot sectors → decompressed, cold → compressed  │
├─────────────────────────────────────────────────────┤
│  Layer 2: Network (bevy_replicon)                   │
│  ├── postcard ile serialize                         │
│  ├── Palette + delta encoding (sadece değişen)      │
│  ├── Deflate sıkıştırma (ağ için)                   │
│  └── ~1-5% ratio (palette sayesinde)                │
├─────────────────────────────────────────────────────┤
│  Layer 3: Disk (Save/Load)                          │
│  ├── postcard ile serialize                         │
│  ├── Palette + RLE + zstd sıkıştırma                │
│  ├── ~2-5% ratio                                    │
│  └── Region file format (Minecraft tarzı)           │
├─────────────────────────────────────────────────────┤
│  Layer 4: GPU (Render)                              │
│  ├── bytemuck ile vertex buffer upload              │
│  ├── Mesh data: #[repr(C)] Vertex { pos, normal }  │
│  ├── Instance data: #[repr(C)] Instance { matrix }  │
│  └── Sıfır kopyalama, doğrudan GPU'ya              │
└─────────────────────────────────────────────────────┘
```

---

## 5. Hybrid Strateji Detayı

### 5.1 Katmanlar

| Katman | Format | Crate | Neden |
|--------|--------|-------|-------|
| **Network** | postcard | `postcard` + `serde` | Serde uyumlu, compact, bevy_replicon ile çalışır |
| **Save/Load** | postcard + zstd | `postcard` + `zstd` | Hızlı, compact, güvenilir |
| **GPU Upload** | bytemuck | `bytemuck` | Sıfır kopyalama, wgpu standardı |
| **Config/Assets** | TOML | `toml` | İnsan-okunabilir, mod desteği |
| **Cache** | postcard | `postcard` | Hızlı, memory-efficient |

### 5.2 Tip Dönüşümleri

```rust
// Network → postcard
let bytes: Vec<u8> = postcard::to_stdvec(&chunk_data)?;

// GPU → bytemuck
let vertices: &[u8] = bytemuck::cast_slice(&mesh.vertices);
queue.write_buffer(&vertex_buffer, 0, vertices);

// Save → postcard + zstd
let bytes = postcard::to_stdvec(&world_state)?;
let compressed = zstd::encode_all(&bytes[..], 3)?;

// Load → zstd + postcard
let decompressed = zstd::decode_all(&compressed[..])?;
let world_state: WorldState = postcard::from_bytes(&decompressed)?;
```

### 5.3 GPU Upload için Tip Tasarımı

```rust
// GPU'ya gönderilecek tipler bytemuck uyumlu olmalı
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct InstanceData {
    model_matrix: [[f32; 4]; 4],
    color: [f32; 4],
}

// Network'e gönderilecek tipler serde uyumlu olmalı
#[derive(Serialize, Deserialize)]
struct ChunkUpdate {
    sector_id: u64,
    changes: Vec<BlockChange>,
    timestamp: u64,
}
```

---

## 6. Risk Analizi

### 6.1 Düşük Risk

| Risk | Olasılık | Etki | Çözüm |
|------|----------|------|-------|
| postcard boyut yetersiz | Düşük | Orta | zstd sıkıştırma ekle |
| bytemuck alignment sorunu | Düşük | Yüksek | #[repr(C)] + padding |

### 6.2 Orta Risk

| Risk | Olasılık | Etki | Çözüm |
|------|----------|------|-------|
| Schema evolution ihtiyacı | Orta | Yüksek | Version field ekle, migration |
| Performans darboğazı | Orta | Orta | Profil yap, hot path'leri optimize |

### 6.3 Yüksek Risk

| Risk | Olasılık | Etki | Çözüm |
|------|----------|------|-------|
| rkyv alignment ile GPU uyumsuzluk | Yüksek | Yüksek | **rkyv kullanma, bytemuck tercih et** |

---

## 7.компоновка (Crate Listesi)

```toml
[dependencies]
# Network + Save/Load serialization
postcard = { version = "1.1", features = ["alloc"] }
serde = { version = "1.0", features = ["derive"] }

# GPU upload
bytemuck = { version = "1.19", features = ["derive"] }

# Config dosyaları
toml = "0.8"

# Sıkıştırma (save/load + network)
zstd = "0.13"

# (Opsiyonel) Zero-copy cache layer
# rkyv = "0.8"  # Sadece gerekirse
```

---

## 8. Karar ve Gerekçe

### **Karar: postcard + bytemuck (Hybrid)**

**Neden postcard?**
1. **bevy_replicon uyumluluğu:** Serde trait'leri gerektirir, postcard serde ile çalışır
2. **En compact serde format:** Varint encoding sayesinde küçük integer'lar 1 byte
3. **Stabil wire format spec:** v1.0+ dokümante
4. **no-std desteği:** Gelecekte embedded/server ayrımı için esneklik
5. **Aktif geliştirme:** James Munns, RustNL talks, postcard-rpc ekosistemi

**Neden bytemuck?**
1. **wgpu standardı:** Endüstri standardı GPU buffer upload
2. **Sıfır kopyalama:** `cast_slice` ile doğrudan `&[u8]`
3. **Compile-time safety:** `Pod` + `Zeroable` derive
4. **#[repr(C)] ile uyumlu:** GPU alignment garantisi

**Neden rkyv DEĞİL?**
1. **Schema evolution yok:** World save formatı değişecek, eski kayıtlar okunamaz
2. **GPU uyumsuzluğu:** rkyv archive'ı doğrudan GPU'ya gönderilemez
3. **bevy_replicon uyumsuzluğu:** serde gerektirir, rkyv farklı trait sistemi
4. **Alignment riski:** `#[repr(Rust)]` padding'i farklı olabilir

**Neden flatbuffers DEĞİL?**
1. **Çok yavaş serialize** (1.03 ms vs 138 µs)
2. **Karmaşık API**
3. **bevy_replicon ile uyumsuz**

---

## 9. Kaynaklar

### Benchmarklar
- [rust_serialization_benchmark](https://github.com/djkoloski/rust_serialization_benchmark) — Kapsamlı Rust serialization benchmarkları
- [rkyv benchmarks](https://david.kolo.ski/blog/rkyv-is-faster-than) — rkyv yazarının benchmarkları
- [FlatBuffers benchmarks](https://flatbuffers.dev/benchmarks) — FlatBuffers resmi benchmarkları

### Format Dokümantasyonu
- [Postcard Wire Format](https://postcard.jamesmunns.com/wire-format) — Postcard format spec
- [rkyv Feature Comparison](https://rkyv.org/feature-comparison.html) — rkyv vs Cap'n Proto vs FlatBuffers
- [rkyv Alignment](https://rkyv.org/format/alignment.html) — rkyv alignment kuralları
- [bytemuck Pod](https://docs.rs/bytemuck/latest/bytemuck/trait.Pod.html) — GPU buffer trait

### Voxel-Specific
- [Veloren Devblog 117](https://veloren.net/blog/devblog-117) — Chunk compression stratejileri
- [Voxel.Wiki Palette Compression](https://voxel.wiki/wiki/palette-compression) — Palette encoding detayları
- [Voxel Terrain Storage](https://zeux.io/2017/03/27/voxel-terrain-storage) — Row-packed format

### Tartışmalar
- [Rust Forum: bincode vs postcard](https://users.rust-lang.org/t/is-it-better-to-use-bincode-or-postcard/88740)
- [HN: rkyv is faster](https://news.ycombinator.com/item?id=26428812)
- [Rust Forum: serialization formats](https://users.rust-lang.org/t/overwhelmed-by-the-vast-variety-of-serialization-formats-which-to-use-when/88440)
- [StackOverflow: GPU alignment](https://stackoverflow.com/questions/75522842/problem-with-aligning-rust-structs-to-send-to-the-gpu-using-bytemuck-and-wgpu)

### bevy_replicon
- [bevy_replicon docs](https://docs.rs/bevy_replicon/latest/bevy_replicon)
- [bevy_replicon GitHub](https://github.com/simgine/bevy_replicon)

---

## 10. Sonraki Adımlar

1. ~~Serialization format kararı~~ → **Bu dosya**
2. **ECS storage strategy** → plans/konusulacak.md #8
3. **Physics engine** → plans/konusulacak.md #5
4. **Asset format** → plans/konusulacak.md #6
5. **Build strategy** → plans/konusulacak.md #7
