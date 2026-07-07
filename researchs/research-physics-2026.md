# Research: Physics Engine Selection for Bevy 0.18 Voxel Engine (Plans §1.11–1.12)

Status check: **mid-2026**. Findings independently verified via websearch + crates.io/github/docs.rs.

---

## Verdict — per engine-choice claim (§1.11)

| Engine | Plan claim | Research result | Verdict |
|---|---|---|---|
| **Rapier 0.32** (Voxels + bevy_rapier 0.34 + Bevy 0.18.1) | Best pragmatic fit | Confirmed: `bevy_rapier3d 0.34.0` requires `bevy ^0.18.1`, `rapier3d ^0.32.0`. Voxels shape (parry) + `set_voxel`/`propagate_voxel_change`/`combine_voxel_states` real. | ✅ **Correct** |
| **Jolt** | ❌ no voxel API, no official Bevy binding | Confirmed. Only `rolt`/`joltc-sys` (best-effort, incomplete, Jolt 5.0.0) and `SlimeYummy/jolt-physics-rs` (single-thread, unpublished). **No voxel collider API; no Bevy plugin.** | ✅ Correct |
| **Bevy XPBD** | ❌ no voxel collider | ⚠️ **Stale.** "Bevy XPBD" is **deprecated**; its successor **Avian** (0.5–0.6, Bevy 0.18) **added `Collider::voxels`** (Parry 0.21, commit #761, `voxels_3d` example). | ❌ Claim outdated |
| **Salva** | ❌ rapier 0.18 pin, non-deterministic | Confirmed. `salva3d 0.9.0` (2024-02) still pins `rapier3d ^0.18`. No 2026 release. | ✅ Correct |
| **PhysX** | ❌ unmaintained Bevy binding | Confirmed worse than plan states: `physx-rs` **ARCHIVED by Embark on 2026-05-21**; `bevy_mod_physx 0.9.0` (2025-10) only supports **Bevy 0.17**, not 0.18. | ✅ Correct (stronger) |

---

## Optimal choice

**Confirmed: Rapier 0.32 + bevy_rapier 0.34 + Bevy 0.18.1 remains the best pragmatic fit.** Rationale unchanged and validated:
- Only engine with a native `Voxels` collider (~1 byte/voxel, internal-edge tracking via `VoxelState`).
- `enhanced-determinism` for server-authoritative sync.
- Live Dimforge ecosystem aligned with Bevy 0.18 / wgpu.

**Revisions / risks the plan under-states:**

1. 🔴 **CCD / shape-casting on Voxels is NOT "unsupported" — plan §1.1 is stale.** The Rapier.js changelog (mirror of the same Rust core) explicitly lists:
   - *"Added support for shape-casting involving Voxels colliders."*
   - *"Added support for CCD involving Voxels colliders."*
   Rust `Voxels` implements `RayCast` and exposes `ccd_thickness()` / `ccd_angular_thickness()`. **Correct the §1.1 table** from `❌ Desteklenmiyor` to `✅ (later 0.3x)`. CCD on *dynamic* voxel bodies + fast projectiles is now viable — valuable for Strata's destruction/debris.
2. 🟡 **Dynamic voxel colliders still skip auto mass/inertia** (confirmed in JS changelog: *"Voxels colliders attached to dynamic rigid-bodies will not run the automatic mass/angular inertia calculation"*). Plan §1.1 "Dynamic ⚠️ Mass/inertia manuel" is correct — keep, but note §1.6 fragment spawn must set `MassProperties` explicitly (plan already does).
3. 🟡 **Voxel-vs-Voxel & Voxel-vs-heightfield narrow-phase gaps persist** (JS changelog: *"Collision-detection between two-voxels colliders, or a voxels collider and a mesh, polyline, or heightfield"* unsupported). Plan's custom layer (§1.5) for voxel-voxel is still justified.
4. 🟡 **Rapier Voxels is explicitly "experimental"** and scales with voxel count (plan §1.1 notes this). WARM surface-only (§1.1a) + tier frequency (§1.7) mitigations remain essential and correct.
5. 🟢 **Broad-phase BVH (Parry, since 0.27) + CollisionGroups** — confirmed, plan §1.2 accurate.

---

## Alternatives & lessons (including MISSED ones)

### Confirmed-correct attributions (§1.12)
- **Ders 1 — Jolt NON_MOVING/MOVING broad-phase split → `CollisionGroups`**: ✅ Accurate. Jolt uses separate broad-phase layers; Rapier `InteractionGroups` is the correct analogue.
- **Ders 2 — XPBD "substep > iteration" (Macklin 2019, *Small Steps in Physics Simulation*)**: ✅ Accurate. Source confirms: *n small substeps × 1 iteration* beats *1 large step × n iterations* (quadratic error reduction, Δt²). `TimestepMode::Fixed { substeps: 4 }` is the right application.
- **Ders 3 — Rapier determinism ⊥ parallel**: ✅ Accurate. `enhanced-determinism` + `parallel` are mutually exclusive features.
- **Ders 4 — Fixed timestep**: ✅ Accurate. Bevy `FixedUpdate` + `TimestepMode::Fixed` prevents tunneling/jitter.
- **Ders 5 — Jolt broad-phase profiling**: ✅ `JPH_TRACK_BROADPHASE_STATS` is a real Jolt C++ macro (QuadTree stats: nodes visited, hits, ticks). Plan's "use Rapier `Counters` to calibrate tier thresholds" is a sound analogue.

### MISSED lessons (not harvested)
1. **XPBD `compliance` (α) / `erp`** — The plan applies XPBD *substeps* but never mentions **compliance**. XPBD (Macklin 2016) makes stiffness **time-step- and iteration-count-independent** via compliance α = 1/stiffness. For Strata's stacks/debris (§1.6) this matters: set `compliance` instead of guessing stiffness per substep. (Rapier uses `JointData` compliance, not XPBD solver — so this lesson is partially *not transferable* to Rapier; worth a note that Rapier is impulse/soft-constraint based, not XPBD.)
2. **Jolt `PxRigidDynamic`-style sleeping is N/A (wrong engine)** — but the *concept* Strata misses: **sleeping/inactive bodies**. Plan §1.5 has a `SleepManager` for falling sand only. Rapier has native body sleeping; for 100+ static sectors this is automatic, but Strata should explicitly ensure terrain bodies sleep and only wake on edit. Not harvested as a "lesson from an alternative" but is a gap.
3. **Jolt `BroadPhaseLayer` count limit (max 2 layers in default impl)** — Jolt's split is literally 2 layers (MOVING/NON_MOVING); Rapier `CollisionGroups` (32 bits) is *more* flexible. Plan could note Rapier's groups *exceed* Jolt's 2-layer model — an improvement, not just a port.
4. **PhysX `PxRigidDynamic` sleeping / `setSleepThreshold`** — listed as a candidate "missed lesson" in the task. Confirmed PhysX exposes `setSleepThreshold` + `setRigidBodyFlag(eSKINNED)`. But PhysX is archived/unmaintained for Bevy 0.18, so harvesting is moot; the *general lesson* — tune sleep thresholds to avoid simulating resting stacks — is already covered by Rapier's native sleeping. **No action needed**, but plan §1.12 could explicitly state "PhysX sleeping lesson subsumed by Rapier native sleeping."
5. **Avian/XPBD `f64` precision + `enhanced-determinism`** — Avian (the engine that *now* has voxels) offers `parry-f64` and an `enhanced-determinism` feature that improves cross-platform reproducibility. If Strata ever re-evaluates, **Avian + Voxels is now a real contender** for determinism-critical sims (Rapier's `enhanced-determinism` is x86-focused). Plan should footnote: *Bevy XPBD → Avian now supports Voxels; re-evaluate if Rapier Voxels experimental gaps bite.*

---

## Alternatives not considered (task §4)

| Option | Status | Lesson |
|---|---|---|
| **box2d** | 2D only | N/A for 3D voxel — correctly excluded. |
| **Custom `glam` + `parry` (no Rapier rigidbody)** | Viable for Strata's *static* terrain + custom CA (§1.5 already does custom layer for voxel-voxel, sand, fluids). | Lesson: terrain collision needs only `parry` narrow-phase (`QueryPipeline`/`Voxels` shape) — Rapier rigidbody solver is overhead for static-only. Plan's ACTIVE full-voxel + KCC (kinematic) already minimizes rigidbody use; consider `parry` `QueryPipeline` only for DISTANT tier AABB queries to drop Rapier sim cost there. |
| **`bevy_rapier` `QueryPipeline` only (no simulation)** | Strong fit for static terrain + raycasts, no `PhysicsPipeline`. | Lesson: for WARM/DISTANT tiers (static-only, no dynamic coupling) run **query-only** — skip Rapier's solver/integration entirely, update `QueryPipeline` manually. Reduces §1.7 cost. Plan doesn't separate "simulation" vs "query" tiers; recommended addition. |

---

## Citations
- bevy_rapier3d 0.34.0 crates.io — requires `bevy ^0.18.1`, `rapier3d ^0.32.0`.
- dimforge/rapier CHANGELOG — Voxels collider added v0.25 (`#823`); ccd test + voxels examples in 0.32 compare.
- dimforge/rapier.js CHANGELOG — "Added support for shape-casting involving Voxels colliders"; "Added support for CCD involving Voxels colliders"; dynamic-voxel mass auto-compute NOT run; voxel-vs-voxel/heightfield narrow-phase gap; `JoltPhysics` issue #327 KCC `computedGrounded` false on Voxels.
- docs.rs/rapier3d `Voxels` — implements `RayCast`, `ccd_thickness`/`ccd_angular_thickness`; "may NOT be ideal for very large sparse worlds."
- avianphysics/avian — Bevy 0.18 (0.5–0.6); commit #761 "Add support for `Voxels` shape"; `voxels_3d` example; issue #945 (compound-of-voxels no collision, open).
- bevy_xpbd docs — **deprecated** in favor of Avian.
- salva3d 0.9.0 (2024-02) — pins `rapier3d ^0.18`; no 2026 update.
- EmbarkStudios/physx-rs — **ARCHIVED 2026-05-21**; bevy_mod_physx 0.9.0 (2025-10) Bevy 0.17 only.
- rolt / joltc-sys — best-effort Jolt 5.0.0 bindings, no Bevy plugin, no voxel API.
- JoltPhysics `BroadPhase.h` / `QuadTree.cpp` — `JPH_TRACK_BROADPHASE_STATS` macro real.
- Macklin et al. 2019, *Small Steps in Physics Simulation* (SCA'19) — substep > iteration.
- Macklin et al. 2016, *XPBD* — compliance/α for time-step-independent stiffness.

---

## Recommended plan edits (concise)
1. §1.1 table: **CCD / shape-casting → ✅ (supported in 0.3x)**, not ❌.
2. §1.11: rename "Bevy XPBD" row → "**Avian** (Bevy XPBD successor): ⚠️ now has Voxels collider (0.4+), but experimental gaps + Rapier determinism ecosystem still favored." Mark as *re-evaluate* rather than hard ❌.
3. §1.12: add **XPBD compliance** note (transferability caveat to Rapier impulse solver); add **PhysX sleeping lesson subsumed by Rapier native sleeping**.
4. §1.7 / §1.2: consider **query-only `QueryPipeline`** for WARM/DISTANT tiers (no `PhysicsPipeline` sim).
