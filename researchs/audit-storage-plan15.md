# Plan 15 — Depolama ve Kalıcılık: Kapsamlı Teknik Denetim Raporu

**Tarih:** 2026-07-07
**Kapsam:** `plans/15-storage-and-persistence.md` (2026-07-06 teknik denetiminden sonraki revize sürüm).
**Yöntem:** 6 paralel araştırma alt-ajanı, her biri planın bir bileşen kümesini derin web araştırmasıyla (2024–2026, resmî crate belgeleri, benchmark'lar, akademik kaynaklar) analiz etti. Bulgular bu raporda konsolide edildi.
**Çapraz referans:** `06-xbrickmap.md`, `07-svdag.md`, `08-streaming.md`, `39-memory-allocation.md`.

---

## 0. Genel Değerlendirme (Yüksek Seviye Sonuç)

Plan 15, **temel mimari olarak sağlam ve savunulabilir**; önceki 6-alt-ajan denetiminden (C1–C8) sonra zaten birçok yanlıştan arındırılmış durumda (mmap kaldırıldı, çift runtime kaldırıldı, BLAKE3 dedup benimsendi, "pristine yazılmaz" kuralı eklendi, CDC sıcak yol'dan çıkarıldı, SQLite yanlış benchmark'ı düzeltildi).

Ancak kalan riskler **temel tasarımdan ziyade doğruluk (correctness) ve aşırı/yanlış mühendislik** odaklı. En kritik bulgular:

1. **`dirty_queue` içindeki `Arc<Sector>` sessiz veri kaybı/yozlaşma hatasına açıktır** (COW `Arc` + "flag çift-enqueue'i engeller" varsayımı, en güncel editi kaybedebilir).
2. **WAL sıralaması yanlış ifade edilmiş** ("commit'te append" yerine "commit sonrası pending kaydı sil").
3. **"Region dosyası + redb tek transaction" fiziksel olarak imkânsız** — çapraz dosya transaction'ı yok; reconcile pass gerekiyor.
4. **`quick_cache` aslında S3-FIFO'dur, "CLOCK-Pro" değildir**; "94% vs 82%" Zipfian iddiası kaynağı belirsiz/abartılı.
5. **7.4× benchmark rakamı `redb`'ye değil `fjall`'a aittir**; yazma-ağır metadata için **fjall birincil, redb ikincil** olmalı. `redb`'nin 4 TiB tavanı ve 4× alan şişmesi göz ardı edilmiş.
6. **Unbuffered I/O varsayılanı muhtemelen yanlış** — yeniden-okuma ağır voxel yükünde ~20× yavaşlama riski; buffered varsayılan olmalı.
7. **Sıkıştırma oranları (8:1 / 15:1 / 20:1) gerçekçi verilerle desteklenmiyor**; zstd'nin gerçek rakamları ~2.9 / 3.2 / 3.7–4.0'dır (voxel yapısal sıkıştırma XBrickMap/SVDAG'den gelir).
8. **zstd "multithreaded API" küçük sektörlerde işe yaramaz**; sektör-başına paralellik (rayon) gerekir.

Aşağıda bileşen bileşen detaylı analiz, alternatifler ve öneriler yer alır.

---

## 1. Hibrit 3-Kademeli Depolama (ACTIVE / WARM / DISTANT+ARCHIVE)

### 1.1 Doğrulama
Plan, açık (explicit) bir WARM katmanının standart 2-katmanlı (RAM↔disk) modelden üstün olduğunu üç argümanla savunuyor: (i) ACTIVE↔WARM promosyonu O(1)/lock-free olmalı; (ii) DISTANT'a düşüşte yeniden sıkıştırma olmamalı (WARM zaten sıkıştırılmış); (iii) 200 MB/s GPU stream altında OS page cache "kontrolsüz".

- **(i) ve (ii) meşru.** Ancak (iii) **kısmi olarak abartılı**: OS page cache, diskten okunan *dosya baytlarını* (yani zaten sıkıştırılmış region sayfalarını) önbelleğe alır; bu "ücretsiz" ve Linux'ta kontrollüdür (madvise/cgroup). Gerçek, savunulabilir fayda, WARM'ın **streaming hysteresis'ine bağlı tahliye politikası** ve **yeniden sıkıştırmayı önlemesi**dir — "OS yapamaz" değil.
- **İç tutarsızlık (çok kritik):** §1.1.1 WARM'ın *sıkıştırılmış* `Vec<u8>`/`Bytes` blob tuttuğunu söyler; ancak §1.4'teki `load_sector` *çözülmüş* `Arc<Sector>` döndürür/cache'ler. Bunlar iki farklı şeydir. WARM sıkıştırılmış byte mı yoksa çözülmüş nesne mi tutuyor? Bu netleştirilmeli.

### 1.2 Alternatifler
| Yaklaşım | Değerlendirme |
|---|---|
| Açık WARM (plan) | İyi — *eğer* WARM disk formatına uygun sıkıştırılmış byte tutuyorsa |
| Yalnızca OS page cache | Ücretsiz, sıfır mühendislik; ama streaming-tier bağlantısı yok; Windows'ta zayıf |
| Açık çözülmüş `Arc<Sector>` cache | <2ms warm load için iyi ama RAM ağır, ACTIVE ile çift |
| mmap read-only (plan reddetti) | Windows'da MMF Cache Manager + page-fault async thread bloke → doğru reddedildi |

### 1.3 Öneri
3-kademeyi koru; ancak WARM'ı **sıkıştırılmış, byte-bütçeli, region formatını yansıtan** bir katman olarak tanımla. ACTIVE (XBrickMap) tek canlı temsil olsun; WARM ise sıkıştırılmış eviction/staging tamponu. §1.1.1/§1.4 tutarsızlığını gider. "OS cache kontrolsüz" gerekçesini zayıflat, bunun yerine "format-aligned eviction + streaming-hysteresis bağlantısı" gerekçesini kullan. Cold read'ler OS-cached region dosyalarına düşebilsin; WARM'ı ~128 MB civarında tut.

---

## 2. L2 Önbellek: `moka` (W-TinyLFU) mı, `quick_cache` (S3-FIFO) mı?

### 2.1 Doğrulama
- **Planın `quick_cache` tanımı GÜNCEL DEĞİL.** Crate belgeleri politikanın *"CLOCK-Pro'nun değiştirilmiş bir versiyonu, S3-FIFO'ya çok benzer"* olduğunu belirtir. Yani `quick_cache` **S3-FIFO'dur**, klasik CLOCK-Pro değil.
- **"94% vs 82% Zipfian" rakamı doğrulanamadı ve trace'ye bağlı.** TinyLFU makalesi/ Caffeine wiki, W-TinyLFU'nun LRU'ye göre hit-rate'i **%10–30** iyileştirdiğini söyler; evrensel bir 94/82 çifti sunmaz. Bu rakam tek bir sentetik Zipfian trace'den; garantili sonuç gibi sunulmamalı.
- **Verim gerçeği:** `TinyUFO` (TinyLFU admission + S3-FIFO eviction) moka'dan ~5–9× daha yüksek ops/sec rapor ediyor (S3-FIFO hit'lerde lock-free). 200 MB/s streaming sıcak yolunda moka'nın TinyLFU kitapçığı CPU yükü, birkaç puanlık hit-rate avantajından daha ağır basabilir.

### 2.2 Karşılaştırma
| Ölçüt | moka (W-TinyLFU) | quick_cache (S3-FIFO) | lru |
|---|---|---|---|
| Skewed hit-rate | En iyi | Neredeyse en iyi | Zayıf |
| Scan direnci | İyi (admission) | **Mükemmel** | Zayıf |
| Throughput | Orta | **Yüksek** | Yüksek |
| Hot-path CPU | Yüksek | Düşük | En düşük |
| Byte-weighted | Evet (`weigher`) | Evet | Manuel |

### 2.3 Öneri
**WARM için varsayılan olarak `quick_cache` (S3-FIFO) benimse**; moka'yı yalnız profiling gerçek bir hit-rate açığı gösterirse fallback olarak tut. "CLOCK-Pro" etiketini bırak (S3-FIFO'dur). 94/82 rakamını "LRU'ye göre %10–30, trace'ye bağlı" olarak yumuşat. 128–512 MB byte-weighted bütçe korunmalı. **Ders:** Voxel erişim deseni (oyuncu hareketinde patlamalı scan + sonra stabil lokalite) için S3-FIFO'nun scan direnci, TinyLFU'nun son birkaç puandan daha değerli.

