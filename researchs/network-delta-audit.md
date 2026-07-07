# Audit: DELTA COMPRESSION + QUANTIZATION (Plan 16 §5)

Scope: network position / rotation / velocity encoding for Strata (Rust + Bevy 0.18,
server-authoritative, ~600 players, 20–30 Hz snapshots, sector-anchored voxel world).
This audit validates the five key decisions against published practice (Glenn Fiedler /
Gaffer On Games, Valve Source, Unity Netcode, Marc B. Reynolds, Rust serialization
benchmarks) and proposes concrete fixes.

Sources are cited inline as `[n]`.

---

## (a) Validation

### A1. Sector-anchored position quantization

**Decision.** `QuantizedPosition { sector: I16Vec3 (keyframe, 48-bit), local: [u16;3]
(12-bit @ 0.05 m, 32 m range) }`.

**Sector size.** Strata sectors are 32×32×32 voxels (`plans/06`). At 1 m/voxel a sector is
32 m. `i16` max = 32767 → 32767 × 32 m = **1,048,544 m ≈ ±1.05 M m** per axis. This matches
the plan's stated bound exactly, so the i16 sector choice is internally consistent.

**i16 range adequacy.** ±1048 km per axis is enormous for a 600-player voxel server. The
only concern is the project's "unlimited height" (sınırsız yükseklik) goal: 1048 km of
vertical extent = 32,768 sectors, which no sane player build reaches, but a truly unbounded
vertical axis would eventually overflow `i16`. Recommendation: keep `i16` for X/Z (and
horizontal spread); for Y, either (i) widen to `i32` (only 32 extra bits, sent rarely as it
is keyframe-only), or (ii) document a hard world bound at ±1 M m. The keyframe is sent only
every 16–32 ticks, so enlarging it has negligible steady-state cost.

**⚠ Spec bug — local precision vs range are inconsistent.** The plan states "12-bit @
0.05 m, 32 m range". These cannot both be true:
- 12 bits = 4096 quanta. At **0.05 m** resolution the range is 4096 × 0.05 = **204.8 m**, not 32 m.
- At a **32 m** range the resolution is 32 / 4096 = **7.8125 mm**, not 0.05 m.

Pick one and make the rest of the scheme consistent:
- **Option (recommended):** local = 12-bit over the 32 m sector → 7.8 mm precision. This is
  tighter than the 0.05 m deltas, so no precision is lost at the anchor boundary. The
  "0.05 m" figure should then apply *only* to the per-tick delta scheme.
- Option: local = 0.05 m resolution → that needs only 640 values = **10 bits** over 32 m
  (you are wasting 2 bits). Use `u10` and save 6 bits/keyframe.

Either way the current text over- or under-allocates and must be corrected before
implementation. (Fiedler quantizes position to 512 values/m ≈ 2 mm for snapshot *interpolation*,
and 4096 values/m for *state synchronization* where quantization is fed back into the sim
`[1]`, so 7.8 mm is well within safe territory.)

**0.05 m delta precision.** This is a reasonable, slightly conservative choice. Fiedler used
512 values/m (≈2 mm) for interpolation and 4096 values/m (≈0.24 mm) when quantization is fed
back into the simulation `[1]`. For *visual* interpolation of remote voxel entities, 0.05 m
(2 cm) is fine; the only risk is visible "stair-stepping" on very slow, smooth movers, which
interpolation hides. Keep 0.05 m for deltas.

### A2. Fiedler multi-level bitpacking

**Decision.** Per-frame delta: 1-bit "small?" → small = 7-bit @0.05 m (±3.2 m/tick); else
1-bit "medium?" → medium = 13-bit; else 12-bit local absolute.

**Does this match Fiedler's actual method?** Fiedler's published greedy result for *position
deltas* is a **two-level** scheme: 1-bit "small" + 5 bits if small `[-16,+15]` (in his
quantized units), otherwise 9 bits `[-256,+255]`, with a fallback to the **absolute** 50-bit
position when a component exceeds the large range `[2]`. For *orientation* deltas he found
`5-8` (small `[-16,+15]`, large `[-128,+127]`) giving 23.3 bits vs 29-bit absolute smallest-three
(80.3%) `[2]`.

