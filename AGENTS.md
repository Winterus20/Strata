# STRATA: YAPAY ZEKA AJANLARI İÇİN GELİŞTİRİCİ REHBERİ (AI AGENT GUIDELINES)

**DİKKAT (AI):** Bu dosya projeyi anlaman, sistem mimarisinin kesin hatlarını kavraman ve Strata voxel motoruna kod geliştirirken vizyona %100 sadık kalman için özel olarak tasarlanmıştır. Bu rehberdeki prensipler **mutlak gerçek (ground truth)** niteliğindedir.

## 1. PROJE ÖZETİ VE VİZYON
Strata, Rust programlama dili ve Bevy ECS 0.18+ üzerine inşa edilen, ultra yüksek performanslı, yeni nesil bir voxel oyun motorudur. Amacımız; Minecraft gibi oyunlardaki sınırlı yükseklik, yavaş CPU chunk meshing problemleri ve bellek sorunlarını kökünden çözmektir. Motor; "Sınırsız Yükseklik" (Cubic Chunks), "GPU-driven rendering/Raytracing" ve "Data-Oriented Design (DOD)" felsefesiyle tasarlanmaktadır.

## 2. KESİN VE DEĞİŞTİRİLEMEZ BİLGİ KAYNAKLARI VE EVRİM SÜRECİ
Projedeki tüm planların güncel durumu, aşamaları ve indeksi her zaman **`plans/01-overview.md`** dosyasında bulunur. Planlar hakkında genel bir bakış açısı edinmek için öncelikle bu dosyaya başvur.

**Plan olgunluğu** (`plans/01-overview.md` §1.1): **`01`–`16` kesinleşmiş** (anayasa); **`17`–`38` taslak** (değişebilir). Çelişkide her zaman `01`–`16` önceliklidir.

Aşağıdaki plan dosyaları **TAMAMLANMIŞTIR** ve projenin anayasasıdır. Üreteceğin tüm kodlar ve mimari kararlar bu dosyalara koşulsuz şartsız dayanmak zorundadır:

*   **`plans/01-overview.md`**: Master indeks, mimari harita, plan olgunluk seviyeleri.
*   **`plans/02-implementation.md`**: Crate organizasyonu, uygulama sırası, sözlük.
*   **`plans/03-ecs-architecture.md`**: ECS state, sistem setleri, BlockEntity vs SectorPalette (§10.6.1).
*   **`plans/04-plugin-api.md`**: Plugin trait, lifecycle, hook sistemi.
*   **`plans/05-block-registry.md`**: Block registry SoA, TOML, SectorPalette, state ownership.
*   **`plans/06-xbrickmap.md`**: XBrickMap, 4-seviyeli palet, GlobalBrickPool, GPU/WGSL.
*   **`plans/07-svdag.md`**: SVDAG (uzak alan), Shared Node Pool + epoch GC, snapshot bake/unbake, 32³ shallow SVDAG, 4-tier ghost pages, ECS (`SectorSvdag`, `NeedsSvdagBake`).
*   **`plans/08-streaming.md`**: 4-tier streaming orkestrasyonu, hysteresis, StreamingManager, GPU feedback öncelik, predictive prefetch, yaşam döngüsü/AOI.
*   **`plans/09-meshing.md`**: Binary greedy meshing, PackedQuad (8B), OccupancyScratch, GigaBuffer (`offset-allocator`), ECS `NeedsRemesh` + async mesh, tier stratejisi (CachedGreedy WARM), NonGreedy transparent/cutout.
*   **`plans/10-render-pipeline.md`**: Unified visibility buffer (64-bit, Aokana Figure 7 layout), atomicMax depth test, Hi-Z occlusion + re-execution, tile–chunk pairs, VRCS (foveated shading), XBrickMap ray trace + SVDAG ray march, seam/crack yönetimi, HDR + ACES + bloom.
*   **`plans/11-world-gen.md`**: Density-function terrain (batch `GenUniformGrid3D`, column caching), Whittaker biome diagram (cached per-column), hybrid cave (3D noise isosurface + worm), hash-grid structure placement, template trees, thermal erosion, PCG32+wyhash RNG, `DensityNode` compile-time flatten (data-driven modding), `WorldGenPlugin` (AsyncComputeTaskPool), WIT modding hooks.

