# 16 — Network Senkronizasyonu (lightyear)

> **Karar:** lightyear (Bevy 0.18, prediction+rollback+interpolation+lag-comp, UDP/WebTransport) — **birincil netcode stack**.
> **Neden:** Server-authoritative 600-oyuncu için en olgun *prediction/rollback/lag-comp* çözümü. bevy_replicon yalnız replication+AOI verir, §7/§9'daki prediction alt sistemi eksikti. lightyear her ikisini de (replication + prediction + interpolation + lag-comp) tek pakette sunar.
> **Transport:** lightyear native UDP (encrypted, reliable/ordered + unreliable) + WebTransport (WASM client). QUIC (Quinn/bevy_quinnet) ayrıca değerlendirildi ama lightyear ile çakışıyor ve 600-oyuncuda CPU vergisi yüksekti (bkz. §1) → birincil stack QUIC değil.
> **Araştırma:** `plans/network-research/` (quic-audit, network-audit) + `16-consolidated-audit-tr.md`

---

## 0. Mimari Genel Bakış (Optimize — lightyear revizyonu)

```
┌─────────────────────────────────────────────────────────────┐
│  Katman 5: Voxel-Specific Systems                           │
│  ├── Optimistic block placement (instant visual)            │
│  ├── Incremental mesh regeneration (dirty bricks only)      │
│  ├── Block change batching (max 200/batch, 50ms flush)      │
│  └── Palette-based chunk compression (zstd + dict + palette)│
├─────────────────────────────────────────────────────────────┤
│  Katman 4: Prediction & Interpolation (lightyear)           │
│  ├── lightyear Prediction (Predicted/PrePredicted/History)  │
│  ├── Partial rollback (divergence point, lightyear rollback)│
│  ├── Input redundancy (sliding-window, tüm unacked)          │
│  ├── Velocity-based smooth correction (render-only)         │
│  ├── Adaptive jitter+loss buffer (EMA + loss-run-length)    │
│  ├── Slerp rotation (default) / Nlerp (<20-30°, hemisphere) │
│  ├── Per-component policy (Lerp/Nlerp/Snap/Hermite)         │
│  └── Lag compensation (lightyear LagCompensationPlugin)     │
├─────────────────────────────────────────────────────────────┤
│  Katman 3: Interest Management + Delta (OPTIMIZE)           │
│  ├── Chunk-based AOI (XBrickMap sector = Room/SPS topic)    │
│  ├── 4-tier visibility (ACTIVE/WARM/DISTANT/ARCHIVE)        │
│  ├── Continuous priority (mesafe+önem → send-rate)          │
│  ├── Sparse + field-level delta (NetPos/NetRot split)       │
│  ├── Position quant (i16 sector + 0.05m local, Fiedler)     │
│  ├── Rotation: yaw u16 / 32-bit smallest-three (10,10,10+2) │
│  ├── Velocity kaldırma (dead reckoning + event-driven)      │
│  └── SVDAG occlusion relevance çarpanı (GPU culling reuse)  │
├─────────────────────────────────────────────────────────────┤
│  Katman 2: Replication (lightyear)                          │
│  ├── lightyear server-authoritative (20-30Hz tick)          │
│  ├── ECS-native snapshot/entity replication                │
│  ├── Interest management (Room/visibility, sector bazlı)    │
│  └── Messages (client-side prediction için)                 │
├─────────────────────────────────────────────────────────────┤
│  Katman 1: Transport (lightyear)                            │
│  ├── Native UDP (encrypted, reliable/ordered + unreliable)  │
│  ├── WebTransport (WASM client) — opsiyonel                 │
│  ├── Channels: ReliableOrdered + Unreliable                │
│  ├── Congestion control (app-level pacing; BBR yalnız QUIC side-channel'da)   │
│  └── 0-RTT reconnect (idempotent-only)                      │
├─────────────────────────────────────────────────────────────┤
│  Katman 0: UDP Socket                                       │
│  ├── Tek UDP socket (600+ connection)                       │
│  ├── SO_REUSEPORT (multi-core Linux)                        │
│  └── 4MB send/recv buffer                                   │
└─────────────────────────────────────────────────────────────┘
```

### Bant Genişliği Özeti (600 Oyuncu)

```
AOI + Sparse + Field-level Delta + Octahedral + Dead Reckoning:
  - Aktif entity: 90 × ~5B = 450 byte/frame
  - Statik entity: 510 × ~0.4B = 204 byte/frame
  - Header: ~75 byte
  - Toplam: ~729 byte/frame × 20Hz = ~14.6 KB/s
  - Input redundancy: +1.8 KB/s
  - Chunk/Blok: +0.7 KB/s
  - GENEL TOPLAM: ~17 KB/s/oyuncu × 600 = ~10.2 MB/s

Mevcut plan: ~21 KB/s/oyuncu × 600 = 12.6 MB/s
Optimize:    ~17 KB/s/oyuncu × 600 = 10.2 MB/s (%19 ek tasarruf)
```

---

## 0.1 Denetim Güncellemeleri (2026-07)

Bu plan, 7 alt-ajanlı kapsamlı teknik denetim (deep web araştırması + karşılaştırmalı analiz) sonucunda aşağıdaki şekilde güncellenmiştir. Tüm düzeltmeler ilgili bölüme `⚠️ Denetim güncellemesi` notu olarak işlenmiştir.

