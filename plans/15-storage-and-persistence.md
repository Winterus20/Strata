# 15 — Depolama Sistemi

> **Durum:** Kesinleşmiş (anayasa `01`–`15`, `AGENTS.md` §2). Bu sürüm 2026-07-06 teknik
> denetiminden (6 paralel alt-ajan: kademeli depolama, SQLite/KV, bölge dosyası/dedup,
> Windows async I/O, CDC/Merkle, save/cloud) gelen bulgularla revize edilmiştir. 2026-07-07'de
> ikinci bir 6-alt-ajan web-araştırması denetimi (bkz. `researchs/audit-storage-plan15.md`) ile
> P0/P1 düzeltmeleri entegre edilmiştir (fjall birincil, dirty-queue snapshot, WAL sıralaması,
> buffered I/O varsayılanı, sıkıştırma oranı hedefleri, atomic-write sırası).
> Tüm değişiklikler `01`–`15` anayasasıyla uyumludur.

---

## 1. Depolama — Hybrid Tiered Storage

### 1.1 Genel Bakış

Strata, **3-kademeli hybrid depolama mimarisi** kullanır. Streaming tier'ları ile depolama
tier'ları *hizalıdır* ama **1:1 çakıştırılmamıştır** (bkz. §1.1.4 — Kalıcılık Kararı Ayrıştırma).

```
┌──────────────────────────────────────────────────────────────────────┐
│                    HYBRID TIERED STORAGE                             │
├──────────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 1: In-Memory (ACTIVE)                                │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │  XBrickMap (doğrudan erişim, O(1))                   │    │    │
│  │  │  ├── Dirty tracking (sticky atomic, sinyal)          │    │    │
│  │  │  └── Object pool: SlotMap/slab (GC churn yok)        │    │    │
│  │  └──────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                  │                                     │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 2: LRU Compressed Cache (WARM)                       │    │
│  │  ┌──────────────────────────────────────────────────────┐    │    │
│  │  │  ~512 MB byte bütçeli (moka W-TinyLFU / quick_cache) │    │    │
│  │  │  zstd level 1 (hız öncelikli) — blob olarak saklanır │    │    │
│  │  │  Write-back (lazy flush)                             │    │    │
│  │  │  └── Async background flush (tek runtime + priority) │    │    │
│  │  └──────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────┘    │
│                                  │                                     │
│  ┌──────────────────────────────────────────────────────────────┐    │
│  │  KATMAN 3: Persistent Storage (DISTANT + ARCHIVE)            │    │
│  │  ┌──────────────────────────┐ ┌──────────────────────────┐   │    │
│  │  │  Region Files (.strata)  │ │  Metadata Store (redb)   │   │    │
│  │  │  32×32×32 sector grupları│ │  ┌────────────────────┐  │   │    │
│  │  │  zstd level 3 / 19       │ │  │ sector_metadata   │  │   │    │
│  │  │  Content-addressable     │ │  │ gc_candidates     │  │   │    │
│  │  │  deduplication           │ │  │ world_config      │  │   │    │
│  │  │  └── unbuffered I/O     │ │  └────────────────────┘  │   │    │
│  │  └──────────────────────────┘ └──────────────────────────┘   │    │
│  └──────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────┘
```

**Neden açık (explicit) bir WARM katmanı?** Endüstri standardı 2 katmandır (RAM ↔ disk);
OS/DB sayfa önbelleği "decompress-edilmiş ama düzenlenemez" bandı örtülü sağlar. Strata'da
WARM **açıkça** tutulur çünkü:
1. ACTIVE↔WARM promosyonu O(1) ve lock-free olmalı (GPU-resident streaming bandı ile hizalı).
2. Bir sektör DISTANT'a düştüğünde yeniden sıkıştırma yapılmamalı (WARM zaten sıkıştırılmış blob tutar).
3. 200 MB/s GPU stream bütçesi altında OS cache kontrolsüzdür.

Bu, Minecraft/Bedrock/Minetest'ten *daha* modern bir tasarımdır; "industry standard" değil,
**2-katmanlı modelin GPU-LOD streaming için ayarlanmış üst kümesidir**.

#### 1.1.1 L2 Cache — elle LRU yerine `moka` / `quick_cache`
Sabit 500-sektör sayısı bellek ayak izini öngörülemez yapar (boş sektör <1 KB, zengin sektör onlarca
KB). Öneri:
- **`moka`** (async, W-TinyLFU, tokio uyumlu, `weigher` ile byte-tabanlı kapasite) veya
  **`quick_cache`** (sync, **S3-FIFO** — CLOCK-Pro'nun değiştirilmiş versiyonu). Hit-rate Zipfian'da
  W-TinyLFU LRU'ye göre tipik **%10–30** iyileştirir (trace'ye bağlı; "94% vs 82%" tek bir sentetik
  trace'e aittir, garantili sonuç değildir). S3-FIFO tarama-dirençli ve yüksek throughput'lu olduğundan
  WARM için varsayılan önerilir; moka yalnız profiling gerçek bir hit-rate açığı gösterirse fallback.
- Kapasite **byte bütçesi** (`maximum_weight`) olsun (örn. 128–512 MB cihaza göre).
- WARM **sıkıştırılmış `Vec<u8>`/`Bytes` blob** tutar; çıkarma O(1), yeniden sıkıştırma yok.
- Promosyon gecikmesi sorun olursa WARM için zstd-1 yerine **LZ4** (~3850 MB/s decompress) düşünülü;
  zstd-3 (DISTANT) ve zstd-19 (ARCHIVE) korunur.

#### 1.1.2 L1 Object Pool — SlotMap/slab
`GlobalBrickPool` = **SlotMap + SecondaryMap** (O(1) alloc/free, stabil anahtar, generational index
→ askıda referans yakalanır). `bumpalo` bir obje havuzu **değildir** (arena'dır, tek brick'i serbest
bırakamaz); yalnızca geçici scratch (mesh gen, pathfinding) için `39-memory-allocation.md` ile hizalı
kullanılır.

#### 1.1.3 Dirty Tracking — `atomic<bool>` yalnızca sinyaldir
 `AtomicBool` ucuz bir bildirimdir, doğruluk ilkesi **değildir**. Sözleşme:
- Flag **sticky** (yalnızca writer `store(true, Release)`); flusher yalnızca kalıcı commit sonrası
  `store(false, Release)` yapar. Erken temizleme (lost dirty) yasaktır.
- `Sector`/`Arc<Sector>` senkronize nesnedir (per-sector lock veya COW `Arc` swap); flag veri koruma
  yapmaz. Flush anında **canlı versiyon pool'dan çözülerek** (lock altında kopya / versioned `Arc`)
  tutarlı snapshot alınır.
- **Dirty queue sektör KOORDİNATI (veya çözülebilir handle) tutar**, snapshot `Arc` değil. Böylece
  COW `Arc` + "flag çift-enqueue'i engeller" deseninin en güncel editi kaybetme bug'ı elenir
  (eski `Arc` kuyrukta kalır, yeni edit asla flush edilmezdi).
- False sharing önlemek için dirty flag'leri **sharded atomic bitset** (`AtomicU64` ile 64 flag
  paketlenir), hot brick verisinden ayrı hizalanmış diziye konur.