**EVRİMLEŞME KURALI (AI DİREKTİFİ):**
Bu `AGENTS.md` dosyası yaşayan bir belgedir. `11`+ bir plan kesinleştiğinde: `01-overview.md` §1.1 tablosu, ilgili planın durumu ve bu dosyadaki özet güncellenmelidir.

*(Önemli Not: `17`–`38` taslaktır — eleştirel oku; `01`–`16` ile çelişirse taslak revize edilir, anayasa değil.)*

---

## 3. MİMARİ TEMELLER (HIZLI REFERANS)

Sürekli ilgili markdown dosyalarına dönmemen için bilmen gereken en kritik kısımların özetini aşağıda bulabilirsin:

### A. BEVY ECS KULLANIM KURALLARI (Ref: 03-ecs-architecture.md)
Sistem tasarlarken kesinlikle uyman gereken Data-Oriented prensipler:

1.  **Filter-First Yaklaşımı (En Kritik Kural):** Query'ler içinde hiçbir zaman tüm entity'leri çağırıp `if option.is_some()` tarzı "Per-entity check" YAPMA. Filtrelemeyi her zaman `With<T>`, `Without<T>` veya Zero-Sized Type (ZST) component'ları (`ChunkDirty`, `NeedsRemesh` gibi) ile *archetype seviyesinde* hallet.
2.  **SoA (Structure of Arrays) vs AoS:** Component'ları olabildiğince parçala. "Hot" (her frame erişilen - örn: Transform) ve "Cold" (nadir erişilen - örn: Metadata) verileri asla aynı Struct içinde tutma.
3.  **Change Detection (Guard Mantığı):** Bevy'de `mut` referans aldığın an, bir atama yapmasan bile `Changed<T>` tetiklenir. Yalnızca veri gerçekten değişiyorsa atama yap: `if old_val != new_val { old_val = new_val; }`.
4.  **Plugin-First Architecture:** Tüm alt sistemleri (Player, World, Physics vb.) ayrı birer Bevy Plugin olarak tasarla ve ana `StrataPlugin` içinde izole bir şekilde birbirine bağla.

### B. XBRICKMAP / KÜBİK CHUNK YAPISI (Ref: 06-xbrickmap.md)
Strata'daki dünya klasik 16x256x16'lık dikey kolonlar **DEĞİLDİR**. Dünyamız her eksende sınırsız ve **32x32x32**'lik Kübik Sektörlerden (Sector) oluşur.

1.  **Hiyerarşik Bitmask Yapısı:**
    *   **Sector (32x32x32):** Bir adet `u64` maskesiyle hangi 8x8x8 Brick'lerin dolu olduğunu bilir (Toplam 64 adet). Sadece dolu Sektörler/Brick'ler RAM'de yer kaplar.
    *   **Brick (8x8x8):** İçinde bir `u64` mask ile hangi 2x2x2 Sub-brick'lerin dolu olduğunu tutar.
    *   **Sub-Brick (2x2x2 = 8 voxel):** Bir `u8` maskesi ve renk paleti indeksleri tutar.
