# QUIC / Quinn Transport Audit for Strata (2026)

**Scope:** Validate the transport claims in `plans/16-network-and-lag-compensation.md` §1, assess QUIC
vs alternatives for a server-authoritative 600-player Bevy/Rust voxel game, and produce actionable
recommendations. All claims are checked against current (2026) primary sources: Quinn/quinn-proto
source & docs, RFC 9000/9001/9221, the `game_sockets` and `TransportProxy` Rust benchmarks, Cloudflare
GSO research, and recent (2024-2026) congestion-control literature.

Sources are linked inline; a consolidated reference list is at the end.

---

## 1. Executive Summary

- **QUIC via Quinn is a *reasonable* choice for Strata**, but it is **not uniquely optimal**. Its real
  wins for a 600-player server are: mandatory TLS 1.3, a *single UDP socket* for all connections,
  connection migration via Connection ID, and unreliable DATAGRAM frames (RFC 9221) for game state.
  These are genuine and match the plan's claims.
- **The biggest plan weakness is framing QUIC as "free"**. QUIC's per-packet AEAD encryption has a real,
  measurable CPU cost (≈2–3.5× bytes/cycle vs TCP per Cloudflare/Google; see §3). At 600 players × 20–30 Hz
  plus input, this is the dominant cost and must be budgeted — not assumed away.
- **The plan already self-corrected the two most important errors** (BBR→Cubic, 0-RTT idempotent-only).
  Current research fully supports those corrections (§4, §5).
- **Per-stream HOL avoidance helps the *reliable* tier, not the bulk unreliable game state** (which uses
  datagrams and has no HOL anyway). RUDP libraries (GameNetworkingSockets, ENet, KCP) already solve HOL
  at the application layer, so QUIC is not unique here (§2).
- **QUIC datagrams are size-limited (~1150 B payload at 1200 B MTU, no fragmentation)**. The plan's
  `ChunkData` = datagram is fine *only* for small incremental deltas; full sector snapshots must be
  app-fragmented or sent on a reliable stream (§6).
- **Maintenance status (2026) is healthy**: bevy_quinnet 0.20.0 (Jan 2026, Bevy 0.18), bevy_replicon_quinnet
  0.19.0 (May 2026, bevy_replicon 0.40), quinn-proto 0.11.14 (Mar 2026). Single-maintainer bus factor is
  the main risk (§7).
- **Recommendation:** Keep QUIC/Quinn as the native transport, but (a) pin to Cubic, (b) raise socket +
  flow-control buffers, (c) cap datagrams to <1.1 KB or fragment per-sector loads, (d) reserve 0-RTT for
  idempotent requests only, (e) keep a WebTransport client path optional for WASM, and (f) benchmark
  against Valve's GameNetworkingSockets (Rust FFI) before committing, since GNS is purpose-built for
  exactly this traffic shape.

---

## 2. (a) Validation of the Plan's Transport Claims

