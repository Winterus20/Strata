# Plan 16 — Ağ Senkronizasyonu ve Lag Compensation: Kapsamlı Teknik Denetim Raporu

> **Tarih:** 2026-07-07
> **Kapsam:** `plans/16-network-and-lag-compensation.md` (taslak plan, §1 olgunluk: taslak)
> **Yöntem:** 9 bileşen başına paralel derin web araştırması yapan alt-ajanlar + karşılaştırmalı analiz + konsolidasyon.
> **Dil:** Rapor Türkçe; teknik terimler İngilizce bırakılmıştır.

---

## 0. Yönetici Özet (Executive Summary)

Plan 16, **mimari olarak sağlam ve endüstri pratikleriyle uyumlu** bir taslaktır. Çekirdek kararların çoğu (lightyear birincil stack, sector-tabanlı AOI, Chebyshev küp tutarlılığı, sürekli priority, Fiedler bitpacking, velocity kaldırma, optimistic block edit + revert, 0ms input gecikmesi, SVDAG tabanlı DISTANT LOD, zstd/lz4 kanal ayrımı) araştırma ile **doğrulanmıştır**.

Ancak denetim **kritik ve uygulanabilirlik açısından zorunlu** bir dizi bulgu ortaya çıkardı:

1. **En kritik bulgu (çapraz kesen):** lightyear **0.27.0 (2026-06-22)** sürümü, replikasyon backend'ini **bevy_replicon** üzerine yeniden kurdu. Plan 16, eski lightyear internals'a (kendi `ReplicationGroup`, legacy delta compression, client→server replication, per-component priority) göre yazılmıştır ve bu varsayımlar artık **geçersiz/beklemede**. Plan, 0.27/Replicon API'sine göre gözden geçirilmelidir.
2. **lightyear, sunucu-taraflı rewind (lag comp) vermez.** `LagCompensationPlugin` yalnızca Avian'a bağlı **client-side** rewind'dir. Strata kendi authoritative rewind yöneticisini yazmalıdır.
3. **Çok sayıda somut spec hatası** var: pozisyon local precision/range çelişkisi, smallest-three `u10` bileşenlerinin signed yazılmaması, BlockChangeBatch'ın MTU sınırına yaklaşması, `sector` alanının her delta'da tekrar gönderilmesi, interpolation eşiklerinin kuantizasyon adımının altında kalması.
4. **"QUIC 2-3.5× per-byte AEAD" gerekçesi yanlıştır** (aynı AES-GCM cipher; gerçek maliyet per-packet). Ancak "native için QUIC reddi" sonucu doğrudur (ayrıca WASM için zaten QUIC/WebTransport kullanıyorsunuz).

**Genel tavsiye:** Planın yönü %90 doğrudur; kalan iş "doğrulama + spec düzeltmeleri + lightyear 0.27'ye yeniden haritalama + 600-oyuncu soak testi".

---

## 1. Transport Katmanı (§1) — lightyear / UDP + netcode