2.  **3-Seviyeli Palet ve Sıkıştırma:** 512 hava blokluk tek tip bir Brick bellek üzerinde 1024 Byte yerine sadece **8 Byte (mask)** kaplar. Boşlukların (Sky, Cave) sisteme yükü pratikte "Sıfır"dır.
3.  **Heap Fragmentation Kesinlikle Yasaktır:** Sektörlerin içi dinamik olarak `Vec` KULLANAMAZ! Blokların kırılıp konulması sürekli memory allocation yaratır. Bu yüzden tüm Voxel verisi `GlobalBrickPool` adı verilen global bir resource üzerinde, **SlotMap** (O(1) allocation/deallocation) ve **SecondaryMap** ikilisiyle tutulur.
4.  **GPU Optimizasyonları ve WGSL:**
    *   **Vertex Packing:** Her Vertex tam olarak **4 Byte**'tır (Pos + Normal + UV + Color tek u32'ye sıkıştırılmış).
    *   **Branchless Ray-Tracing:** WGSL (Shader) döngülerinde ASLA GPU wavefront'u bölen `if-else` dallanmaları kullanılmaz. Bunun yerine `select`, `firstTrailingBit` gibi donanım spesifik intrinsic fonksiyonlarla boşluklar aydınlatma hızında (branchless) atlanır.
    *   **GPU Feedback Loop:** Hangi chunk'ın render edileceğine CPU değil, GPU karar verir. Işının çarptığı chunk'lar bir SSBO'ya atomic operasyonlarla kaydedilir ve CPU bu veriyi okuyarak SADECE o Sektörlerin verisini GPU'ya aktarır. (Müthiş PCIe Bandwidth tasarrufu).

### C. SVDAG / UZAK ALAN (Ref: 07-svdag.md)
Yakın edit **XBrickMap** (`06`); uzak görüntüleme ve Tier 2+ **SVDAG** (`07`).

1.  **Birim = 32³ sektör:** Her sektör kendi sığ SVDAG'ına sahip (max depth ~5). Aokana 256³ edit chunk'ı **kullanılmaz**.
2.  **Bake kaynağı:** `Arc<CompressedChunkData>` snapshot (`06` §1.4) — canlı `Sector` üzerinden doğrudan bake yok.
3.  **Palet:** LOD-0 yaprak = `u8` palet indeksi; unbake `SectorPalette::get_or_insert` (`05` §14).
4.  **4-tier mesafe:** ACTIVE &lt;96m, WARM 96–384m, DISTANT 384–1536m, ARCHIVE ≥1536m (`08` ile hizalı).
5.  **Ghost page table:** SVDAG yüklenirken XBrickMap render devam eder (GigaVoxels DP starvation-free geçiş).
6.  **ECS:** `SectorSvdag`, `NeedsSvdagBake` / `NeedsSvdagUnbake`, `SvdagSnapshot` ayrı network kanalı (`03` §4.4 ile uyumlu).

### D. 4-Tier Streaming (Ref: 08-streaming.md)
Streaming, sektörlerin hangi tier'da olduğunu, ne zaman yükleneceği/boşaltılacağını orkestre eder.

1.  **Tier karar:** `SectorTransform.tier` authoritative kaynak; `SectorEntity.tier` spawn-only immutable.
2.  **Hysteresis:** Komşu tier flip'i bastırılır (`enter_extra_m = 16`); non-adjacent geçişlerde raw tier.
3.  **Dual representation (WARM):** XBrickMap + SVDAG aynı anda; ghost page ile starvation-free.
4.  **Delegation:** `StreamingManager` politika kuyruğu; `SvdagStreamingManager` icra kuyruğu (VRAM + LRU).
5.  **GPU feedback:** `06` SSBO'dan görünür sektörler **High** öncelik; “tüm komşuları yükle” yerine çizilen sektörler.
6.  **Event consumer'lar:** `SectorLoaded/Unloaded` → physics (collider), lighting (lightmap), network (AOI).

### E. MESHING (Ref: 09-meshing.md)
Yakın mesafe görüntüleme **CPU binary greedy** + **GPU vertex pulling**; uzak mesafe `07` SVDAG.