| # | Plan claim (§1) | Verdict | Evidence / Notes |
|---|---|---|---|
| 1 | TLS 1.3 mandatory in QUIC | **CORRECT** | RFC 9001: QUIC authenticates/encrypts every packet with TLS 1.3 keys. Quinn uses rustls. |
| 2 | 1-RTT handshake; 0-RTT reconnect | **CORRECT** | RFC 9000/9001. 0-RTT replayable (see §5). |
| 3 | No HOL blocking (per-stream) | **PARTIAL / OVERSTATED** | True *within* a connection across streams (RFC 9000). But the bulk of game state goes over *datagrams*, which have no ordering/HOL at all. The HOL benefit only matters for the reliable tier, which is low-frequency. RUDP libs solve this at app layer too. |
| 4 | Built-in multiplexing "256 channels" | **MISLEADING** | QUIC stream limit is negotiated per connection (up to 2⁶²); "256" is bevy_quinnet's channel abstraction, not a QUIC constant. Multiplexing is real but not "256 hardwired". |
| 5 | Connection migration via Connection ID | **CORRECT** | RFC 9000 Connection ID. Useful for mobile/Wi-Fi↔cellular; less critical for desktop server-authoritative. |
| 6 | Single UDP socket for 600+ connections | **CORRECT** | Quinn README: "A Quinn endpoint corresponds to a single UDP socket, no matter how many connections." Confirmed. |
| 7 | BBR congestion control recommended | **INCORRECT (already self-corrected)** | Quinn's `BbrConfig` is experimental; default is `CubicConfig`. Plan audit note §1.5 already reverses this to Cubic. See §4. |
| 8 | Unreliable channels = QUIC Datagram (RFC 9221) | **CORRECT** | RFC 9221 DATAGRAM frames are the right tool for GameInput/GameState. Caveat: size limit (§6). |
| 9 | Reliable channels = ordered/reliable QUIC stream | **CORRECT** | Streams are reliable+ordered; UnorderedReliable via one-shot streams per message (Daposto/Quinn guidance). |
| 10 | GSO 97% fewer syscalls; Cloudflare 74% throughput | **PARTIALLY CORRECT** | GSO reduces stack traversals ~45× (1 sendmsg for 64 KB → kernel splits to MTU) → plausibly ~97% fewer *syscalls* on the sender. Cloudflare/LPC paper shows UDP GSO ≈1.7× throughput, hardware LSO +3×. The "74%" figure is not in the cited Cloudflare blog verbatim; treat as approximate. Linux-only; drivers matter (see §5b, issue #2575). |
| 11 | 0-RTT session resumption for instant reconnect | **CORRECT but DANGEROUS if misused** | 0-RTT is replayable (RFC 9001 §9.2). Plan audit note correctly restricts it to idempotent data. See §5. |
| 12 | `enable_segmentation_offload: true` in TransportConfig | **CORRECT (field exists)** | Confirmed in quinn-proto `TransportConfig` (defaults `true`). Minor: it is a Quinn field, not a bevy_quinnet-specific one; verify bevy_quinnet exposes it. |
| 13 | `BbrConfig::default()` recommended | **INCORRECT** | Use `CubicConfig` (Quinn default). `BbrConfig` is labeled experimental in Quinn. |

### Quinn configuration specifics confirmed from source (2026)
- `TransportConfig` default congestion controller = `CubicConfig` (quinn-proto `transport.rs`). ✔ Supports plan's corrected Cubic choice.
- `enable_segmentation_offload` exists, defaults `true`.
- `datagram_send_buffer_size` default = 1 MB; `datagram_receive_buffer_size` default = `STREAM_RWND`. The plan sets both to 64 KB — *smaller* than Quinn's default send buffer; consider raising toward 1 MB.
- `initial_rtt` default = 333 ms (spec-intentional). For games, lower it (the `game_sockets` benchmark sets ~15 ms) so loss detection/retransmission starts faster.

---

## 3. Is QUIC Actually the Best Choice for a 600-Player Server-Authoritative Voxel Game?

**Short answer: It is a strong, safe default, but not provably optimal.** The decision hinges on what
you value:

- **If you want encryption + auth + migration + single-socket + datagrams without writing a custom
  RUDP stack**, QUIC wins decisively on developer effort and security correctness.
- **If you want the absolute lowest latency and CPU at 600 players**, a purpose-built RUDP (Valve
  GameNetworkingSockets) is purpose-built for exactly this shape and avoids per-packet userspace crypto
  overhead you don't strictly need (you can bring your own lighter crypto).

Empirical signal — the `game_sockets` Rust benchmark (2026) explicitly compares **UDP / TCP / QUIC
(Quinn) / GNS (Valve)** under 10 realistic network scenarios with a game-like traffic mix (60 Hz
unreliable movement + 20 Hz reliable state). Their QUIC config disables pacing, uses BBR, and sets a
15 ms initial RTT — i.e., they tried hard to make QUIC game-friendly. The benchmark exists precisely
because the author found the question open. Key takeaway: **even after tuning, QUIC did not beat raw
UDP on latency**; it sat between UDP and TCP, with GNS (Valve) competitive. No single protocol dominated
across all 10 scenarios — confirming there is no free lunch.

CPU reality check (Cloudflare LPC 2018 / Google): serving over QUIC costs up to **3.5× CPU cycles/byte
vs TCP**, ~2× after app-layer fixes, because encryption is per-packet in userspace. For 600 players
this is the cost center. Quinn's crypto is single-threaded on the endpoint driver; mitigation = GSO/GRO,
larger socket buffers, and **multiple endpoints via `SO_REUSEPORT`** (one per core), which the plan
already lists (§0, Layer 0).

**Verdict:** QUIC is justified for Strata *if* the team values security/connection-migration/single-socket
and accepts the CPU tax; it should be load-tested at 600 simulated players before launch. A fallback
evaluation against GNS is cheap insurance (§7).

---

## 4. Alternatives — Comparative Analysis

| Option | Pros | Cons | Fit for Strata |
|---|---|---|---|
| **QUIC / Quinn (plan)** | TLS 1.3 mandatory; single UDP socket; Connection ID migration; RFC 9221 datagrams; mature Rust impl; bevy_quinnet + bevy_replicon_quinnet integration exists | Per-packet crypto CPU cost; Quinn's Cubic is naive (no HyStart++); socket-buffer defaults too small (#2262); single-maintainer bus factor | **Primary** — keep, with tuning |
| **Raw UDP + custom RUDP** | Lowest latency/CPU; total control; no crypto overhead you don't need | You must implement reliability, fragmentation, ordered/unordered, encryption, auth, congestion control, NAT — high effort, easy to get wrong (security!) | Not recommended to hand-roll; only via a library |
| **Valve GameNetworkingSockets (GNS)** via `game-networking-sockets-sys` (Rust FFI) | Purpose-built for games; reliable **and** unreliable *messages* (not streams — better fit); "lanes" with priority/weight for HOL control + bandwidth sharing; AES-GCM encryption modeled on QUIC's design; connection-quality stats; used in CS2/Dota2; single socket for many clients | C++/FFI (build complexity, ABI); P2P/ICE focus; global lock limits multithreaded API calls; not Bevy-native (needs a bridge) | **Strong alternative / benchmark target** |
| **ENet (C)** | Tiny, simple, channels (reliable/unreliable), proven in many shipped games | No encryption; no built-in auth; stream-of-packets model; single-threaded-ish; manual FFI | Viable base if you add crypto yourself; weaker than GNS |
| **KCP (C, `kcprb`/`kcp-deepseek` Rust)** | Very fast retransmit, optional FEC, low latency, small | Unreliable-only not first-class; no encryption/auth; needs wrapping | Good for the unreliable bulk path; pair with a reliable channel |
| **yojimbo / Netcode.io (Glenn Fiedler)** | Message-oriented reliable+unreliable, encryption+signed packets, connection-oriented, auth — "WebSockets for UDP" | C++; no official Rust binding (FFI only); more P2P-oriented | Conceptually ideal; FFI cost |
| **WebTransport (over QUIC, browser)** | Only browser/WASM API exposing UDP-like datagrams + streams; now **Web Platform Baseline (Safari 26.4, Mar 2026)**; TLS 1.3 mandatory; 0-RTT; connection migration | Browser-only; not for native desktop server; server ecosystem younger | **Optional client path** for WASM; not the server transport |
| **TCP (+TLS)** | Easy, reliable, ordered | HOL blocking makes it unusable for real-time state (per Gaffer/Valve/industry) | Control/auth side-channel only, never game state |

**Bottom line:** For a *native* Bevy/Rust server, QUIC/Quinn and GNS are the two credible front-runners.
QUIC gives you more "for free" (security, migration, datagrams) at higher CPU; GNS gives you a
game-shaped message API with less crypto overhead but FFI/build cost. WebTransport is only relevant if
you later target WASM/browser clients.

---

## 5. (b) Deep-Dive on the Four Contested Claims

### 5.1 Per-stream HOL avoidance — does it help game traffic?
- QUIC eliminates *transport* HOL across streams (RFC 9000). Within a single stream, ordering/HOL still
  applies (datagrams avoid it entirely).
- For Strata, the high-frequency game state (`GameInput`, `GameState`, `ChunkData`) is mapped to
  **datagrams** → no HOL at all. The reliable tier (`ReliableEvents`, `BlockChanges`, `Auth`) is
  low-frequency, so HOL *within* the reliable tier is a minor risk; mapping each critical event type to
  its own stream (as `aetheris-protocol` does per ComponentKind) prevents one retransmit from blocking
  another. This is good practice but not a game-changer at Strata's traffic shape.
- RUDP libraries already provide equivalent isolation (GNS "lanes", ENet channels). **So QUIC's HOL
  story is a nice-to-have, not a differentiator** for this workload.

### 5.2 BBR vs Cubic for game traffic (small packets, low BDP, RTT-sensitive)
The plan's audit note already flips BBR→Cubic. Research strongly supports that:

- **Quinn's `BbrConfig` is explicitly experimental**; default is `CubicConfig`. (Confirmed in source.)
- BBR is **model-based and tuned for bulk transfer / throughput**. Game traffic is the opposite: small
  packets, low BDP, extremely RTT-sensitive. BBR's bandwidth-probing phases *induce queuing delay* —
  recent BBRv3 studies (IFIP Networking 2025; MDPI Sensors 2025) show BBRv3 can create queuing delay
  on the order of **~1 RTT even for a single flow**, and is **RTT-unfair** in shallow buffers, dominating
  CUBIC on contended/shared links.
- The "Google reported 33% RTT reduction" figure comes from **bulk-transfer/datacenter** measurements,
  not 20–30 Hz game flows. It does not transfer.
- `game_sockets` uses BBR for QUIC **but also disables pacing** — which removes BBR's main latency
  smoothing benefit, partly defeating the point.
- **Recommendation:** stay on **Cubic** (Quinn default). If you ever ship a bulk-transfer mode (e.g.,
  initial world download), gate BBR behind a feature flag and A/B test. Don't enable BBR for the
  real-time path.

### 5.3 GSO/GRO — valid?
- **Concept valid, Linux-only.** UDP GSO lets one `sendmsg` carry up to 64 KB; the kernel splits into
  MTU-sized segments → far fewer syscalls and stack traversals. LPC 2018 paper (Willem de Bruijn,
  Google): UDP GSO ≈ **1.7× throughput** vs unsegmented UDP; with hardware LSO another ~3×.
- The "97% fewer syscalls" claim is plausible *for the sender*: 1 syscall vs ~53 (64 KB / 1200 B) ≈ 98%
  reduction. The "74% throughput" figure is not verbatim in the cited Cloudflare blog; treat as
  approximate/attributable to later Cloudflare/Tailscale write-ups.
- **Caveats for Strata:**
  - Quinn's GSO can **break on NIC drivers that don't support `UDP_SEGMENT`** — Quinn issue #2575
    documents a regression where GSO auto-deactivation failed and the client got stuck. Test on your
    target NIC; keep `enable_segmentation_offload` but verify behavior, and have a fallback.
  - GSO *batching* can add latency if you wait for a full MSS; for latency-sensitive small packets,
    tune batching (don't delay sends to fill a segment).
  - Benefit is real at 600 players (many small packets → syscall amortization), but it is a *throughput/
    CPU* optimization, not a *latency* one.

### 5.4 0-RTT — valid? Dangerous?
- **Mechanism valid** (RFC 9000/9001). Enables instant reconnect for returning clients.
- **Dangerous by design:** RFC 9001 §9.2 states 0-RTT data *"can be replayed by an attacker, so 0-RTT
  is not suitable for carrying instructions that might initiate any action that could cause unwanted
  effects if replayed."* Additionally the server **cannot authenticate the client** and the client has
  **not demonstrated liveness** in 0-RTT.
- Concretely: sending authoritative input / block-place / ability-use in 0-RTT lets an attacker capture
  and replay packets → duplicate block placement, item duplication, ability spam. This is an **anti-cheat
  hole**, exactly what the plan's §1.4/§1.7 audit notes warn about.
- Best practice (mirrors HTTP's `425 Too Early`): only send **idempotent, state-free requests** (e.g.,
  "list servers", "request session token") in 0-RTT; keep `max_early_data_size` small; process 0-RTT
  data defensively. The plan's correction is correct — keep it.
- Note: TLS session tickets expire after **7 days** (RFC 9001); 0-RTT only helps *returning* clients
  within that window.

---

## 6. Channel Design Review (datagrams for unreliable state @ 20–30 Hz)

- **Mapping is sound:** `GameInput`/`GameState`/`ChunkData` → QUIC DATAGRAM (RFC 9221); reliable events
  → streams. This matches Daposto's QUIC-for-games guidance and the `aetheris-protocol` / `game_sockets`
  designs.
- **Hard size constraint:** RFC 9221 DATAGRAM frames are **not fragmented**. Max datagram size is bounded
  by `max_datagram_frame_size` (recommend 65535) **and** the path MTU. QUIC requires a minimum MTU of
  1200 B; with QUIC/AEAD overhead (~16 B tag + headers), the **safe application payload per datagram is
  ~1140–1150 B** (aetheris-protocol uses 1140 B). Plan's `initial_mtu: 1200` is correct.
- **Implication for `ChunkData`:** a compressed sector/initial chunk load (zstd, plan §6.2) routinely
  exceeds 1150 B. Sending it as a single datagram will be dropped/fail. Therefore:
  - Keep *incremental* `BrickDelta` (plan §4.2, ~15 B each) on datagrams — perfect fit.
  - For *full* sector loads, either (a) fragment at the application layer into <1.1 KB datagrams with
    reassembly, or (b) send over a **reliable stream** (ordered/unordered). The plan's §6.3 BlockChange
    batch (~800 B) fits under the limit — OK, but add a hard `<= 1100 B` guard.
- **Flow control:** datagrams are *not* flow-controlled and can be dropped if the receiver is overloaded;
  Quinn's `datagram_receive_buffer_size` caps this. Raise it (plan sets 64 KB, Quinn default 1 MB) and
  monitor drops.
- **Congestion:** datagrams share the connection's congestion controller with streams — so a flood of
  datagrams still respects Cubic/BBR. Good.
- **Per-component streams:** if you adopt per-ComponentKind streams (aetheris style), cap
  `max_concurrent_bidi/unidi_streams` reasonably; the plan's `2`/`1` is very low for a multi-event game —
  consider a small pool (e.g., a handful per event class) to avoid stream-exhaustion stalls.

---

## 7. bevy_quinnet / Quinn — 2026 Status & Known Issues

**Versions (verified on crates.io / GitHub, 2026):**
- `quinn` / `quinn-proto` 0.11.x — quinn-proto 0.11.14 (Mar 2026). Active.
- `bevy_quinnet` 0.20.0 (Jan 2026) — Bevy 0.18, quinn 0.11.5. Active, single maintainer (Henauxg).
- `bevy_replicon_quinnet` 0.19.0 (May 2026) — bevy 0.18, bevy_replicon 0.40, bevy_quinnet 0.20. Active.
- `bevy_replicon` 0.40.4 (Jun 2026). Active.
- Plan's §2.2 pin (`bevy_replicon_quinnet 0.19`, `bevy_quinnet 0.20`) is **correct for 2026**.

**Known issues / risks to plan for:**
1. **Socket-buffer starvation (#2262, 2025):** Quinn's default `rmem_max` handling caused packet loss at
   the OS socket level under load (Quinn 335 Mbps vs msquic 927 Mbps at default buffers; 897 Mbps after
   raising `rmem_max`). **Action:** raise `net.core.rmem_max` / set explicit `SO_RCVBUF`/`SO_SNDBUF` on
   the Quinn UDP socket, or accept `CAP_NET_ADMIN` for `SO_RCVBUFFORCE`. The plan's "4 MB send/recv
   buffer" (Layer 0) should be applied to the *socket*, not just the transport windows.
2. **Accept-path lock contention (#2633, 2026):** high new-connection rates (e.g., reconnect storms)
   stall the endpoint driver. Mitigated by recent lock-split PR. **Action:** for 600 players, spread
   reconnect load; consider multiple endpoints (`SO_REUSEPORT`) per core.
3. **GSO auto-deactivation regression (#2575):** can hang on drivers lacking `UDP_SEGMENT`. Test on
   deploy hardware; keep a disable path.
4. **Cubic is naive** (no HyStart++), per Quinn maintainers in #2262 — acceptable for games; just don't
   expect BBR-level bulk throughput.
5. **Bus factor:** bevy_quinnet is essentially one maintainer; quinn-proto is healthier but small. Plan
   already lists "quinn-proto fork possible" as a risk — reasonable.
6. **No WASM/browser:** bevy_quinnet does not support browser targets. If WASM is ever wanted, use
   WebTransport (now Baseline 2026) — keep as a documented optional client path.

---

## 8. (c) Key Insights & Lessons Learned

1. **QUIC's value for games is security + ergonomics, not latency.** Raw UDP is still lowest-latency;
   QUIC buys you encryption/auth/migration/single-socket without a custom stack. Pay for it in CPU.
2. **The "free lunch" myth:** per-packet userspace AEAD is the real cost. Budget ~2–3.5× TCP's
   cycles/byte. At 600 players this is the scaling limit, not bandwidth (plan's 10 MB/s is trivial).
3. **HOL avoidance is mostly moot for datagram-based game state**; it matters only for the reliable tier.
4. **BBR is a bulk-transfer tool; Cubic is the right default for games.** The plan's self-correction is
   correct and should be locked in.
5. **0-RTT is a replay vector**, not a feature to ship authoritative commands on. Treat like HTTP 425.
6. **Datagrams are size-capped and unfragmented** — design chunk transfer around <1.1 KB units.
7. **Benchmarks are mixed:** `game_sockets` shows no protocol dominates all scenarios; GNS is a credible
   peer to Quinn for native games. `TransportProxy` (localhost) even shows Quinn *faster* than KCP for
   round-trip/throughput — but localhost hides real crypto/network cost.
8. **Buffer tuning beats algorithm choice** for Quinn throughput (#2262): correct socket buffers first.

---

## 9. (d) Actionable Recommendations for the Strata Plan

**P0 (correctness / security — do before any 600-player test):**
1. **Lock Cubic** as the congestion controller; remove `BbrConfig` from the recommended config (keep
   behind a feature flag only for bulk-transfer profiling). (Aligns with §1.5 audit note.)
2. **Hard-cap datagrams at ≤1100 B** and add application-level fragmentation/reassembly for full sector
   loads; send large initial chunks over a reliable stream, not as a single datagram. (Fixes §1.2
   `ChunkData` risk.)
3. **Restrict 0-RTT to idempotent requests only**; never send `GameInput`/block-place/abilities in
   0-RTT; keep `max_early_data_size` small. (Aligns with §1.7 audit note.)
4. **Replace TOFU with build-time cert pinning + post-handshake signed session token** for auth
   (anti-cheat). (Aligns with §1.4 audit note.)

**P1 (performance / scale — before launch):**
5. **Raise OS socket buffers** (`SO_SNDBUF`/`SO_RCVBUF` ≥ 4 MB, or `SO_RCVBUFFORCE` with
   `CAP_NET_ADMIN`) and Quinn `datagram_send/receive_buffer_size` (toward 1 MB). (Mitigates #2262.)
6. **Set `initial_rtt` to ~15 ms** (not Quinn's 333 ms default) so loss/retransmission triggers fast.
7. **Run multiple Quinn endpoints via `SO_REUSEPORT`**, one per core, to spread crypto + accept load
   for 600 players and reconnect storms. (Mitigates #2633.)
8. **Verify GSO on target NICs**; keep `enable_segmentation_offload: true` but have a disable fallback;
   don't let GSO batching add send latency (tune batch size).
9. **Raise `max_concurrent_bidi/unidi_streams`** above 2/1 if you use per-event-class streams; otherwise
   a stream-exhaustion stall can block reliable delivery.

**P2 (evaluation / future-proofing):**
10. **Benchmark against Valve GameNetworkingSockets** (`game-networking-sockets-sys`) at 600 simulated
    players before final commit — it is purpose-built for this exact traffic and avoids QUIC's crypto
    CPU tax. Cheap insurance.
11. **Keep an optional WebTransport client path** documented for any future WASM/browser target (now
    Web Platform Baseline as of Mar 2026, Safari 26.4). Not the server transport.
12. **Add a load-test harness** (modeled on `game_sockets`) measuring p95/p99 RTT and CPU%/player at
    600 players *before* relying on the 10 MB/s bandwidth estimate, which is optimistic about CPU.

---

## 10. Reference List

- Quinn (Rust QUIC): https://github.com/quinn-rs/quinn — quinn-proto 0.11.14 (Mar 2026)
- Quinn `TransportConfig` source (Cubic default, `enable_segmentation_offload`): https://docs.rs/quinn-proto/latest/src/quinn_proto/config/transport.rs.html
- RFC 9000 (QUIC transport): https://datatracker.ietf.org/doc/html/rfc9000
- RFC 9001 (QUIC TLS / 0-RTT replay, §9.2): https://www.rfc-editor.org/rfc/rfc9001.html
- RFC 9221 (QUIC Datagrams): https://www.rfc-editor.org/rfc/rfc9221.html
- bevy_quinnet 0.20.0 (Bevy 0.18): https://crates.io/crates/bevy_quinnet
- bevy_replicon_quinnet 0.19.0 (bevy_replicon 0.40): https://crates.io/crates/bevy_replicon_quinnet
- bevy_replicon 0.40.4: https://crates.io/crates/bevy_replicon
- `game_sockets` Rust benchmark (UDP/TCP/QUIC/GNS, 2026): https://github.com/VALERE91/game_sockets
- `TransportProxy` Rust KCP-vs-QUIC benchmark (2026): https://github.com/AliRezaBeigy/TransportProxy
- Cloudflare — Accelerating UDP packet transmission for QUIC (GSO): https://blog.cloudflare.com/accelerating-udp-packet-transmission-for-quic/
- LPC 2018 paper — Optimizing UDP for content delivery: GSO, pacing, zerocopy (UDP GSO ≈1.7×): https://oldvger.kernel.org/lpc_net2018_talks/willemdebruijn-lpc2018-udpgso-paper-DRAFT-1.pdf
- Tailscale — QUIC/UDP throughput GSO: https://tailscale.com/blog/quic-udp-throughput
- Quinn issue #2262 (socket-buffer / CPU perf on internet): https://github.com/quinn-rs/quinn/issues/2262
- Quinn PR #2633 (accept lock-split for high conn rate): https://github.com/quinn-rs/quinn/pull/2633
- Quinn issue #2575 (GSO auto-deactivation regression): https://github.com/quinn-rs/quinn/issues/2575
- Daposto — QUIC as a Game Networking Protocol: https://daposto.medium.com/quic-for-gamenetworking-46cf23936228
- aetheris-protocol NETWORKING_DESIGN (QUIC datagrams + per-ComponentKind streams): https://github.com/garnizeh-labs/aetheris-protocol/blob/main/docs/NETWORKING_DESIGN.md
- Valve GameNetworkingSockets (GNS, Rust FFI `game-networking-sockets-sys`): https://github.com/ValveSoftware/GameNetworkingSockets
- BBRv3 performance & RTT unfairness (IFIP Networking 2025): https://networking.ifip.org/2025/images/Net25_papers/1571125683.pdf
- BBR vs Cubic fairness (MDPI Sensors 2025): https://www.mdpi.com/1424-8220/25/17/5374
- BBRv3 wired broadband eval: https://research.cec.sc.edu/files/cyberinfra/files/bbrv3_rev2.pdf
- WebTransport — now Web Platform Baseline (Safari 26.4, Mar 2026): https://calmops.com/network/webtransport-protocol/ ; MDN: https://developer.mozilla.org/en-US/docs/Web/API/WebTransport_API
- Gaffer On Games — Why can't I send UDP from a browser: https://gafferongames.com/post/why_cant_i_send_udp_packets_from_a_browser/
- IPTP 2026 — UDP vs TCP vs QUIC for games: https://www.iptp.net/blog/ultra-low-latency-for-gaming/