- **Kurtarma kaynağı RAM flag'i değil, Metadata Store'un durable `dirty` kolonudur** (ayrı bir
  `dirty_log` WAL'ı gereksizdir — üçlü temsil sıralama bug'ı yaratır). Sıralama invariant'ı:
  *durable region+metadata commit → (opsiyonel log) → `flag.store(false, Release)`*; flag asla
  durable write'dan önce temizlenmez.
- **WARM tanımı:** WARM katmanı **sıkıştırılmış `Vec<u8>`/`Bytes` blob** tutar (region formatını
  yansıtır); §1.4'teki `load_sector` `Arc<Sector>` cache'i ACTIVE/canlı resident set'tir, WARM
  değildir. ACTIVE tek canlı katmandır; WARM sıkıştırılmış eviction/staging tamponudur.

#### 1.1.4 Kalıcılık Kararı Ayrıştırma (streaming tier ≠ storage tier)
Streaming tier = render/LOD yerleşimi (geçici, hysteresis'li). Storage tier = dayanıklılık +
sıkıştırma (dirtylik, düzenleme geçmişi, yeniden-üretilebilirlik ile sürülür). Bu yüzden:
- **Pristine (düzenlenmemiş, seed'den yeniden üretilebilir) sektörün RAM'dan çıkarılması → disk'e
  YAZILMAZ.** En büyük I/O tasarrufu budur; WAL `dirty` biti bu kararı verir.
- **Dirty sektörün çıkarılması → L3'e flush** (DISTANT veya ARCHIVE fark etmez; yalnızca sıkıştırma
  seviyesi farklı: zstd-3 vs zstd-19).
- "regenerable vs persisted" üçüncü boyutu birinci sınıf karar olarak modellenir (metadata store
  `dirty` alanı zaten mevcut).
- WARM cache bir RAM bütçesidir; streaming WARM halkası (96–384 m) ile aynı sayaç olmak zorunda
  değildir — ACTIVE'den düşenleri diske dokunmadan absorbe eder.

---

### 1.2 Region File Formatı

> **Düzeltme (denetim C1/C2):** Orijinal `32×32×1` + `I16Vec2` bölge + `r.0.0.strata` formatı
> `I16Vec3` koordinatla "sınırsız yükseklik" anayasasıyla (plan 06/11) çelişiyordu. 3D bölgeye
> geçiş *zorunluluk* değil, **uniform sınırsız-Y adresleme + ±1 komşu okuma kolaylığı** tercihidir
> (Anvil/CubicChunks zaten kolon başına Y tutar; "2D'nin Y anahtarı yok" iddiası yanlıştır).
> Aşağıdaki revizyon bu gerekçeyle yapılmıştır.

```
r.<rx>.<ry>.<rz>.strata   (3D bölge: 32×32×32 = 32768 sector)
┌────────────────────────────────────────────────────────┐
│ Header (aligned, read-only)                             │
│ ├── Magic: "STRT" (4B)                                │
│ ├── Version: u16                                      │
│ ├── Flags: u16 (compression, dedup, encryption)       │
│ ├── Region coord: I32Vec3 (12B)                       │
│ ├── Sector offsets: [u64; 32768]                      │
│ ├── Sector sizes:   [u64; 32768]                      │
│ └── Sector hashes:  [u64; 32768] (integrity/lookup)   │
├────────────────────────────────────────────────────────┤
│ Dedup Table (değişken)                                 │
│ └── Content-addressable hash → offset (BLAKE3, 32B)   │
├────────────────────────────────────────────────────────┤
│ Sector Payloads (değişken boyut, aligned window)       │
│ ├── SectorHeader (40B, bkz. aşağı)                    │
│ ├── XBrickMap slab data (zstd, frame checksum)        │
│ └── SVDAG subtree (opsiyonel, zstd)                   │
└────────────────────────────────────────────────────────┘
```

**SectorHeader (40B):**
```
coord:        I32Vec3  (12B)  — sınırsız Y için i32 (plan 11 ile uyumlu)
timestamp:    u64      (8B)
flags:        u16      (2B)
content_hash: [u8;32]  (32B)  — BLAKE3, DEDUP ANAHTARI (bkz. §1.3)
checksum:     u64      (8B)   — xxHash64/CRC64, SADECE bitrot integrity
```
- `content_hash` (BLAKE3) ve `checksum` (xxHash64) **farklı algoritmalar**; biri dedup anahtarı,
  diğeri accidental corruption tespiti. Asla tek 64-bit hash ikisi için birden kullanılmaz.
- `content_hash` eski `u64` (xxHash64) hali **kaldırıldı** — çakışma riski (birthday bound ~2³²'de
  %40) sessiz veri bozulmasına yol açar.
- `compression_id` açıkça `flags`'te; okuyucu versiyon evrimi yapabilir.

**mmap:** Denetim §1.4'teki karar geçerlidir — **mmap kullanılmaz** (page fault async thread'i
 bloklar, Windows'ta MMF Cache Manager'dan geçmez). §1.2'deki eski "mmap (sadece read)" notu
 **kaldırıldı**. Yalnızca immutable ARCHIVE pack'leri için, SIGBUS korumasıyla ve read-only olarak
 sınırlı kullanılabilir (önerilmez).