### 1.1 Doğrulama
- **lightyear birincil stack:** ✅ Tutulmalı. Bevy-native, netcode.io güvenlik modeli (AES-256-GCM, Ed25519-signed connect token), WASM için WebTransport desteği.
- **QUIC reddi (native):** ✅ Sonuç doğru, ama **gerekçe yanlış**. QUIC'in "2-3.5× per-byte AEAD CPU" maliyeti iddiası hatalı — netcode.io ve QUIC aynı AES-GCM cipher'ını kullanır; simetrik per-byte maliyet **aynıdır**. Gerçek maliyet: per-packet sabit overhead + handshake + kernel TLS offload yokluğu. Ayrıca **WASM path'iniz zaten QUIC (WebTransport)** — yani "QUIC reddi" aslında "native=UDP+netcode, browser=QUIC" demektir.
- **600 oyuncu olgunluğu:** ⚠️ Doğrulanmamış. lightyear'ın yayınlanmış 600-oyuncu benchmark'ı yok; ECS tabanlı per-packet işleme ve yerleşik bandwidth/pacing controller eksikliği risk. Soak testi zorunlu.
- **Tek UDP socket / 4MB buffer:** Tek socket ✅ doğru. 4MB buffer ⚠️ yetersiz — 8MB varsayılan + `net.core.rmem_max/wmem_max` 16-64MB'ye çıkarılmalı; `nstat UdpRcvbufErrors` ile ölçülmeli.
- **SO_REUSEPORT:** ⚠️ Yalnız çok-worker ölçeklemede; naive hali worker respawn'da reconnect fırtınası yaratır → eBPF stable hashing gerekir.
- **Cert pinning vs TOFU:** ✅ Doğru (TOFU MITM'ye açık). Ancak netcode.io'da **X.509 PKI yok** — "key pinning" (Ed25519 public key) olarak adlandırılmalı.
- **Cubic vs BBR:** ❌ Düzeltme. Ham UDP'de Cubic/BBR **geçerli değil** (kernel TCP algoritmaları). Native UDP'da app-level token-bucket pacing + hafif bandwidth estimation uygula; herhangi bir TCP/QUIC side-channel'da **BBR+fq** kullan (kayıplı WiFi/mobil last-mile'da Cubic'den daha iyi).
- **0-RTT idempotent-only:** ✅ Doğru (WebTransport path'i için).

### 1.2 Alternatifler
| Seçenek | Dil | Şifreleme | Reliable+Unreliable | 600p olgunluk | WASM |
|---|---|---|---|---|---|
| **lightyear+netcode** (seçili) | Rust | AES-256-GCM | ✅ | ⚠️ doğrulanmamış | ✅ |
| GameNetworkingSockets (Valve) | C++/Rust-bind | AES-256-GCM+Ed25519 | ✅ ack-vector | ✅ Steam-scale | ⚠️ relay |
| naia | Rust | WebRTC DTLS | ✅ | ⚠️ | ✅ |
| bevy_quinnet | Rust | QUIC TLS1.3 | ⚠️ | ⚠️ | ❌ |
| ENet/renet/turbulence | C/Rust | ❌ | kısmi | değişken | ❌ |

**En güçlü fallback:** GameNetworkingSockets (`gns-rs`) — eğer soak testi lightyear'ın ECS overhead/pacing eksikliğini 600'da gösterirse.

### 1.3 Tavsiyeler (§1)
1. QUIC reddi gerekçesini düzelt: "per-byte AEAD değil, per-packet overhead + no kernel offload; WASM zaten QUIC".
2. "cert pinning" → "server public-key pinning (not TOFU)".
3. Buffer: ≥8MB + `rmem_max/wmem_max` 16-64MB, `UdpRcvbufErrors` ile izle.
4. SO_REUSEPORT yalnız çok-worker'da + eBPF stable hashing.
5. Congestion: native UDP'da app-level pacing; TCP/QUIC side-channel'da BBR+fq.
6. **Zorunlu 600-oyuncu soak testi** kapısı ekle (CPU, pps, drop rate, ECS system time).

---

## 2. Replication Katmanı (§2) — lightyear / Replicon

### 2.1 Doğrulama
- **Çapraz kesen kritik bulgu:** lightyear **0.27.0 (2026-06-22)** replikasyonu **bevy_replicon** üzerine rebuild etti. Eski `ReplicationGroup`, legacy delta compression, per-component priority ve client→server component replication **kaldırıldı/parity'de değil**. Plan 16 eski internals'a göre yazılmış → **revizyon zorunlu**.
- **Replicon backend'in avantajı:** Her entity state'i tek shared buffer'a serialize edilir, her client o buffer'dan bir aralık kopyalar → MMO-scale fan-out için O(clients×entities) yerine doğru tasarım. 600 oyuncu için güçlü.
- **NetPos/NetRot split:** ✅ Hâlâ doğru ve Replicon'la daha da uyumlu. `replicate_diff()` (Diffable trait) ile field-level delta ücretsiz. Eski lightyear legacy delta API'sine **bağımlı kalma**.
- **Optimistic block edit + revert:** ✅ Endüstri standardı. Minecraft (`Block Changed Ack` + sequence), Veloren (message + compressed broadcast) ile doğrulandı.
- **Client→server authority:** ⚠️ Replicon desteklemiyor. Voxel editler mesaj olarak gittiği için Strata için sorun değil, ama "client-owned entity" (araç kontrolü) engellenir.

### 2.2 Alternatifler
| Seçenek | Tahmin/Interp | AOI | 600p | Not |
|---|---|---|---|---|
| **lightyear 0.27** | ✅ yerleşik | Replicon filter | İyi | Replicon+prediction/interp |
| bevy_replicon+quinnet | ❌ yok | Güçlü | Mükemmel | lightyear'ın "raw" backend'i |
| naia | ❌ yazılır | En iyi (UserScope) | İyi | WASM+grantular AOI |
| GGRS | P2P rollback | yok | ❌ | Doğru reddedildi |

### 2.3 Tavsiyeler (§2)
1. §2'yi lightyear 0.27/Replicon'a göre yeniden yaz; `ReplicationGroup`/legacy delta referanslarını kaldır.
2. NetPos/NetRot'u `replicate_diff()` ile custom compact (de)serialization ile kaydet.
3. **Per-edit `Sequence` id** (Minecraft modeli) ekle — client ghost block reconcile edebilsin.
4. **Block-edit SONUÇLARINI plan-08 streaming channel'ından geçir** (ayrı lightyear block-update mesajı değil). lightyear yalnız *intent* (`BreakBlock`/`PlaceBlock` + sequence) taşısın. Bu double-delivery'yi önler.
5. **Global bandwidth arbiter** (`StreamingManager`'da) tanımla: entity replication + block-edit intent + sector streaming arasında caps/priority.
6. Transport'ı açıkça seç: WASM gerekiyorsa lightyear+WebTransport / naia; değilse `bevy_replicon_quinnet` (QUIC) lean alternatif. quinnet'in WASM yok.

---

## 3. Interest Management / AOI (§3) — Sector-based

### 3.1 Doğrulama
- **Chebyshev (küp-tutarlı) sector AOI:** ✅ Doğru. Dünya 32m küp sector grid'i → üyelik integer sector koordinatında hesaplanmalı. Euclidean kesirli/partial sector üretir (sector atomik birim). `abs+max`, sqrt yok. Küp aşırı-seçimi (R=1'de ~1.9×) sınırlı ve kabul edilebilir.
- **Replication Graph / spatial index:** ✅ Doğru ölçekleme kararı. Unreal Fortnite 100×50k actor'da bunu kullanıyor. **Ders:** Kazanç yalnız spatialization değil, **cached/dirty-flagged subscription listeleridir** — her tick recompute eden index hâlâ O(P·E)'dir.
- **Sürekli priority:** ✅ Sabit 3-tier'den daha iyi (Halo prioritization scheduler). Ama **global per-client bandwidth budget + priority queue** olmadan uzak entity'ler sessizce starve olur.
- **Sector-aligned hysteresis:** ✅ Doğru. Bant genişliği ≥ 1 sector olmalı. Asimetrik olmalı (enter=R+k, exit=R).
- **SVDAG/GPU occlusion = relevance çarpanı:** ⚠️ **Kısmi doğru, en büyük risk.** Occlusion 2-6× trafik azaltır (Boulanger vb. doğrular) AMA **hard gate OLAMAZ** — sadece soft multiplier. Render occlusion (frustum dahil) gameplay relevance değildir; duvar arkasındaki düşmanı cull'etmek desync/haksızlık yaratır. Occlusion (frustum değil) sonucundan besle, floor'u non-zero tut. 2.5×/6× rakamı **ölçülmeli**, varsayılmamalı.

### 3.2 Alternatifler
Uniform grid-of-sectors (seçili) > octree/k-d-tree (uniform dünya için gereksiz) > SpatialOS (overkill). Source-style PVS ≈ grid-cells.

### 3.3 Tavsiyeler (§3)
1. Chebyshev'ı koru; Euclidean'i yalnız ACTIVE içi secondary filter olarak kullan.
2. Sector-Room'ları **cached subscription set** olarak implement et (boundary crossing'de add/remove).
3. **Halo-style per-client priority queue + bandwidth budget** ekle; combat 30Hz pin; en düşük priority'yi drop et.
4. **Asimetrik hysteresis** (enter=R+k sector, exit=R), whole-sector aligned.
5. Occlusion'ı **soft multiplier** yap (frustum değil, occlusion); floor non-zero; 2.5×/6×'ı profille.
6. AOI telemetry ekle (per-tier entity, churn rate, occlusion saving, priority-band dağılımı).

---

## 4. Tier-Bazlı Delta Sync (§4) — BrickDelta

### 4.1 Doğrulama
- **"One BrickDelta = one sub-brick" fixed inline (17B, zero alloc):** ✅ ACTIVE tier için geçerli ve projenin "no Vec in hot path" kuralını karşılıyor. Ancak iki verimsizlik:
  - `sector` (6B) **her delta'da tekrar gönderiliyor** — en büyük önlenebilir maliyet. Minecraft tek section coord'u bir kez gönderir; local 12-bit pos yeter.
  - `mats:[u8;8]` az değişen voxel'de ≤41% israf (k=1'de 17B vs 10B) — bounded ama variable tail daha iyi.