| # | Bölüm | Kritik Düzeltme |
|---|-------|-----------------|
| 1 | §2.2 | Sürüm pini düzeltildi: `bevy_replicon_quinnet 0.18 → 0.19` |
| 2 | §2.4 | `bevy_replicon_attributes` çıkarıldı → yerel `VisibilityFilter` API'si |
| 3 | §2.6 | `EventReader` → `MessageReader` (shared message API düzeltmesi) |
| 4 | §2.5/§3.1 | PriorityMap eşikleri sector sayılarıyla uyumlu (48/80/112m) |
| 5 | §3.2 | Histeresi +16m → sector'a hizalı (+32/+64/+96m) |
| 6 | §4.2 | `BrickDelta Vec<u16>` → sabit inline bit-packed (sıfır alloc) |
| 7 | §5.1 | Sabit 600-bit maske → mask↔varint-index + delta/RLE |
| 8 | §5.3 | `i16` mutlak → sector-anchored `(i32 sector, 12-bit local)` |
| 9 | §5.4 | Oktahedral kod (bozuk) → yaw (2B) / 32-bit smallest-three (4B) |
| 10 | §5.5 | Dead reckoning çarpışma caveat'i eklendi |
| 11 | §6.1 | Palette → `SectorPalette` reuse + bit-pack; deflate→zstd |
| 12 | §6.2 | DISTANT PNG **silindi** → SVDAG zstd (plan 07) |
| 13 | §6.3 | BlockChangeBatch 256→200, 15-bit pos, reliable stream |
| 14 | §7.4 | Adaptif girdi gecikmesi **silindi** (prediction'ı yok eder) |
| 15 | §7.9 | lightyear önerildi; GGRS reddedildi |
| 16 | §8.1 | Interp delay 3× (lossy), buffer `ceil(delay/interval)+2` |
| 17 | §8.2/§8.6 | Nlerp "<0.1°" düzeltildi → Slerp varsayılan |
| 18 | §9.2 | "200ms/10-15 kare" → `HISTORY_TICKS = ceil(200ms/TICK)` |
| 19 | §1.4/§1.5/§1.7 | TOFU→cert pinning; BBR experimental→Cubic; 0-RTT idempotent-only |

> **P0 (uygulanabilirlik için zorunlu):** #1, #2, #8, #9, #14, #12.

---

## 0.2 Revizyon Güncellemeleri (2026-07, lightyear geçişi)

7 alt-ajanlı derin web araştırması (`16-consolidated-audit-tr.md`) sonucunda plan **birincil netcode stack olarak `lightyear`'a çevrildi**. Gerekçe: bevy_replicon yalnız replication+AOI verirken, §7/§9'daki prediction/rollback/lag-comp alt sistemi eksikti; lightyear bunu hazır sunar ve bevy_replicon ile mutually exclusive birincil stack'tir (çelişki C1 çözüldü). Aşağıdaki ek düzeltmeler de uygulandı:

| # | Bölüm | Kritik Düzeltme |
|---|-------|-----------------|
| 20 | Tüm plan | Birincil stack: bevy_replicon+quinnet → **lightyear** (prediction/rollback/interp/lag-comp dahil) |
| 21 | §2/§3 | `PriorityMap` O(P·E) taraması → **spatial index'ten sürülen AOI** (Replication Graph prensibi); küre/küp tutarsızlığı giderildi (Chebyshev/sector-coord) |
| 22 | §3 | Sabit 3-tier frekans → **sürekli priority** (mesafe+önem; kritik combat entity 30Hz) |
| 23 | §3 | SVDAG occlusion → relevance **çarpanı** (üst 6×, ortalama 2.5×); CPU ray-march YOK |
| 24 | §0/§3.4 | Bant genişliği tahmini → **header (~+10%) + AOI burst** eklendi; agregat ~14 MB/s, 100 Mbps = tavan |
| 25 | §5.3 | Pozisyon: `i32`+0.01m → **`i16` sector + 0.05m local** (48 bit tasarruf, görsel fark yok) |
| 26 | §5.4 | 32-bit smallest-three bit layout: geçersiz `11,11,10+2` → **`10,10,10 + 2-bit index = 32`** |
| 27 | §5.6/§12 | Fiedler multi-level bitpacking → **çekirdek faza alındı** (ileri yerine); small=7-bit tune |
| 28 | §5 | Entity-level mask/RLE katmanı **silindi** (lightyear/replicon zaten sparse); `Transform` → `NetPos`/`NetRot` split |
| 29 | §5.5 | Dead reckoning → **event-driven velocity** (impulse/teleport/"at rest→moving" geçişinde) |
| 30 | §6.2 | **zstd dictionary** eklendi (build-time trained, +20-50%); `ChunkData` datagram path → **lz4** |
| 31 | §4.2/§6.1 | `BrickDelta` sub-brick/material tutarsızlığı giderildi (tek sub-brick VEYA mats=popcount) |
| 32 | §7.2 | Input redundancy: sabit "son 3" → **sliding-window (tüm unacked)** |
| 33 | §8.1 | Interp buffer: jitter EMA + **loss-run-length** sizing; mandatory hemisphere dot-flip |
| 34 | §7.9 | lightyear artık **birincil**, bevy_replicon_snap/GGRS tamamen çıkarıldı |

> **P0 (uygulanabilirlik için zorunlu):** #20, #21, #24, #31.
> **P1 (yüksek değer/düşük risk):** #22, #23, #25, #26, #27, #29, #30, #32, #33.

---

## 0.3 Kapsamlı Teknik Denetim (2026-07, 9-alt-ajanlı derin web araştırması)

> **Kaynak:** `researchs/16-audit-report-tr.md` (konsolide Türkçe denetim raporu). 9 bileşen
> (transport, replication, AOI, delta sync, quantization, chunk compression, prediction,
> interpolation, lag-comp) ayrı alt-ajanlarla analiz edildi; her biri için deep web research +
> alternatif karşılaştırması yapıldı. Bu blok, planın öneriler ışığında **zorunlu revizyonlarını**
> içerir. Mimari yön ~%90 doğru bulundu; kalan iş lightyear 0.27'ye haritalama, spec bug
> düzeltmeleri, occlusion'ı soft'a çevirme ve 600-oyuncu soak testidir.

### 0.3.1 Düzeltme Tablosu

| # | Bölüm | Kritik Düzeltme |
|---|-------|-----------------|
| 35 | §1.1 | QUIC reddi gerekçesi düzeltildi: "2-3.5× per-byte AEAD" **yanlış** — netcode.io ve QUIC aynı AES-GCM cipher; maliyet per-packet overhead + no kernel offload. WASM path zaten QUIC (WebTransport). |
| 36 | §1.3 | Socket buffer **4MB → 8MB**; `net.core.rmem_max/wmem_max` 16-64MB; `nstat UdpRcvbufErrors` ile izle. |
| 37 | §1.3 | SO_REUSEPORT yalnız **çok-worker** ölçeklemede; naive hali worker respawn'da reconnect fırtınası → eBPF stable hashing. |
| 38 | §1.4 | "cert pinning" → **server public-key (Ed25519) pinning** (netcode.io X.509 PKI yok; TOFU reddedildi). |
| 39 | §1.5 | Native UDP'da Cubic **geçersiz** → app-level token-bucket pacing + hafif BW estimation; TCP/QUIC side-channel'da **BBR+fq**. |
| 40 | §1 | **600-oyuncu soak testi kapısı** eklendi (CPU/pps/drop/ECS system time); lightyear'ın yayınlanmış 600p benchmark'ı yok. |
| 41 | §2 | **lightyear 0.27.0 (2026-06-22) replikasyonu bevy_replicon üzerine rebuild etti.** Eski `ReplicationGroup`, legacy delta, client→server repl, per-component priority geçersiz/beklemede → plan 0.27/Replicon API'sine revize. |
| 42 | §2.3 | NetPos/NetRot → **`replicate_diff()`** (Diffable trait) ile field-level delta. |
| 43 | §2.6 | Per-edit **`Sequence` id** (Minecraft `Block Changed Ack` modeli) eklendi — client ghost block reconcile. |
| 44 | §2.6 | Block-edit **SONUÇLARI plan-08 streaming'den** geçir; lightyear yalnız *intent* (`BreakBlock`/`PlaceBlock`+seq) taşısın (double-delivery önle). |
| 45 | §2 | **Global bandwidth arbiter** (`StreamingManager`): replication + block-edit intent + sector streaming arası hard cap. |
| 46 | §3 | Chebyshev korundu; sector-Room'lar **cached subscription set** + Halo-style per-client priority queue/budget. |
| 47 | §3.2 | **Asimetrik hysteresis** (enter=R+k sector, exit=R), whole-sector aligned. |
| 48 | §3 | Occlusion **soft multiplier** (hard gate YOK); frustum değil occlusion; 2.5×/6× **profilleden** doğrulanmalı. |
| 49 | §4.2 | **`BrickDeltaBatch { sector, deltas:[SubDelta;N] }`** — sector bir kez, 6B×N kurtarılır (Minecraft container-anchored). |
| 50 | §4.2 | Material tail opsiyonel variable (`popcount(mask)` palette byte). |
| 51 | §4.2 | **`BulkEditBurst`** (brick/sector granular, 64-voxel u64 mask) — explosion/world-edit için. |
| 52 | §4.3 | DISTANT 4-byte root index yalnız client node pool'da varsa; yoksa **SVDAG-fetch + client→server missing-node NACK**. |
| 53 | §4.2 | `mats:u8` = 256 cap; `SectorPalette` aşarsa `u16`/per-sector remap. |
| 54 | §5.3 | **Local precision/range çelişkisi düzeltildi:** 12-bit@0.05m≠32m. Seçenek: (a) 12-bit over 32m→7.8mm (önerilen) VEYA (b) 10-bit@0.05m over 32m. |
| 55 | §5.6 | "medium=13-bit" **tanımsız** → range belirtilmeli veya Fiedler 2-level'a düşürülmeli. |
| 56 | §5.6 | **Greedy histogram tuner** eklendi (gerçek trace ile bit-width arama). |
| 57 | §5.4 | Smallest-three `a:u10,b:u10,c:u10` **signed** yazılmalı (sign-magnitude/linear map); raw unsigned magnitude sign kaybeder. |
| 58 | §5.5 | 3 velocity event'i (impulse/teleport/at-rest→moving) **reliable/ordered** olmalı. |
| 59 | §5.1 | Per-component diffing lightyear/naia'da yerinde doğrulandı (entity-level mask冗余 değil). |
| 60 | §5 | Per-tick snapshot'a **zstd YAPILMAZ**; ruzstd yalnız chunk/SVDAG blob. |
| 61 | §5 | AOI/scoping (plan 08) asıl bandwidth lever — compression tek başına 256kbps'ı tutmaz (~0.8 Mbps/downstream full replikasyonda). |
| 62 | §6.1 | Palette cap **8-bit (256)'ya** çıkarıldı (Minecraft gibi; decode tek-byte lookup). |
| 63 | §6.2 | zstd dictionary **build-time trained + content-hash versioned** ship edilir (runtime negotiation değil). |
| 64 | §6.3 | BlockChangeBatch **200 → ~150** (veya 1-byte type/batch palette) → MTU-safe (≤1000B); + sequence/ack. |
| 65 | §7.1 | **Manual 4-frame snapshot DROP** → lightyear `PredictedHistory` + `with_rollback_condition` + `max_rollback_ticks ~8-12` (çakışma/desync önle). |
| 66 | §7.3 | Correction eşiği **0.05m floor** (quantization step); velocity **snap** (smooth ETME). |
| 67 | §7.2 | Input redundancy cap **~20-30** + RLE; lightyear `InputTimeline` üzerinden. |
| 68 | §7.6 | Optimistic edit layer lightyear component history **DIŞINDA** (ayrı capture-snapshot+revert). |
| 69 | §8.1 | Ratio hysteresis (enter>2%, revert<1%) eklendi — oscillation önle. |
| 70 | §8 | lightyear `Interpolation` buffer **KULLAN** (ikinci circular buffer kurma); yalnız `InterpFn`+`interpolation_delay` ver. |
| 71 | §8.2 | Rotation `InterpFn`: `dot<0 → negate`; `acos(|dot|)>30° → Slerp` else Nlerp (renormalize). |
| 72 | §9 | lightyear `LagCompensationPlugin` **sunucu rewind VERMEZ** (client-side+Avian) → custom `HitboxHistory` ring buffer. |
| 73 | §9.1 | rewind formülü: `incoming_latency + shooter_interp`; `target_tick` clamp `[server-HISTORY_TICKS, server]`; `+0.2s` clock-skew guard. |
| 74 | §9.1 | Client **`interp_ms` (EMA)** rapor etsin; server rewind'da kullansın. |
| 75 | §9.1 | **RAII `RewindGuard`** + single session/shooter + teleport guard + anti-cheat (lag-switch RTT discrepancy, usercmd backtrack verify, burst=1 rewind, statistical aimbot). |
| 76 | §9.2 | `sv_maxunlag` **mode-configurable** (default 200ms competitive; sandbox'ta yükseltilir). |

> **P0 (uygulanabilirlik için zorunlu):** #35, #36, #38, #39, #41, #42, #43, #44, #48, #52, #54, #57, #64, #65, #72, #73, #75.
> **P1 (yüksek değer/düşük risk):** #37, #40, #45, #46, #47, #49, #50, #51, #53, #55, #56, #58, #59, #60, #61, #62, #63, #66, #67, #68, #69, #70, #71, #74, #76.

### 0.3.2 Bölüm Bazında Özet

- **§1 Transport:** lightyear birincil stack doğru; QUIC reddi sonucu doğru ama gerekçe düzeltildi. Buffer 8MB'ya, SO_REUSEPORT koşullu, key-pinning, app-level pacing + BBR.
- **§2 Replication:** lightyear 0.27 = Replicon backend → eski API referansları geçersiz. NetPos/NetRot `replicate_diff()`, per-edit sequence, block-edit sonucu plan-08 streaming, global bandwidth arbiter.
- **§3 AOI:** Chebyshev + continuous priority korundu; cached subscription + Halo budget + asimetrik hysteresis; occlusion YALNIZ soft multiplier.
- **§4 Delta Sync:** `BrickDeltaBatch` (sector batch) + `BulkEditBurst` + DISTANT missing-node fetch; palette 256 cap.
- **§5 Quantization:** local precision/range düzeltildi; medium tier tanımlandı; greedy tuner; smallest-three signed; velocity event'leri reliable; per-tick zstd yok.
- **§6 Chunk Compression:** palette 8-bit (256); zstd dict build-time; batch 150'ye MTU-safe + ack.
- **§7 Prediction:** manual snapshot DROP → lightyear config; correction 0.05m floor; input cap 20-30; optimistic edit ayrı layer.
- **§8 Interpolation:** ratio hysteresis; lightyear buffer kullan; rotation InterpFn.
- **§9 Lag Comp:** custom `HitboxHistory` rewind + düzeltilmiş formül + RAII guard + anti-cheat.

---

## 1. Transport Katmanı (lightyear — UDP / WebTransport)

> **Revizyon (2026-07):** Birincil stack lightyear'a çevrildi. lightyear kendi UDP tabanlı transport'unu (encrypted, reliable/ordered + unreliable) ve WASM için WebTransport'u getirir; bevy_quinnet/Quinn (QUIC) **birincil değildir**. QUIC derin araştırması `plans/network-research/quic-audit-2026.md`'de; 600-oyuncuda CPU vergisi (~2-3.5×/byte AEAD) ve lightyear ile çakışma nedeniyle birincil seçilmedi. İleride QUIC backend'i (valve GameNetworkingSockets benzeri) opsiyonel olarak eklenebilir.

### 1.1 Neden lightyear Transport (UDP tabanlı)?

| Kriter | Ham UDP | lightyear UDP | QUIC (Quinn) |
|--------|---------|---------------|--------------|
| Şifreleme | ❌ Manuel | ✅ Yerleşik (encrypted packets) | ✅ TLS 1.3 zorunlu |
| Güvenilirlik | ❌ Elle | ✅ Reliable+Ordered / Unreliable | ✅ Stream/Datagram |
| Bağlantı kurulumu | — | Hızlı handshake | 1-RTT (0-RTT tekrar) |
| HOL blocking | Yok | Yok (channel bazında) | Yok (stream bazında) |
| CPU vergisi | Düşük | Düşük | ⚠️ Yüksek (~2-3.5×/byte AEAD) |
| WASM client | ❌ | ✅ WebTransport | ❌ |
| Tek socket, 600 conn | Manuel | ✅ | ✅ |

### 1.2 lightyear Kanal Yapılandırması

| Kanal | Tip | Kullanım |
|-------|-----|----------|
| `GameInput` | Unreliable (input) | Oyuncu input'u (prediction) |
| `GameState` | Unreliable | Entity state updates (predicted/interpolated) |
| `ReliableEvents` | ReliableOrdered | Chat, envanter, olay |
| `ChunkData` | Unreliable | Chunk streaming (lz4 ile sıkıştırılır) |
| `BlockChanges` | ReliableOrdered | Blok yerleştirme/kırma |
| `Auth` | ReliableOrdered | Handshake, auth (0-RTT reconnect) |

### 1.3 Native UDP Socket (600+ Oyuncu Optimize)

```rust
// lightyear native UDP transport — temel socket ayarları
UdpSocketConfig {
    // Tek UDP socket, 600+ bağlantı
    recv_buffer_size: 8 * 1024 * 1024,   // 8MB recv buffer (#36; yük altında drop önle)
    send_buffer_size: 8 * 1024 * 1024,   // 8MB send buffer
    // Linux: net.core.rmem_max/wmem_max 16-64MB'ye çıkar; nstat UdpRcvbufErrors ile izle
    // Çok-worker ölçeklemede: SO_REUSEPORT + eBPF stable hashing (#37)
    // (lightyear Io tipine göre platform-specific)
}
// 0-RTT reconnect: idempotent-only (auth/sunucu listesi);
// authoritative input/komut 0-RTT ile TAŞINMAZ (replay koruması)
// Not: WASM client zaten QUIC (WebTransport) kullanır; native path UDP+netcode (#35)
```

### 1.4 Şifreleme & Auth

- **Sunucu:** Kalıcı self-signed sertifika + `save_on_disk` (lightyear transport encryption anahtarı).
- **İstemci:** İlk bağlantıda sunucu **public-key (Ed25519) pinning** — build-time, out-of-band, backup pinset ile (TOFU REDDEDİLDİ — MITM/impersonation'a karşı korumasız). **Not:** netcode.io X.509 PKI kullanmaz; "cert pinning" terimi yerine **key pinning** doğrudur (#38). Handshake sonrası imzalı **session token** auth (anti-cheat için kritik).
- **Production:** Let's Encrypt / dahili CA.

### 1.5 Congestion Control

> ⚠️ **Denetim güncellemesi (#39):** Ham UDP protokolünde Cubic/BBR **geçersizdir** (bunlar kernel TCP/QUIC algoritmalarıdır). Native UDP path'i kendi app-level **token-bucket pacing** + hafif bandwidth estimation'ını uygular (model: GameNetworkingSockets lane sharing). lightyear transport yerleşik bir congestion controller getirmez — uygulama throttle etmelidir.
>
> **BBR:** Yalnızca herhangi bir TCP/QUIC side-channel'da geçerlidir ve kayıplı WiFi/mobil last-mile'da Cubic'ten **daha iyidir** (model-based, bufferbloat azaltır). Böyle bir kanal varsa **BBR + `fq`** (`net.ipv4.tcp_congestion_control=bbr`) kullan; Cubic değil.

### 1.6 0-RTT Session Resumption

- İlk bağlantı: handshake.
- Sonraki bağlantılar: hızlı reconnect (anahtar ön-negotiate).
- ⚠️ 0-RTT paketleri replay edilebilir → yalnızca **idempotent, state-değiştirmeyen** veri (sunucu listesi isteği) ile; asla authoritative input/komut.

---

## 2. Replication Katmanı (lightyear)

> **Revizyon (2026-07):** Birincil stack `lightyear`'a çevrildi (bkz. §0.2 #20). lightyear replication + prediction + interpolation + lag-comp'i tek pakette sunar; bevy_replicon / bevy_replicon_quinnet / bevy_quinnet çıkarıldı.

### 2.1 lightyear Mimarisi

- **Server-authoritative:** Tüm oyun durumu sunucuda yönetilir.
- **ECS-native:** `#[derive(Component, Serialize, Deserialize, PartialEq)]` + `#[protocol]` ile kayıtlı component'ler replicate edilir.
- **Tick-based:** 20-30 Hz replication tick rate (600+ oyuncu için optimize).
- **Snapshot + prediction/interpolation:** lightyear yerleşik client-side prediction, rollback ve interpolation sağlar (§7/§8).
- **Transport:** lightyear native UDP / WebTransport (§1).

### 2.2 lightyear Entegrasyonu

> ⚠️ **Denetim güncellemesi (#41):** lightyear **0.27.0 (2026-06-22)** sürümü replikasyon backend'ini **bevy_replicon** üzerine rebuild etti. Eski `ReplicationGroup`, legacy delta compression, client→server component replication ve per-component priority **kaldırıldı/parity'de değil**. Aşağıdaki API örneği 0.27/Replicon'a güncellenmiştir; plan eski internals'a göre yazılmış kısımları 0.27'ye revize edilmelidir.

```rust
// Cargo.toml
[dependencies]
lightyear = "0.27"            // Bevy 0.18 uyumlu; replikasyon = bevy_replicon backend (#41)
lightyear::prelude::*         // Predicted, PrePredicted, with_rollback_condition, InterpFn

// Uygulama (server + client)
app.add_plugins(LightyearPlugins::new(NetworkSettings::default()))
   .add_server_plugin(ServerPlugin::default())
   .add_client_plugin(ClientPlugin::default());

// Protocol kaydı (0.27: Replicon-backed registration)
// NetPos/NetRot → replicate_diff() ile field-level delta (#42)
app.component::<NetPos>().replicate_diff();   // §5.3 (i16 sector + local quant)
app.component::<NetRot>().replicate_diff();   // §5.4 (yaw u16 / 32-bit ST)
app.component::<Health>().replicate();
app.add_message::<BlockPlaceRequest>();       // §2.6 (intent + sequence #43)
app.add_message::<BlockBreakRequest>();
```

### 2.3 Component Replication Kuralları

```rust
// Prediction/rollback için: Predicted + PrePredicted + History
#[derive(Component, Serialize, Deserialize, Clone, PartialEq)]
#[protocol(vec(NetPos))]
pub struct NetPos(pub QuantizedPosition);   // §5.3

#[derive(Component, Serialize, Deserialize, Clone, PartialEq)]
#[protocol(vec(NetRot))]
pub struct NetRot(pub CompactRotation);      // §5.4

// Client'ta predict edilen component'ler Predicted/PrePredicted ile işaretlenir:
// lightyear otomatik: local input → predict, server echo → reconcile (rollback)
```

> Not: `Transform`'ı `NetPos`/`NetRot` alt-component'lerine **böl** (Denetim #28) — böylece lightyear change-tracking'i field-level delta verir (yalnız değişen eksen/component telden gider). Entity-level mask/RLE katmanı **gereksiz** (lightyear zaten sparse).

### 2.4 Entity Visibility & Interest Management (sector-based)

lightyear `Room` / visibility ile sector-AOI uygulanır. AOI **spatial index'ten** (plan 08 `StreamingManager` grid/octree) sürülür — O(P·E) tarama YOK (Denetim #21, Replication Graph prensibi).

```rust
// Sector'ı bir Room olarak kullan; oyuncu subscribed olduğu sector Room'larını görür
// (lightyear InterestHandler: add_room / subscribe / unsubscribe)
fn update_aoi(
    players: Query<(&PlayerSector, &RoomSubscription)>,
    mut interest: ResMut<InterestHandler>,
) {
    for (player_sector, sub) in &players {
        // Yalnız subscribed sector'lardaki entity'ler aday (O(active))
        for sector in player_sector.aoi_sectors() {       // küp-tutarlı (Chebyshev/sector-coord)
            interest.subscribe(player, sector_room(sector));
        }
    }
}
```

> **Küp/küre tutarlılığı (#21):** Mesafe eşikleri yerine **sector koordinatı** ile üyelik hesapla (3×3×3 / 5×5×5 / 7×7×7 küp). Euclid mesafe küp tier'larını sessizce küreye çevirir.
> **ARCHIVE:** Room'dan çıkar (visibility-hidden) + priority 0.0 — insertion/removal hariç telden tamamen çıkar.

### 2.5 Continuous Priority (Mesafe + Önem → Send-Rate)

Sabit 3-tier frekans yerine **sürekli priority** (Denetim #22): mesafe + önem skoru → (player, entity) başına gönderim aralığı. Kritik (combat/owner) entity'ler mesafeden bağımsız 30Hz.

```rust
fn compute_priority(distance: f32, importance: f32) -> f32 {
    // Örnek eşikler (Chebyshev, sector=32m): ACTIVE~48m→1.0, WARM~80m→0.5, DISTANT~112m→0.1
    let base = if distance < 48.0 { 1.0 }
               else if distance < 80.0 { 0.5 }
               else if distance < 112.0 { 0.1 }
               else { 0.0 };
    (base * importance).clamp(0.0, 1.0)   // combat importance=1.0 → 30Hz pinned
}
```

### 2.6 Messages (Client-Side Prediction için — Block Place/Break)

lightyear `add_message` + client prediction: isteği hem client hem server'da anında uygula, server doğrular (reject'te revert).

```rust
app.add_message::<BlockPlaceRequest>()
   .add_message::<BlockBreakRequest>();

// Client-side prediction: anında uygula, sunucu doğrular
fn handle_block_place(
    mut events: MessageReader<BlockPlaceRequest>,   // lightyear MessageReader
    mut world: ResMut<VoxelWorld>,
) {
    for event in events.read() {
        world.place_block_optimistic(event.position, event.block_type); // anında görsel
    }
}
```

> Not: Voxel mutation (block place/break) entity-component prediction DEĞİL, **world mutation**'dır; lightyear message prediction ile halledilir, revert `world.revert()` ile yapılır.

> ⚠️ **Denetim güncellemesi (#43/#44/#45):**
> - Her block-edit mesajı bir **`Sequence` id** taşır (Minecraft `Block Changed Ack` modeli) — client ghost block'ları deterministik reconcile eder; sunucu `Acknowledge`/correct ile revert yapar.
> - Block-edit **SONUÇLARI (yerleştirilen blok verisi) plan-08 streaming channel'ından** geçir; lightyear yalnız *intent* (`BreakBlock`/`PlaceBlock` + sequence) taşır. Bu double-delivery'yi önler ve tek "dünya nedir" kaynağı sağlar.
> - **Global bandwidth arbiter** (`StreamingManager`): entity replication + block-edit intent + sector streaming arasında hard cap/priority uygular. Replicon'un soft priority'si hard cap değildir.

---

## 3. Interest Management (AOI)

### 3.1 XBrickMap Sector = SPS Topic

Strata'nın sector yapısı (32×32×32) doğal bir Spatial Publish/Subscribe birimi:

> **Revizyon (2026-07, #21/#22/#23/#24):** Üyelik **sector koordinatı** ile (küp-tutarlı, Chebyshev) hesaplanır; Euclid mesafe kullanılmaz. Frekans sabit 3-tier yerine **sürekli priority** (§2.5). SVDAG occlusion bir relevance **çarpanı** olarak uygulanır (üst 6×, ortalama 2.5×; CPU ray-march YOK — render'ın GPU culling sonucu yeniden kullanılır). Bant genişliği hesabına **header (~+10%) + AOI burst** eklenir (agregat ~14 MB/s, 100 Mbps = tavan, §3.4).

```
Oyuncu AOI = {sector | chebyshev(player_sector, sector) <= tier_radius_in_sectors}

Tier boyutları (sector = 32m küp):
  - ACTIVE:  3×3×3 = 27 sector  (küp yarıçap 1, tam XBrickMap, 20Hz)
  - WARM:    5×5×5 = 125 sector (küp yarıçap 2, SVDAG, 10Hz)
  - DISTANT: 7×7×7 = 343 sector (küp yarıçap 3, minimal, 2-5Hz)
  - ARCHIVE: yok (client-side generation)

AOI güncellemesi spatial index'ten (plan 08 StreamingManager) sürülür → O(active),
O(P·E) taraması YOK. Prioriteler §2.5 continuous formülü ile (mesafe × importance).
Occlusion: GPU/SVDAG visibility buffer'ı telden çıkarılacak entity'leri elemek için
relevance test'ine çarpan olarak uygulanır (ortalama ~2.5×, tepe 6×).

### 3.2 AOI Hysteresis (Titreşim Önleme)

> ⚠️ **Denetim güncellemesi:** Sabit +16m yarıçapa göre yanlış ölçeklenir (ACTIVE'de %33, DISTANT'da %14) ve grid ile mis-align. Histeresi **sector'a hizalanmalı** (tam sector dead-band, intra-sector titremeyi önler).

```
Giriş eşiği:  player_distance < AOI_radius + hysteresis_sectors * 32m
Çıkış eşiği: player_distance > AOI_radius

Önerilen margin (tier başına tam sector):
  - ACTIVE:  +32m  (1 sector, ~%67 of 48m radius — toleranslı)
  - WARM:    +64m  (2 sector, ~%80 of 80m radius)
  - DISTANT: +96m  (3 sector, ~%86 of 112m radius)

Bu, oyuncu sınırda durduğunda entity'lerin titrek gelip/gitmesini önler.
```

### 3.3 Visibility Güncelleme Akışı

```
1. Oyuncu pozisyonu değişir
2. SpatialGrid'de (plan 08) yeni hücre/sector hesaplanır
3. Eski AOI ile yeni AOI karşılaştırılır (Chebyshev, sector bazlı)
4. Yeni sector'ler → Room subscribe (lightyear InterestHandler)
5. Eski sector'ler → Room unsubscribe
6. lightyear otomatik: yeni entity'leri gönderir, eski despawn eder
```

### 3.4 Bant Genişliği Tahmini (600 Oyuncu, revize)

| Bileşen | Hesaplama | Bant Genişliği |
|---------|-----------|---------------|
| Entity pozisyon (90 aktif × 5B × 20Hz, Fiedler) | 90 × 5 × 20 | ~9 KB/s |
| Chunk değişiklikleri (10/s × 20B, lz4) | 10 × 20 | 200 B/s |
| Blok değişiklik batch (5/s × 100B) | 5 × 100 | 500 B/s |
| SVDAG güncellemeleri (2/s × 4B) | 2 × 4 | 8 B/s |
| **Payload (steady-state)** | | ~9.7 KB/s/oyuncu |
| **+ QUIC/UDP/IP header (~+10%)** | | ~10.7 KB/s/oyuncu |
| **+ AOI enter/exit burst (create/delete)** | peak | ~12-14 KB/s/oyuncu |

**600 oyuncu × ~14 KB/s ≈ 8.4 MB/s (steady) / ~14 MB/s peak** — 100 Mbps **tavan**; ≥150 Mbps link önerilir (peak katsayısı 1.3-2.0).

> Not: §0'daki 17 KB/s modeli ile tutarlı; §3.4'ün eski "200 entity × 5B" modeli silindi (çift-sayım). Gerçek on-wire header + boundary burst içerir.

---

## 4. Tier-Bazlı Delta Sync

### 4.1 Tier-Bazlı Güncelleme

| Tier | Sync Yöntemi | Paket Boyutu | Frekans |
|---|---|---|---|
| **ACTIVE** | Brick delta (sparse) | 10-50 byte/değişiklik | 20Hz |
| **WARM** | Brick delta + SVDAG root | 10-50B + 4B | 10Hz |
| **DISTANT** | SVDAG root index | 4 byte | 2-5Hz |
| **ARCHIVE** | Compressed SVDAG | 1-5KB | Lazy load |

### 4.2 Brick Delta Formatı

> ⚠️ **Denetim güncellemesi:** `Vec<u16>` heap allocation "no heap fragmentation" kuralını ihlal eder → sabit inline bit-packed struct. 
> ⚠️ **Revizyon (#31):** `mats:[u8;8]` yalnız bir sub-brick'i (8 voxel) kapsar; `changed_sub_bricks` (8-bit) × 8 voxel = 64 değişen voxel olabilir. Tutarsızlığı gidermek için: **tek `BrickDelta` = tek sub-brick** (o zaman `changed_sub_bricks` gereksiz) VEYA `mats` uzunluğu = `popcount(changed_voxels)` (zero-alloc için fixed `mats:[u8;8]` + "tek sub-brick" kısıtı önerilir).

```rust
#[repr(C)]
pub struct BrickDelta {
    pub brick_index: u8,        // 64 brick/sector (4^3) — sector Batch başlığından (#49)
    pub sub_brick: u8,          // tek sub-brick (0-7) — #31: tek sub-brick başına bir delta
    pub changed_voxels: u8,     // 8-bit mask (8 voxel/sub-brick)
    pub mats: [u8; 8],          // palette id per changed voxel (fixed 8 byte, zero alloc)
    // Opsiyonel variable tail: yalnız popcount(changed_voxels) palette byte (#50)
}
/// Sabit ~11 byte (sector'siz) + BrickDeltaBatch header. Allocation YOK, branchless.
/// Birden fazla sub-brick değişirse N adet SubDelta; bulk için Bakiniz BulkEditBurst (#51).

#[repr(C)]
pub struct BrickDeltaBatch {     // #49: sector'ı BİR kez gönder (6B×N kurtarır)
    pub sector: I16Vec3,        // 6 byte (batch başına bir kez)
    pub deltas: Vec<SubDelta>,  // length-prefixed; her SubDelta = BrickDelta (sector'siz)
}
/// Mats:u8 = 256 cap; SectorPalette aşarsa u16/genişlet (#53).
```

> ⚠️ **Denetim güncellemesi (#49/#50/#51/#53):** Orijinal `BrickDelta` her delta'da `sector` (6B) tekrar gönderiyordu (en büyük önlenebilir maliyet). `BrickDeltaBatch` ile sector bir kez gönderilir. Az değişen voxel için `mats` tail'i `popcount(mask)` kadar kısaltılabilir. Çok sub-brick'li kitlesel editler (explosion/world-edit) için `BulkEditBurst` (brick/sector granular, 64-voxel `u64` mask + packed palette) eklenir — 8× per-sub-brick çarpanını önler. `mats:u8` 256 tip cap'ini aşan `SectorPalette` için `u16`/per-sector remap gerekir.

### 4.3 SVDAG Snapshot Sync

```rust
pub fn send_sector_snapshot(sector: &Sector, peer: &mut Peer) {
    if let Some(root_index) = sector.svdag_root {
        // #52: 4-byte root index YALNIZ client node pool'da varsa gönderilir.
        // Yoksa SVDAG-fetch gerekir (aksi halde distant terrain'de delik).
        if peer.has_svdag_node(root_index) {
            peer.send(SvdagRootIndex { sector: sector.coord, root_index });
        } else {
            peer.send(SvdagSubtree {
                sector: sector.coord,
                root_index,
                subtree_data: node_pool.export_subtree(root_index),
            });
        }
    }
}
```

> ⚠️ **Denetim güncellemesi (#52):** DISTANT tier'ın 4-byte SVDAG root index'i, client'ın node data'yı zaten tutması koşuluna bağlıdır. Client eksik node bildirirse (`missing-node NACK`) sunucu subtree blob'unu (reliable) gönderir. ARCHIVE lazy-load bu cache'i doldurur; §4.3 ile §4 sync bağımlılığı açıkça wire edilmelidir.

---

## 5. Delta Compression + Quantization (Optimize)

### 5.1 Sparse Encoding + Bitmask (En Büyük Kazanç)

> ⚠️ **Denetim güncellemesi (#28):** Sabit 600-bit maske her frame 75 byte harcar; değişen entity ≪600 olduğunda israf. Gaffer yaklaşımı: snapshot başına **mask ↔ varint-index anahtarı** + **delta/RLE entity ID'si**. Ayrıca 5.1 (75B header) ile 5.8 (510×0.4B=204B static) muhasebesi çift-sayımdır. **lightyear zaten değişen component'leri gönderir (sparse yerleşik)** → entity-level mask/RLE katmanı **silindi**. Field-level delta için `Transform` → `NetPos`/`NetRot` alt-component split'i yapılır (lightyear change-tracking ücretsiz verir).

```rust
pub struct SparseEntityUpdate {
    pub use_bitmask: bool,        // >50% değiştiyse bitmask, yoksa index listesi
    pub changed_bitmask: BitVec,  // yoğun güncellemede
    pub changed_indices: Vec<u32>,// seyrek güncellemede (delta/RLE encode)
    pub deltas: Vec<EntityDelta>, // sadece değişenler
}
// 90 aktif × 6B + index overhead ≈ 600-700 byte/frame (%87 tasarruf, tutarlı muhasebeyle)
```

### 5.2 Field-Level Delta (Component Mask)

Sadece değişen bileşenleri gönder (x değişti ama y/z değişmedi → sadece x gönder):

```rust
pub struct DeltaHeader {
    pub changed_fields: u8,  // 3 bit: bit0=x, bit1=y, bit2=z
    // Sonrasinda sadece değişen bileşenlerin verisi
}
// Yürüme (x+z değişir): 1 + 4 = 5 byte (%17 tasarruf vs full)
// Düşme (sadece y): 1 + 2 = 3 byte (%50 tasarruf)
```

### 5.3 Position Quantization (Sector-Anchored — DÜZELTİLDİ, #25)

> ⚠️ **Denetim güncellemesi (#25):** Mutlak `i16` ±327.67m = ±10 sector → "sınırsız yükseklik" anayasasıyla çelişir. Sector-anchored çözüm korunur ama: `i32` sector **aşırı** (96 bit/keyframe) → **`i16` sector** (±32767 × 32m = ±1.05M m, yeterli) + **12-bit local @ 0.05m** (1 cm yerine; 1m-voxel dünyada görsel fark yok, 48 bit tasarruf). Per-frame delta Fiedler multi-level (§5.6) ile; absolute keyframe her ~16-32 tick veya teleport'ta.

```rust
#[repr(C)]
pub struct QuantizedPosition {
    pub sector: I16Vec3,   // mutlak sector koordinatı (keyframe/spawn) — 48 bit
    // #54: "12-bit @0.05m, 32m" çelişkiliydi. Seçenek (önerilen):
    // 12-bit over 32m sector → 7.8125mm hassasiyet (delta 0.05m'den daha sıkı, anchor'da kayıp yok).
    // Alternatif: 10-bit @0.05m over 32m (2 bit tasarruf). Decode math buna göre ayarlanmalı.
    pub local: [u16; 3],   // sector içi 12-bit @ 7.8125mm (32m aralık)
}
// Per-frame delta: Fiedler multi-level — #55: "medium" tier range tanımlı OLMALI
// (13-bit @0.05m zaten absolute aralığı kapsar → redundancy; Fiedler 2-level'a düşünülebilir).
// #56: bit-width'ler gerçek trace ile greedy histogram tuner ile ayarlanmalı.
pub struct PositionDelta { /* Fiedler-encoded dx/dy/dz */ }
```

### 5.4 Rotation Compression — Octahedral (İyileştirme)

> ⚠️ **Denetim güncellemesi:** Aşağıdaki `octahedral_encode` kodu **round-trip yapamaz** — `(x,y,z)/(|x|+|y|+|z|)` bivektor yarıçapını atar; decode'da `w = √(1-x̂²-ŷ²-ẑ²)` → `w=0` zorlar, kuaterniyon kurtarılamaz. Kod atılmalı. Öneri: çoğu entity yalnız Y ekseni döner → **`u16` yaw (2 byte)**; tam 3-DOF için **32-bit smallest-three (4 byte)** (11,11,10 + 2-bit index — GEÇERSİZ; doğru: `10,10,10 + 2-bit index = 32`). Doğru oktahedral yalnız yarıçap korunarak yeniden yazılırsa 4B'da geçerlidir.

```rust
// ÖNERİLEN: yaw-only (voxel entity'ler için en iyi, 2 byte)
pub struct CompactRotation { pub yaw: u16 }   // 0-65535 = 0-360°, 0.005° hassasiyet

// Tam 3-DOF: 32-bit smallest-three (4 byte) — DÜZELTİLMİŞ layout
// #57: a/b/c SIGNED olmalı (sign-magnitude: 1 sign + 9 mag, veya [-1/√2,+1/√2]→[0,1023] linear map).
// Raw unsigned u10 magnitude sign kaybeder, reconstruction BOZULUR.
// Quaternion'ı negate et (en büyük component pozitif), 3 küçük component + 2-bit index gönder.
// pub struct QuatSmallestThree { a: i10, b: i10, c: i10, index: u2 }  // toplam 32 bit (signed)
```

**Conditional encoding:** 1-bit "yaw-only?" flag → çoğu entity 2 byte (`u16` yaw), yalnız pitch/roll kullanımında 4 byte (32-bit ST). Ortalama ~2 byte/entity.

### 5.5 Velocity Kaldırma (Dead Reckoning)

Velocity'yi gönderme — client son 2-3 pozisyondan hesaplasın:

```rust
// Client tarafında:
fn estimate_velocity(history: &[(f32, Vec3)]) -> Vec3 {
    if history.len() < 2 { return Vec3::ZERO; }
    let (t1, p1) = history[history.len() - 2];
    let (t2, p2) = history[history.len() - 1];
    (p2 - p1) / (t2 - t1)
}
// Velocity tamamen kaldırılır → entity başına 6 byte tasarruf
```

> ⚠️ **Denetim notu (#29):** Dead reckoning 20-30Hz'de pürüzsüz; ancak çarpışma/knockback/gravite değişimi/teleport'ta sabit-hız varsayımı bozulur. Bu durumlarda sunucu **event-driven velocity** göndermeli (yalnız impulse/teleport/"at rest→moving" geçişinde), per-tick vector göndermek yerine. Teleport/knockback'ta extrapolate yerine **snap** (Source/Valve).
>
> ⚠️ **Denetim güncellemesi (#58):** impulse / teleport / at-rest→moving geçiş event'leri **reliable/ordered** kanalda gönderilmeli (veya ack alana kadar yeniden). Aksi halde client velocity değişimini kaçırır ve desync olur. Dead reckoning interpolation/keyframe window ile sınırlanmalı (#61).

### 5.6 Multi-Level Bitpacking (Fiedler) — ÇEKİRDEK FAZ (#27)

> ⚠️ **Revizyon (#27):** Fiedler bitpacking "ileri/faz 15"ten **çekirdek faza** alındı — en yüksek ROI'li optimizasyon (Gaffer 17.4→0.26 Mbps'i büyük ölçüde bununla sağladı). Planın kendi 5-bit/13-bit/32-bit sayıları Gaffer'ın değil; **gerçek delta histogramı ile greedy tune** gerekir.

Glenn Fiedler'in snapshot compression yaklaşımı (Strata için retune edilmiş):

```
Her delta bileşeni için (0.05m quantization, 20-30Hz):
- 1 bit: "small mı?" flag
- Small: 7 bit [-64, +63] @0.05m = ±3.2m/tick   (yürüme ~10-30 unit/tick için yeterli)
- Değilse: 1 bit "medium mu?"
  - Medium: 13 bit [-4096, +4095] @0.05m
  - Değilse: 12-bit local absolute keyframe (full)
```

> Not: Per-frame `i16` düz delta yerine Fiedler kullan → tipik hareket için ~2× daha iyi (flat 7B → ~3.25B/entity). Mutlak sector-anchored keyframe (§5.3) her ~16-32 tick veya teleport'ta drift'i sıfırlar.

### 5.7 Delta Encoding (Optimize Edilmiş)

> ⚠️ **Denetim güncellemesi:** Bu `OptimizedDeltaEncoder` **framework ile çelişir** ve güncel API'lerle uyumsuz: `QuantizedPosition::from_vec3` (artık sector-anchored, §5.3) ve `octahedral_encode` (bozuk, §5.4) yok. bevy_replicon zaten değişen entity'leri gönderir; field-level delta için `Transform`'ı `NetPos`/`NetRot` alt-component'lerine **bölmek** (replicon change-tracking ücretsiz verir) veya özel delta channel kullanmak gerekir. Aşağıdaki kod yalnız *yaklaşım* örneğidir, olduğu gibi kullanılmaz.

```rust
pub struct OptimizedDeltaEncoder {
    last_positions: HashMap<Entity, QuantizedPosition>,
    last_rotations: HashMap<Entity, [u16; 2]>,  // Octahedral
}

impl OptimizedDeltaEncoder {
    pub fn encode_entity(&mut self, entity: Entity, pos: Vec3, rot: Quat) -> Vec<u8> {
        // ⚠️ İllüstratif: gerçek uygulamada QuantizedPosition sector-anchored (§5.3),
        // rotasyon ise yaw/32-bit smallest-three (§5.4) ile encode edilir.
        let quantized_pos = QuantizedPosition { sector: I32Vec3::ZERO, local: [0; 3] }; // placeholder
        let oct_rot: [u16; 2] = [0, 0]; // placeholder (yaw/ST kullanın)
        let mut buffer = Vec::new();

        // Component mask: hangi bileşenler değişti?
        let mut mask: u8 = 0;
        if let Some(last_pos) = self.last_positions.get(&entity) {
            let dx = quantized_pos.x - last_pos.x;
            let dy = quantized_pos.y - last_pos.y;
            let dz = quantized_pos.z - last_pos.z;

            if dx != 0 { mask |= 0x01; }
            if dy != 0 { mask |= 0x02; }
            if dz != 0 { mask |= 0x04; }

            buffer.push(mask);

            // Sadece değişen bileşenleri gönder
            if dx != 0 {
                if dx.abs() < 128 { buffer.push(dx as u8); }
                else { buffer.extend_from_slice(&dx.to_le_bytes()); }
            }
            if dy != 0 {
                if dy.abs() < 128 { buffer.push(dy as u8); }
                else { buffer.extend_from_slice(&dy.to_le_bytes()); }
            }
            if dz != 0 {
                if dz.abs() < 128 { buffer.push(dz as u8); }
                else { buffer.extend_from_slice(&dz.to_le_bytes()); }
            }
        } else {
            buffer.push(0x07); // Tüm bileşenler değişti (ilk değer)
            buffer.extend_from_slice(&quantized_pos.x.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.y.to_le_bytes());
            buffer.extend_from_slice(&quantized_pos.z.to_le_bytes());
        }

        // Rotation delta (sadece değişim varsa)
        if self.last_rotations.get(&entity) != Some(&oct_rot) {
            buffer.push(0x01);
            buffer.extend_from_slice(&oct_rot[0].to_le_bytes());
            buffer.extend_from_slice(&oct_rot[1].to_le_bytes());
        }

        self.last_positions.insert(entity, quantized_pos);
        self.last_rotations.insert(entity, oct_rot);
        buffer
    }
}
```

### 5.8 Bant Genişliği Karşılaştırması (Güncellenmiş)

| Veri | Mevcut (byte) | Optimize (byte) | Tasarruf |
|---|---|---|---|
| **Position** | 6 (3×i16) | 1-5 (component mask + delta) | %50-83 |
| **Rotation** | 8 (smallest-three) | 2-4 (octahedral/yaw-only) | %50-75 |
| **Velocity** | 6 (3×i16) | 0 (dead reckoning) | %100 |
| **Header** | 0 | 1 (component mask) | - |
| **Toplam/entity/frame** | **20** | **3-10** | **%50-85** |

**600 oyuncu, %15 aktif:**
```
Mevcut:  600 × 10B = 6000 byte/frame
Optimize: 90 × 6B + 510 × 0.4B = 744 byte/frame
Tasarruf: ~%87
```

---

## 6. Chunk Compression (Veloren İlhamı)

### 6.1 Palette-Based Encoding

> ⚠️ **Denetim güncellemesi:** `BrickPalette { Vec<BlockType>, Vec<u8> }` plan 05/06 `SectorPalette` (256 entry, `u16` block registry) ile **çelişir** ve zaten mevcut 4-seviye zincirini yeniden icat eder. 15 tip cap yüksek-çeşitlilik brick'i encode edemez. Yerine dynamic-bit encoding (`bits = ceil(log2(n_types))`, 1-8 bit, Direct fallback) + `SectorPalette` yeniden kullanımı. `indices: Vec<u8>` 512B yer kaplar; bit-packed (Minecraft long-array) ile 128B gerçekleşir.

```rust
// Her brick (8×8×8 = 512 voxel) için palette — SectorPalette ile paylaşılır
// #62: palet cap 8-bit (256) olarak güncellendi (Minecraft gibi; decode tek-byte lookup).
// 16 tip cap yerine 256'ya çıkar → yüzey brick'leri Direct path'ine zorlanmaz.
pub struct BrickPalette {
    palette: [BlockType; 256],      // max 256 tip (plan 05/06 SectorPalette)
    bits_per_voxel: u8,             // ceil(log2(unique)), 1-8 (dynamic)
    indices: PackedBitBuffer,       // bit-packed, 512 * bits bit (tight, long-aligned DEĞİL)
}
// 4 blok tipi: 2 bit/voxel → 512*2 = 1024 bit = 128 byte (vs 1024 byte ham) ✅
// Yüksek çeşitlilik: Direct (raw) fallback (Minecraft Single/Indirect/Direct modeli)
```

### 6.2 Chunk Verisi Sıkıştırma

| Yöntem | Boyut | Kalite | Kullanım |
|--------|-------|--------|----------|
| Palette-based (1-15 blok) | log2(n) bit/blok | Kayıpsız | ACTIVE chunk'lar (birincil compressor) |
| **zstd + dictionary** | +%20-50 (small payload) | Kayıpsız | WARM/ARCHIVE reliable path (#30) |
| **lz4** | 1.5-2× | Kayıpsız | `ChunkData` unreliable datagram path (#30) |
| Deflate | ~%17 (ham) | Kayıpsız | ⚠️ EN YAVAŞ; kullanma |
| Çeyrek çözünürlük PNG + Lanczos | %3-5 | Kayıplı | ❌ SİLİNDİ (bkz. aşağı) |

> ⚠️ **Denetim güncellemesi (DISTANT, #30):** "Çeyrek çözünürlük PNG + Lanczos (DISTANT)" **silinmelidir**. DISTANT (384-1536m) plan 07 uyarınca tamamen **SVDAG** (`SectorSvdag` node blob) ile render edilir. WARM sıkıştırmada deflate yerine **zstd** (dictionary desteği, deflate'dan 5-30x hızlı).
>
> **Revizyon (#30):** (a) **zstd dictionary** eklenir — build-time trained, client build ile ship edilir (Discord/Oodle pattern, small self-similar paletted brick bitstream'lerde +20-50%). (b) **Per-channel codec:** `ChunkData` unreliable datagram path → **lz4** (en hızlı, latency>ratio; kayıp AOI ile re-request); reliable snapshot/SVDAG → **zstd+dict**. (c) "25-27%" paleti + zstd birlikte; palet birincil, zstd ikincil — benchmark harness (Veloren örneği) ile ölçülmeli.
>
> ⚠️ **Denetim güncellemesi (#63):** zstd dictionary **build-time trained + content-hash versioned** olarak ship edilir (runtime negotiation DEĞİL — Discord'ın terk ettiği karmaşıklıktan kaçınmak için). Beklenen +20-50% (gerçekçi; prod game 70-90%'a varır). Payload dict-compressed daha büyükse plain zstd'e fallback (per-message ucuz karşılaştırma).

### 6.3 Block Change Batching

```rust
pub struct BlockChangeBatch {
    pub sector: SectorCoord,
    pub changes: Vec<(LocalPos15, BlockType)>,  // #64: max ~150/batch (MTU-safe), 15-bit packed pos
    pub sequence: u32,                           // #43: per-edit ack için (Minecraft modeli)
}

impl BlockChangeBatch {
    pub fn should_flush(&self) -> bool {
        // #64: 150 cap → ~600B + envelope + encryption ≈ ≤1000B (1200B MTU ceiling altında headroom)
        self.changes.len() >= 150 || self.age() > Duration::from_millis(50)
    }
}
```
> ⚠️ **Denetim notu (#31):** `LocalPos` 32³ sector içinde 3×5 bit = 15 bit → 2 byte packed; `BlockType` `u16` (plan 05) = 2 byte. Değişiklik başına 4B × 150 = ~600B (+header). `BlockChanges` **lightyear ReliableOrdered** kanalında MTU sorunu yok; datagram fallback için ~150 cap güvenli (200 yerine, encryption+envelope headroom'u için). 50ms flush = 20Hz. Sunucu reddederse `BrickDelta` echo ile revert + re-mesh.
>
> ⚠️ **Denetim güncellemesi (#64):** 200/batch 1200B MTU ceiling'ine yakındı (encryption+envelope ile headroom az). **~150'e düşürüldü** (veya per-batch 1-byte type/256 cap). Ayrıca **sequence/ack** (Minecraft `block_changed_ack`) eklendi — client ghost block revert'ı tam snapshot re-send olmadan yapar.

---

## 7. Client-Side Prediction & Reconciliation (Optimize)

### 7.1 Partial Rollback (Divergence Point'ten Yeniden Simülasyon)

Mevcut: Tüm `pending_inputs` listesini baştan simüle et (10-15 kare).
Optimize: Her 4 karede bir snapshot kaydet, sadece sapma noktasından itibaren yeniden simüle et (2-5 kare).

```rust
pub struct PredictionState {
    pub confirmed_state: PlayerState,
    pub pending_inputs: Vec<InputSequence>,
    pub predicted_state: PlayerState,
    pub snapshots: [(u32, PlayerState); 4],  // Her 4 karede bir kaydet
}

impl PredictionState {
    pub fn apply_input(&mut self, input: InputSequence) {
        self.pending_inputs.push(input.clone());
        self.predicted_state = self.simulate_single(&self.predicted_state, &input);

        // Her 4 karede bir snapshot kaydet
        if input.sequence % 4 == 0 {
            let idx = (input.sequence / 4) as usize % 4;
            self.snapshots[idx] = (input.sequence, self.predicted_state.clone());
        }
    }

    pub fn reconcile(&mut self, server_state: PlayerState, server_seq: u32) {
        // En yakın snapshot'ı bul
        let (snap_seq, snap_state) = self.find_closest_snapshot(server_seq);

        // Snapshot'tan itibaren yeniden simüle et
        self.predicted_state = snap_state;
        self.pending_inputs.retain(|i| i.sequence > snap_seq);
        for input in &self.pending_inputs {
            self.predicted_state = self.simulate_single(&self.predicted_state, input);
        }
    }
}
// Re-simülasyon: 10-15 kare → 2-5 kare (%67 azalma)

> ⚠️ **Denetim güncellemesi (#65):** Bu **manuel 4-frame snapshot** yaklaşımı lightyear'ın yerleşik `PredictedHistory` + `with_rollback_condition` + `max_rollback_ticks` ile **ÇAKIŞIR** (double rollback/desync riski). Uygulamada manuel snapshot **DROP** edilir; lightyear `Predicted`/`PredictedHistory` kullanılır:
> ```rust
> // lightyear 0.27: sadece yapılandır, yeniden icat etme
> app.add_prediction().with_rollback_condition(|confirmed, predicted| /* voxel mismatch */)
>    .with_rollback_policy(RollbackPolicy { state: true, input: true, max_rollback_ticks: 12 });
> ```
> lightyear zaten divergence tick'ten re-simüle eder (2-5 kare); `max_rollback_ticks ~8-12` CPU'yu sınırlar.
```

### 7.2 Input Redundancy (Paket Kaybı Toleransı) — #32

> ⚠️ **Revizyon (#32):** Sabit "son 3 input" Overwatch sliding-window (tüm unacked input'lar) ile değiştirildi. Sabit 3, ~100ms RTT + ≥30Hz üzerinde başarısız (200ms/60Hz'de 12 unacked). lightyear `InputMessage` zaten unacked input'ları gönderir.

```rust
// lightyear: her input paketi TÜM unacked input'ları taşır (sliding window)
// 10% paket kaybında bile tüm input'lar ulaşır
// Bant genişliği: ~input_boyutu × unacked_sayısı × tick (RTT'e bağlı)
// #67: window ~20-30 input ile CAP'lenir (loss spike bandwidth patlamasını önler) + RLE
```
> ⚠️ **Denetim güncellemesi (#67):** All-unacked sliding window doğru, ama **bounded** olmalı (~20-30 input). 200ms/60Hz'de ~12 input/packet ≈ 7-12 KB/s upstream/player × 600 = 4-7 MB/s ingress (feasible ama yüksek). lightyear `InputTimeline` üzerinden uygulanır; RLE/compression ile kayıp spike'larında patlama önlenir.

### 7.3 Velocity-Based Smooth Correction (Momentum Koruyucu)

Mevcut: basit lerp (momentum kaybı). Optimize: velocity-based correction:

```rust
fn smooth_correction(current: Vec3, target: Vec3, velocity: Vec3, dt: f32) -> (Vec3, Vec3) {
    let error = target - current;
    let correction_speed = 8.0;  // saniyede
    let correction = error * (1.0 - (-correction_speed * dt).exp());
    let new_pos = current + correction;
    let new_vel = velocity + correction * correction_speed;
    (new_pos, new_vel)
}
```

**Hibrit yaklașım (Overwatch ilhamı):**

| Hata Büyüklüğü | Düzeltme Yöntemi |
|---|---|
| < 0.01m | Yok (float precision toleransı) |
| 0.01m - 0.1m | Velocity-based smooth (4-8 frame) |
| 0.1m - 1.0m | Hızlı lerp (2-3 frame) |
| > 1.0m | Anında snap (teleport) |

> ⚠️ **Denetim notu:** Smooth correction **yalnız render transform'a** uygulanmalı (logical state snap, velocity mutate edilmemeli). "Yok" eşiği (0.01m) kendi kuantizasyon adımının (0.05m, §5.3) üzerine çıkarılmalı — aksi halde her frame küçük düzeltmeler tetiklenir.
>
> ⚠️ **Denetim güncellemesi (#66):** Correction eşiği **0.05m floor** olmalı (kuantizasyon adımı). Önerilen tablo: `<0.05m → yok`; `0.05-0.10m → smooth (critically-damped spring, ~2-4 frame)`; `0.10-1.0m → fast lerp (2-3 frame)`; `>1.0m → snap`. **Velocity'yı smooth ETME — reconcile'da snap et.** Sabit-frame lerp yerine frame-rate-independent spring tercih edilir.

### 7.4 Adaptive Input Delay (Jitter Buffer) — ❌ DÜZELTİLDİ

> ⚠️ **Denetim güncellemesi:** Yerel oyuncu **input'una gecikme eklemek prediction'ın amacını yok eder** (Gambetta/Valve/Overwatch: sıfır input gecikmesi). Bu bölüm **çIKARILDI**; gecikme yalnız **uzak entity'lerin interpolation**'una uygulanır (§8.1). Plan §9.2 zaten "girdi gecikmesi 0" diyor — bu bölümle çelişiyordu.

```rust
// DOĞRU YAKLAŞIM: input anında uygulanır (0ms). Jitter tamponu yalnız uzak entity render'ında.
pub struct AdaptiveInterpDelay {  // bkz. §8.1
    base_delay: Duration,
    jitter_estimate: Duration,      // EMA (alpha=0.2)
    buffer_depth: usize,            // sadece REMOTE entity interpolasyonu
    min_buffer: usize,              // 2
    max_buffer: usize,              // 8
}
// Yüksek jitter → interpolation buffer büyür, input gecikmesi DEĞİL
```

### 7.5 Voxel-Specific Prediction

| İşlem | Prediction Türü | Gecikme |
|-------|----------------|---------|
| Oyuncu hareketi | Component-based (continuous) | 0ms (lokal) |
| Blok yerleştirme | Event-based (optimistic) | 0ms, sunucu doğrular |
| Blok kırma | Event-based (raycast prediction) | 0ms, sunucu doğrular |
| Fizik (düşen bloklar) | Sunucu yetkili, interpolation | 100-150ms |
| Chunk yükleme | Predictive streaming (yön tahmini) | Değişken |

### 7.6 Optimistic Block Placement

```rust
fn predict_block_place(
    mut events: MessageReader<BlockPlaceRequest>,  // lightyear MessageReader
    mut world: ResMut<VoxelWorld>,
    mut pending: ResMut<PendingBlockChanges>,
) {
    for event in events.read() {
        // 1. Anında görsel güncelle (local voxel data)
        world.place_block_optimistic(event.position, event.block_type);
        // 2. Mesh regeneration'ı async tetikle
        mesh_queue.enqueue(event.chunk_coord);
        // 3. Pending listesine ekle (sunucu doğrulama bekliyor)
        pending.insert(event.id, PendingChange::Place(event.position, event.block_type));
    }
}
// Sunucu reddederse: blok kaldırılır, mesh tekrar üretilir
```
> ⚠️ **Denetim güncellemesi (#68):** Optimistic edit layer lightyear **component history DIŞINDA** tutulmalı (Minecraft `BlockStatePredictionHandler` modeli). Capture-snapshot → apply + async dirty-brick regen → pending list (sequence ile) → reject'te `world.revert()`. lightyear'in entity rollback'u dünya edit'lerini double-handle etmesin. Dirty brick'ler bütçeyle yeniden mesh edilir (§7.7).

### 7.7 Incremental Mesh Regeneration

Her blok değişikliğinde tüm chunk mesh'i yeniden üretilmesin:

```rust
pub struct PredictionMeshQueue {
    dirty_bricks: HashMap<ChunkCoord, HashSet<BrickCoord>>,
    max_per_frame: usize,  // Frame başına max mesh regeneration
}

impl PredictionMeshQueue {
    pub fn process(&mut self, meshes: &mut Assets<Mesh>) {
        let budget = self.max_per_frame;
        for (chunk, bricks) in self.dirty_bricks.drain().take(budget) {
            regenerate_brick_meshes(chunk, &bricks, meshes);
        }
    }
}
```

### 7.8 Determinism (İleri Aşama)

Cross-platform determinism gerekirse fixed-point math:

```rust
pub struct Fixed(i32);  // 16.16 format
impl Fixed {
    const SCALE: i32 = 65536;
    pub fn from_f32(val: f32) -> Self { Self((val * Self::SCALE as f32) as i32) }
    pub fn to_f32(&self) -> f32 { self.0 as f32 / Self::SCALE as f32 }
    pub fn add(self, other: Self) -> Self { Self(self.0.wrapping_add(other.0)) }
    pub fn mul(self, other: Self) -> Self {
        Self(((self.0 as i64) * (other.0 as i64) >> 16) as i32)
    }
}
// Unity ECS Galaxy Sample bu yaklaşımı kullanıyor
```

### 7.9 Prediction Katmanı — lightyear (BİRİNCİL, #34)

> ⚠️ **Revizyon (#34):** lightyear artık **birincil netcode stack** (bkz. §0.2 #20). Çelişki giderildi: bevy_replicon/bevy_replicon_snap/GGRS tamamen çıkarıldı. lightyear prediction + rollback + interpolation + lag-comp'i hazır sunar; §7/§8/§9 doğrudan lightyear API'leriyle uygulanır.

- **lightyear:** Birincil — `Predicted`/`PrePredicted`/`History` (rollback), `LagCompensationPlugin`, interpolation built-in (Bevy 0.18, `deterministic` feature opsiyonel).
- **GGRS ❌:** P2P lockstep fighting-game modeli, server-authoritative 600-oyuncu voxel ile uyumsuz.
- **bevy_replicon_snap ❌:** PoC, Bevy 0.15'te takılı, üretim yok.
- **bevy_timewarp:** değerlendirilebilir hafif alternatif (gerekirse).

> **Entegrasyon:** §7.1 partial rollback, §8 interpolation, §9 lag comp → lightyear'ın `ComponentHistory<T>` + `LagCompensationPlugin` ile. Voxel mutation (block place/break) lightyear message prediction ile (§2.6), dünya rollback'i `world.revert()`.

---

## 8. Entity Interpolation (Optimize)

### 8.1 Circular Snapshot Buffer (Adaptive)

> ⚠️ **Denetim güncellemesi:** `2 × send_interval` yalnız 1 paket kaybını tolare eder; Gaffer kayıplı ağ için **3×** önerir. Buffer sabit `8` magic number olmamalı: `ceil(delay/interval)+2`. Bu gecikme **input gecikmesi DEĞİL** — yalnız uzak entity render'ı (§7.4 düzeltmesine bakın).
> ⚠️ **Revizyon (#33):** 3× kuralı **loss-driven**, jitter değil. Buffer `max(loss_run_length × send_interval + margin, jitter_p95 + headroom)` ile size edilmeli; jitter EMA (α=0.2) yalnız bir girdi. Aksi halde burst loss altında yine under-run olur. lightyear interpolation bunu yerleşik yönetir (`InterpolationDelay` + `jitter`).

```rust
pub struct SnapshotBuffer {
    snapshots: Vec<Option<Snapshot>>,   // boyut = ceil(delay/interval) + 2
    head: usize,
    jitter_ema: f32,                    // EMA-based jitter estimate
    target_delay: f32,                  // Adaptive interpolation delay
}

impl SnapshotBuffer {
    pub fn push(&mut self, snapshot: Snapshot) {
        let arrival_delta = snapshot.timestamp - self.last_arrival;
        let jitter_sample = (arrival_delta - self.expected_interval).abs();
        self.jitter_ema = 0.2 * jitter_sample + 0.8 * self.jitter_ema;

        // Adaptive delay: INTERP_RATIO × send_interval + jitter (clean=2, lossy=3)
        let ratio = if self.loss_rate > 0.02 { 3.0 } else { 2.0 };
        self.target_delay = (ratio * self.send_interval + self.jitter_ema)
            .clamp(self.send_interval, self.max_delay);

        let cap = (self.target_delay / self.send_interval) as usize + 2;
        if self.snapshots.len() != cap { self.snapshots.resize(cap, None); }
        self.snapshots[self.head % cap] = Some(snapshot);
        self.head += 1;
    }
}

> ⚠️ **Denetim güncellemesi (#69/#70):** (1) `ratio` 2↔3 oscillation yapabilir → **hysteresis** ekle (enter >2% loss'ta 3'e, <1%'de 2'ye dön). (2) **lightyear `Interpolation` + `InterpFn` + `interpolation_delay` zaten buffer'ı verir** — kendi ikinci circular buffer'ını KURMA (double delay/bookkeeping). Yalnız policy (`InterpFn`) + `interpolation_delay` değerini ver.
```

### 8.2 Per-Component Interpolation Policy

Her bileşen aynı interpolasyon yöntemini kullanmamalı:

```rust
pub enum InterpolationPolicy {
    Lerp,       // Position, Scale
    Nlerp,       // Rotation (Slerp'ten 3-5x hızlı; yalnız <20-30° küçük açılarda sub-degree)
    Snap,       // Health, VoxelType, AnimationState
    Hermite,    // Hızlı hareket eden entity'ler (opt-in)
}

// Bevy ECS'de:
#[derive(Component)]
pub struct InterpolationPolicy(pub InterpolationPolicy);

// Rotation: Slerp varsayılan; Nlerp yalnız <20-30° küçük açılarda (⚠️ "<0.1° altı 60°" iddiası yanlış:
// gerçek relative hata ≤90°'de ~0.9-6°, 180°'e yakın 15-20°). nlerp/slerp için **zorunlu
// hemisphere dot-flip** (`if dot<0 { -b }`) — aksi halde antipodal (quaternion 4-D angle→180°)
// durumda numerik kararsız. Hata tablosu *quaternion 4-D angle* cinsinden okunmalı.
fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let dot = a.dot(b);
    let b = if dot < 0.0 { -b } else { b }; // En kısa yol + hemisphere check (zorunlu)
    (a * (1.0 - t) + b * t).normalize()
}

> ⚠️ **Denetim güncellemesi (#71):** Rotation tek bir `InterpFn` olarak uygulanır:
> ```rust
> // lightyear InterpFn<NetRot>: dot<0 → negate; acos(|dot|) > 30° → Slerp else Nlerp (renormalize)
> ```
> Bu CPU-optimal policy'dir (per-snapshot açı 20-30Hz'de neredeyse her zaman küçüktür). Snap component'lar (`ComponentSyncMode` ile interp disable) asla blend edilmez.
```

### 8.3 Velocity-Based Extrapolation (Buffer Underrun)

Snapshot gelmediğinde velocity-based devam:

```rust
fn extrapolate(last: &Snapshot, velocity: Vec3, elapsed: f32) -> EntityState {
    let max_extrapolation = 2.0 * send_interval; // Max: 2×send_interval
    let t = elapsed.min(max_extrapolation);
    EntityState {
        position: last.position + velocity * t,
        rotation: last.rotation, // Sabit tut
    }
}
// Max extrapolation aşıldı: son bilinen pozisyonda kal veya fade-out
```

### 8.4 Hermite Interpolation (Opt-in, Hızlı Hareket)

C1 süreklilik (tangent-continuous), lineer'den çok daha iyi:

```rust
fn hermite_interpolate(p0: Vec3, p1: Vec3, m0: Vec3, m1: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    p0 * (2.0*t3 - 3.0*t2 + 1.0)
    + m0 * (t3 - 2.0*t2 + t)
    + p1 * (-2.0*t3 + 3.0*t2)
    + m1 * (t3 - t2)
}

// Tangent hesaplama (Catmull-Rom):
// tangent[i] = 0.5 * (position[i+1] - position[i-1])
// Ek bandwidth gerektirmez — pozisyon verisinden otomatik üretilir
```

### 8.5 Spawn/Despawn Interpolasyonu

```rust
// Spawn: %50 alpha ile başla, 2-3 snapshot boyunca fade-in
fn spawn_fade_in(alpha: f32) -> f32 {
    (alpha * 2.0).min(1.0)  // 0.0 → 1.0 arası 2 snapshot'ta
}

// Despawn: 200ms fade-out
fn despawn_fade_out(elapsed: f32) -> f32 {
    (1.0 - elapsed / 0.2).max(0.0)  // 200ms'de 1.0 → 0.0
}
```

### 8.6 Snapshot Interpolation Parametreleri (Güncellenmiş)

| Parametre | Değer | Gerekçe |
|-----------|-------|---------|
| Buffer boyutu | 4-8 snapshot | Adaptive jitter buffer |
| Interpolasyon gecikmesi | `2 × send_interval + jitter_ema` | Adaptive (sabit değil!) |
| Position interpolation | Lerp (default), Hermite (opt-in) | %90 durumda lerp yeterli |
| Rotation interpolation | **Slerp** (varsayılan) / Nlerp (<20-30° için) | Nlerp 3-5x hızlı ama ≥90°'de birkaç derece hata |
| Extrapolation | Velocity-based, max `2 × send_interval` | Buffer underrun'da |
| Spawn fade-in | 100ms (2-3 snapshot) | Yumuşak giriş |
| Despawn fade-out | 200ms | Yumuşak çıkış |
| Jitter estimation | EMA (alpha=0.2) | Adaptive buffer boyutu |

---

## 9. Lag Compensation

### 9.1 Sunucu Taraflı Geri Sarma (Server-Side Rewind)

> ⚠️ **Revizyon:** lightyear `LagCompensationPlugin` + `History` component'i ile yerleşik sağlanır (§7.9). Aşağıdaki mantık lightyear API'si üzerinden uygulanır.

> ⚠️ **Denetim güncellemesi (#72):** lightyear `LagCompensationPlugin` **sunucu-taraflı rewind VERMEZ** — yalnız client-side + Avian-coupled'dir. Strata **kendi authoritative rewind yöneticisini** yazar (aşağıdaki `HitboxHistory`). `History<Transform>` kullanma.

```rust
// #72/#73: custom authoritative server rewind (HitboxHistory ring buffer)
struct HitboxHistory { records: [(Tick, HitboxState); HISTORY_TICKS] } // pos, angles, size, alive, sim-time

fn lag_compensated_hit_detection(shooter: &Player, server_tick: u32) -> bool {
    // #73: incoming_latency (≈ rtt/2, symmetric) + SHOOTER'ın render interp'i (target'ın DEĞİL)
    let incoming_ticks = shooter.measured_incoming_ms / TICK_DURATION;
    let interp_ticks   = shooter.reported_interp_ms / TICK_DURATION;  // #74: client EMA rapor eder
    let rewind_ticks   = (incoming_ticks + interp_ticks)
        .clamp(0, MAXUNLAG_TICKS);
    let target_tick = (server_tick - rewind_ticks)
        .clamp(server_tick - HISTORY_TICKS, server_tick);  // +0.2s clock-skew guard (#73)
    // #75: RAII RewindGuard — non-shooter/non-teammate hitbox'ları target_tick'e rewind (lerp), scope exit'te restore
    let _guard = RewindGuard::new(target_tick);
    raycast(shooter.aim, current_hitbox_positions())
}
```

> ⚠️ **Denetim notu (#sertleştirme / #75):** (1) Rewind'a **shooter'ın** interpolation delay'ini kat (target'ın DEĞİL — uzak görünen pozisyonu shoot anına çek). (2) **Teleport guard:** teleport anında rewind atla (pozisyon sıçramasını yok say; Source 64-unit threshold). (3) **usercmd tick replay clamp** (anti-backtrack): rewind_tick sunucu tick'ten fazla geriye gitmesin. (4) **Asla voxel world rewind etme** — yalnız entity hitbox'ları (dinamik voxel "shoot-through" edge case'i document edilmeli).
>
> ⚠️ **Denetim güncellemesi (#75 — anti-cheat):** Eksik hardening: (a) **lag-switch detection** — server-measured RTT vs reported ping discrepancy; (b) **usercmd tick window verify** (anti-backtrack, join-öncesi tick referansı engelle); (c) **burst weapon = 1 rewind per usercmd** (RAII guard, single active session per shooter); (d) server-side fire-rate/cooldown; (e) **statistical accuracy/headshot monitoring** (aimbot). (f) rewind hitbox size/alive flag'i de store edilmeli; restore RAII ile.

### 9.2 Parametreler

| Parametre | Değer | Gerekçe |
|-----------|-------|---------|
| Lag compensation limit | 200ms (**mode-configurable** `sv_maxunlag`, #76) | Anti-cheat sınırı (Valve 1.0s; Strata competitive'de daha sıkı, sandbox'ta yükseltilir) |
| Rollback penceresi | `HISTORY_TICKS = ceil(200ms / TICK_DURATION)` | ⚠️ "10-15 kare" sabit değil; tick rate'e bağlı (20Hz→4, 30Hz→6, 60Hz→12) |
| Girdi gecikmesi | 0 (prediction ile) | En iyi oyuncu hissi |
| Maks gecikme | 200ms+ → lag compensation devre dışı | Adil oyun |

---

## 10. Input Buffering

```rust
pub struct InputBuffer {
    pub inputs: [InputSequence; 64],
    pub head: u8,
    pub count: u8,
}

impl InputBuffer {
    pub fn push(&mut self, input: InputSequence) {
        let idx = (self.head + self.count) % 64;
        self.inputs[idx as usize] = input;
        self.count = self.count.min(63) + 1;
    }

    pub fn pop(&mut self) -> Option<InputSequence> {
        if self.count == 0 { return None; }
        let input = self.inputs[self.head as usize];
        self.head = (self.head + 1) % 64;
        self.count -= 1;
        Some(input)
    }
}
```

---

## 11. Server Reconciliation Flow

```
Client:  Input[1] → Input[2] → Input[3] → (predict) → Render
         ↓           ↓           ↓
Server:  ──────────── Input[1] ── Input[2] ── Input[3] → Validate
         ↓
Client:  ←── ServerState(seq=3) ── Reconcile pending inputs → Render
```

---

## 12. Performans Hedefleri (Optimize)

| Metrik | Mevcut Plan | Optimize Hedef | Not |
|--------|-------------|---------------|-----|
| Stack | bevy_replicon+quinnet | **lightyear** (prediction+rollback+interp+lag-comp) | §0.2 #20 |
| Replication tick rate | 20-30Hz | 20-30Hz | Değişmedi |
| Entity overhead/frame | 2-14 byte | **3-10 byte** | Fiedler + field-level delta (#27/#28) |
| Bant genişliği/oyuncu | ~21 KB/s | **~10-14 KB/s** (peak) | AOI + optimized delta + header/burst (#24) |
| Sunucu bant genişliği | 12.6 MB/s | **~8.4 MB/s steady / ~14 MB/s peak** | 600 oyuncu, ≥150 Mbps link (#24) |
| Prediction re-sim | 10-15 kare | **2-5 kare** | lightyear rollback (#34) |
| Rotation overhead | 8 byte | **2-4 byte** (yaw u16 / 32-bit ST 10,10,10+2) | #26 |
| Velocity overhead | 6 byte | **0 byte** (event-driven) | Dead reckoning (#29) |
| Statik entity overhead | full | **~0.4 byte** | lightyear sparse |
| Interpolation delay | 100-150ms sabit | **Adaptive (loss+jitter)** | #33 |
| Rotation interpolation | Slerp | **Slerp default + Nlerp <20-30°** | hemisphere check (#33) |
| Bağlantı kurulumu | 1-RTT/0-RTT | 1-RTT/0-RTT | lightyear reconnect |
| Reconnect süresi | ~0ms | ~0ms | Session resumption |
| Sunucu CPU | ~3ms/tick | ~3ms/tick | 600 connection (QUIC CPU vergisi yok) |

---

## 13. Risk Matrisi

| Risk | Olasılık | Etki | Azaltma |
|------|----------|------|---------|
| lightyear + bevy_replicon çelişkisi | Çözüldü | — | Tek stack = lightyear (#20) |
| lightyear 0.27 Replicon backend olgunluğu | Orta | Orta | 0.27 API'sine revize (#41); soak testi (#40) |
| 600+ oyuncu tek sunucu (lightyear ECS overhead) | Orta | Yüksek | Zone/shard + **600p soak testi kapısı** (#40) |
| Occlusion hard-gate desync/haksızlık | Orta | Yüksek | Occlusion **soft multiplier** (#48) |
| Manual snapshot vs lightyear rollback çakışması | Yüksek→Çözüldü | — | lightyear config'e düşürüldü (#65) |
| Sunucu-taraflı rewind yok (lightyear client-only) | Yüksek→Çözüldü | — | Custom HitboxHistory (#72) |
| SO_REUSEPORT reconnect fırtınası | Düşük | Orta | Yalnız çok-worker + eBPF stable (#37) |
| QUIC per-byte AEAD gerekçe yanlışlığı | Düşük→Çözüldü | — | WASM zaten QUIC; native UDP+netcode (#35) |
| 600+ oyuncu tek sunucu | Orta | Yüksek | Zone/shard mimarisi |
| AOI güncelleme overhead (O(P·E)) | Yüksek→Çözüldü | — | Spatial index'ten sürülür (#21) |
| Konsantrik/burst loss | Orta | Orta | loss-run-length buffer sizing (#33) |
| WASM desteği | Düşük | Düşük | lightyear WebTransport |
| lightyear bus factor (tek bakıcı) | Düşük | Orta | Topluluk + fork yedeği |
| zstd dictionary eksik (eski) | Çözüldü | — | Build-time dict (#30) |

---

## 14. Uygulama Sırası (Güncellenmiş)

| Faz | Ne | Ne Zaman | Beklenen Kazanç |
|-----|-----|----------|-----------------|
| 1 | lightyear kurulumu (UDP/WebTransport, **0.27 / Replicon backend**, #41) | Hemen | Temel stack |
| 2 | Basit server-authoritative replication + protocol (30Hz, `replicate_diff()`, #42) | Hafta 1-2 | Temel |
| 3 | Sector-based AOI (lightyear Room, spatial-index driven, küp-tutarlı, cached subscription, #46) | Hafta 3-4 | %80-90 bandwidth (#21) |
| 4 | Continuous priority + field-level delta (NetPos/NetRot split) + **Halo-style budget** (#46) | Hafta 5-6 | %70-80 ek tasarruf (#22/#28) |
| 5 | Fiedler multi-level bitpacking (çekirdek) + **greedy histogram tuner** (#56) | Hafta 7-8 | %20-40 ek tasarruf (#27) |
| 6 | Position quant (i16 sector + **12-bit@7.8mm**, #54) + rotation (yaw/32-bit ST **signed**, #57) | Hafta 9-10 | %50-75 quant tasarruf (#25/#26) |
| 7 | Velocity kaldırma (dead reckoning + event-driven, **event'ler reliable**, #58) | Hafta 11-12 | %20-30 ek tasarruf (#29) |
| 8 | Snapshot interpolation (adaptive, loss-run-length, **ratio hysteresis**, Slerp+Nlerp InterpFn, #69/#71) | Hafta 13-14 | Yumuşak görsel (#33) |
| 9 | Input redundancy (sliding-window, **cap ~20-30 + RLE**, #67) | Hafta 15-16 | Paket kaybı toleransı (#32) |
| 10 | Prediction + **lightyear rollback config** (PredictedHistory, no manual snapshot, #65) | Hafta 17-18 | %67 prediction CPU (#34) |
| 11 | Velocity-based smooth correction (**0.05m floor**, velocity snap, #66) | Hafta 19-20 | Momentum koruma |
| 12 | Lag compensation (**custom HitboxHistory rewind**, formül düzeltme, anti-cheat, #72/#73/#75) | Hafta 21-22 | Adil hit detection (#sertleştirme) |
| 13 | Optimistic block placement (**ayrı edit layer + sequence/ack**, #43/#68) + incremental mesh regen | Hafta 23-24 | Anında blok geri bildirim |
| 14 | zstd dictionary (build-time, #63) + lz4 datagram codec + **BrickDeltaBatch/BulkEditBurst** (#49/#51) | Hafta 25+ | +20-50% chunk (#30) |
| 15 | SVDAG occlusion relevance **soft** çarpanı (#48) | İleri | ~2.5× ort. AOI (#23) |
| 16 | Fixed-point math (determinism, gerekirse) | İleri | Cross-platform |
| 17 | Zone/shard mimarisi + **600p soak testi** (#40) | İleriki fazlar | Ölçeklenme |

---

## 15. Kaynaklar

### Temel Kütüphaneler
- [lightyear (birincil stack)](https://github.com/cBournhonesque/lightyear) — Bevy 0.18, prediction+rollback+interpolation+lag-comp, UDP/WebTransport
- [lightyear book](https://cbournhonesque.github.io/lightyear/book/)
- [bevy_replicon docs](https://docs.rs/bevy_replicon) — (artık birincil değil; referans)
- [Quinn TransportConfig](https://docs.rs/quinn/latest/quinn/struct.TransportConfig.html) — (QUIC, birincil değil)
- [RFC 9221 - QUIC Datagrams](https://www.rfc-editor.org/rfc/rfc9221.html)
- [plans/network-research/quic-audit-2026.md](plans/network-research/quic-audit-2026.md) — QUIC değerlendirmesi
- [plans/network-research/network-audit-2026-07.md](plans/network-research/network-audit-2026-07.md) — prediction/lag-comp denetimi
- [plans/network-research/16-consolidated-audit-tr.md](plans/network-research/16-consolidated-audit-tr.md) — konsolide Türkçe rapor
- [researchs/16-audit-report-tr.md](researchs/16-audit-report-tr.md) — **0.3 Kapsamlı Teknik Denetim** (9-alt-ajan, 2026-07)
- [lightyear 0.27.0 release (Replicon backend)](https://github.com/cBournhonesque/lightyear/releases/tag/0.27.0) — #41
- [lightyear ↔ Replicon issue #1350](https://github.com/cBournhonesque/lightyear/issues/1350) — shared buffer, priority sınırları
- [OWASP Pinning Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Pinning_Cheat_Sheet.html) — #38 key pinning
- [OneUptime UDP buffer & CC guide](https://oneuptime.com/blog/post/2026-03-20-optimize-udp-buffer-sizes/view) — #36 socket buffer
- [LWN SO_REUSEPORT](https://lwn.net/Articles/853637) — #37 reconnect fırtınası
- [Discord zstd dictionary](https://discord.com/blog/how-discord-reduced-websocket-traffic-by-40-percent) — #63
- [Valve Lag Compensation (Source SDK)](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/server/player_lagcompensation.cpp) — #72/#73/#75
- [UCSD CSE125 LagCompensation 2026](https://cse125.ucsd.edu/2026/cse125g2/docs/LagCompensation_8hpp.html) — full-RTT vs half-RTT
- [Glenn Fiedler Delta Compression (greedy tuner)](https://gist.github.com/gafferongames/bb7e593ba1b05da35ab6) — #56

### Delta Encoding & Quantization
- [Snapshot Compression - Gaffer On Games](https://gafferongames.com/post/snapshot_compression)
- [Quaternion Quantization - Marc B. Reynolds](http://marc-b-reynolds.github.io/quaternions/2017/05/02/QuatQuantPart1.html)
- [Octahedral Compression - Riot Games](https://technology.riotgames.com/news/compressing-skeletal-animation-data)
- [FVector_NetQuantize - Unreal Engine](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/Engine/FVector_NetQuantize)
- [Quake 3 Network Model - Fabien Sanglard](https://fabiensanglard.net/quake3/network.php)
- [XOR Delta Compression Trick - DemoFox](https://blog.demofox.org/2018/06/04/a-neat-trick-for-compressing-networked-state-data)
- [Dead Reckoning for Networked Games](https://uwspace.uwaterloo.ca/bitstreams/7a6acc97-26fe-450d-9b27-8b5c88998224/download)

### Prediction & Reconciliation (lightyear birincil)
- [lightyear prediction/rollback/lag-comp docs](https://cbournhonesque.github.io/lightyear/book/)
- [Gabriel Gambetta - Client-Side Prediction](https://www.gabrielgambetta.com/client-side-prediction-server-reconciliation.html)
- [Valve Developer Wiki - Prediction](https://developer.valvesoftware.com/wiki/Prediction)
- [Overwatch Gameplay Architecture - GDC (Tim Ford)](https://www.gdcvault.com/play/1024001/-Overwatch-Gameplay-Architecture-and)
- ~~GGRS~~ (❌ P2P lockstep, uyumsuz, #34)
- ~~bevy_replicon_snap~~ (❌ PoC, Bevy 0.15 takılı, #34)
- [bevy_timewarp GitHub](https://github.com/RJ/bevy_timewarp) — (opsiyonel hafif alternatif)
- [Gaffer on Games - Floating Point Determinism](https://gafferongames.com/post/floating_point_determinism)
- [Unity ECS Galaxy Sample - Determinism](https://github.com/Unity-Technologies/ECSGalaxySample/blob/main/_Documentation/determinism.md)

### Interpolation
- [Snapshot Interpolation - Gaffer On Games](https://gafferongames.com/post/snapshot_interpolation)
- [Hermite Splines in Networked Games](https://www.generalreasoning.com/blog/2025/08/23/hermite-splines.html)
- [Unity Netcode - Interpolation](https://docs.unity3d.com/Packages/com.unity.netcode@1.6/manual/interpolation.html)
- [Quaternion Interpolation - Gabor Makes Games](https://gabormakesgames.com/blog_quats_interpolate.html)
- [Valve Developer Wiki - Interpolation](https://developer.valvesoftware.com/wiki/Interpolation)

### Voxel-Specific
- [Veloren Chunk Compression](https://veloren.net/blog/devblog-117)
- [Minecraft Chunk Format](https://minecraft.wiki/w/Java_Edition_protocol/Chunk_format)
- [Palette Compression - Voxel.Wiki](https://voxel.wiki/wiki/palette-compression)

### Genel
- [Unreal Replication Graph](https://www.unrealengine.com/tech-blog/replication-graph-overview-and-proper-replication-methods) — (AOI pattern referansı)
- [Valve Lag Compensation](https://developer.valvesoftware.com/wiki/Lag_Compensation)
- [Cloudflare GSO Optimization](https://blog.cloudflare.com/accelerating-udp-packet-transmission-for-quic) — (QUIC, birincil değil)
- [BBR Congestion Control](https://www.ietf.org/archive/id/draft-cardwell-iccrg-bbr-congestion-control-01.html)
- [Source Engine Networking](https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking)
- [Valorant 128-Tick Servers](https://www.riotgames.com/en/news/valorants-128-tick-servers)
- [Rust Serialization Benchmarks](https://github.com/djkoloski/rust_serialization_benchmark)