Your three-level (small / medium / absolute) is a legitimate generalization, **but**:
1. **The bit-widths are not histogram-tuned.** Fiedler explicitly "wrote a short ruby script
   to find the best encoding with a greedy search" over his dataset `[2]`. Your 7 / 13 / 12
   numbers are plausible but unvalidated. At 0.05 m, 7-bit signed = ±64 → ±3.2 m/tick; at
   30 Hz that is ±96 m/s — far beyond any voxel entity, so the "small" bucket will almost
   never overflow (good for robustness, wasteful for size). Fiedler's 5-bit window covered a
   32 m/s cube sim at 60 Hz (`~0.5 m/tick`) comfortably `[1]`. For Strata you can likely drop
   "small" to 6 bits (±1.6 m/tick) or widen the window, *after* measuring real movement.
2. **"medium = 13-bit" range is undefined.** 13-bit signed = ±4096 → ±204.8 m at 0.05 m,
   which is essentially the entire absolute range. A 13-bit "medium" @0.05 m is therefore
   redundant with the absolute fallback unless it uses a coarser resolution. The spec must
   state the medium resolution/range, or collapse the tier.
3. **Fallback target.** Confirm the "else 12-bit local absolute" is an *absolute keyframe*
   (sector + local), not a large *relative* value. Mixing the two is a classic off-by-one
   bug. Fiedler's fallback is the full absolute baseline position `[2]`.
4. **Context-aware arithmetic coding beats bitpacking by ~25%** (Fabian Giesen's result cited
   by Fiedler `[2]`). For 600 players this is a real option *if* CPU allows; bitpacking is
   safer and faster, which is the right call for a voxel hot path. Keep bitpacking as the
   primary; benchmark an arithmetic coder (e.g. `ruzstd`/ANS) only if bandwidth is the
   bottleneck.

**Verdict:** sound architecture, **must** (i) define medium's range, (ii) run a greedy
histogram tuner over real Strata motion traces, (iii) fix the local precision/range bug from
A1 so the absolute fallback size is correct.

### A3. Rotation compression

**Decision.** yaw-only `u16` (2 B) for most entities; full 3-DOF = 32-bit smallest-three
`{ a:u10, b:u10, c:u10, index:u2 }`; 1-bit conditional flag.

