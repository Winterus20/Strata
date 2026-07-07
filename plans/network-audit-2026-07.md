# Technical Audit — Strata Network Plan: Prediction, Reconciliation & Lag Compensation

**Scope:** Validation of claims in `plans/16-network-and-lag-compensation.md` §7, §8, §9, §10, §11.
**Method:** Deep web research (Gabriel Gambetta, Valve Source SDK, Overwatch GDC 2017/Tim Ford, Quake 3, lightyear/GGRS/bevy_replicon ecosystem) + comparative analysis.
**Date:** 2026-07-07

---

## (a) Validation of Plan Claims

### A1. Partial rollback — divergence-point re-simulation (§7.1)
**Claim:** Snapshot every 4 frames; re-simulate only 2–5 frames from divergence instead of full 10–15 frame replay (~67% reduction).

**Verdict: Technique is sound; the stated metric is weakly justified.**

- The *standard* industry approach already only replays **unacknowledged** inputs, not the full pending window. Valve's `CPrediction::ComputeFirstCommandToExecute` replays from `incoming_acknowledged + 1` to `outgoing_command` — i.e. only commands the server hasn't confirmed yet (bounded by RTT + buffering, typically 6–12 at 60 Hz) [Valve `prediction.cpp`](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/client/prediction.cpp). Overwatch replays "every buffered input forward to the present moment" on misprediction, using a fixed 16 ms command frame (60 Hz) and normally buffering only 1–4 commands [Edgegap Overwatch deep-dive](https://edgegap.com/blog/game-backend-deep-dive-overwatch-2016-netcode-architecture-rollback).
- The 4-frame checkpoint does **not** reduce the *normal* replay cost (already small); it bounds the **worst-case** replay after a stall/large desync. So the "10–15 frames → 2–5 frames, 67%" framing misrepresents the baseline — a correct implementation already replays ~the unacked window, not 10–15 full frames.
- **Checkpoint snapshots are a legitimate optimization** (lightyear stores `PredictionHistory` per predicted entity and rolls back to the earliest mismatch tick — effectively the same idea, [`lightyear::prelude::Predicted`](https://docs.rs/lightyear/latest/lightyear/prediction/struct.Predicted.html)). Saving full `PlayerState` copies every 4 frames is cheap for one player but must be bounded; for Strata the predicted state is heavier than Overwatch's (velocity, physics, possibly inventory + in-flight block edits), so snapshot size matters.
- **Recommendation:** Tie checkpoint cadence to the max unacked window (e.g. snapshot every `K` frames where `K ≈ max acceptable re-sim`) rather than a fixed 4. Keep the ring small and bounded. Do not count "67% CPU reduction" as a primary benefit — count it as *worst-case bounding*.

### A2. Input redundancy — send last 3 inputs (§7.2)
**Claim:** Quake 3 approach; 3× repeat; 20 B × 3 × 30 Hz = 1.8 KB/s/player.

**Verdict: Arithmetic correct; approach acceptable but superseded by sliding-window redundancy.**

- Quake 3's actual model: the client sends a command **every client frame** and the server delta-compresses against the last *acknowledged* state; reliable data is resent until acked [Quake 3 Network Model — Sanglard](https://fabiensanglard.net/quake3/network.php). It is not a fixed "last 3" repeat; it is a **command stream** with implicit ack.
- Overwatch's modern form is a **sliding window**: "rather than sending only the current frame's input, the client bundles every input since the last server-acknowledged movement state into a single packet" [Edgegap](https://edgegap.com/blog/game-backend-deep-dive-overwatch-2016-netcode-architecture-rollback). This strictly dominates a fixed 3× repeat because held keys compress well and it recovers from any number of dropped packets.
- **Critical caveat:** Fixed "last 3" is **insufficient at high RTT + high tick**. At 200 ms RTT and 60 Hz there are ~12 unacked inputs; 3 repeats cover only 3. The plan's 30 Hz assumption (3 inputs ≈ 100 ms) leaves 100 ms of RTT uncovered.
- Bandwidth: 1.8 KB/s at 30 Hz is fine; at 60 Hz it doubles to 3.6 KB/s (~1.08 MB/s aggregated for 600 players) — still negligible vs. state. The QUIC datagram `GameInput` channel is unreliable/ordered-less, so app-level redundancy is appropriate (QUIC's own retransmission only covers reliable streams).
- **Alternatives assessed:**
  - *FEC (forward error correction):* used in VoIP (Opus); adds fixed parity overhead, helps constant loss, weak vs. burst loss, wasteful — not ideal for input.
  - *Ack-based reliable resend:* adds ≥1 RTT before the input is applied → kills prediction. Rejected for movement.
  - *Sliding-window (all unacked):* best; minimal overhead, maximal recovery.
- **Recommendation:** Replace "last 3" with **all unacked inputs** (bounded by the §10 input buffer of 64), mirroring Overwatch. Keep cost under control by sending deltas/compressed commands.

### A3. Velocity-based smooth correction + snap thresholds (§7.3)
**Claim:** <0.01 m none; 0.01–0.1 m smooth (4–8 frames); 0.1–1.0 m fast lerp; >1.0 m snap (teleport).

**Verdict: Thresholds well-aligned with industry; one structural caveat + one quantization caveat.**

- The pattern (small = invisible, medium = smooth, large = snap) matches published guidance: 4AM Games' smooth reconciliation shows constant-factor lerp for small errors and "for very large mispredictions… don't smooth — just snap" [4AM Games](https://fouramgames.com/blog/fast-paced-multiplayer-implementation-smooth-server-reconciliation); Socratopia's FPS chapter recommends exponential smoothing (5–20 /s) and snapping large errors as visible gameplay events [Socratopia Ch.27a](https://www.socratopia.app/library/math-for-game-devs-en/chapter-31). Overwatch uses adaptive interpolation delay + favors-the-shooter smoothing rather than hard position snaps, but the threshold philosophy is consistent.
- **Structural caveat:** The plan's `smooth_correction` returns `(new_pos, new_vel)` and mutates the *logical* predicted state. Industry best practice (4AM Games, lightyear) is to **snap the true/authoritative predicted position and smooth only the *render/visible* transform**, and to **ignore collision during correction** ("ignoring collision and physics in the correction process is actually desirable, or else the player would snag on colliders"). Mutating velocity during correction can make client physics fight the server.
- **Quantization caveat:** §5.3 quantizes local position to **0.01 m** (12-bit). A "<0.01 m → no correction" threshold is at the noise floor of your own quantization; reconciliation diffs will routinely be ~1 quantization step. Recommend threshold ≥ **1–2 quantization steps** (e.g. 0.02–0.05 m) to avoid jitter.
- lightyear provides exactly this via `CorrectionPolicy` + `correction_ticks` (interpolate render from current to corrected over N ticks) [`PredictionManager`](https://docs.rs/lightyear/latest/lightyear/prelude/struct.PredictionManager.html). Strong fit.

### A4. 0 ms local input delay; adaptive input delay for local player removed (§7.4)
**Claim:** Local player input must have 0 ms delay; adaptive input delay only for remote interpolation.

**Verdict: CORRECT and well-grounded.**

- Gambetta, Valve and Overwatch all apply local input **immediately**. Overwatch deliberately reduced server-side command buffering from 4 → 1 to bring the client *closer* to the present [Overwatch Developer Update / GDC](https://www.youtube.com/watch?v=vTH2ZPgYujQ). Adding local input delay defeats the purpose of prediction.
- Nuance: lightyear exposes an `input_delay` knob, but documents it as a deliberate trade-off — "If the input delay is greater than the RTT, there should be no mispredictions at all, but the game will feel more laggy" [`avian_physics` example](https://github.com/cBournhonesque/lightyear/blob/main/examples/avian_physics/README.md). Use it only as an **opt-in** for *other players' predicted entities / projectiles*, never the local player.
- **Recommendation:** Keep 0 ms for local player (matches plan). Optionally expose `input_delay` only for predicted-remote/projectile smoothing.

### A5. Lag compensation — server-side rewind (§9)
**Claim:** 200 ms limit (`sv_maxunlag`); `HISTORY_TICKS = ceil(200ms / TICK_DURATION)` ring buffer; rewind only hitbox entities then restore.

**Verdict: VALID; matches Valve Source precisely, with two hardening notes.**

- Valve `player_lagcompensation.cpp`: `sv_maxunlag` default 1.0 s, clamped to `[0, sv_maxunlag]`; `targettick = cmd->tick_count - lerpTicks`; if `|delta| > 0.2 s` fall back to latency-based time [Valve SDK](https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/server/player_lagcompensation.cpp). CS:GO/TF2 harden `sv_maxunlag` to **0.2 s** specifically to block backtrack exploits [TF2 anti-cheat issue #4664](https://github.com/ValveSoftware/Source-1-Games/issues/4664).
- **Ring-buffer size depends on tick rate, not a fixed frame count** — exactly as the plan now states (20 Hz→4, 30 Hz→6, 60 Hz→12). Valve prunes `LagRecord`s older than `sv_maxunlag` per player [same file, `FrameUpdatePostEntityThink`].
- **Rewind only relevant entities:** Valve iterates active players, skips self, bots, spectators, and anything failing `WantsLagCompensationOnEntity` — i.e. only collidable/hitbox entities. For Strata: rewind **players/mobs/projectiles only; never the voxel world** (it doesn't move). Correct.
- **Two hardening notes the plan should add:**
  1. *Include the target's interpolation/lerp delay* in the compensation (Valve adds `lerpTicks`). Without it you **under-rewind** and favor the shooter less than intended.
  2. *Teleport guard:* Valve leaves an entity in its new position if it moved farther than `sv_lagcompensation_teleport_dist` during rewind — essential for Strata teleports/ability blinks, otherwise a rewind could yank a teleported player back.
  3. *Anti-backtrack:* clamp usercmd tickcount replay (`sv_maxusrcmdprocessticks` analog) so clients cannot forge old ticks to hit further in the past [TF2 issue #4664](https://github.com/ValveSoftware/Source-1-Games/issues/4664).

### A6. Input buffering ring of 64 (§10)
**Claim:** `[InputSequence; 64]` ring buffer.

**Verdict: Reasonable.** 64 inputs ≈ 1 s at 60 Hz / 2 s at 30 Hz — comfortably covers max unacked window + redo headroom. Valve's `MULTIPLAYER_BACKUP` is similar in spirit. Keep `count` bounded and consider pruning inputs older than `sv_maxunlag` to bound memory.

---

## (b) Viable Alternatives + Comparative Analysis

### Library / approach matrix for a server-authoritative 600-player voxel game

| Option | Model | Bevy 0.18 compat | Prediction+Rollback | Interpolation | Lag Comp | Interest Mgmt | Verdict for Strata |
|---|---|---|---|---|---|---|---|
| **lightyear 0.26** | Server-auth, transport-agnostic (UDP/WebTransport(QUIC)/WS/Steam) | ✅ (0.26 ↔ Bevy 0.18) [table](https://docs.rs/crate/lightyear/latest) | ✅ built-in (history + rollback to earliest mismatch) | ✅ built-in | ✅ built-in | ✅ Rooms | **Best off-the-shelf fit** |
| **bevy_replicon_snap 0.2.6** | Server-auth, owner-prediction via `Predict` trait | ❌ stuck at Bevy 0.15 / replicon 0.29 [crates.io](https://crates.io/crates/bevy_replicon_snap) | ⚠️ basic, event-based | ✅ | ❌ none | ❌ | **Reject** (incompatible + unmaintained, last publish 2024-03) |
| **GGRS** | P2P rollback (GGPO reimpl), requires full determinism + save/load state | ✅ (engine-agnostic Rust) | ✅ but P2P lockstep | ❌ | ❌ | ❌ | **Reject** (no server-authoritative; 600-player P2P infeasible) [docs](https://docs.rs/ggrs/latest/ggrs/) |
| **bevy_timewarp** | Rollback layer *on top of* bevy_replicon; buffers component state | depends on replicon version | ✅ rollback | ❌ (you add) | ❌ (you add) | reuse replicon | **Viable if you keep bevy_replicon** [repo](https://github.com/RJ/bevy_timewarp) |
| **Custom `ComponentHistory<T>`** | Hand-rolled prediction/rollback + lag comp | ✅ (you write it) | ✅ (you write it) | ✅ (you write it) | ✅ (you write it) | reuse replicon `VisibilityFilter` | **Maximum control; high effort; lightyear is essentially this + glue** |

**Key architectural inconsistency in the plan:** §0/§2 prescribe **bevy_replicon + bevy_replicon_quinnet (QUIC)** as the stack, while §7.9 recommends **lightyear**. These are **mutually exclusive as a primary stack** — lightyear is a *complete* transport + replication + prediction + interpolation + lag-comp stack and does **not** sit on bevy_replicon. Adopting lightyear means replacing bevy_replicon + bevy_quinnet/quinnet with lightyear's own replication (and using its WebTransport/QUIC backend for the QUIC benefits). You would lose bevy_replicon's `VisibilityFilter` AOI and instead use lightyear **Rooms** / interest management. This decision must be made explicitly; the plan currently implies both, which is contradictory.

**Recommended split (best of both):**
- Use **lightyear** for *entity* prediction, rollback, interpolation, and lag compensation (players, mobs, projectiles). It gives QUIC via WebTransport and Bevy 0.18 support out of the box.
- Keep a **custom optimistic block-placement / world-mutation layer** (plan §7.5/§7.6) because voxel edits are *server-authoritative world mutations*, not entity-component prediction. lightyear's prediction is entity/component oriented; block edits are better handled as predicted events with server validation + revert, exactly as the plan describes.

**Why not bevy_replicon_snap:** incompatible Bevy version + no lag comp + unmaintained.
**Why not GGRS:** P2P lockstep, no authoritative server, determinism burden — directly contradicts Strata's 600-player server-authoritative model [GGRS docs](https://docs.rs/ggrs/latest/ggrs/).

---

## (c) Key Insights & Lessons

1. **Replay is already bounded by the unacked window**, not a fixed 10–15 frames. The "partial rollback" win is *worst-case bounding* via checkpoints, not a per-frame saving. Don't over-sell the 67%.
2. **Sliding-window input redundancy beats fixed-N repeat** (Overwatch). Fixed 3 is unsafe above ~100 ms RTT at ≥30 Hz.
3. **Smooth the render transform, snap the logical state.** Mutating velocity during correction makes client physics fight the server and can snag on colliders. Use lightyear's `correction_ticks` pattern.
4. **Quantization noise floor:** reconcile thresholds must sit *above* your own 0.01 m quantization step or you'll jitter constantly.
5. **0 ms local input delay is non-negotiable** for responsiveness; adaptive delay belongs only on remote interpolation. `input_delay` (lightyear) is an opt-in for predicted-remote entities, not the local player.
6. **Lag comp: include target lerp delay, add teleport guard, clamp usercmd tick replay** (anti-backtrack). 200 ms is the right CS:GO-hardened default.
7. **The plan's two networking stacks conflict.** lightyear ≠ bevy_replicon; pick one primary. lightyear + custom world-mutation layer is the coherent design.
8. **Voxel world is never rewound** for lag comp — only moving entities. This keeps server cost low and matches Valve.
9. **Determinism is desirable but not mandatory** for client prediction (Unity Netcode notes you "should aim for… without achieving it" — corrections handle drift) [Unity Netcode intro](https://docs.unity3d.com/Packages/com.unity.netcode%406.5/manual/intro-to-prediction.html). lightyear's `deterministic` feature is opt-in and only needed if you want input-replication lockstep.

---

## (d) Actionable Recommendations

| # | Area | Action | Priority |
|---|------|--------|----------|
| 1 | §0/§2/§7.9 | **Resolve the stack contradiction.** Decide: (A) lightyear as primary stack (replaces bevy_replicon+quinnet; use WebTransport/QUIC + Rooms for AOI), or (B) keep bevy_replicon + bevy_timewarp for prediction/rollback. Document the choice. | P0 |
| 2 | §7.1 | Keep divergence-point replay (already standard) **and** add bounded checkpoint snapshots, but reword the "67% reduction" claim to "worst-case replay bounding." Cadence = function of max unacked window. | P1 |
| 3 | §7.2 | Replace "last 3 inputs" with **all unacked inputs** (sliding window), bounded by the §10 buffer of 64. Retain 1.8 KB/s estimate only at 30 Hz; recompute at 60 Hz (3.6 KB/s). | P1 |
| 4 | §7.3 | Separate logical state (snap) from render transform (smooth). Add `correction_ticks`-style smoothing. Raise the no-correction threshold to ≥0.02–0.05 m (above quantization noise). | P1 |
| 5 | §7.4 | Keep 0 ms local input delay. Expose lightyear `input_delay` only as opt-in for predicted-remote/projectile smoothing. | P0 (already correct) |
| 6 | §9.2 | Keep 200 ms / `ceil(200ms/TICK)`. **Add:** (a) include target `lerpTicks` in compensation; (b) teleport-distance guard on rewind; (c) clamp usercmd tickcount replay (anti-backtrack). Rewind players/mobs/projectiles only — never the voxel world. | P1 |
| 7 | §7.5/§7.6 | Keep optimistic block placement + incremental dirty-brick mesh regen as a **custom** layer independent of entity prediction (lightyear doesn't model voxel world mutation). Ensure revert path is O(dirty bricks), not full chunk remesh. | P1 |
| 8 | §10 | Bound the 64-slot input ring; prune inputs older than `sv_maxunlag` to cap memory. | P2 |
| 9 | §8.1 | Adaptive interpolation delay for **remote** entities only — already corrected. Keep `ceil(delay/interval)+2` (Gaffer 3× for lossy). | P2 |
| 10 | Ecosystem | If choosing lightyear: pin **lightyear 0.26** (Bevy 0.18) and the `deterministic` feature only if/when input-lockstep is needed; enable `webtransport` for QUIC. If keeping bevy_replicon: add **bevy_timewarp** for rollback and implement lag comp manually per Valve's algorithm. | P0/P1 |

---

## Sources
- Gabriel Gambetta — Client-Side Prediction & Reconciliation: https://www.gabrielgambetta.com/client-side-prediction-server-reconciliation.html
- Gabriel Gambetta — Lag Compensation: https://gabrielgambetta.com/lag-compensation.html
- Valve Developer Wiki — Lag Compensation: https://developer.valvesoftware.com/wiki/Lag_Compensation
- Valve Source SDK — `player_lagcompensation.cpp`: https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/server/player_lagcompensation.cpp
- Valve Source SDK — `prediction.cpp`: https://github.com/ValveSoftware/source-sdk-2013/blob/master/src/game/client/prediction.cpp
- Overwatch GDC 2017 (Tim Ford) + Developer Update: https://www.youtube.com/watch?v=vTH2ZPgYujQ ; deep-dive: https://edgegap.com/blog/game-backend-deep-dive-overwatch-2016-netcode-architecture-rollback
- Quake 3 Network Model (Sanglard): https://fabiensanglard.net/quake3/network.php
- TF2 backtrack / sv_maxunlag hardening: https://github.com/ValveSoftware/Source-1-Games/issues/4664
- 4AM Games — Smooth Server Reconciliation: https://fouramgames.com/blog/fast-paced-multiplayer-implementation-smooth-server-reconciliation
- Socratopia — Client Prediction & Reconciliation (Ch.27a): https://www.socratopia.app/library/math-for-game-devs-en/chapter-31
- Unity Netcode — Intro to Prediction: https://docs.unity3d.com/Packages/com.unity.netcode%406.5/manual/intro-to-prediction.html
- lightyear (docs + Bevy compat table): https://docs.rs/crate/lightyear/latest ; prediction/rollback: https://docs.rs/lightyear/latest/lightyear/prelude/struct.PredictionManager.html ; avian example: https://github.com/cBournhonesque/lightyear/blob/main/examples/avian_physics/README.md
- bevy_replicon_snap (crates.io, Bevy 0.15): https://crates.io/crates/bevy_replicon_snap
- GGRS (docs): https://docs.rs/ggrs/latest/ggrs/
- bevy_timewarp: https://github.com/RJ/bevy_timewarp
- bevy_replicon (official, lists prediction/rollback integrations): https://docs.rs/bevy_replicon/latest/bevy_replicon/