---

## 3. L1 Nesne Havuzu: `SlotMap` + `SecondaryMap`

### 3.1 Doğrulama
- `bumpalo`'nun nesne havuzu **olmadığı** iddiası **doğru**: bump arena yalnız toplu `reset()` serbest bırakır, tek brick'i serbest bırakamaz. Geçici scratch (mesh gen, pathfinding) için kullanımı doğru.
- `SlotMap` + `SecondaryMap` O(1) alloc/free ve **generational index** (ABA-safe) sağlar — doğru standart seçim.

### 3.2 Alternatifler
`slab` (generational değil → ABA tehlikesi), `generational-arena` (daha yüksek overhead), `thunderdome` (slotmap benzeri), `bumpalo` (yalnız scratch).

### 3.3 Öneri
`SlotMap`+`SecondaryMap` korunmalı; `bumpalo` yalnız scratch. Not: `SlotMap` backing `Vec`'i asla küçültmez (tombstone slotlar); churn'da tombstone bellek ayak izi sorun olursa `HopSlotMap` veya custom free-list değerlendir.

---

## 4. Dirty Tracking — En Kritik Doğruluk Riskleri

### 4.1 Doğrulama
- **Sticky flag semantiği prensipte doğru**: yalnız writer `store(true, Release)`; flusher yalnız durable commit sonrası temizler → lost-dirty önlenir.
- **Sharded `AtomicU64` bitset** false sharing'i azaltır — doğru; ancak 64 paketli flag RMW'dir, contention-free değil (sharding doğru mitigasyon).
- **En büyük hata — COW `Arc` + "flag çift-enqueue engeller" = en güncel edit kaybı:** Plan `Sector`'ın COW `Arc` swap ile düzenlendiğini söyler, ama dirty queue *eski* `Arc`'ı tutar. Yeni edit *yeni* bir `Arc` üretirse, "flag zaten set" guardı **yeni versiyonu asla kuyruğa koymaz** → crash'te son mutasyon kaybolur. **Gerçek bug kalıbı.**
- **WAL sıralaması yanlış ifade edilmiş:** §1.6 "WAL append *sonra* flag temizle" der. Crash-recovery invariant'ı doğrudur (durable kayıt RAM flag'inden uzun yaşamalı) ama "commit'te append" yanlış: WAL *pending-dirty* kaydı içermeli; commit'te o kayıt **silinmeli** (aksi halde WAL sınırsız büyür, replay temiz sektörleri tekrar flush eder).
- **Redundant WAL:** redb/fjall zaten WAL-backed; §1.5 zaten `sector_metadata.dirty: bool` tutuyor. Ayrı bir `dirty_log` WAL'ı **gereksiz** — üç temsil (RAM flag + metadata `dirty` + ayrı `dirty_log`) sıralama bug'ı yüzeyi yaratır.