- **Brick-granular bulk:** 8 sub-brick'i 8 ayrı delta = 136B; brick-granular = 79B (~42% tasarruf). İnteraktif edit için doğru granularity, ama **bulk path (explosion/world-edit) gerekli**.
- **DISTANT = 4-byte SVDAG root index:** ✅ Sadece client node data'yı zaten tutuyorsa güvenli. **Eksik:** missing-node fetch fallback (aksi halde distant terrain'de delik). ARCHIVE lazy-load doğal populate mekanizması ama §4 wire etmemiş.

### 4.2 Alternatifler
Minecraft (container-anchored packed VarLong), Luanti (whole MapBlock resend+compress), Veloren (full snapshot on load + sparse edits). Strata'nın "sector tekrar gönder" modeli yalnız container coord'u tekrar gönderen tasarım.

### 4.3 Tavsiyeler (§4)
1. **`BrickDeltaBatch { sector, deltas:[SubDelta;N] }`** ekle — sector bir kez, her SubDelta = brick+sub_brick+mask+mats. 6B×N kurtarır.
2. Material tail'i variable yap (yalnız `popcount(mask)` palette byte, pozisyon set bit'lerden).
3. **`BulkEditBurst`** mesajı (brick/sector granular, 64-voxel u64 mask) ekle.
4. **DISTANT→ARCHIVE dependency:** 4-byte root index yalnız client node pool'da varsa; yoksa SVDAG-fetch + client→server missing-node NACK.
5. **Palette genişliğini doğrula:** `mats:u8` = 256 cap. `SectorPalette` 256'yı aşarsa `u16`'a genişlet veya per-sector remap.

