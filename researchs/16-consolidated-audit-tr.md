# Plan 16 — Ağ Senkronizasyonu ve Lag Compensation: Kapsamlı Teknik Denetim Raporu

> **Hazırlayan:** 7 uzman alt-ajan (transport, replication, AOI, delta/quantization, chunk compression, prediction/lag-comp, interpolation) tarafından paralel derin web araştırması + karşılaştırmalı analiz.
> **Tarih:** 2026-07-07
> **Kapsam:** `plans/16-network-and-lag-compensation.md` (tüm bölümler)
> **Yöntem:** Her bileşen bağımsız alt-ajanla analiz edildi; 2024-2026 SOTA kaynakları, kütüphane sürümleri (bevy_replicon 0.40, bevy_quinnet 0.20, bevy_replicon_quinnet 0.19) ve üretim MMO uygulamaları (BigWorld, Photon Fusion, Albion, Unreal Replication Graph, Aokana, Gaffer/Valve/Overwatch) ile doğrulandı.

---

## 1. Yönetici Özet (Executive Summary)

**Genel Değerlendirme:** Plan 16, mimari olarak **2024-2026 SOTA ile güçlü şekilde hizalı** ve teknik olarak sağlamdır. Özellikle daha önceki (2026-07) iç denetim notları büyük ölçüde **araştırmayla doğrulandı** (BBR→Cubic, TOFU→cert pinning, bozuk oktahedral kodun çıkarılması, 0-RTT idempotent-only kısıtı, sector'a hizalı histeresis, adaptive input delay'in çıkarılması, `bevy_replicon_attributes`ın çıkarılması). Plan bir "taslak" (16-38 arası) olmasına rağmen, mevcut haliyle uygulanabilir ve doğrudur.

**En kritik bulgular (uygulamadan önce çözülmesi gereken):**

| # | Bulgu | Önem | Bölüm |
|---|-------|------|-------|
| C1 | **Çelişkili stack:** §2 `bevy_replicon` derken §7.9 `lightyear` öneriyor — ikisi birbirinin yerine geçer, aynı anda birincil stack olamaz | KRİTİK | §2/§7.9 |
| C2 | **`PriorityMap` O(P·E) taraması** (600 oyuncu × ~600 entity × 20Hz ≈ 7.2M kontrol/s) ölçeklenmez; Replication Graph prensibi gereği spatial index'ten sürülmeli | YÜKSEK | §2.5/§3 |
| C3 | **Küre vs küp AOI tutarsızlığı:** Euclid mesafe eşikleri küp tier'larını sessizce küreye çeviriyor; üyelik grid üzerinden hesaplanmalı | YÜKSEK | §2.5/§3 |
| C4 | **Bant genişliği tahmini eksik:** Header (~+10%) ve AOI giriş/çıkış burst'leri (create/delete) hesaba katılmamış → gerçek agregat ~14 MB/s, 100 Mbps "rahat" değil tavan | ORTA | §0/§3.4 |
| C5 | **zstd sözlüğü (dictionary) eksik** — en yüksek ROI'li ek; voxel trafiğinde +20-50% kazanç | ORTA | §6.2 |
| C6 | **Fiedler çok-seviyeli bitpacking** "ileri/faz 15"e konmuş ama en yüksek ROI'li optimizasyon; çekirdek faz olmalı | ORTA | §5.6/§12 |

**Doğrulanan güçlü kararlar:** QUIC/bevy_quinnet seçimi, sector-as-SPS-topic AOI, SVDAG-for-DISTANT (PNG silindi), palette + zstd, sector-anchored pozisyon kuantizasyonu, yaw-only `u16` rotasyon, dead reckoning, 0ms lokal input gecikmesi, server-side rewind (200ms).

---

## 2. Bileşen Bazlı Denetim

### 2.1 Transport Katmanı — QUIC + bevy_quinnet (§1)