1.  **Birim:** 32³ sektör mesh; 64³ padded buffer yok — kenar için **komşu sektör ±1 okuma** (`09` §4.0).
2.  **Hot path:** `OccupancyScratch` (heap-free bitmask); `PackedQuad` 8B/quad; `VisibilityTable` (`05` §18).
3.  **WARM tier:** `CachedGreedy` — ACTIVE mesh GigaBuffer'da kalır, re-mesh 0µs (`08` §3.3).
4.  **Incremental:** `NeedsRemesh` ZST + `AsyncComputeTaskPool`; `World::get_sector` bypass yasak (`03`).
5.  **VRAM:** `GigaBuffer` + `offset-allocator` (TLSF); opaque / transparent ayrı draw batch.
6.  **AO:** 0=occluded, 3=open; `AO_CURVE` uniform; opsiyonel Faz 1b `ao_safe` greedy (`block-mesh-bgm`).

### F. RENDER PIPELINE (Ref: 10-render-pipeline.md)
GPU compute-driven, unified visibility buffer pipeline. Tüm tier'lar aynı 64-bit buffer'a yazar.

1.  **9-pass pipeline:** Depth Pre-Pass → Tile Selection + Hi-Z → XBrickMap RT → SVDAG RM → Hi-Z Re-Execution → VRCS Color Resolve → Entity Composite → Deferred Lighting + HDR → Hi-Z Build.
2.  **Visibility Buffer (64-bit, Aokana Figure 7):** bit[0:23] voxel_pos, bit[24:36] sector_id, bit[37:39] normal, bit[40:63] depth (reversed-Z). `atomicMax` ile en yakın piksel seçimi.
3.  **Tile–Chunk pairs:** Ekran 8×8 tile, her tile'dan screen ray → görünür sektör çiftleri → indirect dispatch.
4.  **Hi-Z Re-Execution:** Aokana katkısı — önceki frame Hi-Z ile culled tile'lar mevcut frame Hi-Z ile tekrar test; ghosting eliminasyonu (~0.2ms).
5.  **VRCS (DOOM DtDA):** Fovea 1:1, Mid 2×2, Periferik 4×4 tile shading; ~%60-80 thread azalması.
6.  **Beam optimization:** Hi-Z'den ray başlangıç tahmini; açık alanlarda %30-40 traversal hızı.
7.  **Seam/crack:** Border-aware traversal + TierBlendManager (ECS Component) + mip discontinuity düzeltme.

---

## 4. YAPAY ZEKA GELİŞTİRİCİSİ OLARAK UYMAN GEREKEN KURALLAR (AI DIRECTIVES)

*   **Öneri/Kod Getirirken:** Her değişiklik `01`–`10` anayasa'ya uymalıdır (özellikle `03` ECS, `06` XBrickMap, `07` SVDAG, `08` Streaming, `09` Meshing, `10` Render). Performansı öldürecek OOP/kolay yollara başvurma.
*   **UI, world-gen vb. (`17`+):** Taslak planlarla çelişki varsa `01`–`16` esas alınır; taslağı revize et veya kullanıcıya bildir.
*   **Tekerleği Yeniden İcat Etme:** Rust projelerinde zaten projeye dahil edilmiş `slotmap`, `dashmap`, `bevy::prelude::*`, `bytemuck` gibi kütüphanelerin sağladığı optimizasyon yollarını kullan. Voxel veya sistem mimarisi için dışarıdan yeni, sisteme ağır gelecek çözümler önerme.
*   **Performans her şeydir:** Bir sistemi sadece çalışması için yazma. Bellek satırı (Cache Line) uygunluğu, paralellik (Scheduler bağımsızlığı) ve GPU'ya binen yük her zaman kontrol edilmelidir.

---

## 5. BEVY 0.17+ API DEĞİŞİKLİKLERİ (2026-06)

**DİKKAT:** Bevy 0.17+ ile birlikte aşağıdaki API isim değişiklikleri yapılmıştır. **Tüm kod örnekleri ve implementasyonlar bu yeni terminolojiyi kullanmalıdır.**