---

## 5. Delta Compression + Quantization (§5)

### 5.1 Doğrulama
- **Sector-anchored i16 (±1.05M m):** ✅ Sesli (Y ekseni "sınırsız yükseklik" için i32'ye genişletme veya ±1M m hard bound düşün).
- **⚠ SPEC HATASI — local precision/range çelişkisi:** "12-bit @0.05m, 32m range" ikisi birden doğru olamaz. 12-bit@0.05m = 204.8m; 32m@12-bit = 7.8mm. Düzelt: (a) 12-bit over 32m → 7.8mm (önerilen, "0.05m" yalnız delta'ya uygulanır) VEYA (b) 10-bit@0.05m over 32m (2 bit tasarruf).
- **Fiedler multi-level bitpacking:** ✅ Mimari doğru, ama: (1) bit-width'ler histogram-tuned değil (Fiedler kendi dataset'inde greedy search yaptı); (2) "medium=13-bit" range tanımsız (absolute ile redundant olabilir); (3) fallback'ın absolute keyframe olduğu netleştirilmeli.
- **Rotation:** yaw-only u16 ✅; 32-bit smallest-three ✅. **⚠ SPEC HATASI:** `a:u10,b:u10,c:u10` **signed** yazılmalı (sign-magnitude veya linear map). Raw unsigned magnitude sign kaybeder, reconstruction bozulur.
- **Velocity kaldırma (dead reckoning + event-driven):** ✅ Valve/Source modeli (remote entity velocity ağda YOK, Hermite türetir). "active/at-rest" flag kritik. 3 event (impulse/teleport/at-rest→moving) **reliable** olmalı.
- **Entity-level mask/RLE silindi:** ✅ Doğru (lightyear/naia per-component diff zaten sparse).
- **600p bant genişliği gerçeği:** Tam replikasyon ~0.8 Mbps/downstream → 256kbps hedefi compression tek başına **tutmaz**; AOI/scoping (plan 08) asıl lever.

### 5.2 Alternatifler
| Alan | En iyi | Not |
|---|---|---|
| Position | Sector-anchored (plan) | en iyi fit |
| Delta | Fiedler 2-level veya 3-level (tuned) | arithmetic coding +25% ama CPU |
| Rotation | smallest-three 32-bit (plan) | octahedral 32-bit marjinal daha iyi ama decode zor |
| Velocity | Source/Fiedler model (plan) | her zaman gönderme = waste |
| Serde | hot=hand-rolled bitpack; RPC=bitcode/postcard; blob=ruzstd | per-tick zstd YAPMA |

### 5.3 Tavsiyeler (§5)
1. Local quantization spec'ini düzelt (A1 seçeneklerinden biri).
2. "medium" tier'ı tanımla veya Fiedler 2-level'a düşür.
3. **Greedy histogram tuner** ekle (gerçek trace'lerle bit-width arama).
4. Smallest-three'u **signed** olarak düzelt.
5. 3 velocity event'ini reliable yap; dead reckoning'i interp+keyframe window ile sınırla.
6. Per-component diffing'in yerinde olduğunu doğrula.
7. Per-tick snapshot'a zstd **yapma**; ruzstd yalnız chunk/SVDAG blob'a.
8. Rotation delta-encoding'i Phase-2 optimizasyon olarak ekle.

---

## 6. Chunk Compression (§6)

### 6.1 Doğrulama
- **Palette (dynamic 1-8 bit + Direct fallback) + PackedBitBuffer:** ✅ Minecraft'tan daha iyi (Minecraft 4-bit floor; Strata 1-bit). 16-type cap **conservative** → 8-bit (256)'e çıkar (Minecraft gibi; decode tek-byte lookup).
- **lz4 (unreliable) vs zstd+dict (reliable):** ✅ Doğru. Deflate reddi doğru.
- **zstd dictionary +20-50%:** ✅ Gerçekçi (Discord 2.5×, prod game 70-90%). Build-time trained + content-hash versioned ship et. Runtime negotiation (Discord'ın terk ettiği) DEĞİL.
- **DISTANT = SVDAG(zstd), PNG+Lanczos silindi:** ✅ Doğru. PNG lossy rasterized, GPU ray-march pipeline'ı (plan 10) ile çelişir.
- **BlockChangeBatch (200/batch, 15-bit pos, 50ms, ReliableOrdered):** ✅ Çoğunlukla doğru. 15-bit pos ✅. **MTU riski:** 200×4B ≈ 800B + envelope + encryption ≈ 820-860B, 1200B altında ama headroom az. **~150'e düşür** veya per-batch palette/1-byte type.
- **Serde:** Voxel payload = custom bitstream ✅; structured control = FlatBuffers/Cap'n Proto (protobuf YOK hot path'te).

### 6.2 Tavsiyeler (§6)
1. Dynamic-bit + Direct + tight PackedBitBuffer koru.
2. **Indirect palette cap'ı 8-bit (256)'ya çıkar.**
3. zstd dict'i build-time ship et (content-hash versioned).
4. **200/batch → ~150** veya 1-byte type/batch palette ile MTU-safe yap (≤1000B).
5. **Block-change sequence/ack** ekle (Minecraft `block_changed_ack` gibi) — ghost block revert.
6. Custom bitstream koru; FlatBuffers/Cap'n Proto yalnız structured msg'ler için.
7. RLE'yi yalnız static terrain'de optional pre-pass olarak düşün.

---

## 7. Client-Side Prediction & Reconciliation (§7)

### 7.1 Doğrulama
- **Partial rollback (manual 4-frame snapshot):** ❌ **Çakışma.** lightyear zaten `PredictedHistory` + tick-granular rollback yapar (`earliest_mismatch_input`, `with_rollback_condition`, `max_rollback_ticks`). Manual 4-frame snapshot redundant + lightyear'ınkiyle çakışır (double rollback/desync). **Drop et, lightyear'ı configure et.**
- **Input redundancy sliding window (all unacked):** ✅ Overwatch modeli doğru. Ama **bounded** olmalı (~20-30 input). 200ms/60Hz'de ~12 input/packet ≈ 7-12 KB/s upstream/player × 600 = 4-7 MB/s ingress (feasible ama yüksek).
- **Velocity-based smooth correction + threshold:** ✅ Yön doğru, ama **BUG:** `<0.01m none` + smooth 0.01m, oysa quantization step **0.05m**. 0.02-0.05m hataları noise → smooth edilir. Düzelt: `<0.05m none`; 0.05-0.10m smooth; 0.10-1.0m fast lerp; >1.0m snap. **Velocity'yı smooth etme, snap et.**
- **0ms input delay (adaptive silindi):** ✅ Doğru (Valve/Overwatch/SnapNet). Remote interpolate edildiği için 0ms doğru.
- **Optimistic block place/break + revert:** ✅ Minecraft `BlockStatePredictionHandler` ile doğrulandı. Lightyear component history **dışında** ayrı layer olarak tut (double rollback önle).

### 7.2 Alternatifler
lightyear built-in rollback (adopt) >> manual snapshot (reject). GGRS (reject: P2P, cheating, no 600p). Fully client-auth (reject). Minecraft-style optimistic (adopt).

### 7.3 Tavsiyeler (§7)
1. **Manual 4-frame snapshot'ı DROP et** → lightyear `Predicted` + `with_rollback_condition` + `max_rollback_ticks ~8-12`.
2. Threshold'ı düzelt (no-op floor = quantization step 0.05m); critically-damped spring; velocity snap.
3. Input redundancy'yi lightyear `InputTimeline`'da all-unacked sliding window, **cap ~20-30**, RLE.
4. 0ms input delay koru; jitter buffer yalnız remote interp.
5. Optimistic edit layer: capture server-state snapshot → apply + async dirty-brick regen → pending list (ack keyed) → reject'te `world.revert()`. lightyear component rollback **dışında**.
6. 600p için Rooms/interest + bandwidth cap + priority'ye güven; `tc netem` ile profille.

---

## 8. Entity Interpolation (§8)

### 8.1 Doğrulama
- **Adaptive buffer (ratio 2/3 + jitter_ema, `ceil(delay/interval)+2`):** ✅ Valve/Fiedler ile uyumlu. Loss tolerance snapshot interval cinsinden (20Hz ve 30Hz'de otomatik doğru). **Eksik:** ratio oscillation hysteresis (enter >2%, revert <1%).
- **Per-component policy (Lerp/Nlerp/Slerp/Snap/Hermite):** ✅ Nlerp <30° + **mandatory hemisphere flip** (`dot<0 → negate`) CPU-optimal. Slerp fallback hızlı spinner'lar için.
- **Velocity extrapolation (cap 2× send_interval):** ✅ Bounded, opt-in. Default = freeze.
- **Hermite (Catmull-Rom):** ✅ Fiedler ile doğrulandı, **zero extra bandwidth** (tangent buffered snapshot'lardan türetilir). Position-only opt-in.
- **lightyear çakışması:** lightyear `Interpolation` + `InterpFn` + `interpolation_delay` verir. **Kendi circular buffer'ını KURMA** (double delay). Yalnız policy + delay ver.

### 8.2 Tavsiyeler (§8)
1. ratio 2/3 koru + **hysteresis** ekle.
2. lightyear `interpolation_delay`'i sür; ikinci buffer KURMA.
3. Rotation `InterpFn`: `dot<0 → negate`; `acos(|dot|)>30° → Slerp` else Nlerp (renormalize).
4. Snap components → `ComponentSyncMode` ile interp disable.
5. Extrapolation opt-in (fast movers), position-only, ≥2 snapshot, cap 2×, exponential blend-back, default freeze.
6. Hermite = centripetal Catmull-Rom, position-only opt-in.
7. Spawn/despawn fade = client alpha/scale tween (interp dışında).

---

## 9. Lag Compensation (§9)

### 9.1 Doğrulama
- **⚠ Kritik:** lightyear `LagCompensationPlugin` **client-side** + Avian-coupled. **Sunucu-taraflı rewind YOK.** Strata kendi authoritative rewind yöneticisini yazmalı (`HitboxHistory` ring buffer). `History<Transform>` kullanma.
- **rewind_tick formülü:** `rtt/2` yaklaşık ve **interpolation term eksik**. Doğru: `incoming_latency + shooter_interp` (shooter'ın render interp'i, target'ın DEĞİL). `rtt/2` symmetric approximation; Strata interpolated remote players kullandığı için half-RTT+interp doğru (full-RTT yalnız other-player prediction'da).
- **HISTORY_TICKS = ceil(200ms/tick):** ✅ maxunlag=200ms ile tutarlı. 200ms **aggressive** (Source default 1.0s) → mode-configurable yap.
- **4 hardening note:** (1) interpolation = shooter'ın (relabel), (2) teleport guard ✅, (3) usercmd clamp ✅, (4) never rewind world ✅ (dynamic voxel "shoot-through" edge case'i document et).
- **Eksik hardening:** lag-switch detection (server RTT vs reported ping), multi-shot/burst consistency (RAII RewindGuard), rewind hitbox size/alive, anti-backtrack timestamp verify, statistical aimbot monitoring.

### 9.2 Tavsiyeler (§9)
1. `History<Transform>` → custom `HitboxHistory` ring buffer (pos, angles, size, alive, sim-time), depth=`HISTORY_TICKS`.
2. Düzeltir formül: `incoming_ticks + shooter_interp_ticks`, clamp `[0, maxunlag_ticks]`; `target_tick` clamp `[server_tick-HISTORY_TICKS, server_tick]`; `+0.2s` clock-skew guard.
3. Client'lar `interp_ms` (EMA) rapor etsin; server rewind'da kullansın.
4. **RAII `RewindGuard`**: non-shooter/non-teammate hitbox'ları `target_tick`'e rewind (iki kayıt arası lerp), scope exit'te restore; single active session per shooter.
5. Teleport guard (delta > threshold, Source 64 unit).
6. **Anti-cheat**: lag-switch (RTT discrepancy), usercmd tick window verify, one rewind per usercmd (burst), server-side fire-rate, statistical accuracy monitor.
7. `sv_maxunlag` configurable (default 200ms competitive).
8. Dynamic voxel "shoot-through" edge case'i document et + coarse world-version check.

---

## 10. Çapraz Kesen Tavsiyeler (Cross-Cutting)

1. **Planı lightyear 0.27 / Replicon'a yeniden haritala** (§2, §7, §8, §9 etkilenir). Eski API referanslarını temizle.
2. **600-oyuncu soak testi kapısı** tüm transport/replication/prediction kararlarına ekle. lightyear pre-1.0, published 600p benchmark yok.
3. **Global bandwidth arbiter** (StreamingManager) — replication, block-edit intent, sector streaming arasında hard cap. Replicon'un soft priority'si hard cap değil.
4. **Block-edit sonuçlarını plan-08 streaming'den geçir** — double-delivery önle, single source of truth.
5. **Spec bug'ları düzelt** (Öncelik P0): §5 local precision/range, §5 smallest-three signed, §4 sector tekrar, §4 MTU, §7 correction threshold, §1 QUIC gerekçe, §9 rewind formülü.
6. **Occlusion'ı soft multiplier yap** — hard gate desync yaratır.
7. **lightyear'ın buffer/history/rollback'unu yeniden icat etme** — yalnız policy layer'ı yaz.

---

## 11. Doğrulama Sonucu (Validation Verdict)

| Bölüm | Karar | Durum |
|---|---|---|
| §1 Transport | lightyear UDP + netcode | ✅ Yön doğru / gerekçe düzelt |
| §1 QUIC reddi | native QUIC yok | ✅ Sonuç / gerekçe yanlış |
| §2 Replication | lightyear (Replicon 0.27) | ⚠️ API revizyonu zorunlu |
| §2 NetPos/NetRot | split + diff | ✅ |
| §3 AOI | Chebyshev sector | ✅ |
| §3 Continuous priority | distance×importance | ✅ + budget gerek |
| §3 Occlusion multiplier | soft | ⚠️ Hard gate OLAMAZ |
| §4 BrickDelta | fixed inline, 1 sub-brick | ✅ + batch + missing-node |
| §5 Quantization | sector-anchored + Fiedler | ✅ + 2 spec bug |
| §5 Smallest-three | 32-bit | ✅ + signed düzelt |
| §5 Velocity kaldırma | dead reckoning + event | ✅ |
| §6 Chunk comp | palette + lz4/zstd+dict | ✅ + cap 256 + MTU |
| §7 Prediction | lightyear rollback | ✅ manual snapshot DROP |
| §7 Input redundancy | all-unacked window | ✅ + bound |
| §7 0ms input delay | no adaptive | ✅ |
| §8 Interpolation | adaptive + per-component | ✅ + hysteresis |
| §9 Lag comp | server rewind | ⚠️ lightyear VERMEZ, yaz |

**Sonuç:** Plan 16'nın mimari yönü %90 doğru ve endüstriyle uyumlu. Kalan iş; lightyear 0.27'ye yeniden haritalama, somut spec bug düzeltmeleri, occlusion'ı soft'a çevirme ve 600-oyuncu soak testi. Kritik (anayasa 01-15 ile çelişmeyen) taslak plan olduğu için bu düzeltmeler plana işlenebilir.