**Doğrulama:**
- ✅ **Doğru:** TLS 1.3 zorunlu, 1-RTT/0-RTT reconnect, Connection-ID migration, tek UDP socket (600+), datagram (RFC 9221) unreliable state için, `enable_segmentation_offload` gerçek Quinn alanı.
- ✅ **İç denetimle uyumlu:** BBR→**Cubic** (Quinn `BbrConfig` "Experimental"; BBR düşük-BDP oyun trafiğinde gecikmeyi artırır, "33% RTT" iddiası datacenter bulk içindir). 0-RTT **idempotent-only** (RFC 9001 §9.2 replay).
- ⚠️ **Abartılı/yanıltıcı:** "256 kanal" bevy_quinnet soyutlaması, QUIC sabiti değil; per-stream HOL önleme oyun state'i *datagram* kullandığı için pek katkı yapmıyor (RUDP zaten HOL'u çözer). GSO "97% syscall" gönderici-taraflı ve Linux-only; "74% throughput" Cloudflare blog'da verbatim değil.

**Temel risk:** QUIC'in paket başına AEAD maliyeti TCP'ye göre ~2-3.5× CPU/byte → 600 oyuncu ölçeklenmesinin asıl sınırı bant değil **CPU**. Quinn socket buffer default'u yük altında paket kaybına yol açar (#2262) → `SO_RCVBUF/SNDBUF` yükseltilmeli. Datagram'lar ~1150B max (1200B MTU) → tam `ChunkData` sector yükleri fragment edilmeli veya reliable stream'e alınmalı.

**Alternatifler:**
| Yaklaşım | Artı | Eks | Strata'ya uygunluk |
|---|---|---|---|
| QUIC/Quinn (plan) | Şifreleme/stream/migration hazır | Yüksek CPU, handshake maliyeti | ✅ Uygun (güvenlik basitliği) |
| Valve GameNetworkingSockets | Oyuna özgü, QUIC crypto vergisiz | Bevy entegrasyonu el yapımı | ⚠️ QUIC'e commit öncesi benchmark gerek |
| raw UDP + ENet/KCP/yojimbo | En düşük gecikme/CPU | Şifreleme/auth elle | ❌ Strata güvenlik modeli için fazla iş |
| WebTransport | WASM/browser client | Native'de gereksiz | ⚠️ Opsiyonel WASM yolu |

**Öneri:** QUIC/Quinn koru (2026 bakım sağlıklı: bevy_quinnet 0.20, bevy_replicon_quinnet 0.19, quinn-proto 0.11.14), **Cubic pinle**, buffer yükselt, datagram <1.1KB cap, 0-RTT kısıtla, `SO_REUSEPORT` multi-endpoint. Commit öncesi **Valve GameNetworkingSockets ile benchmark**.

### 2.2 Replication Katmanı — bevy_replicon 0.40 (§2)