| Eski (≤0.16) | Yeni (0.17+) | Kullanım |
|---|---|---|
| `EventWriter` | **`MessageWriter`** | Event yazma |
| `EventReader` | **`MessageReader`** | Event okuma |
| `OnAdd` / `OnRemove` | **`Add`** / **`Remove`** / **`Insert`** / **`Replace`** / **`Despawn`** | Lifecycle hooks |
| `Trigger<E>` | **`On<E>`** | Observer parametresi |

**Not:** `Event` trait ve `app.add_event::<T>()` aynı kalır; sadece reader/writer isimleri değişti.

---

## 6. ARAŞTIRMA DOĞRULAMALARI (2026-06)

Strata'nın kesinleşmiş planları (01-16) 2024-2026 SOTA araştırmalarıyla **yüksek uyumlu**. Temel mimari değişiklik gerekmiyor — tüm öneriler **eklemeli**.

### Doğrulanan Kararlar

| Karar | Plan | Doğrulama |
|-------|------|-----------|
| Flat `crates/` layout | 02 | matklad/rust-analyzer validated |
| Archetype-based ECS | 03 | SAC 2026 benchmark: ~10× faster than OOP |
| XBrickMap 3-level bitmask | 06 | SOTA aligned |
| SVDAG + ghost page | 07 | GigaVoxels DP comparable, wgpu uyumlu |
| Aokana visibility buffer | 10 | SOTA, no voxel-specific alternative |
| BlockEntity hybrid | 03/05 | Unity community validated |
| T0/T1/T2 modding | 04 | cyubeVR validated |
| Binary greedy meshing | 09 | SOTA aligned |
| 4-tier streaming | 08 | Aokana + GigaVoxels validated |

### P0 — Kesin Öneriler (Phase 1)

1. **`bitflags` crate** → BlockFlags (Plan 05)
2. **`cargo-hakari`** → Build time ~%50 kazanç (Plan 02)
3. **Change detection optimization** → `set_if_neq()`, `bypass_change_detection()` (Plan 03)

### P1 — Önerilen (Phase 1-2)

4. **`lld` linker** → Windows'ta 5-30s linking kazancı (Plan 02)
5. **`strata-types` crate** → Ortak tipler ayrı crate (Plan 02)
6. **LODError zorunlu** → "Opsiyonel" → "zorunlu" (Plan 08)
7. **TOML+RON hibrit** → Enum/state tanımları RON (Plan 05)

*Ayrıntılar: `plans/01-overview.md` §7*

---

## 7. ÇALIŞMA TARZI VE TERCİHLER (WORKING STYLE)

Bu bölüm, `.cursor/` klasöründeki tercihleri (rules, instructions, memory) tek kaynağa
toplayarak `AGENTS.md`'i "yaşayan rehber" olarak güncel tutar. Kod İngilizce, sohbet Türkçe.

### A. ROL VE ONAY SINIRLARI (Ref: rules/01, memory)
- Ajan = Strata için **otonom uygulayıcı + danışman/yönlendirici**.
- Çoğu task'ı her adımda sormadan uygula; ama **kritik kararlarda durup kullanıcıya
  onay sor**. Kritik = çekirdek mimari (XBrickMap, SVDAG, streaming tier, ECS sistem
  yerleşimi, render pipeline), public API değişikliği, bağımlılık ekleme, ya da anayasa
  (`01`–`16`) ile çelişen herhangi bir şey.
- "planla" / "plan" dendiğinde önce **Plan moduna** geç, yaklaşımı onayına sun, sonra uygula.

### B. DİL (Ref: rules/01, 03)
- **Kod, tanımlayıcılar, yorumlar, commit mesajları:** İngilizce.
- **Sohbet, açıklama, kullanıcıya sorular:** Türkçe.
- Plan dosyaları Türkçe kalır (çevirme).

