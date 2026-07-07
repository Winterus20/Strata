# Plan 15 — Depolama ve Kalıcılık: Kapsamlı Teknik Denetim Raporu (Türkçe)

**Tarih:** 2026-07-06
**Kapsam:** `plans/15-storage-and-persistence.md` (Bölüm 1 + 38 + 43)
**Yöntem:** 6 paralel araştırma alt-ajanı (web araştırması + karşılaştırmalı analiz), bileşen bazında.
**Önemli not:** Plan 15 bir *taslak* planıdır (AGENTS.md §2: `15`–`38` değişebilir; `01`–`14` anayasa). Aşağıdaki bulgular taslağın revize edilmesini önerir, anayasayı değil.

---

## 0. Yönetici Özeti (Executive Summary)

Plan 15'in **mimari şekli sağlam ve savunulabilir**. 3-kademeli hibrit depolama, bölge dosyası + SQLite ayrımı, ve özellikle açık/kapalı (GPU LOD streaming ile hizalı) WARM katmanı çoğu voxel motorundan *daha* modern bir tasarımdır. Ancak denetim **kritik çelişkiler ve en az 5 adet doğruluk bozan (correctness-breaking) hata** ortaya çıkardı:

| # | Bileşen | Bulgu | Önem |
|---|---|---|---|
| C1 | **Sınırsız yükseklik çelişkisi** | `I16Vec3` sektör koordinatı ±1M voxel/Y ile sınırlı → "her eksende sınırsız" anayasasıyla (plan 06/11) çelişir | 🔴 Blocker |
| C2 | **Y adresleme hatası** | `32×32×1` + `I16Vec2` bölge koordinatı + `r.0.0.strata` → diskte Y katmanı için anahtar yok | 🔴 Blocker |
| C3 | **SQLite benchmark ters** | Plan "SQLite Fjall'dan hızlı" diyor; gerçekte SQLite batch yazmada ~7.4×, rastgele okumada ~4× **daha yavaş** | 🔴 |
| C4 | **Dedup hash çakışması** | `xxHash64` (u64) hem dedup anahtarı hem integrity → çakışmada sessiz veri bozulması | 🔴 |
| C5 | **Merkle ağacı güvenlik tiyatrosu** | Leaf/node ayraç (domain separation) yok → second-preimage forgery; root hiç doğrulanmıyor | 🔴 |
| C6 | **CDC ref-count kırık** | Sadece artırma var; azaltma/GC yok → chunk store sızıntısı, GC ölü kod | 🔴 |
| C7 | **CloudProvider `dyn` uyumsuz** | `async fn` içeren `Box<dyn CloudProvider>` Rust'ta derlenmez | 🟠 |
| C8 | **mmap çelişkisi** | §1.2 "mmap (sadece read)" diyor, §1.4 "mmap kullanmıyoruz" diyor | 🟠 |

---

## 1. Katmanlı Depolama Mimarisi (§1.1)

### 1.1 Doğrulama
3-katmanlı şekil (RAM ↔ sıkıştırılmış cache ↔ disk) tüm büyük voxel motorlarının yaptığı şeydir. Strata'nın farkı, GPU-yönelimli cubic 32³ sektörler ve SVDAG LOD'ları için **açık/kapalı bir WARM (sıkıştırılmış RAM) katmanı** eklemesidir — bu Minecraft/Bedrock'ın OS sayfa önbelleğinden veya DB blok önbelleğinden *örtülü* olarak aldığı "decompress-edilmiş ama düzenlenemez" bandı açıkça kontrol eder. Bölge dosyası + SQLite ayrımı Minetest (`map.sqlite`) ve Bedrock (LevelDB+Zlib) ile uyumludur.