**Doğrulama (tüm iddialar doğrulandı):**
- ✅ bevy_replicon 0.40.0 (2026-05-17, Bevy 0.18) server-authoritative, ECS-native, Postcard serializer, `bevy_replicon_quinnet` resmi QUIC backend.
- ✅ Native `VisibilityFilter` + `Scope=SingleComponent` GERÇEK API (0.35 Iris rework'ten beri var). `bevy_replicon_attributes` çıkarımı **doğru** (max bevy 0.16/0.33 destekler).
- ✅ `PriorityMap` (mesafe bazlı throttling) ve `SharedMessageAppExt` + `MessageReader<LocalOrRemote<M>>` (0.40 gerçek API) — block place/break prediction için birincil desteklenen yol.
- ✅ Cargo pin'leri (`bevy_replicon 0.40`, `bevy_replicon_quinnet 0.19`, `bevy_quinnet 0.20`) çözümlenebilir ve tutarlı.

**İki önemli nuans:**
1. `PriorityMap` yalnız **mutasyonları** throttler; insert/remove her zaman gider. ARCHIVE tier'ı tamamen telden çıkarmak için `VisibilityFilter` (`Scope=SingleComponent<VoxelPayload>`) ile **birlikte** kullanılmalı.
2. AOI *güncelleme algoritması* kritik: docs örneği `players.iter_combinations::<2>()` → **O(n²)**. 600 oyuncuda ~180k çift/kontrol. AOI, plan 08 streaming spatial index'ten (grid/octree) sürülmeli, brute-force değil.

**Alternatifler:** lightyear (prediction/rollback/lag-comp birinci sınıf, WebTransport), bevy_renet (düşük seviyeli UDP, elle visibility), naia (Web-first Rooms), turbulence (TERK). lightyear ile bevy_replicon **tamamlayıcı** (replicon replication+AOI, lightyear prediction katmanı) — ikisi mutually exclusive birincil stack DEĞİL.

**Öneri:** Stack'i koru, exact version pinle. AOI'yi spatial index'ten sür. `SingleComponent` + `PriorityMap` kombinasyonunu tier'lar için belgele (ARCHIVE = visibility-hidden + 0.0). Postcard'ı koru ama **büyük voxel payload'larını (sector snapshot) serde bypass edip raw byte kanalından** gönder (en büyük kazanç). Erken QUIC 600-oyuncu load testi.

### 2.3 Interest Management / AOI (§3)

**Doğrulama:**
- ✅ **Sector-as-SPS-topic (32m³) planın en güçlü kararı.** SPS 2024'te Minecraft'ta 6× mesaj azaltımı gösterdi; 32m hücre Photon Fusion varsayılanıyla birebir. Voxel/streaming tier'larıyla (plan 07/08) mükemmel hizalama = nadir DOD kazancı.
- ✅ **Sector-aligned histeresis** (+32/+64/+96m) BigWorld `grid_hysteresis` ve Albion iki-dikdörtgen modeliyle üretim-kanıtlı. Önceki sabit +16m doğru reddedildi.
- ⚠️ **"6× occlusion" tavan, ortalama değil.** Boulanger (NetGames'06) 6×'ı engel-yoğun sahneler için ölçtü; açık arazide 1.5-2.5×. Bütçe ~2-3× ortalama. SVDAG/GPU occlusion buffer'ı **yeniden CPU ray-march etme** — render'ın zaten hesapladığı culling sonucunu tüket (MeshNet 2026: GPU'da ~0 CPU ile 8-15×).
- ⚠️ **Küre vs küp tutarsızlığı** (C3): Euclid eşikleri küp tier'larını küreye çeviriyor. Üyelik grid/sector koordinatından hesaplanmalı (Chebyshev).
- ⚠️ **Bant genişliği:** §0 (90/510 modeli) ile §3.4 (200-entity modeli) çelişiyor; birini sil. Gerçek on-wire ≈ +10% header + boundary burst.

**Alternatifler:** Replication Graph (Unreal, per-class node'ları bir kez kur, 600 client'a yeniden kullan — O(P·E)'yi ortadan kaldırır), Quadtree/Octree (sector grid ile redundan), Adaptive/VELVET (yoğunlukta AOI daralt), priority-based broadcasting (Unreal DynamicSpatialFrequency, Photon SetPriority).

**Öneri:** §2.5 `PriorityMap` O(P·E) taramasını **grid-driven membership** ile değiştir (per-sector entity listesi = SPS subscriber seti). AOI üyeliğini küp-tutarlı yap (Chebyshev veya sector koord). Sürekli priority (3 sabit tier yerine mesafe+önem skoru; kritik combat entity 30Hz). SVDAG occlusion'ı relevance *çarpanı* olarak kullan (üst sınır 6×, ortalama 2.5×). Konglomera adaptif throttle (DISTANT'ı önce düşür). İki BW tablosunu birleştir + header/burst ekle.

### 2.4 Delta Compression & Quantization (§4, §5)

**Doğrulama:**
- ✅ **Sparse encoding çoğunlukla replikasyon framework'ünü yineler** — bevy_replicon zaten changed-component replication yapar. Elle 600-bit mask GEREKSİZ. Ancak framework **field-level** delta yapmaz → çözüm `Transform`'ı `NetPos`/`NetRot` alt-component'lerine bölmek (ücretsiz change-tracking).
- ✅ **Sector-anchored pozisyon** (i32 + 12-bit local) "sınırsız yükseklik" anayasasıyla uyumlu. Ancak: `i32` sector aşırı (96 bit/keyframe); **`i16` sector** (±1.048km) yeterli + 48 bit tasarruf. **0.01m (1cm) aşırı hassas** — 1m-voxel dünyada 0.05m görsel olarak ayırt edilemez, 10-11 bit local yeterli.
- ✅ **Octahedral çıkarımı DOĞRU** — oktahedral bir *yön/normal* codec'i (2 DOF), quaternion (3 DOF rotasyon) için kategori hatası. SVDAG/lighting normaları için ayrılmalı.
- ✅ **yaw-only `u16` (2B)** çoğu Y-eksen entity için en iyi; 32-bit smallest-three (4B) tam 3-DOF için. Ancak bit layout **"11,11,10+2" GEÇERSİZ** (11+11+10=32, index'e yer yok) → düzelt: `10,10,10 + 2-bit index = 32 bit` (kalite tabanı).
- ✅ **Dead reckoning** (velocity kaldırma) Gaffer/Valve ile güçlü destekli. İyileştirme: per-tick velocity yerine **event-driven velocity** (impulse/teleport/"at rest→moving" geçişinde).
- ⚠️ **Fiedler multi-level** planın kendi sayıları değil; Gaffer veri-tabanlı greedy tune kullanmış. Yürüyüş ~10-30 unit/tick @5cm → small window 7-bit [−64,+63] olmalı. Bu en yüksek ROI optimizasyonu, "ileri"ye konmamalı.
- ⚠️ §5.8 "pozisyon 1-5B" yanıltıcı; flat i16+mask 3 eksen hareketinde **7B** worst-case.

**Alternatifler & bant genişliği (değişen entity/tick, 20Hz, 5cm):**
| Şema | 1 eksen | 3 eksen yürüme | Not |
|---|---|---|---|
| Plan flat i16+mask | 3B | 7B | basit |
| **Fiedler multi-level** | 2B | 3.25B | dominant durumda ~2× daha iyi |
| Unreal NetQuantize100 abs | 11.25B | 11.25B | keyframe |

Aggregate aktif entity: Plan 9B → Optimize (Fiedler+yaw) 5.25B ≈ **%40 daha iyi**.

**Öneri:** Pozisyon `i16` sector + 12-bit @0.05m; per-frame Fiedler (small=7-bit, medium=13-bit, full=12-bit keyframe, her ~16-32 tick veya teleport'ta absolute). Rotasyon default `u16` yaw + 1-bit flag→32-bit ST (`10,10,10+2`). Velocity: dead reckoning + event-driven. **Entity-level mask katmanını sil**, `Transform`'ı alt-component'lere böl. Fiedler'ı çekirdek faza al, gerçek delta histogramı ile breakpoint tune et. `BrickDelta` 15B zero-alloc koru (ama `mats:[u8;8]` bir sub-brick ile sınırlı — açıklığa kavuştur).

### 2.5 Chunk Compression / SVDAG (§4, §6)

**Doğrulama:**
- ✅ **Palette-based (dynamic-bit + Direct fallback)** Minecraft/Voxel.Wiki ile endüstri standardı. 4 tip: 2bit×512 = 128B vs 512B ham — matematik doğru. `SectorPalette` reuse doğru.
- ✅ **zstd > deflate** kesin (lzbench + Minetest üretim kanıtı). Ancak "25-27%" paleti + zstd birlikte; palet birincil compressor, zstd ikincil.
- ✅ **SVDAG-for-DISTANT (PNG silindi) planın en gelecek-sağlam kararı.** SVDAG (Kämpe 2013: 0.08 bit/voxel, no decompress, GPU traverse), GigaVoxels (cone-trace LOD), **Aokana 2025** (açık-dünya voxel, shallow SVDAG max depth~5, 64-bit visibility buffer — plan 10 ile birebir). PNG raster overhang/cave/LOD kaybeder, ray-march pipeline ile uyumsuz.
- ✅ **Block change batching:** 15-bit pos matematiği EXACT (32³→5bit/axis). 200 cap güvenli/konservatif. 4B/change (2B pos + 2B u16) doğru.
- ✅ **BrickDelta ~15B zero-alloc** "no heap fragmentation" kuralıyla uyumlu.

**Alternatifler (codec):**
| Codec | Ratio | Hız | Strata |
|---|---|---|---|
| **zstd+dict** | +20-50% (small payload) | hızlı | ✅ reliable WARM/ARCHIVE |
| zstd | 2-2.5× | 100s MB/s | reliable snapshot |
| **lz4** | 1.5-2× | ~1GB/s | ✅ unreliable datagram ChunkData (latency>ratio) |
| brotli | en iyi text | çok yavaş | ❌ hot traffic |

**Öneri:** zstd (reliable) + **build-time trained zstd dictionary** (Discord/Oodle pattern, +20-50%). `ChunkData` datagram path için **lz4** (kayıp AOI ile re-request). 25-27%'yi palette+zstd combined olarak reframe et + benchmark harness (Veloren örneği gibi). BrickDelta sub-brick/material-length tutarsızlığını düzelt. HashDAG-style incremental SVDAG edit (brick değişince tam re-bake yerine pointer/leaf delta) ACTIVE→WARM geçişinde kullan.

### 2.6 Prediction, Reconciliation & Lag Compensation (§7, §9, §10, §11)

**Doğrulama:**
- ⚠️ **Partial rollback (10-15→2-5 frame, %67):** teknik sound ama metrik yanıltıcı — Valve/Overwatch zaten sadece unacked input'ları replay eder (RTT ile sınırlı). 4-frame checkpoint yalnız worst-case'i bound'lar.
- ⚠️ **Input redundancy (sabit "son 3"):** 1.8KB/s matematiği doğru ama Overwatch sliding-window (tüm unacked) ile aşıldı. Sabit 3, ~100ms RTT + ≥30Hz üzerinde başarısız (200ms/60Hz'de 12 unacked).
- ✅ **Smooth correction** threshold'ları endüstriyle uyumlu (small=0.01m altı görünmez, large=snap). Düzelt: yalnız **render** transform'u smooth et (logical state snap, velocity mutate etme), no-correction floor'u kendi kuantizasyon adımının (0.05m) üzerine çıkar.
- ✅ **0ms lokal input delay** Gambetta/Valve/Overwatch ile doğru; jitter buffer yalnız REMOTE entity.
- ✅ **Lag comp** Valve `sv_maxunlag=0.2` ile uyumlu. Ekle: target lerp delay, teleport guard, usercmd tick replay clamp (anti-backtrack). **Asla voxel world rewind etme.**
- ✅ **bevy_replicon_snap (PoC, Bevy 0.15'te takılı) ve GGRS (P2P lockstep, 600-player server-auth ile uyumsuz) DOĞRU reddedildi.**

**Çelişki (C1):** §2 `bevy_replicon` derken §7.9 `lightyear` önerliyor. Bunlar mutually exclusive birincil stack. Önerilen tutarlı tasarım: **lightyear** (entity prediction/rollback/interp/lag-comp) + **custom optimistic block-placement layer** (voxel mutation, entity-component prediction değil). VEYA replicon koru + lightyear'ı yalnız prediction katmanı olarak ekle (tamamlayıcı). Karar verilmeli.

**Öneri:** (1) Tek stack seç. (2) Sliding-window input redundancy. (3) Render-smoothing ile logical snap'i ayır + quantization-aware threshold. (4) Lag comp'ı lerp-delay + teleport guard + tick-replay clamp ile sertleştir. (5) lightyear seçilirse 0.26 (Bevy 0.18) pinle. (6) Input buffer 64 ring koru.

### 2.7 Entity Interpolation (§8)

**Doğrulama:**
- ✅ **Circular buffer `ceil(delay/interval)+2`, 3× lossy/2× clean, jitter EMA (α=0.2)** Gaffer ile uyumlu. Gap: 3× kuralı **loss-driven**, jitter değil — buffer `max(loss_run_length, jitter_p95)` ile size edilmeli.
- ✅ **Per-component policy** (Lerp/Nlerp/Snap/Hermite) Valve/coherence/Gaffer ile standart. Discrete (health/voxel/anim) Snap doğru.
- ✅ **Slerp default + Nlerp small angle** Gaffer'ın "constant angular velocity" gerekçesiyle destekli. **Düzelt:** nlerp/slerp "180° problemi" aslında **antipodal quaternion degeneracy** (dot<0) — her iki için zorunlu **hemisphere dot-flip** explicit yapılmalı; error tablosu *quaternion 4-D angle* cinsinden yazılmalı.
- ✅ **Hermite/Catmull-Rom** Gaffer demo + coherence ile shipping teknik (position artifacts düzeltilir). İki varyant: velocity-Hermite (bandwidth) vs Catmull-Rom (4-sample, sıfır bandwidth). Position'a sınırla (Gaffer orientation'da gereksiz buldu).
- ✅ **Extrapolation 2×send_interval cap** Gaffer'ın güvenli 50-250ms penceresinde. Absolute ms'e çevir (yüksek tick'te 2×interval ~30ms yetersiz). Default under-run = **hold/freeze**, extrapolation yalnız predictable motion.
- ✅ **Spawn 100ms / despawn 200ms** makul; ≥2 snapshot buffered before fade-in.

**Öneri:** Buffer sizing'e loss-run-length ekle. Rotation: Slerp default + Nlerp small angle + **mandatory hemisphere check**. Position: hem velocity-Hermite hem Catmull-Rom (default Lerp). Extrapolation `min(2×interval, 250ms)` absolute + hold default. Fade'i opacity/scale'e uygula. Tick rate artışını (30pps→150ms, 60pps→85ms) gecikme azaltma kolu olarak belgele (Plan 05 compression ile mümkün).

---

## 3. Çapraz Kesen Kritik Bulgular

1. **Stack çelişkisi (C1):** bevy_replicon vs lightyear birincil stack kararı acil çözülmeli. Öneri: replicon+quinnet koru (replication+AOI olgun), prediction/rollback için lightyear'ı additive katman olarak değerlendir; VEYA tam lightyear geçişi (daha riskli, daha fazla yeniden yazım).
2. **Ölçeklenme anti-pattern'i (C2/C3):** O(P·E) PriorityMap taraması + küre/küp tutarsızlığı → spatial index'ten (plan 08 grid/octree) AOI üretimi zorunlu.
3. **Bant genişliği muhasebesi (C4):** Header + burst eklenmeden agregat ~14 MB/s; 100 Mbps tavan, ≥150 Mbps öner.
4. **En yüksek ROI optimizasyonlar eksik/yanlış fazda:** Fiedler bitpacking (çekirdek) ve zstd dictionary (ekle).
5. **SVDAG/DISTANT + octahhedral-silindi + AOI-sector-topic:** Planın en gelecek-sağlam, SOTA-öncü kararları — dokunma.

---

## 4. Eyleme Dönüştürülebilir Öneriler (Öncelikli)

### P0 — Uygulamadan önce zorunlu (mimari tutarlılık)
1. **Stack çelişkisini çöz (C1):** §2 ya da §7.9'ten birini netleştir. Öneri: `bevy_replicon` + `bevy_replicon_quinnet` birincil stack; prediction için lightyear additive (veya tam geçiş kararı verilip plan güncellenir).
2. **AOI'yi spatial index'ten sür (C2):** §2.5 `PriorityMap` O(P·E) döngüsünü per-sector entity listesi (SPS subscriber) ile değiştir. Replication Graph prensibi.
3. **Küp-tutarlı AOI (C3):** Euclidean threshold yerine sector koordinatı/Chebyshev ile tier üyeliği.
4. **BrickDelta sub-brick/material tutarsızlığını düzelt** (§4.2/§6.1): tek sub-brick sınırı VEYA `mats` uzunluğu = popcount(changed_voxels).

### P1 — Yüksek değer, düşük risk
5. **zstd dictionary ekle (C5):** build-time trained, client build ile ship; +20-50% small payload.
6. **Fiedler multi-level'ı çekirdek faza al (C6):** gerçek delta histogramı ile breakpoint tune (small=7bit).
7. **Per-channel codec:** lz4 (unreliable datagram ChunkData) + zstd+dict (reliable snapshot).
8. **Position quantization:** `i16` sector + 12-bit @0.05m; `i32`+0.01m yerine.
9. **Rotation bit layout düzelt:** `10,10,10+2 = 32-bit` smallest-three (geçersiz 11,11,10+2 yerine).
10. **Interpolation buffer loss-run-length sizing** + mandatory hemisphere dot-flip.

### P2 — İyileştirme / ince ayar
11. **Sürekli priority** (3 sabit tier yerine mesafe+önem); kritik entity 30Hz.
12. **SVDAG occlusion'u relevance çarpanı** (üst 6×, ortalama 2.5×); CPU ray-march YOK.
13. **Sliding-window input redundancy** (sabit 3 yerine).
14. **Event-driven velocity** (per-tick yerine).
15. **Transform'ı NetPos/NetRot'a böl** (field-level delta için).
16. **Congestion-adaptive AOI throttle** (DISTANT'ı önce düşür).
17. **BW tablolarını birleştir** + header/burst ekle, agregatı ~14 MB/s olarak güncelle.

---

## 5. Güncellenmiş Risk Matrisi

| Risk | Olasılık | Etki | Azaltma |
|------|----------|------|---------|
| bevy_replicon vs lightyear çelişkisi | Yüksek | Yüksek | Tek stack kararı (P0-1) |
| 600+ oyuncu QUIC CPU vergisi | Orta | Yüksek | Erken load test; Valve GNS benchmark; renet fallback |
| PriorityMap O(P·E) ölçeklenmeme | Yüksek | Yüksek | Spatial index AOI (P0-2) |
| Bant genişliği under-count | Orta | Orta | Header+burst ekle, 150Mbps planla |
| zstd dictionary eksik | Düşük | Orta | Build-time dict (P1-5) |
| AOI boundary burst | Orta | Orta | Hysteresis + neighbor overlap |
| bevy_quinnet bakım | Düşük | Yüksek | quinn-proto fork / renet backend |

---

## 6. Sonuç

Plan 16, daha önceki iç denetim notlarıyla zaten büyük ölçüde doğruya yaklaşmış ve bu denetimde **bağımsız derin araştırmayla doğrulanmıştır**. Mimari kararlar (QUIC, sector-AOI, SVDAG-DISTANT, palette+zstd, sector-anchored quant, dead reckoning, 0ms input, server rewind) 2024-2026 SOTA ile uyumludur. Kalan açıklar **mimari tersine çevirme değil, uygulama doğruluğu ve fazlama** sorunlarıdır: (1) bevy_replicon/lightyear stack çelişkisi, (2) O(P·E) AOI taraması, (3) küre/küp tutarsızlığı, (4) bant genişliği muhasebesi, (5) eksik zstd dictionary ve yanlış fazlanmış Fiedler bitpacking.

**Öneri:** P0 maddeleri çözüldükten sonra Plan 16 "kesinleşmiş" (01-15 anayasa seviyesine yakın) olarak işaretlenebilir; P1 maddeleri uygulama sırasının çekirdek fazlarına taşınmalıdır.

---

### Kaynaklar (alt-ajanlar tarafından tarandı)
- bevy_replicon 0.40 docs/releases, bevy_replicon_quinnet 0.19 deps, bevy_quinnet 0.20
- Quinn TransportConfig, RFC 9221 (QUIC Datagrams), RFC 9001 §9.2 (0-RTT)
- Gaffer On Games (Snapshot Compression, Snapshot Interpolation), Gabriel Gambetta (Prediction/Interpolation), Valve Wiki (Networking/Lag Comp), Overwatch GDC (Tim Ford)
- Unreal Replication Graph, Photon Fusion, BigWorld/Cimmeria, Albion Online IM
- Boulanger et al. NetGames'06, McGill thesis, MeshNet 2026, Smit & Engelbrecht 2024 (SPS)
- Minecraft palette/long-array, Voxel.Wiki, Veloren chunk benchmarks, Minetest/Luanti zstd
- SVDAG (Kämpe 2013), GigaVoxels, Aokana 2025 (arXiv:2505.02017), HashDAG
- Riot Games quaternion compression, Marc B. Reynolds QuatQuant, meshoptimizer, jpreiss/quatcompress
- lightyear, bevy_renet, naia, GGRS, bevy_replicon_snap GitHub
- lzbench, Discord zstd+dictionary blog, Oodle Network