**Yaw-only `u16`.** 16-bit yaw = 360/65536 ≈ **0.0055°** resolution — more than enough for any
voxel entity. Most mobs/items need only heading. Correct and cheap. (Keep pitch for the
*local* player client-side; only replicate pitch if the entity's view matters to others.)

**Smallest-three layout — mostly correct, one critical clarification.** The classic
smallest-three (Fiedler `[1]`, Unity Netcode `QuaternionCompressor` `[3]`, `jpreiss/quatcompress`
mean error ~0.08° at 32-bit `[4]`) works as: find the component of largest *absolute* value,
drop it, send its 2-bit index + the other three components. Because `q` and `-q` represent the
same rotation, **negate the whole quaternion so the dropped (largest) component is positive**;
then the three transmitted components carry their own signs.

⚠ **The plan's `a:u10, b:u10, c:u10` are written as *unsigned*.** The three smallest components
are **signed** values in `[-1/√2, +1/√2]` (range ≈1.414) `[1]`. If `u10` is treated as unsigned
magnitude without a sign, you lose the sign of each component and reconstruction is wrong. Two
valid encodings:
- sign-magnitude: 1 sign bit + 9 magnitude bits per component (as Unity's compressor does,
  packing sign into the 10th bit `[3]`), or
- map `[-1/√2, +1/√2]` → `[0, 1023]` linearly (no explicit sign bit), decode by reversing.
Either way the *total* is 2 + 10 + 10 + 10 = 32 bits — correct — but the spec must state the
components are **signed** (or sign-encoded), not raw `u10` magnitude. Fix the wording.

**Range trick is correctly implied.** Encoding the three smallest in `[-1/√2,+1/√2]` rather
than `[-1,+1]` is the standard precision win `[1]` and is compatible with 10-bit fields. Good.

**Conditional flag.** 1-bit select yaw-only (17 bits) vs full smallest-three (33 bits) is
sound. Most entities take the cheap path.

### A4. Velocity removal (dead reckoning + event-driven)

**Decision.** Velocity removed from network; clients dead-reckon; velocity re-sent only on
impulse / teleport / "at-rest → moving" transitions.

**Does Valve/Source send velocity?** Mostly *no* for remote (interpolated) entities. The
Source SDK interpolates remote entities with **Hermite** splines that *derive* apparent
velocity from consecutive samples, and `m_vecVelocity` is explicitly **commented out** of the
interpolated vars in `c_baseentity.cpp` ("Removing this until we figure out why velocity
introduces view hitching") `[5]`. Velocity is predicted only for the *local* player via client
side prediction, not networked to others. So dropping network velocity for remote entities is
exactly Source's model. ✅

Fiedler makes the same point with the **"active" flag**: when an object is at rest,
`active=false` and no velocity is sent (assumed zero); only moving objects carry velocity, and
objects that *just* came to rest get elevated resend priority until acked `[6]`. Your
"at-rest → moving transition triggers velocity" is precisely this. ✅

**Risks & mitigations (all already covered by the plan's event list):**
- **Collision / blocking:** dead reckoning can push an entity into/through geometry the client
  doesn't know about. Mitigation: server is authoritative and the next absolute/large delta
  snaps it back; keep the keyframe interval short enough (16–32 ticks is fine) and never let
  dead-reckoning run past the interpolation buffer.
- **Knockback / impulse:** sudden velocity change must be an *event* (you have it). Without it
  the client would drift wrongly until the next position sample.
- **Teleport:** must be an *event* forcing an absolute keyframe and clearing the dead-reckoning
  integrator (you have it).
- **Packet loss during a transition:** the transition event itself must be **reliable/ordered**
  (or re-sent until acked with the elevated-priority scheme Fiedler describes `[6]`), otherwise
  the client misses the velocity change and desyncs.

**Verdict:** industry-standard and correct. The only strict requirement is that the three
event types travel on a **reliable** channel (or are re-sent until acked), and that dead
reckoning is bounded by the interpolation/keyframe window.

### A5. Entity-level mask / RLE layer deleted

**Decision.** Removed because "lightyear already sparse."

Lightyear (and naia) replicate via **per-component diffing**: only changed `Property<>` fields
are sent, with a 1-bit/`DiffMask` change flag per component (`[7]`, `[8]`). naia's `Property`
wrapper yields "0 or 1 bit" when a component is unchanged `[8]`. So an explicit entity-level
mask/RLE is indeed redundant — the sparseness already exists at component granularity, which is
*finer* than entity granularity. ✅ This is the right call; deleting it avoids double work.

⚠ One caveat: this assumes Strata's replication rides on lightyear/naia (or a homegrown
per-component diff). If you ever send a hand-rolled monolithic snapshot, you would need your
own per-field change bits. Confirm the snapshot serializer emits per-component diffs, not
whole-entity blobs.

---

## (b) Alternatives comparison

### B1. Position anchoring

| Scheme | Bits (typical) | Notes |
|---|---|---|
| Absolute float32 ×3 | 96 | baseline, wasteful |
| Absolute quantized (Fiedler 512/m) ×3 | 50 | `[1]` |
| **Sector-anchored (this plan)** | 48 (keyframe) + 7–13/tick delta | best fit for unbounded voxel world |
| Varint absolute (i32 sector + f32) | variable, large on keyframe | simpler, no anchor math |

Sector-anchoring is the right choice for a cubic-chunk voxel world: the anchor (sector) is
shared/cacheable and deltas stay tiny. Keep it.

### B2. Delta bitpacking

| Scheme | Relative size | Trade-off |
|---|---|---|
| 2-level small/large + absolute (Fiedler) | 26.1 bits avg pos delta `[2]` | proven, simpler |
| 3-level small/medium/absolute (this plan) | tunable, potentially smaller | needs histogram tuning |
| Context-aware arithmetic (Giesen, via Fiedler) | ~25% smaller than best bitpack `[2]` | CPU cost, more complex |
| Full absolute every tick | 50+ bits | no delta logic, high BW |

The plan's 3-level is a strict superset of Fiedler's 2-level and can match or beat it *once
tuned*. Recommend implementing a greedy tuner (`[2]`) and measuring; keep arithmetic coding as
a future optimization.

### B3. Rotation encoding

| Scheme | Bits | Mean angular error | Source |
|---|---|---|---|
| yaw-only u16 | 16 | n/a (yaw only) | this plan — good for mobs |
| smallest-three 32-bit | 32 | ~0.08° | `[1]`,`[4]` — this plan |
| smallest-three 24–26-bit (8b comps) | 24–26 | ~0.3–0.9° | `[9]` quat8x3 — too coarse for players |
| octahedral 2×11 + 10 | 32 | ~0.027° (n_avg) | `[9]` oct11x2+a10 — slightly better, harder GPU decode |
| Cayley 30-bit | 30 | ~0.12° | `[9]` — algebraic, has spare orientation bit |

Smallest-three 32-bit is the pragmatic standard (Unity, Fiedler, jpreiss all use it). Octahedral
at 32-bit is marginally more accurate but offers no meaningful network win and complicates
decode; not worth it unless you also need GPU reconstruction. 24-bit is too lossy for player
avatars but acceptable for cosmetic props. **Keep smallest-three 32-bit; fix the signed-component wording (A3).**

### B4. Velocity

| Model | Sends velocity? | Fit for Strata |
|---|---|---|
| Source/Valve remote entities | No (Hermite-derived) `[5]` | ✅ matches plan |
| Fiedler "active" flag | Only when moving `[6]` | ✅ matches plan |
| Always-send velocity | Yes | wasteful, causes hitching `[5]` |
| Pure dead reckoning, no events | Yes | desyncs on impulse/teleport |

Plan matches the two best-regarded models. Keep.

### B5. Serialization / snapshot framing crates

| Crate | Role | Relevance |
|---|---|---|
| `bitcode` | bitwise serializer, 1-bit ints, field grouping, auto-vectorized, tiny output, Zstd-friendly `[10]` | excellent for the *leaf* field encoding, but you still hand-roll the delta/bitpack logic |
| `bincode` / `postcard` | fast binary serde, varint option `[10]` | fine for messages/RPCs, not for tight per-tick deltas |
| `ruzstd` (zstd) | general compression | good for *chunk/world* data and large one-off snapshots, NOT per-tick (latency + CPU). Fiedler notes bitstreams are already near-optimal; zstd helps only on structured/repetitive payloads `[11]` |
| lightyear / naia | full replication w/ per-component delta `[7]`,`[8]` | provides the sparse diff layer the plan relies on |

**Recommendation:** hand-rolled bitpacking (Fiedler-style) for the hot per-tick position/rotation
deltas (fast, branch-light, no alloc); `bitcode` or `postcard` for RPCs/events; `ruzstd` only for
infrequent large transfers (world chunks, initial state, SVDAG bake blobs per `plans/07–08`). Do
**not** zstd every 30 Hz snapshot.

---

## (c) Key insights / lessons

1. **Fiedler's numbers are dataset-specific.** His 5/9-bit position and 5/8-bit orientation
   widths came from a greedy search over *his* cube sim (max 32 m/s) `[1]`,`[2]`. Strata's
   movement distribution is different (walking/flying voxel entities, teleports, elytra-like
   flight). Re-derive widths from real traces or you will either waste bits or overflow the
   "small" bucket.
2. **Bitpacking is near-optimal for this workload; arithmetic coding is a ~25% nice-to-have**
   `[2]`. Don't gold-plate the encoder before measuring real bandwidth.
3. **Source/Valve prove you can drop networked velocity entirely** for remote entities and still
   look smooth — interpolation derives it `[5]`. This de-risks the plan's boldest decision.
4. **The "active / at-rest" flag is the linchpin** of velocity removal `[6]`. Missed
   at-rest→moving or teleport events desync clients; those events must be reliable.
5. **Smallest-three must carry signed components** (or sign-encoded). Writing `u10` as unsigned
   magnitude is a silent correctness bug `[3]`,`[1]`.
6. **The local-precision/range inconsistency (A1) will silently waste bandwidth or lose
   precision** depending on which number the implementation honors. Fix before coding.
7. **600-player full replication will not fit a 256 kbps link.** A rough estimate: position
   ≈ 3.5 B/tick + rotation ≈ 2 B/tick at 30 Hz ≈ 5.5 B × 30 = 165 B/s *per replicated entity*.
   600 entities → ~99 KB/s ≈ **0.8 Mbps downstream per client** — ~3× the 256 kbps target.
   This is exactly why Strata's sector/AOI scoping (`plans/08`) and lightyear interest
   management are mandatory: only entities in view are replicated. The compression scheme is
   necessary but not sufficient; scope is the real lever.

---

## (d) Actionable recommendations

1. **Fix the local quantization spec (A1).** Choose: 12-bit over 32 m sector → 7.8 mm (precision
   note: "0.05 m" applies to deltas only), *or* 10-bit @0.05 m over 32 m. Update the struct
   comment and any decoder math accordingly.
2. **Define the "medium" tier explicitly (A2).** Specify its resolution/range; if it equals the
   absolute range, drop the tier to Fiedler's 2-level (small/large + absolute).
3. **Add a greedy histogram tuner (A2).** Record real per-tick position/rotation deltas on a
   live server, then search bit-widths (small/medium/large windows) for minimal average bits,
   à la Fiedler's ruby script `[2]`. Re-run after movement code stabilizes.
4. **Correct the smallest-three spec to signed components (A3).** State `a/b/c` are signed
   (sign-magnitude or linear map into `[0,1023]`); keep 2-bit index + negate-quaternion trick.
5. **Make the three velocity events reliable (A4).** Impulse / teleport / at-rest→moving must use
   an ordered-reliable channel or resend-until-acked (Fiedler's elevated-priority scheme `[6]`).
6. **Bound dead reckoning by the interpolation + keyframe window (A4).** Never integrate past the
   interpolation buffer; keyframe every 16–32 ticks is appropriate.
7. **Confirm per-component diffing is in place (A5).** Verify the snapshot serializer emits
   per-field change bits (lightyear `Property` / naia `DiffMask`); otherwise re-add a minimal
   per-field mask.
8. **Don't zstd per-tick snapshots (B5).** Hand-roll Fiedler bitpacking for hot deltas; use
   `bitcode`/`postcard` for RPCs; reserve `ruzstd` for chunk/world/SVDAG blobs.
9. **Lean on AOI/interest management for the 600-player budget (c-7).** Compression alone cannot
   hit 256 kbps at full replication; sector-scoped visibility (`plans/08`) is the dominant lever.
10. **Consider delta-encoding rotation too.** Fiedler got 80.3% of absolute smallest-three by
    delta-encoding the smallest-three directly (90% of frames keep the same largest-component
    index) `[2]`. Add as a Phase-2 optimization for full-3-DOF entities.

---

## References

- `[1]` Glenn Fiedler, *Snapshot Compression* — https://gafferongames.com/post/snapshot_compression/
- `[2]` Glenn Fiedler, *Delta Compression* (greedy multi-level search, 26.1-bit pos, 23.3-bit quat, Giesen −25%) — https://gist.github.com/gafferongames/bb7e593ba1b05da35ab6 and https://gafferongames.com/post/snapshot_compression/
- `[3]` Unity Netcode, `QuaternionCompressor.cs` (sign bit per component, √2/2 range) — https://github.com/Unity-Technologies/com.unity.netcode.gameobjects/blob/develop/com.unity.netcode.gameobjects/Components/QuaternionCompressor.cs
- `[4]` jpreiss/quatcompress (32-bit smallest-three, ~0.08° mean error) — https://github.com/jpreiss/quatcompress
- `[5]` Valve Source SDK, `c_baseentity.cpp` / `interpolatedvar.h` (velocity removed from interp, Hermite) — https://github.com/ValveSoftware/source-sdk-2013 ; Source multiplayer networking — https://developer.valvesoftware.com/wiki/Source_Multiplayer_Networking
- `[6]` Glenn Fiedler, *Networking for Physics Programmers* (active/at-rest flag, priority accumulator) — https://www.gamedevs.org/uploads/networking-for-physics-programmers.pdf
- `[7]` Lightyear (Bevy netcode, snapshot interpolation, per-component replication) — https://github.com/cBournhonesque/lightyear
- `[8]` naia (per-property delta, `DiffMask`, Tribes 2 model) — https://github.com/naia-lib/naia
- `[9]` zeux.io, *Quantizing tangent frames* (octahedral vs smallest-three tables) — https://zeux.io/2026/04/30/quantizing-tangent-frames/ ; Marc B. Reynolds, *Quaternion quantization* — http://marc-b-reynolds.github.io/quaternions/2017/05/02/QuatQuantPart1.html
- `[10]` bitcode / bincode / rust_serialization_benchmark — https://github.com/SoftbearStudios/bitcode , https://github.com/djkoloski/rust_serialization_benchmark
- `[11]` Glenn Fiedler, *Sending Large Blocks of Data* (zstd for chunk transfers, not per-tick) — https://gafferongames.com/post/sending_large_blocks_of_data/