### 1.2 Endüstri Karşılaştırması
| Motor | Canlı RAM | Orta katman | Kalıcı depolama | Sıkıştırma |
|---|---|---|---|---|
| Minecraft Java (Anvil) | RAM chunk | yok (OS cache) | `.mca` bölge (32×32), NBT | Deflate; LZ4 opsiyonel |
| Minecraft Bedrock | RAM chunk | yok | LevelDB KV + Zlib | Zlib/subchunk |
| Minetest | RAM mapblock | yok | SQLite/LevelDB/PostgreSQL | backend'e bağlı |
| Veloren | RAM `Chunk` (sparse) | yok | `rusqlite` metadata | — |
| **Strata** | XBrickMap | **açık WARM (zstd-1)** | Bölge + SQLite | zstd 1/3/19 |

**İçgörü:** "Standart" **2 katmandır** (RAM ↔ disk). Orta WARM katmanı *standart değil*; yalnızca GPU-resident streaming bandı ve `07/08`'deki copy-free brick↔SVDAG geçişleri gerektirdiği için haklı. Plan "industry standard" dememeli; **2-katmanlı modelin GPU-LOD streaming için ayarlanmış bir üst kümesi** olarak çerçevelenmeli.

### 1.3 Öneri
- WARM katmanının *neden* var olduğunu belgele: (i) ACTIVE↔WARM promosyonu O(1) ve lock-free olmalı; (ii) DISTANT'a düşüşte yeniden sıkıştırma olmamalı; (iii) 200 MB/s GPU stream bütçesi altında OS cache kontrolsüz.
- **Pristine (düzenlenmemiş, seed'den yeniden üretilebilir) sektörler disk'e YAZILMAMALI** — en büyük I/O tasarrufu budur (bkz. §3).

**Kaynak:** Anvil — minecraft.wiki/w/Anvil_file_format; Minetest — wiki.minetest.org/Database_backends; Aokana I3D 2025 — arxiv.org/abs/2505.02017.

---

## 2. Önbellek (LRU, ~500 sektör, zstd-1) (§1.1 / §1.7)

### 2.1 Doğrulama
zstd-1 "hız öncelikli" için makuldür (~510 MB/s sıkıştır, ~1550 MB/s aç). Ancak **500 sektör yanlış boyutlandırılmış**: bu sabit bir *sayı*, gerçek byte'a bağlı değil. Boş sektörler <1 KB'ye, zengin/edilmişi sektörler onlarca KB'ye sıkışır → bellek ayak izi öngörülemez.

### 2.2 Alternatifler — Çıkarma Politikası
LRU tarama kirliliği (scan pollution) yapar. Modern politikalar daha iyi:

| Politika | Hit-rate (Zipfian) | Rust seçeneği |
|---|---|---|
| LRU (mevcut) | ~82% (baz) | `lru` |
| 2Q / ARC | +10-20% | (elle) |
| **W-TinyLFU (Caffeine)** | **~94%** | **`moka`** |
| CLOCK-Pro | TinyLFU'ye yakın | **`quick_cache`** |
| S3-FIFO | ARC ile eşit, basit | (ref impl) |

`moka` (async, tokio uyumlu, `weigher` ile byte-tabanlı kapasite, lock-free) Strata için doğrudan uygun. `quick_cache` (CLOCK-Pro, senkron, küçük) alternatifidir.

### 2.3 Öneri
- Elle yazılmış LRU → **`moka`** (async TinyLFU) veya **`quick_cache`** (CLOCK-Pro).
- Kapasite **sektör sayısı değil, byte bütçesi** olsun (`maximum_weight`/`weigher`); 500 sektör bir *taban* olabilir.
- WARM **sıkıştırılmış byte blob** tutmalı (promosyonda decompress edilir); çıkarma O(1), yeniden sıkıştırma yok.
- Promosyon gecikmesi sorun olursa WARM için zstd-1 yerine **LZ4** (~3850 MB/s decompress) düşünülebilir; zstd-3 (DISTANT) ve zstd-19 (ARCHIVE) korunur.

**Kaynak:** Caffeine — github.com/ben-manes/caffeine; moka — moka-rs/moka; zstd — facebook/zstd; LZ4 — lz4.github.io/lz4.

---

## 3. Streaming↔Depolama 1:1 Eşlemesi (§1.1)

### 3.1 Doğrulama
1:1 eşleme (ACTIVE→L1, WARM→L2, DISTANT+ARCHIVE→L3) "convenient" ama sorunlu: plan zaten L3'ün hem DISTANT hem ARCHIVE'a hizmet ettiğini kabul ederek 1:1 iddiasını deliyor.

### 3.2 Neden Ayrıştırma Daha İyi
- **Streaming tier** = görünüm/LOD yerleşimi + mesafe (hysteresis'li, geçici).
- **Storage tier** = dayanıklılık + sıkıştırma gücü (dirtylik, düzenleme geçmişi, yeniden-üretilebilirlik ile sürülür).

**Sorunlar:** (1) Pristine sektörler ACTIVE→DISTANT düştüğünde *seed'den yeniden üretilebilir*; disk'e yazmak boş I/O. (2) Streaming flip'leri hysteresis tamponlu ve salınımlı → storage eviction'ı thrash edebilir. (3) ARCHIVE'nin render/physics'i yok, saf storage kavramı.

### 3.3 Öneri
- Yerleşimi hizalı tut ama **yazma/kalıcılık kararını ayır**.
- **Pristine sektörün RAM'dan çıkarılması → disk'e yazma YOK** (en büyük I/O tasarrufu).
- **Dirty sektörün çıkarılması → L3'e flush** (DISTANT veya ARCHIVE fark etmez).
- "regenerable vs persisted" üçüncü boyutunu plana birinci sınıf karar olarak işle (`dirty` biti zaten SQLite WAL'da var).

---

## 4. Obje Havuzu vs Arena (§1.1)

### 4.1 Doğrulama
L1 "Object pool (GC churn yok)" doğru ve `06`/`AGENTS.md` ile uyumlu: `GlobalBrickPool` = **SlotMap + SecondaryMap** (O(1) alloc/free, stabil anahtar). `39-memory-allocation.md` doğru kombinasyonu seçmiş: mimalloc (global) + bumpalo (per-frame scratch) + slab (paket havuzu) + per-sector mi_heap.

### 4.2 Kritik Ayrım
**bumpalo bir obje havuzu DEĞİLDİR** — arena'dır; tek bir brick'i serbest bırakamaz. Brick'ler için doğru primitive SlotMap/slab'dir (`06` zaten belirtiyor).

### 4.3 Öneri
- Brick'ler için SlotMap/slab obje havuzu korunmalı; bumpalo canlı dünya verisi için kullanılmamalı.
- bumpalo yalnızca geçici scratch (mesh gen, pathfinding) için, per-frame reset.
- WARM cache değerleri sıkıştırılmış blob → havuz gerektirmez, `Vec<u8>`/`Bytes` olarak sahiplen.
- Raw slab `usize` yerine **generational index** (SlotMap) tercih edilmeli (askıda referans yakalanır).

---

## 5. Dirty Tracking (`atomic<bool>`) (§1.1 / §1.4)

### 5.1 Doğrulama
`AtomicBool` ucuz, lock-free bir **sinyal**dir; doğruluk ilkesi DEĞİLDİR. Plan eksik belirtmiş. Tehlikeler:
1. **TOCTOU/lost-update:** `data[i]=v; dirty.store(true)` arasında preemption → stale okuma veya kaçırılan flush (crash'te kayıp).
2. Flag payload'ı korumaz; aynı sektörü eşzamanlı düzenleyen thread'ler için `Arc<Sector>` altında per-sector lock/COW gerekir.
3. **Erken temizleme:** flusher `store(false)` yaparken yazar `true` yaparsa ikinci yazı kaybolur → flag **sticky (sadece set)** olmalı, yalnızca kalıcı commit+WAL sonrası temizlenmeli.
4. **False sharing:** hot brick verisiyle aynı cache line'daki per-sector AtomicBool çapraz-çekirdek invalidation fırtınası → sharded atomic bitset (`AtomicU64` ile 64 flag).
5. **Crash dayanıklılığı:** RAM flag'i crash'te kaybolur; **WAL `dirty_log` gerçek kaynaktır**.

### 5.2 Öneri
- `AtomicBool`'a çapraz-thread veri güvenliği için güvenme (sadece sinyal).
- `Ordering::Release` (store) / `Ordering::Acquire` (load); flag payload yazımından *sonra* set, kalıcı commit+WAL'dan *sonra* temizle.
- **Sticky flag + dirty queue `Arc<Sector>` tutar** modeli (flag sadece double-enqueue'i önler).
- Kurtarma WAL `dirty_log`'a bağlı.

---

## 6. SQLite vs Fjall + Şema (§1.5)

### 6.1 Benchmark İddiası TERS — Çok Kritik
Plan "SQLite batch insert ~23ms vs Fjall ~50ms" diyor. Gerçek (redb benchmark):

| op | redb | lmdb | rocksdb | fjall | **sqlite** |
|---|---|---|---|---|---|
| batch writes | 1595ms | 942ms | 451ms | **353ms** | **2625ms** |
| random reads | 1138ms | 637ms | 2911ms | 2177ms | **4283ms** |

SQLite batch yazmada fjall'dan **~7.4×**, rastgele okumada **~4× daha yavaş**. Planın sayıları muhtemelen `synchronous`/durability ayarları eşleştirilmeden ölçülmüş.

### 6.2 SQLite Uygun mu?
Strata'nın streaming yolu zaten authoritative tier/state'i ECS'te (`SectorTransform.tier`), canlı veriyi `GlobalBrickPool`'da tutuyor. Metadata yalnızca **streaming load/unload ve save** sırasında (sıcak değil, ılık yol) kullanılır. SQLite'nın maliyeti:
- **Single-writer lock** (WAL'da bile tek yazar); burst unload'ları serialize eder.
- **FFI + copy**: rusqlite zero-copy değil.
- **WAL `-shm`** gerektirir; network dosya sistemlerinde çalışmaz.

### 6.3 Şema İncelemesi
- `PRIMARY KEY (region_x, region_z, local_x, local_z, local_y)` → **iyi** (bölge prefix range scan).
- `idx_dirty WHERE dirty=1` → **geçerli ve faydalı** (partial index).
- `idx_tier` → **gereksiz** (düşük kardinalite; tier zaten ECS'te; SQLite görmezden gelir).
- `content_hash INTEGER` → **risk**: 64-bit hash "sınırsız" dünyada çakışır (birthday bound ~2³²'de %40). **BLOB (≥16 byte)** olmalı.
- `gc_candidates(content_hash PRIMARY KEY, ...)` → aynı BLOB sorunu.

### 6.4 Öneri
1. Benchmark'ı reddet; pinned durability ile yeniden ölç.
2. **`redb`** (birincil) veya **`fjall`** (yazma-ağır) benimse — iki tablo: paketlenmiş sektör koordinatı / hash.
3. `idx_tier`'ı düşür; `content_hash`'i **BLOB** yap; `world_config`'i ikinci KV tabloya taşı.
4. Veya Anvil tarzı **bölge dosyası başlığı** (dependency'siz, en iyi cache locality) — metadata'yı dosyanın yanına koyar.
5. SQLite tutulursa: `auto_vacuum=INCREMENTAL` + `incremental_vacuum`; `PASSIVE` checkpoint online, `TRUNCATE` yalnızca kapatmada. **`TRUNCATE`'ı periyodik GC olarak kullanma** (corruption riski + gerçek space reclaim yapmaz; VACUUM gerekir).

**Kaynak:** redb — github.com/cberner/redb; rusqlite #1621; Fjall 3.0 — fjall-rs.github.io; SQLite WAL — sqlite.org/wal.html; Partial indexes — sqlite.org/partialindex.html.

---

## 7. Bölge Dosyası Formatı + Dedup (§1.2 / §1.3)

### 7.1 🔴 Blocker: Y Adresleme Yok
`r.0.0.strata` + `I16Vec2` bölge koordinatı + `32×32×1 = 1024 sektör` → **diskte Y için anahtar yok**. Anvil 2D bölge kullanabilir çünkü Y chunk *içinde*; Strata her slot'a bir Sector koyduğundan 2D bölge tüm dikey adreslemeyi kaybeder. Büyük hata.

### 7.2 🔴 Blocker: `I16Vec3` Sınırsız Yükseklikle Çelişir
`i16` sektör koordinatı ±32767 sektör = **±1.048.576 voxel/Y** ile sınırlı. Anayasa "her eksende sınırsız" (plan 06) ve plan 11 `i32` dünya koordinatı kullanıyor. `i32`→`i16` dönüşümü silent truncation + uzak sektörlere aliasing (felaket çapraz-yazım) riski.

### 7.3 Sabit Slot Tablosu Riski
1024-slot sabit dizi Anvil'ın bilinen hatalarını miras alır: (i) bir bölge asla 1024'ten fazla sektör tutamaz → Y'yi kapsamak için Y-katmanı başına dosya (dosya patlaması); (ii) değişken payload için free-list/compaction gerekir (§1.8'de gevşek).

### 7.4 Dedup (`xxHash64`) Çakışma Riski
64-bit hash "sınırsız" dünyada çakışabilir → iki farklı payload aynı hash → **sessiz veri bozulması**. Ayrıca ref-count (`HashMap<u64,u64>`) thread-safe/atomic/crash-consistent DEĞİL.

### 7.5 Öneri
1. **Y bileşeni ekle** (bölge `r.<rx>.<ry>.<rz>.strata` veya 3D bölge `32×32×32`).
2. **`I16` → `i32`** (veya `i32`'yi kapsayan paketlenmiş 64-bit). Dönüşüm assert'i koy.
3. **Dedup anahtarı = BLAKE3/SHA-256 (BLOB)**; ayrı bir 64-bit `xxHash64`/`CRC64` **sadece integrity** için. Asla tek 64-bit hash ikisi için birden kullanma.
4. **ref_count'ları SQLite'da transaction içinde** atomik güncelle (`INSERT … ON CONFLICT DO UPDATE`); in-memory `DedupTable` yalnızca cache olsun.
5. Dedup yeniden kullanımda payload hash'ini **yeniden doğrula**.
6. Sabit slot yerine 3D bölge veya KV/SQLite primary index; `u64` offset/size + crash-safe compaction.

---

## 8. Async I/O Stratejisi (Windows) (§1.4)

### 8.1 mmap Kararı DOĞRU
"mmap kullanmıyoruz — page fault async thread'i bloklar" iddiası **doğru**. mmap + async, sayfa hatası `await` noktası olmadığından concurrency'yi sessizce tek-iş parçacığına düşürür (Huon 2024). Windows'ta MMF görünümleri Cache Manager'dan geçmez, `FILE_FLAG_NO_BUFFERING` yok sayılır.

### 8.2 `tokio::spawn_blocking` mı, Gerçek Async IOCP mı?
Tokio Windows'ta *gerçek* async dosya I/O yapmaz; `tokio::fs` zaten `spawn_blocking`'e düşer. Pattern makul ama:
- **`compio`** (Windows'ta IOCP native, Linux'ta io_uring) gerçek completion-based async verir, worker-thread block yok.
- Ancak Strata Bevy projesi → Bevy kendi `AsyncComputeTaskPool`/`IoTaskPool`'unu sağlar. İkinci bir tam Tokio runtime = iki scheduler.

### 8.3 İki Ayrı Runtime (write_pool/read_pool) — Over-engineering
Windows I/O tek IOCP altında birleşir. İki runtime = iki thread pool, iki IOCP, çapraz-pool öncelik yok. **Tek runtime + priority channel** kullan.

### 8.4 `read_sector_aligned` + Değişken Payload
`FILE_FLAG_NO_BUFFERING` ile offset/length/buffer **sector-aligned** (512B logical, 4KB physical) olmalı. Değişken sıkıştırılmış payload doğrudan okunamaz → aligned **window** oku (`VirtualAlloc` ile 4KB aligned, yeniden kullan), slice çıkar. Yazımda length'i yukarı yuvarla.

### 8.5 Prefetch Yanlış Yerleştirilmiş
`load_sector` içinde `prefetch.enqueue(coord)` **şimdi yüklenecek** coord'u enqueue ediyor — bu prefetch değil. Prefetch **tahmini gelecek** coord'ları (hareket konisi) enqueue etmeli.

### 8.6 Öneri
- mmap kararını koru; §1.2'deki "mmap (sadece read)" notunu sil veya yalnızca immutable archive ile sınırla.
- **Tek runtime**, priority channel; write'ı `THREAD_PRIORITY_BELOW_NORMAL`'a pinle.
- `read_sector_aligned` → aligned window; `VirtualAlloc` + 4KB physical alignment; buffer reuse.
- Prefetch'i `PrefetchManager::update(camera_pos, camera_vel)` olarak ayrı tut; hareket konisi + GPU visibility feedback + in-flight dedup set; `Arc<Sector>` cache'te.
- buffered vs unbuffered'ı benchmark'la, `FILE_FLAG_NO_BUFFERING`'i flag'le geç.

**Kaynak:** Huon 2024 — huonw.github.io/blog/2024/08/async-hazard-mmap; MS File Buffering — learn.microsoft.com/.../fileio/file-buffering; compio — compio-rs/compio; Bvckup2 Fast Bulk IO; tokio #2926.

---

## 9. Content-Defined Chunking (GearHash) + BLAKE3 Merkle (§1.9)

### 9.1 🔴 B1: `gear_state` her byte'da ilerlemiyor
`should_split` yalnız `chunk_size >= min_chunk_size` iken çağrılıyor → ilk byte'larda hash ilerlemiyor → shift-resilience bozuluyor (CDC'nin sebebi).

### 9.2 🔴 B2: `gear_state` sektörler arası reset edilmiyor
`&mut self` alanı taşınıyor, reset yok → aynı sektör izole vs 2. sektör olarak farklı chunk'lara bölünür → reload sonrası dedup tekrarlanamaz (deterministik değil).

### 9.3 🔴 B3: Ref-count yalnız artıyor
`store_sector` sadece `+=1`; azaltma/`remove_payload` yok → chunk store sızar, GC (§1.8) ölü kod. Düzenleme eski chunk'ları pinler.

### 9.4 🟠 B4: Merkle ağacı güvenlik tiyatrosu
Leaf = `BLAKE3(raw)`, node = `BLAKE3(childA‖childB)` → **domain separation yok** → second-preimage forgery (Monero MRL-0002, Bitcoin CVE-2012-2459). `root` hiç saklanıp doğrulanmıyor → `verify_chunk` yalnızca kendi hash'ini karşılaştırıyor, ekstra integrity sıfır. BLAKE3'in native `PARENT` flag'ı bypass ediliyor.

### 9.5 🟠 B5: `HashMap<[u8;32], ChunkData>` RAM'de
"Milyarlarca chunk"iddasıyla RAM `HashMap`'i fiziksel olarak imkansız (LinkedIn FishDB 59M key ~1.75GB index, resize 15s stall). Chunk store disk-backed (bölge dosyası) + bounded in-RAM index olmalı.

### 9.6 CDC Değer mi?
32³ sektör çoğunlukla hava, KB ölçeğinde. CDC'nin değeri büyük akışlerde shift-resilience; voxel sektörlerde nadir. **Sabit content-hash (§1.3) çoğu dedup'u yakalar**. CDC yalnız büyük bölge dosyası / cloud diff'te tutulmalı.

### 9.7 50-80% İddiası Gerçekçi Değil
Bu rakamlar büyük cross-version dataset'lerden (Borg/restic). Voxel sektörler küçük/bağımsız/çoğunlukla hava → gerçekçi **~20-40%** (en iyi tekrarlı durumda), çoğu durumda ~%0. CDC uplift'ı "büyük shifted data"ya özgü.

### 9.8 Öneri
1. Tehdit modeli belirle: yalnızca local → `xxHash64`/`XXH3` yeterli; cloud/multiplayer → BLAKE3 ama o zaman her şey crypto-correct olmalı.
2. **Per-sector CDC'yi hot path'ten çıkar**; yalnız büyük bölge/cloud-diff'te tut.
3. **Ref-count'ı düzelt** (azaltma + `gc_candidates` popülasyonu + `remove_payload`).
4. **Chunk store disk-backed**; in-RAM index bounded + pre-sized.
5. **Merkle'yı düzelt veya sil**: domain separation (`0x00` leaf / `0x01` node) + root sakla/doğrula + proofs; veya tek `BLAKE3(whole sector)`.
6. Hash şemasını uzlaştır (§1.2/§1.3 `xxHash64` u64 vs §1.9 `BLAKE3` [u8;32]).
7. Dedup hedeflerini gerçekçi etiketle.

**Kaynak:** FastCDC USENIX'16 — usenix.org/conference/atc16/presentation/xia; Xet — huggingface.co/docs/xet; BLAKE3 — github.com/BLAKE3-team/BLAKE3; Nethermind second-preimage; Bitcoin Optech CVE-2012-2459; Borg/restic benchmarks.

---

## 10. Save/Load (Plan 38) + Cloud (Plan 43)

### 10.1 Serialization Format Belirsiz
`serde::{Serialize, Deserialize}` ama format **pin'lenmemiş** (JSON ima, RON başka yerde). Format seçimi başlı başına versioning kararı.

| Format | Boyut | Roundtrip | Self-describing |
|---|---|---|---|
| JSON | 2.4MB | 16ms | ✅ |
| RON | 2.1MB | 49ms | ✅ (yavaş) |
| MessagePack | 610KB | 5.7ms | ⚠️ |
| **Postcard** | **380KB** | **2.8ms** | ❌ |

**Öneri:** `postcard` (binary) + versioned envelope içinde. RON yalnızca modding/enum config için (plan 05). `f32` pozisyon fixed bytes; `ItemStack` enum `#[repr(u8)]` + manuel serde.

### 10.2 Save-Format Versioning EKSİK
Plan `generator_version: u32` tutuyor ama **save-format/schema version YOK**. `generator_version` "hangi terrain algoritması" der, "hangi byte layout" değil. Binary format'ta alan yeniden adlandırma/enum değişimi migrate edilmezse çöp deserialize eder.

**Öneri:** `SaveEnvelope { magic, save_version: u32, schema_id, timestamp, payload: Vec<u8> }` + zincirlenmiş saf fonksiyon migratörleri + golden-file CI test matrisi + load sonrası validation (clamp, enum membership).

### 10.3 Auto-Save Stratejisi Eksik
Sadece `auto_save_interval` yetersiz: crash'te bir interval kaybı + değişmemiş state'i yazma israfı + corruption safety yok.

**Öneri:** **dirty_flag + max_interval** hibrit; **temp-write → fsync → atomic rename → .bak rotation → checksum header**; serialize+write'i `AsyncComputeTaskPool`'a offload.

### 10.4 🟠 CloudProvider `dyn` Uyumsuz + `Merge` İmkansız
`Box<dyn CloudProvider>` içinde native `async fn` Rust'ta **derlenmez** (async-trait gerekir veya static dispatch). `ConflictResolution::Merge` opaque binary blob için **uygulanamaz** — gerçek sistemler (Steam Auto-Cloud, Obsidian Sync) yalnız structured/text veride merge yapar; binary için last-write-wins / AskUser.

**Öneri:** `#[async_trait]` veya enum static dispatch; `Merge`'i blob için **çıkar**, `UseNewest`/`AskUser`(+`KeepBoth`)/`UseLocal`/`UseCloud` kullan; clock-skew guard (gelecek timestamp >5dk reddet); `SaveVersion`'a `client_uuid` ekle + download'da hash doğrula.

### 10.5 Crate Organizasyonu
`save` ve `cloud_save` ayrı tutulmalı ama **envelope + version + migration `save`'te (veya `save-core`) paylaşılmalı** ki iki crate uyumsuz formata drift etmesin. `cloud_save` feature-gated optional dependency olsun.

**Kaynak:** PSeitz/test_serde_formats; postcard — postcard.jamesmunns.com; ArcadeOn save-system; StraySpark UE5 versioning; async-trait; Obsidian Sync; Bugnet cloud-save.

---

## 11. Birleştirilmiş Öneriler (Öncelik Sıralı)

| # | Bileşen | Mevcut | Öneri | Öncelik |
|---|---|---|---|---|
| 1 | Y adresleme | `32×32×1` + `I16Vec2` | **3D bölge + Y bileşeni** | 🔴 |
| 2 | Koordinat tipi | `I16Vec3` | **`i32`** (sınırsız Y ile uyum) | 🔴 |
| 3 | Metadata DB | SQLite (yanlış benchmark) | **`redb`/`fjall`** veya bölge başlığı | 🔴 |
| 4 | Dedup hash | `xxHash64` u64 | **BLAKE3 BLOB** + ayrı integrity checksum | 🔴 |
| 5 | Merkle ağacı | domain分离 yok, root yok | **düzelt veya sil** | 🔴 |
| 6 | CDC ref-count | sadece artırma | **azaltma + GC + disk-backed store** | 🔴 |
| 7 | CloudProvider | `dyn` + `Merge` | **`#[async_trait]` + gerçekçi conflict** | 🟠 |
| 8 | mmap çelişkisi | §1.2 vs §1.4 | §1.2 notunu sil/sınırla | 🟠 |
| 9 | Cache | elle LRU 500 | **`moka`/`quick_cache`, byte-weighted** | Yüksek |
| 10 | Tier coupling | 1:1 | **decouple; pristine = no write** | Yüksek |
| 11 | Dirty flag | `atomic<bool>` | **sticky + queue holds Arc + WAL** | Yüksek |
| 12 | Save format | belirsiz | **postcard + versioned envelope** | Yüksek |
| 13 | Auto-save | timer-only | **dirty + max_interval + atomic write** | Yüksek |
| 14 | Async runtime | 2 ayrı | **tek runtime + priority** | Orta |
| 15 | Prefetch | yanlış yer | **hareket konisi, ayrı update** | Orta |
| 16 | zstd | 1/3/19 | koru; WARM'da LZ4 opsiyon | Orta |

---

## 12. Anayasa Uyumu (AGENTS.md §2)
Tüm öneriler `01`–`14` anayasasıyla uyumludur:
- `06` XBrickMap / GlobalBrickPool (SlotMap object pool) → Bölüm 4, 7 doğrular.
- `08` Streaming tier/hysteresis → Bölüm 3 (decoupling) ile uyumlu.
- `03` ECS filter-first / change detection → Bölüm 5 (dirty flag Release/Acquire).
- `11` `i32` world-gen koordinatı → Bölüm 7 (`I16` çelişkisi).
Plan 15 taslak olduğundan, bu bulgular doğrudan taslağın revizyonu içindir; anayasa değişmez.

---

## 13. Kaynaklar (Seçkin)
- redb / fjall benchmarks: github.com/cberner/redb, fjall-rs.github.io
- SQLite WAL & partial indexes: sqlite.org/wal.html, sqlite.org/partialindex.html
- Anvil/Bedrock/Minetest formats: minecraft.wiki, wiki.vg, wiki.minetest.org
- mmap async hazard: huonw.github.io/blog/2024/08/async-hazard-mmap
- Windows File Buffering / IOCP: learn.microsoft.com/.../fileio/file-buffering
- compio: compio-rs/compio
- FastCDC (USENIX'16): usenix.org/conference/atc16/presentation/xia
- BLAKE3: github.com/BLAKE3-team/BLAKE3
- Caffeine/moka: github.com/ben-manes/caffeine, moka-rs/moka
- Postcard: postcard.jamesmunns.com
- async-trait: docs.rs/async-trait
- Obsidian/Steam Cloud conflict: obsidian.md/help/sync, simondalvai.org

---
*Bu rapor 6 paralel araştırma alt-ajanının (kademeli depolama, SQLite/KV, bölge dosyası/dedup, Windows async I/O, CDC/Merkle, save/cloud) bulgularının konsolidasyonudur. Kod değişikliği yapılmamıştır; plan 15 taslağının revizyonu için girdi niteliğindedir.*