#### 1.2.1 Denetim 2026-07-07 — Header Boyutu ve Tek Otoriter İndeks
 Üç `[u64; 32768]` dizisi toplam **768 KB/bölge** (256 KB × 3) kaplar; çoğu slot boşken bile ödenir
 (1000 açık bölge → ~0.75 GB yalnız header). Ayrıca `sector_metadata` (§1.5) zaten
 `file_offset, payload_size, content_hash` tuttuğundan bölge header'ı **redb/fjall ile çift
 kaynaktır**. Karar: **redb/fjall tek otoriter indeks** olsun; bölge dosyası payload blob + kompakt
 **trailer** (presence bitmap + yalnız mevcut sektörler için dense kayıt) olarak indirilsin, trailer
 yalnız crash recovery/doğrulama için kullanılsın.

---

### 1.3 Content-Addressable Deduplication

> **Düzeltme (denetim C4):** Dedup anahtarı `xxHash64` (u64) → **BLAKE3 (`[u8;32]`)**. Ref-count'lar
> metadata store'da transaction içinde tutulur; in-memory tablo yalnızca cache'tir.

```rust
// Dedup anahtarı = BLAKE3 (strong, collision-safe).
// Integrity checksum = ayrı xxHash64/CRC64 (accidental bitrot only).
pub struct DedupTable {
    // İkincil cache; otorite metadata store'dadır (transaction içinde ref_count).
    index: HashMap<[u8; 32], u64>,
}

impl DedupTable {
    pub fn store_sector(
        &mut self,
        store: &mut MetadataStore,   // redb/fjall transaction
        coord: SectorCoord,
        payload: &[u8],
    ) -> Result<u64> {
        let hash: [u8; 32] = blake3::hash(payload).into();

        if let Some(&offset) = self.index.get(&hash) {
            store.inc_refcount(&hash)?;   // atomik UPDATE ref_count = ref_count + 1
            return Ok(offset);
        }

        let offset = region.append_payload(payload)?;
        store.insert_dedup(&hash, offset)?;   // content_hash PK + ref_count = 1
        self.index.insert(hash, offset);
        Ok(offset)
    }

    // Azaltma / GC: ref_count 0 -> payload sil (bkz. §1.8).
    pub fn release_sector(&mut self, store: &mut MetadataStore, hash: &[u8; 32]) -> Result<()> {
        if store.dec_refcount(hash)? == 0 {
            let offset = self.index.remove(hash).unwrap();
            region.free_payload(offset)?;
        }
        Ok(())
    }
}
```

**Beklenen tasarruf:** Sabit blok (whole-sector) dedup ile tekrarlayan geometri için **~%10–40**
 (yalnız **persisted/dirty baytlara** scoped; pristine sektörler §1.1.4 gereği diske yazılmaz). Hava
 sektörleri tek bir collapse ile bedava kazanılır; voxel gerçekliği çoğunlukla ~%0'dır (plan §1.9
 tablosu dürüst rakamdır). "%30–60" dünya boyutuna değil dirty set'e aittir; dedup+GC karmaşıklığı
 küçük dirty set için kazançtan ağır basabilir — değerlendirme gerekebilir.