### C. GIT / COMMIT (Ref: rules/01, memory)
- Kendiliğinden commit yapma. Doğrudan `main` üzerinde çalış (feature branch yalnızca
  kullanıcı isterse).
- Her mantıksal aşama sonunda commit öner (mesaj + kapsam); yalnızca onayla commit oluştur.
- Commit önerisinden önce `cargo fmt` + `cargo clippy --all-targets` + `cargo test` temiz olmalı.

### D. KAPSAM KORUMASI / SCOPE GUARD (Ref: rules/01, 02, memory)
- Çoğu dosyayı otonom düzenle.
- Şu **kritik dosyaları** düzenlemeden önce MUTLAKA sor: anayasa planları (`01`–`16`),
  `CLAUDE.md`, `AGENTS.md`.
- Yeni bağımlılık (Cargo.toml değişimi) kritik sayılır → önce sor.

### E. HATA DAVRANIŞI (Ref: rules/01)
- Derleme/test hatasında önce kendi başına düzeltmeyi dene.
- Çözüm bulamazsan güncel doğru API/çözüm için **WebSearch** kullan.
- Yine çözmezsen kullanıcıya rapor ver (hata + denediklerin + bulguların). Sessiz döngüye girme.

### F. İLERLEME VE ARAŞTIRMA (Ref: rules/01, memory)
- Çok adımlı task'larda ilerlemeyi **TodoWrite** ile göster.
- İlgiliyse uygulamadan önce `researchs/` klasörünü otomatik referans al.

### G. KOD STİLİ VE LİNT (Ref: rules/02)
- Rust 2024 edition, hedef Windows.
- Voxel hot path'te heap fragmentation yasak: `GlobalBrickPool` (SlotMap + SecondaryMap),
  canlı voxel verisi için per-sector `Vec` yok (`06` §B.3).
- Change-detection hassas yazımlarda `set_if_neq()` / `bypass_change_detection()` kullan.
- Yorumlar kodu anlatan anlatı (narration) değil; yalnızca bariz olmayan intent, trade-off,
  kısıt için. Public API'ye minimal `///` yeterli.
- Crate yerleşimi: flat `crates/`; ortak tipler `strata-types` crate'inde (`02`).

### H. TEST (Ref: rules/04)
- Commit önermeden önce `cargo test` (veya `-p <crate>`); tüm testler geçmeli.
- Performans-kritik modüllerde (XBrickMap, SVDAG, meshing) tam değer yerine
  property-based / round-trip testleri (encode→decode→compare) tercih et.
- Çekirdek veri yapıları: round-trip + empty/full/edge boundary testleri zorunlu.
- Hot path'lerde gerektiğinde `cargo bench` girişi ekle (brick pool alloc/free, greedy quad sayısı).

### I. DİĞER SABİT TERCİHLER (Ref: instructions, memory)
- **Memory:** Yalnızca kritik, kalıcı kararlar; geçici task detayı `.cursor/`'da. Yeni
  session'da `.cursor/`'ı yeniden oku.
- **CI/CD:** Şimdilik yok (`plans/33` ileride).
- **Otomatik Review:** Kritik PR'larda bugbot + security-review çalıştır.
- **Debug:** Uygun gördüğünde log + debug HUD kullan.
- **Bağımlılık sürümleri:** En optimize/stable sürüme sabitle (CLAUDE.md fixed-version);
  körce "latest"e yükseltme, performans/stability ile gerekçelendir.
- **Onay görgüsü:** Sorarken açık uçlu "ne yapalım?" yerine somut seçenekler sun
  (`question` aracı), trade-off'u kısaca belirt, kullanıcı karar versin.
- **Docs check:** Manuel "docs check" / plan tazelik kontrolü `.cursor/docs-check.md`
  prosedürüyle, yalnızca kullanıcı tetiklediğinde işletilir; planları onaysız düzenleme.
