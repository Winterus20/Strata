# Konuşulacak Temel Kararlar

Bevy ECS + wgpu mimarisi kesinleşti. Aşağıdaki konular henüz karara bağlanmadı.

---

## 1. Memory Allocator

**Durum:** ✅ ÇÖZÜLDÜ — Detaylı analiz: `plans/39-memory-allocation.md`

**Karar:** mimalloc v3 global allocator + hybrid per-subsystem strateji

**Neden mimalloc?**
- Windows + Linux uyumlu (jemalloc Windows MSVC'de çalışmaz)
- %30-50 performans artışı (system allocator'a göre)
- Trivial entegrasyon (3 satır)
- Unreal Engine, Roblox, Xbox, CPython'da production'da kanıtlanmış
- Aktif geliştirme (Microsoft-backed)

**Hybrid strateji:**
- Global: mimalloc v3 (`#[global_allocator]`)
- Chunk data: `mi_heap_new/destroy` (O(1) bulk free)
- Network: per-connection heap (disconnect'te bulk free)
- Mesh generation: bumpalo arena (per-frame reset)
- Pathfinding: bumpalo (per-query drop)
- Network packets: slab pool (pre-alloc)
- GPU buffers: wgpu buffer pool (free-list)

---

## 2. Thread Model

**Durum:** ✅ ÇÖZÜLDÜ — Detaylı analiz: `plans/thread_model.md`

**Karar:** Advanced Hybrid (37/40) — Bevy task pool'ları + 9 ileri optimizasyon

**Mimari:**
- Main Thread: ECS schedule (deterministik kritik yol)
- ComputePool: Chunk gen, mesh gen, lighting BFS, pathfinding
- AsyncComputePool: SVDAG bake, save snapshots
- IoPool: Network I/O, disk I/O, asset loading

**9 Optimizasyon:**
1. Priority-based task scheduling (P0-P4)
2. Frame budget / time slicing (2ms chunk, 2ms mesh, 1.5ms light)
3. Tier-aware thread allocation (4-tier streaming ile uyumlu)
4. Predictive streaming integration
5. Double-buffered mesh upload
6. System set optimization (7 set, 200-300 sistem)
7. Adaptive thread pool sizing
8. Cache-aware chunk ordering
9. Graceful degradation

---

## 3. Network Protocol

**Durum:** ✅ ÇÖZÜLDÜ — Detaylı analiz: `plans/16-network-and-lag-compensation.md`

**Karar:** QUIC (Quinn/bevy_quinnet) + bevy_replicon + bevy_replicon_quinnet

**Neden QUIC?**
- TLS 1.3 zorunlu (güvenlik varsayılan)
- 0-RTT reconnect (disconnect → anında tekrar bağlanma)
- Stream multiplexing (256 kanal, HOL blocking yok)
- Tek UDP socket ile 600+ connection
- GSO/GRO optimizasyonu (Linux'de %97 daha az syscall)
- BBR congestion control (bufferbloat yok, %33 RTT azalması)
- RFC 9221 Datagram (unreliable game state için ideal)

**Neden bevy_replicon?**
- En olgun Bevy replication (v0.40, 1009 commit)
- Server-authoritative mimari (anti-cheat)
- Entity visibility API (AOI için temel)
- bevy_replicon_attributes ile sector bazlı filtreleme
- Backend-agnostik (QUIC/UDP/WebSocket)

**Karşılaştırma Sonucu:**

| Seçenek | Puan | Neden |
|---------|------|-------|
| **QUIC + bevy_replicon** | **7.9/10** | En dengeli: olgun, güvenli, esnek |
| Steam + QUIC fallback | 7.5/10 | En iyi deneyim ama karmaşık |
| renet2 + bevy_replicon | 6.5/10 | Basit ama riskli (tek bakımcı) |
| aeronet | 6.0/10 | En Bevy-native ama olgun değil |

**Kaynaklar:**
- [bevy_quinnet GitHub](https://github.com/Henauxg/bevy_quinnet)
- [bevy_replicon GitHub](https://github.com/simgine/bevy_replicon)
- [bevy_replicon_quinnet](https://github.com/Henauxg/bevy_replicon_quinnet)
- [Quinn GitHub](https://github.com/quinn-rs/quinn)
- [RFC 9221 - QUIC Datagrams](https://www.rfc-editor.org/rfc/rfc9221.html)

---

## 4. Serialization Format

**Durum:** ✅ ÇÖZÜLDÜ — Detaylı analiz: `plans/40-serialization-format.md`
**Optimizasyon:** `plans/41-serialization-optimization.md`

**Karar:** postcard (network + save/load) + bytemuck (GPU upload)

**Neden postcard?**
- bevy_replicon serde gerektirir → postcard serde ile çalışır
- En compact serde format (varint encoding, 1 byte < 128)
- Stabil wire format spec (v1.0+)
- no-std desteği
- Flavor sistemi (COBS, CRC) ile framing + checksum

**Neden bytemuck?**
- wgpu endüstri standardı GPU buffer upload
- `cast_slice` ile sıfır kopyalama
- Compile-time safety (`Pod` + `Zeroable` derive)
- `#[repr(C)]` ile alignment garantisi

**Neden rkyv DEĞİL?**
- Schema evolution yok → eski world save'ler okunamaz
- GPU uyumsuzluğu → rkyv archive doğrudan GPU'ya gönderilemez
- bevy_replicon uyumsuz → serde değil, kendi trait sistemi

**Optimizasyonlar (opsiyonel):**
- SDEC delta codec (4.3× daha küçük packet)
- zstd dictionary (%15-25 ek sıkıştırma)
- bumpalo arena (O(1) bulk free)
- bytemuck struct field reordering (%20-33 memory azalma)

**Kaynaklar:**
- [postcard docs](https://docs.rs/postcard)
- [bytemuck docs](https://docs.rs/bytemuck)
- [sdec-repgraph](https://lib.rs/crates/sdec-repgraph)

---

## 5. Physics Engine

**Durum:** ✅ ÇÖZÜLDÜ — Detaylı analiz: `plans/42-physics-engine.md`

**Karar:** Rapier (rapier3d) + bevy_rapier3d + custom voxel collision (XBrickMap ile)

**Seçenekler:**

| Yaklaşım | Hazır | Voxel Support | Performans | Determinizm |
|----------|-------|--------------|-----------|-------------|
| Rapier (genel amaçlı) | ✅ | ⚠️ Ek entegrasyon | İyi | ✅ enhanced-determinism |
| Custom voxel physics | ❌ | ✅ Tam voxel-aware | En iyi | ✅ Senin kontrolün |
| parry (sadece collision) | ⚠️ | ⚠️ | İyi | ✅ |
| Rapier + custom voxel collision | ✅ | ✅ Hibrit | İyi | ✅ |

**Neden kritik?**
- Rapier genel amaçlı → voxel dünyası için özel collision gerekli
- XBrickMap ile collision detection (O(1) block lookup)
- Falling sand, fluid interaction gibi voxel-specific fizik
- Destruction (Voronoi fracture) voxel-aware olmalı

**Öneri:** Rapier (karakter controller, rigid body) + custom voxel collision (XBrickMap ile)

**Kaynaklar:**
- [Rapier voxel integration](https://rapier.rs/docs/user_guides/rust/voxel_worlds)
- [bevy_rapier voxel example](https://github.com/dimforge/bevy_rapier)

---

## 6. Asset Format

**Durum:** ✅ ÇÖZÜLDÜ

**Karar:** RON (Rusty Object Notation)

**Neden RON?**
- Bevy ekosisteminin favorisi (Reflect API ve Serde ile kusursuz entegrasyon).
- Blokların karmaşık, iç içe geçmiş özelliklerini ve Rust enum'larını ifade etmede TOML'dan çok daha yetenekli.
- Yorum satırlarını destekler, mod yapımcıları için idealdir.
- Not: Save/Load ve Network cache için zaten 4. maddede (Postcard) karar verilmiştir, bu format sadece okunabilir/düzenlenebilir asetler (block registry, config) için kullanılacaktır.

**Kaynaklar:**
- [RON format](https://github.com/ron-rs/ron)

---

## 7. Build Strategy

**Durum:** ✅ ÇÖZÜLDÜ

**Karar:** Hybrid Cargo Workspace

**Yapı:**
- Ortak mantık, ECS ve network paketleri `crates/` altındaki paylaşılan kütüphanelerde (library crate) tutulur.
- Pencereli oyun istemcisi `bin/client` altında (render + input dahil) ayrı bir binary olarak derlenir.
- Sunucu uygulaması `bin/server` altında (GPU ve render hariç, tamamen headless) ayrı bir binary olarak derlenir.

**Avantajları:**
- Hem client hem server aynı blok ve ağ paketini kullanır (sıfır kod tekrarı).
- Paralel derleme (compile time) hızlanır.
- Sunucu uygulaması olabilecek en düşük bellek ve işlemciyle çalışır.

**Kaynaklar:**
- `plans/44-build-strategy.md`

---

## 8. ECS Storage Strategy

**Durum:** ✅ ÇÖZÜLDÜ

**Karar:** Hybrid — Table (default) + SparseSet + ECS dışı özel veri yapıları

**Neden Hybrid?**
- Bevy sadece 2 storage destekler: Table (archetype) ve SparseSet
- Flat Array / custom storage Bevy'de yok — XBrickMap bunu ECS dışı yapıyor
- Table: hızlı iterasyon, yavaş ekleme/çıkarma (~0.5-2 ns/entity iterasyon)
- SparseSet: hızlı ekleme/çıkarma, yavaş iterasyon (~2-5 ns/entity ekleme)
- Eurographics 2024 benchmark: %5-10 üstünde değiştirme oranında SparseSet kazanır

**Uygulama:**

| Component | Storage | Neden |
|-----------|---------|-------|
| `SectorPosition` | Table (default) | Her entity'de var, nadiren değişir |
| `SectorData` | Table (default) | Core data, meshing için iterasyon |
| `SectorMeshState` | Table (default) | Version tracking |
| `ChunkDirty` | SparseSet | Sık eklenir/kaldırılır |
| `NeedsRemesh` | SparseSet | Her blok değişikliğinde |
| `NeedsColliderUpdate` | SparseSet | Geçici flag |
| `TierChange` | SparseSet | Nadir, geçici event marker |
| Blok verisi | ECS dışı | XBrickMap + GlobalBrickPool |

**Ek optimizasyonlar (Plan 03'e işlendi):**
- Hot/Cold data split
- ZST marker component'lar
- Immutable component'lar (Bevy 0.16+)
- Disabled component (uzak chunk'lar)
- ComponentHooks ile otomatik spatial index
- Block Entity pattern (per-block entity felaketi önlemi)

**Risk: Bevy SparseSet'i kaldırırsa (#19164):** Düşük risk. `Option<ChunkDirty>` veya `HashSet<Entity>` Resource ile migration kolay.

---

## Karar Sırası (Önerilen)

1. ~~**Memory allocator** → mimalloc (hemen, 1 gün)~~ ✅ Çözüldü
2. ~~**Thread model** → Advanced Hybrid~~ ✅ Çözüldü
3. ~~**ECS storage strategy** → Hybrid~~ ✅ Çözüldü
4. ~~**Serialization** → postcard + bytemuck~~ ✅ Çözüldü
5. ~~**Network protocol** → QUIC + bevy_replicon~~ ✅ Çözüldü
6. ~~**Physics engine** → Rapier + custom voxel collision (1 hafta)~~ ✅ Çözüldü
7. ~~**Asset format** → RON (Rusty Object Notation)~~ ✅ Çözüldü
8. ~~**Build strategy** → Hybrid Cargo Workspace~~ ✅ Çözüldü

---

## Ne Zaman Karar Verilmeli?

| Karar | Ne Zaman | Neden |
|-------|----------|-------|
| ~~Memory allocator~~ | ~~**Şimdi**~~ | ✅ Çözüldü → `plans/39-memory-allocation.md` |
| ~~Thread model~~ | ~~**Şimdi**~~ | ✅ Çözüldü → `plans/thread_model.md` |
| ~~ECS storage~~ | ~~**Şimdi**~~ | ✅ Çözüldü — Hybrid (Table + SparseSet + ECS dışı) |
| ~~Serialization~~ | ~~Implementasyon başlamadan~~ | ✅ Çözüldü → `plans/40-serialization-format.md` |
| ~~Network protocol~~ | ~~Faz 4 (network) öncesi~~ | ✅ Çözüldü → `plans/16-network-and-lag-compensation.md` |
| ~~Physics engine~~ | ~~Faz 1 (physics) öncesi~~ | ✅ Çözüldü → `plans/42-physics-engine.md` |
| ~~Asset format~~ | ~~Faz 6 (gameplay) öncesi~~ | ✅ Çözüldü — RON seçildi |
| ~~Build strategy~~ | ~~**Şimdi**~~ | ✅ Çözüldü — Hybrid Workspace seçildi |