**Güvenlik:** Dedup yeniden kullanımında payload hash'i **yeniden doğrulanır** (re-hash + eşitleme)
 → bu yalnız **bitrot**'u yakalar; gerçek BLAKE3 çarpışmasında (P≠Q, BLAKE3(P)==BLAKE3(Q)) re-hash
 check geçer ama yanlış veri döner. Asıl koruma **256-bit digest boyutudur** (2¹²⁸ collision
 resistance). İkincil ayırt edici olarak payload `length` eklenebilir. `content_hash` **sıkıştırılmış
 payload** üzerinde hesaplanmalı (aynı hava sektörleri aynı compressed blob'a → BLAKE3 capture eder).
 Bellek için primary yolda **16-byte (128-bit) BLAKE3** kullanılabilir (index yarıya iner), collision
 durumunda tam hash ikincil doğrulama.

---

### 1.4 Async I/O Stratejisi (Windows-optimize)

**mmap kullanılmaz** — page fault async thread'i bloklar, concurrency'yi tek-iş parçacığına düşürür
(Huon 2024). Windows'ta yeniden-okuma ağır voxel yükünde **buffered I/O varsayılan** alınmalı;
 **unbuffered I/O (`FILE_FLAG_NO_BUFFERING`) yalnız feature-flag arkası + benchmark ile**
 etkinleştirilmeli. Rastgele 4 KB okumada unbuffered ~22× regresyon (50 MB/s vs 1100 MB/s) yapabilir;
 voxel motoru aynı sektörleri sürekli yeniden okur (geri dönüş, LOD pop-in). Unbuffered yalnız
 seçici yazma yolunda değerlendirilmeli (kendi WARM cache'in varken OS cache ile çakışma yönetimi için).

> **Düzeltme (denetim §8):** İki ayrı runtime (`write_pool`/`read_pool`) over-engineering. Tek
> runtime + priority channel kullanılır.

```rust
pub struct AsyncStorageBackend {
    runtime:      tokio::runtime::Handle,   // TEK runtime
    flush_sched:  FlushScheduler,           // düşük öncelik, ayrı task
    prefetch:     PrefetchManager,          // hareket konisi (bkz. aşağı)
    read_priority: mpsc::Sender<LoadReq>,   // yüksek öncelik okuma
}

impl AsyncStorageBackend {
    pub async fn load_sector(&self, coord: SectorCoord) -> Result<Arc<Sector>> {
        if let Some(cached) = self.cache.get(&coord) {
             return Ok(cached);            // ACTIVE/canlı resident set (Arc<Sector>); WARM ≠ bu
         }
        // Yüksek öncelikli okuma; spawn_blocking yerine compio (IOCP) değerlendir.
        let data = tokio::task::spawn_blocking({
            let coord = coord;
            move || region.read_sector_aligned_window(coord)  // aligned window
        }).await??;

        let sector = Arc::new(self.decompress_and_deserialize(&data)?);
        self.cache.insert(coord, sector.clone());
        Ok(sector)
    }
}
```

**Windows I/O notları:**
- `read_sector_aligned_window`: `FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED`; offset/length/buffer
  **sector-aligned** (512B logical, 4KB physical — `IOCTL_STORAGE_QUERY_PROPERTY` ile probe).
  Değişken sıkıştırılmış payload için aligned **pencere** okunur, slice çıkarılır. `VirtualAlloc`
  ile 4KB-aligned buffer **yeniden kullanılır** (per-thread ring).
- **Runtime:** tek IOCP; write'lar `THREAD_PRIORITY_BELOW_NORMAL`'a pinlenir, read'lar normal.
  İki runtime yerine priority channel. `compio` (Windows IOCP native) gerçek async dosya I/O için
  değerlendirilebilir; aksi halde Bevy `AsyncComputeTaskPool` (tokio-backed) kullanılır, ikinci
  runtime eklenmez.
- **Prefetch (denetim §8.5 düzeltmesi):** `load_sector` içinde `prefetch.enqueue(coord)` YANLIŞ —
  bu şimdi yüklenecek coord. Doğru:
  ```rust
  impl PrefetchManager {
      pub fn update(&mut self, cam_pos: Vec3, cam_vel: Vec3) {
          for k in 1..=N {
              let future = cam_pos + cam_vel * (k as f32) * DT;
              let coord = sector_coord(future);
              if self.in_flight.insert(coord) {  // dedup
                  self.enqueue_low_priority(coord);
              }
          }
      }
  }
  ```
  Hareket konisi + GPU visibility feedback (plan 08 §5) öncelik boost + frustum cull eviction.

  > **Düzeltme (denetim 2026-07-07):** Prefetch'in **birincil sürücüsü GPU visibility/frustum**
  > (plan 08 §5) olmalı; `cam_vel` konisi yalnız smoothing + teleport guard + bounded dedup ile
  > ikincildir. Hız ≠ bakış yönü (oyuncu yana bakıp ilerleyebilir); ham velocity jitter/thrash
  > yaratır; `/tp`/respawn dev koni üretir → teleport tespiti + radial reload gerekir. `in_flight`
  > dedup set'i TTL/cap ile sınırlanmalı.
- buffered vs unbuffered `FILE_FLAG_NO_BUFFERING` arkası feature flag ile geçilmeli (varsayılan buffered).

---

### 1.5 Metadata Store (redb / fjall)

> **Düzeltme (denetim C3):** Planın "SQLite Fjall'dan hızlı" benchmark iddiası **ters** ve
> güvenilmezdi. Güncel benchmark (5M KV, NVMe): **fjall batch yazmada ~7.4×**, redb ~1.65× SQLite'dan
> hızlıdır; redb rastgele okumada ~3.8× iyidir. Strata'nın saf-Rust, zero-copy, hot-streaming
> anayasası için **`fjall` (birincil, yazma-ağır/LSM)** ve **`redb` (ikincil, okuma-optimize COW
> B-tree)** benimsenir. redb'nin **4 TiB sabit tavanı** ve COW ölü-sayfa şişmesi (4× alan) nedeniyle
> metadata **world/region-group başına shard** edilmelidir. SQLite yalnızca geri dönüş seçeneği.

**Neden redb/fjall:**
- Zero-copy `get()` (redb `AccessGuard`), MVCC (okuyucu/yazıcı bloke etmez), saf Rust (no FFI,
  no `-shm`), memory-safe → plan 02/06 ethos'u ile uyumlu.
- SQLite: single-writer lock, FFI+copy, WAL `-shm` network FS'de çalışmaz, `bundled` build
  optimizasyon tuzağı.

**Şema (redb Tables — SQL değil):**
```rust
// sector_metadata: paketlenmiş koordinat -> metadata
type SectorMetaKey = [u8; 24];   // 3×i64 (rx,ry,rz + lx,ly,lz packed) veya 6×i32
type SectorMetaVal = (
    file_offset: u64,
    payload_size: u64,
    content_hash: [u8; 32],   // BLAKE3 BLOB — INTEGER DEĞİL
    timestamp: u64,
    tier: u8,
    dirty: bool,
);
// gc_candidates: content_hash -> (ref_count, marked_at)
type GcKey = [u8; 32];
type GcVal = (ref_count: u32, marked_at: u64);
// world_config: key -> value
type ConfigKey = [u8; 32];   // veya String
type ConfigVal = [u8; 256];
```

- `content_hash` **BLOB (`[u8;32]`)** — asla INTEGER (çakışma).
- `idx_dirty WHERE dirty=1` karşılığı: redb'de `SectorMetaVal.dirty` üzerinden range/scan; partial
  index gereksiz (KV zaten prefix scan). `idx_tier` **kaldırıldı** (düşük kardinalite; tier zaten
  ECS'te `SectorTransform.tier`, plan 08).
- Transaction içinde `ref_count` atomik güncellenir (`inc_refcount`/`dec_refcount`), crash recovery
  WAL replay ile.

**SQLite geri dönüş (yalnızca istenirse):**
- `content_hash` mutlaka `BLOB` (≥16B); `idx_tier` düşür; `idx_dirty WHERE dirty=1` partial index
  koru.
- GC/space reclaim: `auto_vacuum=INCREMENTAL` + `incremental_vacuum`; `PASSIVE` checkpoint online,
  `TRUNCATE` yalnızca kapatmada. **`wal_checkpoint(TRUNCATE)` periyodik GC DEĞİLDİR** (space reclaim
  yapmaz, corruption riski var).

#### 1.5.1 Denetim 2026-07-07 — Transaction, Cache ve GC Doğruluğu
- **"Tek transaction" imkânsızdır:** Region dosyası unbuffered I/O ile yazılır; redb/fjall
  transaction yalnız kendi store'unu kapsar. İki crash penceresi vardır. Çözüm: (1) payload'ı bölgeye
  yaz → (2) redb/fjall txn'i (onu işaret eden) commit et → (3) startup'ta region vs store taraması
  (dangling ref drop, orphan GC). redb `repair()` store integrity'yi korur; region↔store reconcile
  pass eklenmeli.
- **In-memory `HashMap` cache txn dışında mutasyona uğramamalı**; cache'i store ile read-through/
  reconciled yap (eşzamanlı flush task'ları double-append/orphan üretmesin).
- **Ref sahipliği net olmalı:** tam bir ref canlı mantıksal sektör örneği başına; `dec_refcount`
  yalnız count>0 iken, underflow/double-free'e karşı guard'lı.
- **GC sıralaması:** `BEGIN txn → dec ref_count (0 ise remove) → COMMIT → fiziksel payload sil`.
  Crash → güvenli orphan (sonradan sweep), asla dangling değil.
- **Terminoloji:** redb COW'dur, "WAL replay" değil "son tutarlı root'a rollback"; fjall gerçek
  WAL'dır.

---

### 1.6 Write-Back Pipeline

> **Düzeltme:** Dirty queue **sektör koordinatı** tutar (snapshot `Arc` değil, bkz. §1.1.3);
> flag yalnızca double-enqueue'i önler. WAL: pending kaydı durable commit **sonrası silinir**, sonra
> flag temizlenir (commit'te append yanlıştır — WAL sınırsız büyür).

```rust
pub struct FlushScheduler {
    dirty_queue:   VecDeque<(SectorCoord, Arc<Sector>)>,  // Arc<Sector> = dirty record
    in_flight:     HashMap<SectorCoord, JoinHandle<()>>,
    max_batch_size: usize,
    max_wait_time: Duration,
    flush_interval: Duration,
}

impl FlushScheduler {
    pub async fn run(mut self) {
        let mut ticker = tokio::time::interval(self.flush_interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => self.flush_if_needed().await,
                _ = self.max_wait_expired() => self.flush_all().await,
            }
        }
    }

    async fn flush_batch(&mut self, batch: Vec<(SectorCoord, Arc<Sector>)>) {
        let by_region = self.group_by_region(&batch);
        let tasks: Vec<_> = by_region.into_iter().map(|(region, sectors)| {
            tokio::task::spawn_blocking(move || {
                // 1) compress (sektör-başına paralel: rayon/AsyncComputeTaskPool;
                //    zstd in-frame nbWorkers <1MB sektörde işe yaramaz → per-sector parallelism)
                // 2) dedup check (BLAKE3) — ref_count transaction içinde
                // 3) write region + metadata store update (tek transaction)
                // 4) WAL append (dirty_log) -> SONRA in-memory flag temizle
            })
        }).collect();
        for task in tasks { task.await.unwrap(); }
        // Flush sonrası sticky flag temizle (Release); WAL append'ten sonra.
    }
}
```

- zstd **multithreaded API** flush path'inde kullanılır (<50ms batch hedefi).
- Pristine sektörler kuyruğa girmez (§1.1.4) → flush yalnız dirty sektörler.

---

### 1.7 Tier-Bazlı Compression Stratejisi

| Tier | Compression | Hedef | Beklenen Oran |
|---|---|---|---|
| **WARM (cache)** | zstd-1 (veya LZ4 promosyon gecikmesi için) | Hız > boyut | 3:1 |
| **DISTANT** | zstd-3 | Denge | 8:1 |
| **ARCHIVE** | zstd-19 | Boyut > hız (write-once/read-rarely) | 15:1 |
| **Dedup payload** | zstd-3 + BLAKE3 dedup | Tekrar eden geometri | 20:1+ |

- zstd **frame checksum** (`XXH64` frame içi) ikinci bağımsız integrity katmanı olarak açılır.
- ARCHIVE zstd-19 yazımı yavaş ama read-rarely → kabul edilebilir; CPU bütçesi uygunsa zstd-15..17
  çoğu oranı yakalar.

> **Düzeltme (denetim 2026-07-07):** Oranlar (3:1 / 8:1 / 15:1 / 20:1) **sparse-voxel en-iyi-durum
> hedefleridir**, zstd'nin gerçek rakamları Silesia'da ~2.9 / 3.2 / 3.7–4.0'dır. Asıl sıkıştırma
> **XBrickMap/SVDAG yapısındandır**; zstd ikincil entropy katmanıdır — oranlar doğru atfedilmeli.
> XXH64 frame checksum yalnız WARM/DISTANT için açılmalı (ucuz, decoder-enforced); DEDUP/ARCHIVE'de
> BLAKE3 tek hash = dedup + integrity olarak yeterlidir, ayrı xxHash64 alanı isteğe bağlı pre-check'e
> indirgenmeli. Performans hedefleri **NVMe-class storage** varsayar (HDD random read <5 ms'yi aşar).

---

### 1.8 Garbage Collection & Compaction

> **Düzeltme (denetim §9 B3 / §6):** Orijinal tasarımda `gc_candidates` hiç popüle edilmiyor,
> `DedupTable::remove_payload` yoktu → GC ölü koddu. Ref-count artık §1.3/§1.5'te transaction içinde
> doğru tutulur; GC gerçekçi hale geldi.

```rust
pub struct GarbageCollector {
    store: MetadataStore,   // redb/fjall
    region: RegionFile,
}

impl GarbageCollector {
    pub async fn run_gc(&mut self) {
        // ref_count = 0 olan adayları bul (gc_candidates veya range scan)
        let candidates = self.store.drain_zero_refcount().await;
        for hash in candidates {
            self.region.free_payload(self.store.lookup_offset(&hash)).await;
        }
        self.compact_regions().await;
        // redb/fjall: kendi compaction'ı (COW/LSM) — SQLite VACUUM gerekmez.
    }

    async fn compact_regions(&mut self) {
        // Canlı payload'ları yeni dosyaya kopyala, eski sil + rename,
        // metadata offset'leri transaction içinde güncelle.
    }
}
```

- redb (COW) / fjall (LSM) kendi compaction'ını yapar; SQLite'taki `VACUUM` exclusive-lock hitch'i yok.
- SQLite geri dönüşünde: `incremental_vacuum` (exclusive lock yok), `TRUNCATE` yalnızca kapatma.

> **Düzeltme (denetim 2026-07-07):** GC sıralaması açık olmalı: txn içinde `dec ref_count` → commit
> → fiziksel payload sil (crash → güvenli orphan). **Ref-count reclaim** (<200 ms) ile **region
> compaction** (paced, atomic write-new→fsync→rename→redb-offset-txn; multi-GB region 200 ms'yi aşar)
> ayrı tutulmalı.

---

### 1.9 Content-Defined Chunking (GearHash) + BLAKE3 Merkle

> **Düzeltme (denetim §9 — B1/B2/B3/B4/B5):** Per-sector CDC hot path'ten **çıkarıldı**; yalnız
> büyük bölge dosyası / cloud-diff senaryolarında kullanılır. Aşağıdaki düzeltmeler hem per-sector
> hem large-stream kullanım için geçerlidir.

#### GearHash ile Sınır Belirleme (düzeltilmiş)
```rust
pub struct ContentDefinedChunker {
    gear_state: u64,
    min_chunk: u32, max_chunk: u32, target: u32,
    boundary_mask: u64,   // popcount'a göre boundary olasılığı = 2^-popcount
}

impl ContentDefinedChunker {
    pub fn new() -> Self {
        // GEAR_TABLE compile-time sabit, doğrulanmış tablo (srijs/rust-gearhash veya Xet).
        Self { gear_state: 0, /* ... */ }
    }
    pub fn reset(&mut self) { self.gear_state = 0; }   // B2: her sektörde reset

    pub fn should_split(&mut self, byte: u8) -> bool {
        self.gear_state = (self.gear_state << 1) ^ GEAR_TABLE[byte as usize]; // B1: HER byte'da ilerle
        (self.gear_state & self.boundary_mask) == 0
    }
    // chunk_sector: hash'i HER byte'da güncelle; sınır testini yalnız min_chunk sonrası yap.
}
```
- `GEAR_TABLE` **tanımlanmalı** (doğrulanmış sabit tablo); rastgele tablo dağılımı bozar.
- `boundary_mask` popcount'u hedef chunk boyutuna göre (örn. ~2KB hedef → ~11 sıfır-bit).

#### MerkleTree (düzeltilmiş — veya kaldır)
```rust
const LEAF: u8 = 0x00;
const NODE: u8 = 0x01;

impl MerkleTree {
    fn leaf_hash(chunk: &[u8]) -> [u8; 32] {
        blake3::keyed_hash(KEY, &[&[LEAF], chunk].concat()).into()  // domain separation
    }
    fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
        blake3::keyed_hash(KEY, &[&[NODE], left, right].concat()).into()
    }
    // root SAKLANIR ve verify_chunk proof ile kökü doğrular (B4: root artık kullanılıyor).
}
```
- **Domain separation** (`0x00` leaf / `0x01` node) → second-preimage forgery engellenir.
- `root` saklanır ve inclusion proof ile doğrulanır; aksi halde Merkle "güvenlik tiyatrosu"dur.
- Küçük/bağımsız voxel sektörlerde Merkle gereksiz; tek `BLAKE3(whole sector)` yeterli. **Merkle
  yalnız partial/incremental verification gerektiğinde tutulur.**

#### ChunkedDedupStorage (disk-backed, düzeltilmiş)
```rust
pub struct ChunkedDedupStorage {
    chunk_store: MetadataStore,   // hash -> offset (disk-backed, RAM index bounded)
    sector_chunks: MetadataStore, // SectorCoord -> Vec<[u8;32]>
    // RAM HashMap<[u8;32], ChunkData> YOK — "milyarlarca chunk" RAM'de imkansız.
}
```
- Chunk store **disk-backed** (bölge dosyası); in-RAM index bounded + pre-sized (`with_capacity`).
- ref_count artırma **ve azaltma** mevcut (B3 düzeltildi); `remove_payload` uygulanır.

#### Performans (gerçekçi)
| Metrik | Sabit Sector | Content-Defined | Not |
|---|---|---|---|
| Dedup (identical sectors) | %30-60 | %30-60 | Sabit blok yeterli |
| Dedup (shifted large data) | düşük | %50-80 | Yalnız büyük akış |
| Voxel 32³ sektör gerçekçi | ~%20-40 | ~%20-40 | Çoğu durumda ~%0 (hava) |
| Integrity | xxHash64 | BLAKE3 + domain-sep Merkle | Güvenli |

> **Not:** "%50-80 CDC uplift" voxel sektörler için gerçekçi DEĞİLDİR; bu rakamlar büyük
> cross-version dataset'lerden (Borg/restic). Strata'da whole-sector BLAKE3 dedup çoğu kazancı verir.

---

### 1.10 Performans Hedefleri (Depolama)

| Metrik | Hedef | Not |
|---|---|---|
| Hot load (cache hit) | <0.1ms | RAM'den `Arc` clone |
| Warm load (cache miss) | <2ms | Decompress + deserialize |
| Cold load (disk) | <5ms | Unbuffered I/O + decompress |
| Batch save (64 sector) | <50ms | Paralel zstd + metadata txn |
| Write throughput | >500MB/s | Multi-thread unbuffered |
| Dedup tasarrufu (sabit) | %30-60 | whole-sector BLAKE3 |
| Dedup tasarrufu (large-stream CDC) | %50-80 | Yalnız bölge/cloud-diff |
| Crash recovery | <100ms | Metadata WAL replay |
| GC cycle | <200ms | Background, COW/LSM compaction |
| Integrity verification | BLAKE3 + xxHash64 frame | Chunk/sector-level |

---

# 38 — Game State Save/Load

## 1. Genel Bakış
Oyuncu verisi, dünya metadata ve session yönetimi. Chunk storage'dan ayrıdır.

### Temel Prensipler
- **Player data:** Envanter, pozisyon, sağlık, XP
- **World metadata:** Seed, zaman, hava durumu, keşif verisi
- **Session:** Çoklu dünya desteği
- **Auto-save:** Dirty-flag + max-interval hibrit

---

## 2. Serialization & Versioned Envelope

> **Düzeltme (denetim §10.1/§10.2):** Format belirsizdi (`serde` + bilinmeyen). `postcard` (binary,
> en hızlı/küçük) + **versioned envelope** benimsenir. RON yalnızca modding/enum config (plan 05).

```rust
#[derive(Serialize, Deserialize)]
pub struct SaveEnvelope {
    pub magic:        [u8; 4],        // "STSV"
    pub save_version: u32,           // SAVE_FORMAT_VERSION — generator_version'DEN AYRI
    pub schema_id:    u32,
    pub timestamp:    u64,
    pub payload_size: u32,
    pub payload_hash: [u8; 32],       // BLAKE3 integrity
    pub payload:      Vec<u8>,        // postcard::to_vec(&SaveDataVn)
}
```

- `SAVE_FORMAT_VERSION` (ayrı alan) yalnız on-disk şekil değişince bump edilir; `generator_version`
  terrain algoritmasını, `save_version` byte layout'unu belirtir.
- `ItemStack` enum `#[repr(u8)]` + manuel serde → reorder güvenli.
- `f32` pozisyon fixed bytes; cross-arch float bit pattern'e güvenme.

**Migration:** `Load → read version → v1→v2→…→vN saf fonksiyon migratör zinciri`. Her migratör
pure (no I/O, no global). **Golden-file CI test matrisi**: her yayınlanan versiyon için bir fixture,
güncele migrate assert edilir. Load sonrası validation: range clamp (health≥0), enum membership,
bozuk veri → `.bak`'a geri dön, asla sessiz çöp yükleme.

---

## 3. Player Save Data
```rust
#[derive(Serialize, Deserialize)]
pub struct PlayerSaveData {
    pub uuid: String,
    pub position: [f32; 3],
    pub rotation: [f32; 2],
    pub health: f32,
    pub hunger: f32,
    pub xp: f32,
    pub xp_level: u32,
    pub inventory: Vec<Option<ItemStack>>,
    pub game_mode: u8,
    pub explored_chunks: Vec<ChunkCoord>,
}
```

---

## 4. World Metadata
```rust
#[derive(Serialize, Deserialize)]
pub struct WorldMetadata {
    pub name: String,
    pub seed: u64,
    pub created_at: u64,
    pub last_played: u64,
    pub playtime_seconds: u64,
    pub time_of_day: f32,
    pub weather: WeatherState,
    pub spawn_point: [i32; 3],
    pub generator_version: u32,
}
```

---

## 5. Save Manager (atomic write + dirty-flag)

> **Düzeltme (denetim §10.3):** `auto_save_timer` tek başına yetersiz (crash'te kayıp + boş I/O +
> corruption riski). Hibrit + atomic write.

```rust
pub struct SaveManager {
    pub session: Option<Session>,
    pub auto_save_max_interval: f32,   // max staleness cap
    pub dirty: bool,                    // değişimde set (ECS Changed<T> ile)
    pub auto_save_timer: f32,
}

impl SaveManager {
    // Dirty-flag tetikler; interval yalnızca cap.
    pub fn maybe_save(&mut self, dt: f32, world: &World, players: &[PlayerSaveData]) {
        self.auto_save_timer += dt;
        if self.dirty && self.auto_save_timer >= self.auto_save_max_interval {
            self.save_world_atomic(world, players);
            self.auto_save_timer = 0.0;
            self.dirty = false;
        }
    }

    fn save_world_atomic(&self, world: &World, players: &[PlayerSaveData]) {
         // 1) serialize (game thread, hızlı)
         // 2) atomic yazım sırası:
         //    a) serialize -> save.dat.tmp
         //    b) fsync(save.dat.tmp)  (+ Windows'da dizin handle fsync)
         //    c) varsa rename(save.dat -> save.bak)   (son iyi kopyayı koru)
         //    d) rename(save.dat.tmp -> save.dat)      (NTFS atomik)
         //    e) hata -> rename(save.dat -> save.corrupt); restore save.bak -> save.dat
         // 3) payload_hash (BLAKE3) header'da; corrupt -> .corrupt sakla
         // 4) DOSYA I/O'sunu `IoTaskPool`'a offload (AsyncComputeTaskPool CPU işidir)
     }
}
```

---

## 6. Session Management
```rust
pub struct Session {
    pub world_id: String,
    pub players: Vec<String>,
    pub started_at: u64,
    pub is_multiplayer: bool,
}
```

---

## 7. Crate Organizasyonu
```
crates/
  save/
    ├── mod.rs
    ├── envelope.rs      // SaveEnvelope + version + migration engine
    ├── manager.rs
    ├── player_data.rs
    ├── world_metadata.rs
    ├── session.rs
    └── auto_save.rs
```
`envelope` + `version` + `migration` `save` crate'inde (cloud_save da paylaşır, bkz. §43).

---

# 43 — Cloud Save & Backup

## 1. Genel Bakış
Oyuncu verilerini otomatik yedekler ve bulut senkronizasyonu sağlar.

### Temel Prensipler
- **Auto-backup:** Belirli aralıklarla
- **Sync:** Çoklu cihaz
- **Conflict resolution:** Gerçekçi politika (binary blob için Merge yok)
- **Versioning:** Geçmiş save'lere dönüş

---

## 2. Cloud Save Manager (dyn-safe)

> **Düzeltme (denetim §10.4):** `Box<dyn CloudProvider>` native `async fn` ile **derlenmez**.
> `#[async_trait]` veya static dispatch (enum) gerekir. `Merge` opaque blob için imkansız → çıkarıldı.

```rust
#[async_trait]
pub trait CloudProvider: Send + Sync {
    async fn upload(&self, key: &str, data: &[u8]) -> Result<()>;
    async fn download(&self, key: &str) -> Result<Vec<u8>>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    async fn delete(&self, key: &str) -> Result<()>;
}

pub struct CloudSaveManager {
    pub provider: Box<dyn CloudProvider>,   // #[async_trait] ile dyn-safe
    pub sync_interval: Duration,
    pub last_sync: Instant,
    pub pending_uploads: Vec<PendingUpload>,
}

pub struct SaveVersion {
    pub timestamp: u64,
    pub size: u64,
    pub hash: [u8; 32],    // BLAKE3
    pub is_cloud: bool,
    pub client_uuid: [u8; 16],   // idempotent upload key
}
```

- **Statik dispatch önerilir:** provider seti kapalı/küçükse (`enum Provider { Disk, S3, Steam }`)
  zero-alloc ve `Send`-dostu; `#[async_trait] Box<dyn>` yalnız açık plugin provider için. Strata
  perf-sensitive.
- **Idempotent key = `client_uuid + hash`** (yalnız `client_uuid` aynı client'in iki save'ini
  çakıştırır). Download'da `hash` doğrulanır (integrity).
- **Clock-skew:** client-saat yerine **server-time/HLC** ile `UseNewest`; ayrıca lower-bound (rollback)
  guard eklenmeli.

---

## 3. Conflict Resolution (gerçekçi)

> **Düzeltme:** `Merge` opaque binary blob için uygulanamaz (Steam Auto-Cloud, Obsidian Sync yalnız
> structured/text merge yapar). Binary için last-write-wins / AskUser.

```rust
pub enum ConflictResolution {
    UseNewest,        // timestamp / server-time wins (Steam/Obsidian tarzı)
    UseLocal,
    UseCloud,
    AskUser,          // Local vs Cloud (time + playtime) + opsiyonel KeepBoth
    // Merge: YALNIZCA structured DTO deserialize + per-field kural (XP=max, inventory=union).
    // Opaque postcard blob için kullanılmaz.
}
```

- **Clock-skew guard:** gelecek timestamp >5dk reddet; server time'a sync et.
- Surface resolution UI (sessiz overwrite = kayıp ilerleme + negatif review).
- Cloud "aynı şema başka kopya"; pull'da aynı migration zinciri çalıştırılır.

---

## 4. Crate Organizasyonu
```
crates/
  cloud_save/
    ├── mod.rs
    ├── manager.rs
    ├── provider.rs      // #[async_trait] CloudProvider
    ├── sync.rs
    ├── conflict.rs      // UseNewest/AskUser(+KeepBoth); Merge yalnız structured DTO
    └── versioning.rs
```
`cloud_save` **feature-gated optional dependency** (plan 04: ağır external dep yok). `save` crate'inin
`envelope`/`version`/`migration` tipini **paylaşır** → iki crate uyumsuz formata drift etmez.

---

## 5. Denetim Sonrası Değişiklik Özeti (2026-07-06)
| # | Bileşen | Eski | Yeni |
|---|---|---|---|
| C1 | Y adresleme | `32×32×1` + `I16Vec2` | 3D bölge `r.<rx>.<ry>.<rz>` (32768 slot) |
| C2 | Koordinat | `I16Vec3` | `i32` (sınırsız Y) |
| C3 | Metadata DB | SQLite (yanlış benchmark) | `redb`/`fjall` (SQLite geri dönüş) |
| C4 | Dedup hash | `xxHash64` u64 | BLAKE3 `[u8;32]` + ayrı xxHash64 checksum |
| C5 | Merkle | domain-sep yok/root yok | düzelt veya tek BLAKE3(whole sector) |
| C6 | CDC ref-count | sadece artırma | artırma+azaltma, disk-backed, hot path'ten çıkarıldı |
| C7 | CloudProvider | `dyn` + `Merge` | `#[async_trait]` + gerçekçi conflict |
| C8 | mmap | §1.2/§1.4 çelişkili | mmap kullanılmaz (not kaldırıldı) |
| — | Cache | elle LRU 500 | `moka`/`quick_cache`, byte-weighted |
| — | Tier coupling | 1:1 | decouple; pristine = no disk write |
| — | Dirty flag | `atomic<bool>` | sticky + queue holds Arc + WAL |
| — | Save format | belirsiz | `postcard` + versioned envelope |
| — | Auto-save | timer-only | dirty + max_interval + atomic write |
| — | Async runtime | 2 ayrı | tek runtime + priority |
| — | Prefetch | yanlış yer | hareket konisi, ayrı update |

### 5.1 Denetim 2026-07-07 (6-alt-ajan web araştırması — bkz. `researchs/audit-storage-plan15.md`)

| # | Bileşen | Eski | Yeni |
|---|---|---|---|
| D1 | Metadata birincil | `redb` birincil | **`fjall` birincil**, redb ikincil (7.4× fjall'a ait); redb 4 TiB tavanı + shard |
| D2 | Dirty queue | `Arc<Sector>` snapshot | queue koordinat + flush'ta canlı snapshot (COW en-güncel-edit kaybı bug'ı) |
| D3 | WAL sıralaması | commit'te append | pending kaydı durable commit sonrası **sil** → flag temizle |
| D4 | Unbuffered I/O | varsayılan | **buffered varsayılan**, unbuffered feature-flag (yeniden-okuma ~22× regresyon) |
| D5 | L2 cache | `quick_cache`=CLOCK-Pro | **S3-FIFO**; "94% vs 82%" yumuşatıldı (trace'ye bağlı %10–30) |
| D6 | Sıkıştırma oranı | 3/8/15/20:1 | **sparse hedef**; zstd gerçek ~2.9/3.2/3.7; asıl sıkıştırma yapısal (XBrickMap/SVDAG) |
| D7 | zstd MT | in-frame `nbWorkers` | **sektör-başına rayon** paralelliği (<1MB sektörde MT işe yaramaz) |
| D8 | Dedup %30-60 | dünya boyutu | **dirty set'e scoped ~%10–40**; compressed payload üzerinde hash |
| D9 | Bölge header | 256 KB | **768 KB** (3 dizi); redb tek otoriter, trailer-only index |
| D10 | Atomic write | bozuk sıra | 5-adımlı tmp→fsync→bak→rename→corrupt; `IoTaskPool` |
| D11 | Cloud dispatch | `Box<dyn>` | kapalı set için **enum dispatch**; idempotent = `uuid+hash`; server-time/HLC |
| D12 | Prefetch | `cam_vel` birincil | **GPU visibility/frustum birincil**; velocity smoothing + teleport guard |
| D13 | WARM tanımı | §1.1.1↔§1.4 çelişkili | WARM = sıkıştırılmış byte; `Arc<Sector>` cache = ACTIVE resident set |
| D14 | xxHash64 checksum | ayrı alan | BLAKE3+zstd-frame yeterli; isteğe bağlı pre-check'e indir |