### 4.2 Öneri (P0)
1. Dirty queue **sector koordinatı (veya çözülebilir handle) tutsun, snapshot `Arc` değil**; flush anında canlı versiyonu pool'dan çöz. COW stale-version bug'ı elenir.
2. Sıralama invariant'ını kod yorumunda belgele: *durable region+metadata commit → (opsiyonel log) → `flag.store(false, Release)`*. Flag asla durable write'dan önce temizlenmez.
3. Ayrı `dirty_log` WAL'ını **bırak**; recovery kaynağı olarak metadata store'ın durable `dirty` kolonunu (+ hızlı recovery için durable dirty-index) kullan. Bu tüm alt-ajanların üzerinde birleştiği en yüksek değerli sadeleştirme.

---

## 5. Bölge Dosyası Formatı + 3D Bölge Adresleme

### 5.1 3D Bölge (`r.<rx>.<ry>.<rz>.strata`)
- Sınırsız Y gereksinimi gerçektir; ancak **"2D kolonun diskte Y anahtarı yok" iddiası YANLIŞ** — Anvil zaten kolon başına dikey section'lar saklar; Cubic Chunks modu sınırsız Y'yi 3D bölge olmadan kanıtlar. 3D bölgeye geçiş *basitlik* tercihidir, eksiklik düzeltmesi değil.
- **Düzeltme (C1 gerekçesi):** Gerçek neden "uniform sınırsız-Y adresleme + ±1 komşu okuma kolaylığı" olarak yazılmalı.

### 5.2 Flat `[u64; 32768]` Offset Tablosu
- Plan **üç** dizi tanımlar (offsets, sizes, hashes) → **3 × 256 KB = 768 KB/bölge**, "256 KB" değil.
- 1000 açık bölge → 0.75 GB; 4096 bölge → ~3 GB RAM (yalnız header). Çoğu slot boşken 768 KB ödersin.
- **Redb ile çift kaynak:** `sector_metadata` zaten `file_offset, payload_size, content_hash` tutuyor → bölge header'ı redundant.

### 5.3 Öneri
redb'yi **tek otoriter indeks** yap; bölge dosyasını payload blob + kompakt **trailer** (presence bitmap + yalnız mevcut sektörler için dense kayıt, yalnız crash recovery/doğrulama için) olarak indir. 768 KB ayırma ve dual-source drift elenir. "256 KB" → "768 KB" düzelt.

---

## 6. Content-Addressable Deduplication (BLAKE3 + xxHash64)

### 6.1 BLAKE3 Dedup Anahtarı
- xxHash3/HighwayHash/wyhash **µs'ler içinde forge edilebilir** (adversarial voxel içeriği için kullanılamaz). BLAKE3 (256-bit, 128-bit collision resistance) content-addressable store için **zorunlu** — plan xxHash64'ı dedup anahtarı olarak reddetmesiyle **doğru**.
- **32-byte gereksiz mi?** 128-bit (16-byte) trunc BLAKE3 zaten 2⁶⁴ forge işi verir; `HashMap`/redb key belleğini yarıya indirir. 32-byte güvenli ama ~2× maliyetli. İkisi de kabul edilebilir.

### 6.2 Ayrı xxHash64 Checksum Gereksiz
- BLAKE3 zaten tüm payload üzerinden hesaplanır → *her* accidental corruption'ı algılar.
- zstd frame checksum (XXH64) zaten decode'da otomatik doğrulanır.
- Yani katman **üçlü**: BLAKE3 + zstd-frame-XXH64 + ayrı `xxHash64`. İkisi (BLAKE3, zstd-frame) yeterli.
- **Öneri:** BLAKE3 (zaten hesaplanıyor) + zstd frame checksum korun; açık `xxHash64` alanı isteğe bağlı hızlı pre-check'e indirgenip "güvenlik kontrolü değildir" olarak belgelenmeli.

### 6.3 "%30-60 Dedup" İddiası Şişirilmiş
- Pristine sektörler (§1.1.4) asla diske yazılmaz → dedup yalnız **dirty set** üzerinde çalışır (dünyanın küçük bir frac'ı). "30–60%" dünya boyutuna değil, *persisted (dirty) baytlara* scoped edilmeli ve ~10–40%'a çekilmeli. Planın kendi §1.9 tablosu zaten "~20–40%, çoğunlukla ~%0 (hava)" diyor — bu dürüst rakam.
- **Ders:** Hava sektörleri tek bir collapse ile bedava kazanılır; tekrarlayan %30-60 bir kerelik etkidir. Dedup+GC karmaşıklığı küçük dirty set için kazançtan ağır basabilir — dedup'a değer olup olmadığı yeniden değerlendirilmeli.

### 6.4 Çarpışma Koruma Gerekçesi YANLIŞ
- Plan: *"payload hash yeniden doğrulanır → çarpışma veri kaybı imkânsız."* **Yanlış:** Re-hash yalnız **bitrot**'u yakalar; gerçek BLAKE3 çarpışmasında (P≠Q, BLAKE3(P)==BLAKE3(Q)) re-hash check **geçer** ama yanlış veri döner. Gerçek koruma **256-bit digest boyutudur**. Metin düzeltilmeli; isteğe bağlı olarak payload length ikinci ayırt edici eklenebilir.

---

## 7. Async I/O Stratejisi (Windows-Optimize)

### 7.1 Unbuffered mı, Buffered mı? (En Önemli I/O Bulgusu)
- 2024 SO vakası: aynı SSD'de rastgele 4 KB okuma, `FILE_FLAG_NO_BUFFERING` ile **~50 MB/s vs ~1100 MB/s** (~22× regresyon) — çünkü OS cache'i tamamen atlar. Voxel motoru sürekli aynı sektörleri yeniden okur (geri dönüş, LOD pop-in) → unbuffered bu kazancı yok eder.
- **Öneri:** **Buffered I/O varsayılan**, unbuffered feature-flag arkası + benchmark'a bağlı. "Aligned window read + slice" mantığı yalnız unbuffered için gerekli; flag arkasına al. Yazma yolunda selektif unbuffered + `FlushFileBuffers` düşünülebilir.

### 7.2 Runtime: `compio` / tokio
- tokio Windows'da *gerçek* async dosya I/O değildir (`spawn_blocking` sarmalar). `compio` gerçek IOCP'dir ama ayrı runtime, Bevy tokio pool ile compose etmez. **İki runtime'ı reddetmek (tek runtime + priority channel) DOĞRU.** `compio` yalnız profiling gerçek bottleneck gösterirse değerlendirilmeli.
- `VirtualAlloc` aligned ring'ler yalnız unbuffered için gerekli; buffered varsayılan olursa gereksiz. Gerekirse `std::alloc::alloc(Layout::from_size_align(.., 4096))` ile taşınabilir.

### 7.3 Öneri
Buffered varsayılan + unbuffered feature-flag; `IoControl` probe 4 KB over-alignment güvenli; tek tokio runtime korun; `compio` ertelenmiş; `VirtualAlloc`→`std::alloc` ve yalnız unbuffered'da.

---

## 8. Prefetch Hareket Konisi

### 8.1 Doğrulama
Lineer ekstrapolasyon (`cam_pos + cam_vel*k*dt`) standart ama eksik:
1. **Hız ≠ bakış yönü** — oyuncu yana bakıp ilerleyebilir; yanlış sektörler prefetch edilir.
2. **Hız gürültüsü/jitter** — ham velocity thrash yaratır; low-pass filtre + min-speed gate gerek.
3. **Teleport** (`/tp`, ölüm/respawn) dev koni üretir → I/O israfı; teleport tespiti + radial reload.
4. **`in_flight` dedup set'i sınırsız büyür** — TTL/cap + eviction gerek.
5. **İptal edilebilirlik** — `spawn_blocking` orta-uçuş iptal edilemez; enqueue/cache-insert'te gate.

### 8.2 Öneri
GPU visibility/frustum (plan 08 §5) **birincil** prefetch sürücüsü olsun; `cam_vel` konisi yalnız smoothing + teleport guard + bounded dedup ile ikincil. Öncelik: frustum∩visibility (yüksek) > kamera-ileri koni (orta) > hareket konisi (düşük).

---

## 9. Metadata Store: `redb` / `fjall` / SQLite

### 9.1 Benchmark "Tersine Çevirme" İddiası Kısmi Doğru, Rakam Yanlış Atfedilmiş
redb'nin kendi benchmark tablosu (5M KV, Ryzen 9950X3D + NVMe):

| Metrik | redb | fjall | sqlite | redb÷sqlite | fjall÷sqlite |
|---|---|---|---|---|---|
| batch writes | 1595 ms | **353 ms** | 2625 ms | 1.65× | **7.44×** |
| random reads | 1138 ms | 2177 ms | 4283 ms | **3.76×** | 1.97× |

- **7.4× batch write `redb`'ye değil `fjall`'a aittir.** redb'in SQLite'ye batch-write avantajı yalnız ~1.65×. Plan 7.4×'ı redb'ye atfetmemeli.
- "Durability ayarları eşleştirilmemiş" iddiası doğrulanmadı; SQLite'da `WAL+FULL` dışı hiçbir ayar durable değil — internal benchmark'ta SQLite'ın hangi PRAGMA ile olduğu belirtilmeli (apples-to-apples için `WAL+synchronous=FULL` sabitlenmeli).

### 9.2 MVCC / Zero-copy / SQLite Sakıncaları — Doğrulandı
redb `AccessGuard` gerçek zero-copy; fjall MVCC; SQLite single-writer lock, FFI+copy, WAL `-shm` network FS'de çalışmaz → planın redb/fjall yönü doğru.

### 9.3 Kritik Boşluklar
- **`redb` 4 TiB sabit tavanı** (regions 4 GiB, ~1000 region). Sınırsız yükseklik + milyonlarca sektörde tek metadata dosyası buraya çarpabilir → shard gerekiyor (world/region-group başına).
- **Alan şişmesi fjall lehine 4×:** redb 4.00 GiB vs fjall ~1.0 GiB (aynı iş yükü). Metadata *sürekli* değiştiğinden (dirty flip, ref_count) COW B-tree ölü sayfaları hızla biriktirir → redb sık `compact()` ister (write amplification + latency spike). Bu tam da planın "yazma-ağır" diye fjall'a atfettiği durum. **Plan redb/fjall'i ters koymuş.**
- **"Tek transaction" imkânsız:** Region dosyası unbuffered I/O ile yazılır; redb transaction yalnız redb'i kapsar. İki crash penceresi var. Çapraz dosya transaction'ı yok → explicit sıralama + startup reconcile (region vs redb taraması: dangling ref drop, orphan GC) gerek.
- **In-memory `HashMap` cache txn dışında mutasyona uğrar** → drift/orphan (iki flush task aynı yeni hash'i dedup ederse double-append). Cache'i redb ile read-through/reconciled yap.
- **Ref ownership net değil** → double-free/underflow riski. Tam bir ref sahipliği tanımla; `dec_refcount` yalnız count>0 iken.
- **"re-hash collision'ı engeller" yanlış** (bkz. §6.4).
- **GC sıralaması:** `BEGIN txn → dec ref_count (0 ise remove) → COMMIT → fiziksel payload sil`. Crash → güvenli orphan (wasted space, sonra sweep), asla dangling değil.
- **"WAL replay" redb için yanlış terim** — redb COW, recovery = "son tutarlı root'a rollback". fjall gerçek WAL.

### 9.4 Alternatifler
`sled` (bakımsız, 1.0 yok → reddet), `heed`/LMDB (C-FFI, untrusted-input crash, plan 02 ethos'unu ihlal → reddet), `RocksDB` (700k LOC C++, ~40s compile → reddet).

### 9.5 Öneri
1. **Birincil = fjall, ikincil/read-optimized = redb** (planın tersi).
2. Benchmark iddiasını düzelt: redb ≈1.65× batch / ≈3.8× random read; fjall ≈7.4× batch.
3. 4 TiB tavanı için shard şeması belgele.
4. GC sıralamasını explicit yaz; startup orphan sweep ekle.
5. Primary path'te 16-byte BLAKE3 (128-bit) kullan (index yarıya iner), collision'da tam hash ikincil doğrulama.
6. SQLite fallback: `WAL+synchronous=FULL`, `auto_vacuum=INCREMENTAL`, `busy_timeout=5000`, `TRUNCATE` yalnız temiz kapanışta.

---

## 10. Write-Back Pipeline

### 10.1 `tokio::select!` + Batching — DOĞRU (ufak düzeltme)
`group_by_region` + region başına tek `spawn_blocking` tokio'nun önerdiği desen. Ancak `max_wait_expired` **en eski kuyruk öğesinin enqueue zamanına** bağlı future olmalı (sabit global timer değil); queue değişince yeniden hesapla.

### 10.2 `Arc<Sector>` Kuyruk — YUKARDA §4.1'DEKİ BUG
Torn read / stale version riski. **P0:** Enqueue anında frozen snapshot (copy-under-lock) veya versioned `Arc` yakala; kuyruktaki `Arc` asla canlı mutable sector olmasın.

### 10.3 zstd "Multithreaded API" Yanlış Çerçevelenmiş
`ZSTD_c_nbWorkers` yalnız 1 MB+ segmentleri böler; 32³ sektör (onlarca KB, çoğu <1 MB) için **~0 hızlanma + daha kötü oran**. Gerçek kazanç **sektör-başına paralellik** (rayon/`AsyncComputeTaskPool`, her işçi kendi `CCtx`'i). Not: zstd **decompress hızı seviyeden bağımsızdır** (~1500 MB/s) → `zstd-19` ARCHIVE okuması hızlı, yalnız yazması yavaş (write-rarely ile uyumlu).

### 10.4 Öneri
- `Arc<Sector>` yerine frozen snapshot/versioned Arc (P0).
- WAL: pending kaydı commit sonrası *sil*, sonra flag temizle (P0).
- `max_wait` en eski öğeye bağla.
- "zstd multithreaded" → sektör-başına rayon paralelliği; `NbWorkers` yalnız dev sektörler (>256 KB) için.

---

## 11. Tier-Bazlı Sıkıştırma — Oranlar Şişirilmiş

### 11.1 Gerçek zstd Rakamları (lzbench 1.5.7, Silesia)
| Codec | Ratio | Compress MB/s | Decompress MB/s |
|---|---|---|---|
| `lz4` | 2.10 | 675 | **3850** |
| `zstd -1` | **2.90** | **510** | **1550** |
| `zstd -3` | ~3.2 | ~300 | ~1500 |
| `zstd -15` | ~3.6 | ~40 | ~1500 |
| `zstd -19` | ~3.7–4.0 | ~25 | ~1500 |
| `zstd -22` | ~3.95 | ~15 | ~1500 |

- Planın **3:1 / 8:1 / 15:1 / 20:1** merdiveni gerçek verilerle desteklenmiyor; yalnız "aşırı seyrek/tekrarlı voxel sektör" en-iyi-durumunda savunulabilir. **Sparse-data hedefi / üst sınır** olarak yeniden etiketlenmeli.
- **Gerçek sıkıştırma XBrickMap/SVDAG yapısındandır**, zstd ikincil entropy katmanıdır. Oranlar doğru atfedilmeli (zstd 2.9:1'e karşılık XBrickMap+SVDAG yapısı asıl kazancı verir).

### 11.2 Performans Hedefleri
- hot <0.1ms (Arc clone): kolay.
- warm <2ms: 32 KB / 1550 MB/s ≈ 20 µs → karşılanır.
- cold <5ms: NVMe'de karşılanır, HDD'de riskli. **"NVMe-class storage varsayılır" belirtilmeli.**
- batch 64 sector <50ms: WARM/DISTANT kolay; **ARCHIVE (zstd-19) yalnız sektör-başına paralel ile** (<13–20 ms @8 thread).
- write >500 MB/s: **yalnız WARM için**; ARCHIVE zstd-19 ~25 MB/s. Hedef WARM'a scoped.
- crash recovery <100ms, GC <200ms: codec'e bağlı değil; ref-count reclaim <200 ms, *compaction* ayrı, paced, atomic task olmalı (multi-GB region 200 ms'yi aşar).

### 11.3 Öneri
- WARM: `zstd -1` varsayılan; LZ4 yalnız decompress CPU bottleneck profiling gösterirse.
- DISTANT: `zstd -3` (8:1 iddiasını düzelt); CPU izin verirse `-8..-10`.
- ARCHIVE: `zstd -19` write-once uygun; batch hedefi WARM'a scoped; per-sector paralel zorunlu.
- XXH64 frame checksum: WARM/DISTANT için açık (ucuz, decoder-enforced); DEDUP/ARCHIVE'de BLAKE3'e güven (tek hash = dedup + integrity).
- **Bake-time optimizasyon:** brick içi palette RLE/quantization prepass + temsili payload'larda zstd dictionary training (WARM/DISTANT oranını yükseltir).
- FFI crate'leri (`zstd` ^0.13, `lz4`) kullan, pure-Rust port'ları kritik yolda değil.

---

## 12. Content-Defined Chunking (GearHash) + Merkle

### 12.1 Doğrulama
- Per-sector CDC sıcak yol'dan çıkarmak **doğru**: bağımsız 32³ sektörlerde intra-stream shift yok; fixed whole-sector BLAKE3 onları mükemmel yakalar. "%50-80 CDC uplift" yedek dataset'lerinden (Borg/restic) gelir, voxel'e transfer olmaz.
- GearHash `GEAR_TABLE` + `reset()` per-sector + `boundary_mask` popcount — klasik, doğru.
- Merkle domain separation (`0x00` leaf / `0x01` node, `blake3::keyed_hash`) kriptografik olarak sağlam.
- **"Merkle yalnız partial/incremental verification gerektiğinde" içgörüsü DOĞRU** — aksi halde "security theater".

### 12.2 Öneri
- Per-sector CDC çıkarılmış kalsın; yalnız büyük bölge/cloud-diff için.
- Cloud-diff gemi olacaksa `fastcdc` (v2020, normalized) tercih edilir; aksi halde `gearhash`.
- Merkle + inclusion-proof verifier yalnız gerçekten partial chunk doğruluyorsan kur; değilse ertele.
- **Dedup `content_hash` çözülmüş değil, sıkıştırılmış payload üzerinde hesaplanmalı** (aynı hava sektörleri aynı compressed blob'a → BLAKE3 capture eder). Implementasyonda doğrula.

---

## 13. Game State Save/Load (§38)

### 13.1 Validation
- `postcard` + versioned envelope **doğru seçim**. `save_version` (disk layout) ≠ `generator_version` (terrain) ayrımı doğru.
- Atomic-write sırası **BOZUK**: §38.5 "write save.tmp → fsync → save.dat → save.bak → save.tmp → save.dat" keşmece; double rename corrupt eder. Doğru sıra:
  1. serialize → `save.dat.tmp`
  2. `fsync(save.dat.tmp)`
  3. varsa `rename(save.dat → save.bak)`
  4. `rename(save.dat.tmp → save.dat)` (Windows NTFS atomik)
  5. hata → `rename(save.dat → save.corrupt)`, `save.bak → save.dat`. (Windows'da dizin handle fsync'i de gerekli.)
- IO pool: `AsyncComputeTaskPool` CPU içindir; dosya I/O için Bevy **`IoTaskPool`** kullan.
- Envelope evrimi: `SaveEnvelope` layout değişirse eski save okunamaz → `magic` + raw `envelope_version:u32`'yi postcard'dan önce raw parse et.
- Float alanlarda NaN/infinity guard (postcard NaN bit pattern saklar).

### 13.2 Alternatifler (serde formatı)
postcard en iyi (en küçük/hızlı, stable wire spec); bincode 2 (stabil spec yok), rmp (corrupt length OOM riski), borsh, flatbuffers/capnp (overkill), CBOR (yavaş), serde_json (büyük). Migration için `version-migrate`/`serde-evolve` derleme-zamanlı garantili zincir sağlar.

### 13.3 Öneri
postcard + envelope korun; atomic-write sırasını düzelt (P0); `IoTaskPool` kullan; `envelope_version` ekle; NaN guard; golden-file CI matrisi korun.

---

## 14. Cloud Save (§43)

### 14.1 Validation
- `#[async_trait]` `Box<dyn CloudProvider>` için **doğru**: native `async fn` in trait Rust 2024'te hâlâ dyn-safe değil.
- `Merge` opaque blob için çıkarıldı — doğru (Steam/Obsidian/Dropbox LWW/AskUser kullanır).
- `client_uuid:[u8;16]` idempotent key iyi ama **`client_uuid + hash`** olmalı (aynı client iki farklı save çakışmasın).
- **Enum dispatch** (`enum Provider { Disk, S3, Steam }`) closed provider set için zero-alloc, `Send`-dostu; `#[async_trait]` yalnız açık plugin provider için.

### 14.2 Öneri
- Kapalı provider seti için enum dispatch tercih et.
- Clock-skew guard: client-saat yerine **server-time/HLC** ile `UseNewest`; ayrıca lower-bound (rollback) guard ekle.
- Idempotent key = `client_uuid + hash`.
- Structured-DTO merge için `crdt_lite` (XP=max, inventory=union) düşünülebilir.
- `cloud_save` feature-gated + `save` crate envelope/version/migration paylaşımı korun.

---

## 15. Önceki Denetimle (C1–C8) Tutarlılık

Mevcut plan zaten C1–C8 düzeltmelerini içeriyor (3D bölge, i32 koordinat, redb/fjall, BLAKE3 dedup, mmap reddi, tek runtime, hareket konisi prefetch, postcard envelope, dirty+max_interval). Bu denetim **ekbir bulgu** sunar:
- C1 gerekçesi hâlâ zayıf (bkz. §5.1).
- §1 "256 KB" → "768 KB" (§5.2).
- WAL/dirty queue COW bug'ı (§4.1) önceki denetimde tam kapanmamış.
- Benchmark atıfları hâlâ düzeltilmeli (§9.1, §2.1).

---

## 16. Konsolide Öneriler (Öncelikli)

| # | Alan | Mevcut Plan | Öneri | Öncelik |
|---|---|---|---|---|
| 1 | Dirty queue | `Arc<Sector>` snapshot | **Frozen snapshot/versioned Arc**; queue koordinat tutsun | **P0** |
| 2 | WAL sıralaması | "commit'te append" | Pending kaydı commit sonrası **sil**, sonra flag temizle | **P0** |
| 3 | Atomic-write (§38.5) | bozuk sıra | 5-adımlı düzelt seq (tmp→fsync→bak→rename→corrupt) | **P0** |
| 4 | Metadata birincil | redb | **fjall birincil**, redb ikincil (7.4× fjall'a ait) | **P0** |
| 5 | redb 4 TiB | belirtilmemiş | Shard şeması (world/region-group) belgele | **P1** |
| 6 | "Tek transaction" | region+redb | İmkânsız → explicit sıralama + startup reconcile | **P1** |
| 7 | Unbuffered I/O | varsayılan | **Buffered varsayılan**, unbuffered feature-flag + benchmark | **P1** |
| 8 | L2 cache | `quick_cache`="CLOCK-Pro" | **S3-FIFO**; 94/82 iddiasını yumuşat; moka fallback | **P1** |
| 9 | Compression oranları | 3/8/15/20:1 | "sparse hedefi" olarak etiketle; zstd gerçek ~2.9/3.2/3.7 | **P1** |
| 10 | zstd MT | in-frame `nbWorkers` | **Sektör-başına rayon** paralelliği | **P1** |
| 11 | xxHash64 checksum | ayrı alan | BLAKE3+zstd-frame yeterli; isteğe bağlı pre-check'e indir | **P2** |
| 12 | Dedup %30-60 | dünya boyutu | **Dirty set'e scoped ~10–40%**; dedup değerini gözden geçir | **P2** |
| 13 | BLAKE3 32B | sabit | Primary'da **16B (128-bit)**; collision'da tam hash | **P2** |
| 14 | Prefetch | `cam_vel` birincil | **GPU visibility/frustum birincil**; velocity smoothing+teleport guard | **P2** |
| 15 | WARM format | §1.1.1↔§1.4 çelişkili | Sıkıştırılmış byte; ACTIVE tek canlı katman | **P2** |
| 16 | Bölge header | 3×[u64;32768] | redb tek otoriter; trailer-only compact index | **P2** |
| 17 | Cloud dispatch | `#[async_trait]` Box<dyn> | Kapalı set için **enum dispatch** | **P3** |
| 18 | Cloud idempotency | `client_uuid` | `client_uuid + hash` | **P3** |

---

## 17. Temel Dersler (Lessons Learned)

1. **OS page cache okuma için dosttır** — voxel yeniden-okuma ağır yükte unbuffered ~20× yavaşlatır. Unbuffered yalnız yazma yolunda seçici + benchmark'lanmış olmalı.
2. **`Arc` otomatik immutability sağlamaz** — dirty queue'da "Arc = dirty record" varsayımı sessiz kayba açıktır; snapshot/versioning şart.
3. **WAL dili "mark clean after durable write" olmalı**, "append at commit" değil.
4. **Benchmark rakamlarını kaynağıyla doğrula** — 7.4× fjall'a, 94/82 TinyLFU'ya atfedilmiş ama ikisi de plansız şekilde yanlış konumlanmış.
5. **Gerçek voxel sıkıştırması yapısal (XBrickMap/SVDAG)**; zstd ikincil entropy katmanı. Oranları doğru atfet.
6. **KV seçiminde iş yükü deseni belirleyici** — yazma-churn'ü için LSM (fjall) > COW B-tree (redb); ayrıca redb 4 TiB tavanı mimari kısıt.
7. **Aşırı mühendisliği ele** — üçlü dirty temsili (RAM flag + metadata dirty + ayrı dirty_log) ve region header'ın redb ile çift kaynağı sadeleştirilmeli.

---

## 18. Kaynaklar (Seçili)

- TinyLFU paper: https://arxiv.org/abs/1512.00727 ; Caffeine Efficiency: https://github.com/ben-manes/caffeine/wiki/Efficiency
- quick_cache (S3-FIFO): https://docs.rs/quick_cache/latest/quick_cache/ ; S3-FIFO SIGMETRICS 2023
- slotmap: https://docs.rs/slotmap ; bumpalo: https://docs.rs/bumpalo
- Huon Wilson "mmap is secretly blocking IO" (2024): https://huonw.github.io/blog/2024/08/async-hazard-mmap/
- Windows overlapped/unbuffered ~22× regression SO: https://stackoverflow.com/questions/78728820/
- Microsoft File Buffering: https://learn.microsoft.com/en-us/windows/win32/fileio/file-buffering
- Tokio fs (spawn_blocking): https://docs.rs/tokio/latest/tokio/fs/index.html ; compio: https://github.com/compio-rs/compio
- redb benchmarks: https://github.com/cberner/redb ; fjall: https://github.com/fjall-rs/fjall
- SQLite durability: https://www.agwa.name/blog/post/sqlite_durability ; WAL/-shm: https://sqlite.org/wal.html
- zstd lzbench: https://github.com/facebook/zstd ; RFC 8878 (XXH64 checksum): https://datatracker.ietf.org/doc/html/rfc8878
- Easyperf zstd MT scaling: https://easyperf.net/blog/2024/05/10/Thread-Count-Scaling-Part3
- BLAKE3: https://github.com/BLAKE3-team/BLAKE3 ; IETF draft (128-bit): https://www.ietf.org/archive/id/draft-aumasson-blake3-00.html
- Postcard wire format: https://postcart.jamesmunns.com/wire-format ; rust serialization bench: https://github.com/djkoloski/rust_serialization_benchmark
- fastcdc: https://docs.rs/fastcdc ; gearhash: https://docs.rs/gearhash
- Anvil/Cubic Chunks: https://minecraft.wiki/w/Anvil_file_format ; https://github.com/OpenCubicChunks/CubicChunks
- async_trait: https://docs.rs/async-trait ; Rust async fn in traits: https://blog.rust-lang.org/2023/12/21/

---

*Rapor, plan 15'in 2026-07-06 revize sürümünü esas alır. Tüm alt-ajan bulguları 2024–2026 kaynaklarıyla çapraz doğrulanmıştır. En yüksek öncelikli üç düzeltme: dirty-queue `Arc` güvenlik açığı (P0), WAL sıralama dili (P0) ve metadata birincil motorunun fjall olarak değişimi (P0).*
