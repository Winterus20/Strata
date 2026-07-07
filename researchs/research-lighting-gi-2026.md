# Strata Lighting & Indirect GI — Research Pass (2024–2026)

**Scope:** State-of-the-art indirect global illumination and lighting for a GPU-driven voxel engine
(Strata: 32³ Sectors, XBrickMap, SVDAG far-field, 64-bit Aokana-style visibility buffer).
**Date:** 2026-07-06
**References** grounded against `plans/13-lighting.md` (L0–L4) and `plans/23-environment-time-weather.md`.

---

## 0. TL;DR — What Strata's plan gets right, and the gaps

**Right (keep these):**
- L0/L1/L2 (analytic sun + CPU SIMD BFS block light + column-first sky light) is the correct, proven
  foundation. This is the *Minecraft/Fabric/Starlight* lineage and it is the right call for voxel worlds —
  cheap, edit-friendly, deterministic, GPU-independent. The 16-bit packed `LightData` and WLP (whole-light
  packed) arithmetic are good. Keep it.
- Tiering ACTIVE/WARM/DISTANT/ARCHIVE aligned to streaming (plans 08) is correct.
- Using the visibility buffer as the GI query entry point (L3/L4 read `hit.voxel_coord`) is architecturally
  sound and GPU-driven.

**Gaps (must fix before L3/L4 ship):**
1. **L3 "Clustered Voxel GI" as specified is not real GI** — it is a single-bounce, O(n²) cluster-to-cluster
   gather with a 3D-Bresenham visibility test and `1/(1+d²)` attenuation. This is a radiosity-style hack that
   will (a) be O(clusters²) = up to ~10⁶ pairwise tests per sector, (b) bleed light badly through thin walls,
   (c) have no energy conservation, and (d) has no path to multi-bounce or glossy. See §4.
2. **L4 SVDAG "cone trace" is underspecified and re-traces from scratch every 10 frames** — no temporal
   coherence, no irradiance cache, no probe grid. Cone tracing into an SVDAG is viable but the plan's WGSL
   has a 64-iteration hard loop with hand-waved aperture growth and no LOD-anchored sampling. See §3.
3. **No DDGI / radiance-cache layer** — the plan jumps from "clustered hack" to "raw cone trace," skipping
   the industry-standard middle ground (probe grids / surfels / irradiance caches) that actually ships in
   games (DDGI, GIBS, Lumen). See §4/§5.
4. **No ReSTIR / reservoir resampling** considered anywhere — the plan uses fixed 6-cone sampling. With
   emissive voxels (lava, glowstone, torches as area lights) ReSTIR GI would be a massive win. See §2.
5. **Temporal accumulation is naive** (single blend factor, no disocclusion/rejection, no variance). See §6.
6. **No GPU light-propagation option** for the edit-driven world; CPU BFS is fine for L1 but a streaming
   world with frequent edits benefits from a GPU flood / JFA distance field for *occlusion* (sky light).
   See §7.
7. **Day/night sky is a gradient skybox + sun position only** — replace with Hillaire 2020 atmosphere (or
   Precomputed Atmospheric Scattering). Cheap, physically based, gives correct aerial perspective + sun
   disk + multiple scattering. See §8.

---

## 1. Neural Irradiance Volumes (NIV, Coomans et al. 2026 / "Adobe NIV")

**Paper:** "Real-time Rendering with a Neural Irradiance Volume," *arXiv:2602.12949*, Eurographics 2026.
(Note: this is 2026, slightly newer than the plan's "Adobe NIV 2024" citation — the technique matured.)

**Architecture:**
- Regresses a continuous **5D irradiance field** E(x, n) → irradiance, conditioned on position + direction.
- Dual-branch MLP + **multi-level hash encoding** (Müller 2022 Instant-NGP style).
- Memory **1–5 MB** for medium scenes; **0.19–1.35 ms** inference at 1080p on consumer GPUs.
- Input: **G-buffer only** (position + normal). No RT, no denoiser.
- Claims **≥10× better quality than probe grids at equal memory**.

**How it could replace voxel cone tracing for distant GI:**
- NIV is essentially a *learned irradiance volume*. For Strata's DISTANT/ARCHIVE tiers it is a strong fit:
  train offline on SVDAG-baked radiance (as the plan's §1.12 suggests), then at runtime do
  `G-buffer → NIV → indirect diffuse`. No cone marching, no probe management.
- Works on **dynamic/unseen objects** (queries are positional, not surface-bound) — important for Strata
  where the player edits geometry.

**Tradeoffs / why NOT to adopt yet:**
- **Training data dependency:** needs a high-quality ground-truth baker (path-traced SVDAG) to train.
  Strata does not have one yet. You'd build the baker anyway for validation.
- **Non-composable / non-linear:** Iwanicki's Neural Light Grid talk (SIGGRAPH 2024, CoD) explicitly
  warns that neural representations can't be linearly composited at runtime (uber-bake, day/night blending),
  which breaks Strata's "analytic sun + composable tiers" philosophy. The plan's `13-lighting.md §1.12`
  wants to *blend* NIV with dynamic lighting — this is exactly the hard part.
- **Tensor-core requirement** for the MLP (or it eats ALU). On a pure WGSL/vulkan path without matrix
  intrinsics, inference is slower than the paper's CUDA numbers.
- **Black-box bias / leakage** around edits until retrained.

**Recommendation:** Keep NIV as a **Phase 6 (distant-only) optional backend**, not a core tier. First build
the SVDAG path-traced baker (also needed for §3/§5 validation). Expose NIV behind a trait so it can be
swapped with DDGI. Do **not** let it block the L3/L4 pipeline.

---

## 2. ReSTIR GI / ReSTIR PT (2021–2026)

**Papers:**
- ReSTIR (Bitterli et al. 2020, SIGGRAPH) — direct light, millions of lights, 6–65× faster.
- ReSTIR GI (Ouyang et al. 2021, HPG) — resamples *multi-bounce indirect paths*; 9.3×–166× MSE vs PT at 1 spp.
- ReSTIR PT Enhanced (Lin, Kettunen, Wyman 2026, i3D) — 2–3× faster, unifies DI+GI reservoirs, duplication
  maps for disocclusion, footprint-based reconnection; closer to production.
- RTXDI SDK ships ReSTIR GI (`GI/` bridge functions).

**Can a voxel / SVDAG engine use reservoir resampling?**
- **Yes, and it is arguably the best fit for Strata's emissive blocks.** In Strata, lava/glowstone/torches
  are *area lights* (emissive voxels). ReSTIR DI already handles "millions of lights" — your emissive
  voxels are exactly that domain. ReSTIR GI extends it to indirect: each BRDF-sampled secondary bounce
  becomes a "light sample" and is resampled across pixels + time.
- The visibility buffer gives you `hit.voxel_coord` + normal for free — the exact G-buffer ReSTIR needs.
- SVDAG provides the ray-trace target for both the primary GI bounce *and* the validation/visibility rays.

**Practicality on consumer GPUs:**
- ReSTIR DI/GI needs **ray tracing** (or at least ray marching). Strata already marches SVDAG/XBrickMap on
  the GPU, so *visibility* is cheap; the cost is the reservoir buffers + spatial/temporal resampling passes.
- Traverse Research (2024) reports a fully ReSTIR-based DDGI system at **1440p / RTX 3080: indirect diffuse
  1.97 ms + denoiser 0.7 ms + irradiance cache 0.47 ms** — well within a frame budget.
- Key practical notes from the literature:
  - Sample the hemisphere **uniformly**, not cosine-weighted, for indirect (avoids low-prob grazing
    contributors → high variance). Traverse Research, RTXDI docs.
  - Use **blue-noise** sample patterns to maximize spatial/temporal reuse quality.
  - Clamp secondary-surface roughness to avoid sharp caustics blowing up variance (RTXDI `RestirGI.md`).
  - Reservoir buffer ≈ 1–2 vec4 per pixel; cheap.

**Recommendation:** Adopt **ReSTIR GI as the L4 (and optionally L3) indirect sampler** instead of fixed
6-cone tracing. It is the single highest-leverage change: replaces hand-tuned cone counts/apertures with a
provably-good (unbiased or controllable-bias) estimator, and reuses the SVDAG march you already have.
Pair with a small SVGF/ReLAAP-style denoiser (§6). This kills §3's "cone aperture schedule" problem entirely.

---

## 3. SVDAG / Sparse Voxel DAG Cone Tracing

**Foundations:**
- SVDAG (Kämpe et al. 2013, TOG "High Resolution Sparse Voxel DAGs") — merges isomorphic subtrees of an
  SVO into a DAG; 1–3 orders of magnitude fewer nodes than SVO. No decompression, fast traversal.
- Voxel Cone Tracing (Crassin et al. 2011, "Interactive Indirect Illumination Using Voxel Cone Tracing";
  GTC 2012 slides) — pre-filter radiance into a 3D mip pyramid, trace cones sampling the pyramid with
  quadrilinear interpolation.
- Aokana (Fang et al. 2025, PACMCGIT / arXiv:2505.02017) — confirms **multiple shallow SVDAGs** beat one
  deep SVDAG (cache-friendly, less non-contiguous pointer chasing). Strata's plan 07 already adopts this.

**Best practices for cone tracing:**
- **Cone aperture schedule:** standard Crassin approach uses ~6 cones for diffuse (1 up + 5 around at
  ~π/3 ring) with aperture `tan(α) ≈ 1/ (2·π)` growing with distance, OR a fixed aperture ~0.5–0.75 rad
  matched to cone footprint = pixel footprint. The plan's hardcoded `cone_aperture = 0.5` with
  `cone_width *= 1.5` per step is a plausible start but **must be LOD-anchored**: at SVDAG LOD `k`, the
  node size is `2^k`; set step `t += node_size` and `cone_width = aperture · t` so you sample at the mip
  level whose texel ≈ cone footprint (Crassin's quadrilinear filter). This is what avoids banding/aliasing.
- **Hierarchical LOD march:** descend the DAG; when `cone_width > node_size`, skip the subtree's interior
  and read the pre-filtered node radiance. This is the whole point of a DAG + mip pyramid — do NOT march
  64 leaf steps (the plan's `for i<64` literal is wrong; should terminate on LOD threshold).
- **Where it breaks down:**
  - **Thin geometry / leaking:** cone tracing blurs through 1-voxel walls → light bleed. Mitigate with
    opacity-weighted accumulation + visibility-aware stopping, or pair with DDGI (§5).
  - **Anisotropy:** a 3D mip pyramid pre-filters isotropically; grazing/concave corners lose energy.
  - **High-frequency emissive:** small bright voxels get averaged away at low LOD → dim indirect. ReSTIR
    (§2) samples the actual emitter, avoiding this.
  - **Edit churn:** re-baking the radiance mip pyramid on every edit is expensive. Decouple *occupancy DAG*
    (cheap, from XBrickMap) from *radiance DAG* (expensive, cached, updated on a budget).

**Recommendation:** Keep SVDAG as the *far-field ray/cone target*, but **drive it with ReSTIR GI
reservoirs** (§2) rather than a fixed cone loop. If you keep any cone tracing, fix the LOD-anchored step
(throttle by `cone_width vs node_size`, not a fixed 64) and pre-filter radiance into the SVDAG leaf bricks.
Treat the plan's `13-lighting.md §1.6` WGSL as pseudocode to be rewritten, not a spec.

---

## 4. Clustered Voxel GI / Light Clusters (the plan's L3)

**The plan (§1.5):** cluster 8×8×8 brick-quarters into ~500–1000 clusters/sector; 3D-Bresenham visibility;
`1/(1+d²)` irradiance gather, O(cluster²) pairwise.

**Is it sound?** Partially, as a *radiosity approximation*, but with serious flaws:
- **O(clusters²) is the killer.** 1000 clusters → 10⁶ pairwise visibility rays *per sector*, every 5 frames
  (`§1.3` claims <3 ms — not credible at 10⁶ Bresenham traces/sector × many sectors). Need either a
  hierarchical gather or a probe cache.
- **3D Bresenham in voxel space for inter-cluster visibility is leaky and aliased** (integer stepping, no
  cone footprint). Crassin/LPV use SH radiance + propagation, not pairwise rays, precisely to avoid this.
- **No energy conservation / no multi-bounce.** `accumulated_irradiance /= visible_count` is a normalize, not
  a transport solve — it doesn't conserve energy and over-brightens sparse configurations.
- **Better-established alternatives that ship:**
  - **DDGI** (§5) — probe grids, the modern standard.
  - **Light Propagation Volumes (LPV, Kaplanyan & Dachsbacher 2010)** — SH radiance volume + iterative
    propagation; exactly the "flood through the grid" idea but done right with spherical harmonics and
    geometry-aware pushing. Cascaded LPV extends range.
  - **Surfel-based (GIBS, EA SEED, SIGGRAPH 2021/2024)** — dynamic diffuse GI via surfels, shipped in
    EA Sports College Football 25 at 60 fps on consoles. Good voxel analog: each lit voxel face = a surfel.
  - **Clustered voxel GI (Cosin Ayerbe & Patow 2022, *Computers & Graphics*; code:
    github.com/AlejandroC1983/cvrtgi)** — literally this idea, but uses **normals per cluster + lazy
    evaluation for camera-visible voxels + SH**, not naive `1/d²`. Their result: real-time first bounce.
  - **Voxel-based GI (Thiedemann et al. 2011, I3D)** and the 2025 extension "Including reflections in
    real-time voxel-based GI" (ScienceDirect 2025) — ray-traced reflections + interreflections on a voxel
    field, higher quality + temporal stability.

**Recommendation:** Replace L3's pairwise hack with **one of:**
- **(A) DDGI probe grid per sector** (recommended — see §5), or
- **(B) SH-LPV / clustered-SH radiosity à la Cosin Ayerbe 2022** if you want a pure voxel propagation with
  no ray tracing.
Either way: store **SH (L2, 9 coeffs × RGB)** per cluster/probe, not a single irradiance vector; do iterative
propagation; cull with the sector bitmask (the plan's `LightCullingMask` is good and reusable). Drop
`1/(1+d²)` normalization.

---

## 5. DDGI (Dynamic Diffuse Global Illumination, Majercik et al. 2019 / JCGT)

**Paper:** "Dynamic Diffuse Global Illumination with Ray-Traced Irradiance Fields," *JCGT 8(2), 2019*.
(Reference open-source GLSL: jcgt.org.)

**What it is:** a grid of **light probes** (octahedral-mapped radiance + depth/visibility) updated by GPU
ray tracing, with a **visibility-aware moment-based interpolant** that kills light leaks. Extends classic
irradiance probes to dynamic scenes.

**Performance (from the paper + follow-ups):**
- ~**1.0 ms/frame diffuse GI** (RTX 2080 Ti, 1080p), including amortized BVH update + trace + gather.
- IS-DDGI (Liu et al. 2023, I3D) adds MIS ray allocation → **1.27×–2.47× faster total**, 3.29×–6.64×
  faster probe ray tracing, same quality.
- Scales 1080p→4K, no noise, no bake, works with skybox + emissive + area lights.

**Memory & perf at 32³ sector scale:**
- A DDGI probe grid is **sparse and per-sector**. For a 32³ sector, a probe spacing of 4–8 voxels → 4³–8³ =
  64–512 probes/sector. Each probe: octahedral radiance (e.g. 8×8 RG16F × 2 = ~1 KB) + visibility. So
  ~64–512 KB/sector of probe data — comparable to the plan's `13-lighting §1.2` light-data budget
  (~128–256 KB/sector), and **far more correct**. Only ACTIVE/WARM sectors need live probes.
- Probes update on a **rotating schedule** (a few sectors per frame), not all at once — fits Strata's
  streaming/tier model perfectly.

**Why it beats the plan's L3/L4 split:**
- DDGI *is* the near+mid indirect GI. It naturally handles both L3 (near, high-res probes) and feeds L4
  (distant probes coarser, or just rely on SVDAG/ReSTIR for far). It is the missing middle layer.
- It composes with ReSTIR (Traverse Research 2024 built a ReSTIR-based DDGI) and with SVDAG (use SVDAG as
  the trace target for probe rays).
- No cone aperture tuning, no pairwise O(n²), no leaking (moment-based visibility reject).

**Recommendation:** **Adopt DDGI as the canonical L3 indirect-diffuse backend**, with SVDAG (or ReSTIR over
SVDAG) as the probe-ray tracer. Probe update cadence keyed to the streaming tier (ACTIVE probes update every
N frames; WARM probes every M; DISTANT relies on coarser probes or NIV §1). This is the single biggest
correctness improvement available and it slots into the existing tier system.

---

## 6. Temporal Accumulation for Voxel Light

**The plan (§1.8):** single `mix(history, current, blend)` with a static `voxel_blend_factor`. This is
insufficient and will ghost/blur.

**What SOTA does (SVGF, NRD, Ott 2025 voxel-world TAA):**
- **Backproject via motion vectors** (camera + any voxel edit delta), fetch history at `prev_pixel` with a
  **2×2 (or 3×3 for thin features) bilinear tap**.
- **Reject history** when depth / object-id / **voxel-coord** / normal disagree (voxel worlds have *sharp
  geometric features* — Ott 2025 explicitly calls standard TAA bad for voxels and builds a
  data-driven TAA to minimize artifacts). For Strata, use the visibility-buffer `voxel_pos` + `sector_id` as
  the disocclusion key — exact, no float error.
- **Variance-guided filtering:** estimate per-pixel variance, widen the spatial kernel where variance is high
  (SVGF, Schied et al. 2017). NVIDIA's NRD library is the production reference.
- **Blend factor:** exponential moving average with **adaptive alpha** (low when history valid & static,
  high after edits/disocclusion). Clamp to avoid "boiling." Cap frame count (Unigine recommends ≤60) to
  bound lag.
- **Voxel-specific:** when a voxel is edited, invalidate its history texel (and neighbors) immediately —
  don't let EMA smear the old value. The plan's streaming `SectorLoaded/Unloaded` events (plan 08) are the
  perfect invalidation signal.

**Reference:** Ott, "Real-Time Global Illumination for Voxel Worlds," TU Wien bachelor thesis 2025
(cg.tuwien.ac.at) — specialized temporal resampling + data-driven TAA for voxel worlds; directly relevant.
Also Wisp wiki temporal-accumulation notes; NVIDIA NRD temporal docs.

**Recommendation:** Upgrade §1.8 to **voxel-keyed temporal reprojection + variance-guided spatial cleanup**
(reuse the 64-bit visibility buffer's `voxel_pos`/`sector_id` as the history key). Feed it from ReSTIR/DDGI
output. Invalidate on edit/load events.

---

## 7. GPU Compute Light Propagation vs CPU SIMD (edits, flood fill, JFA)

**L1 Block Light (CPU SIMD BFS) — keep.** The plan's `bfs_simd` at 5–52 µs/propagation is excellent and
edits are inherently CPU events (player breaks/places a block). SIMD BFS + two-phase removal is the right
call; GPU would add sync latency worse than the CPU cost. **No change.**

**Where GPU wins — sky-light occlusion / distance fields:**
- For **L2 sky light**, the expensive part on edit is re-propagating horizontal spread under overhangs.
  A **GPU jump-flooding algorithm (JFA)** distance/occlusion field per sector computes "distance to
  sky/nearest empty" in O(log n) passes (Rong & Tan 2006; Wikipedia JFA; JFA+1/JFA+2 variants fix errors).
  This is exactly the "flood fill on GPU" the question asks about, and it is the right tool for *occlusion
  fields*, not for *colored light transport*.
- **JFA practicality:** ~log2(32)=5–10 passes of a trivial compute shader per sector; trivially parallel
  over voxels; perfect for a streaming world where many sectors edit at once. No ray tracing needed.
- **CPU SIMD still wins for:** directed colored light transport (block light) where directionality + WLP
  packed arithmetic matter and edits are sparse/single-source. GPU flood loses the "only update the
  changed region" locality that two-phase removal exploits.

**Hybrid recommendation:**
- **Block light (L1):** CPU SIMD BFS, keep as-is.
- **Sky light (L2):** optionally compute a **JFA sky-occlusion/distance field on GPU** per dirty sector to
  seed the heightmap source (faster for big edits / explosions / world-gen), but the column-first BFS
  propagation can stay CPU. Benchmark before moving.
- **Indirect (L3/L4):** GPU compute (DDGI probe trace / ReSTIR) — already GPU in the plan.
- **Irradiance caching / flood:** if you keep any LPV-style propagation (§4-B), do it as **GPU iterative
  SH propagation** (LPV), not CPU.

**Reference:** Rong & Tan "Jump Flooding in GPU with Applications to Voronoi Diagram and Distance Transform"
(I3D 2006); bhavana.io ISPC-vs-compute SDF/JFA comparison (2024); Unity mesh-to-SDF compute shaders.

---

## 8. Day/Night Cycle — Analytic Sun + Sky/Atmosphere

**The plan (§23):** gradient skybox + sun/moon position + stars. Too weak for a "production" engine and
inconsistent with Strata's physically-based ambitions.

**Adopt Hillaire 2020 — "A Scalable and Production Ready Sky and Atmosphere Rendering Technique"**
(EGSR 2020; sebh.github.io/publications/egsr2020.pdf; UE Sky Atmosphere source on GitHub
`sebh/UnrealEngineSkyAtmosphere`):
- **No high-dimensional LUTs** (unlike Bruneton 2008), so no LUT artifacts; cheaper and scalable.
- **Transmittance LUT** (2D) + **Multi-scattering LUT** + **Sky-View LUT** (2D, view-dependent) + optional
  **Aerial Perspective** computed per-pixel via ray march through the transmittance LUT.
- **Multiple-scattering approximation** in real time (single param), correct sunset/sunrise (the hard case
  for brute-force PT).
- **Performance:** ~0.14 ms sky + 0.31 ms total at 1280×720 on a GTX 1080; scales to iPhone 6s (~1 ms total
  in Fortnite). LUTs updatable time-sliced over frames.
- Seamlessly switches to on-screen ray march for space/planetary views.

**Why it fits Strata:**
- The sun direction drives **L0 analytic direct light** (already in plan) *and* the sky ambient/skylight
  term for **L2**. Currently L2 sky light is a flat "15 at top" — replace the sky-source radiance with the
  atmosphere's sky-luminance integral (ether the Sky-View LUT sampled per probe/cluster normal, or a
  cheap analytic ambient from the transmittance LUT). This makes day/night actually *physically* light the
  world, not just recolor a skybox.
- Volumetric clouds / weather (plan 23) can shadow the atmosphere via the transmittance LUT (Hillaire
  supports volumetric shadowing).

**Also consider:** "Physically Based Sky, Atmosphere & Cloud Rendering in Frostbite" (Hillaire SIGGRAPH 2016)
for the clouds integration path; Bruneton 2008 only if you need extreme accuracy and can afford LUT updates.

**Recommendation:** Replace `23-environment-time-weather.md §4` sky with **Hillaire 2020**. Drive sun
direction from `DayNightCycle`, and feed the atmosphere sky-luminance into L2 sky-light radiance + L0
ambient. Keep the gradient as a fallback for low-end.

---

## 9. Concrete Recommended Changes (actionable)

| # | Change | Replaces | Effort | Impact |
|---|--------|----------|--------|--------|
| 1 | **Adopt DDGI probe grid** (SH L2, octahedral radiance, moment-based visibility) as canonical L3 indirect-diffuse. | §1.5 clustered pairwise hack | Med | Correctness: kills leak/energy/perf issues |
| 2 | **Drive L4 (and L3) indirect sampling with ReSTIR GI reservoirs** over SVDAG; trace emitter voxels as area lights. | §1.6 fixed 6-cone | Med | Quality + handles emissive blocks; reuses SVDAG march |
| 3 | **Rewrite L4 cone march** to be LOD-anchored (`step = node_size`, stop when `cone_width > node_size`), pre-filter radiance into SVDAG leaf bricks. | §1.6 WGSL | Low | Removes banding/alias; correctness |
| 4 | **Upgrade temporal accumulation** to voxel-keyed reprojection (use visibility-buffer `voxel_pos`/`sector_id`), variance-guided spatial cleanup, edit/load invalidation. | §1.8 | Med | No ghosting on edits |
| 5 | **Keep L1 CPU SIMD BFS**; optionally add **GPU JFA sky-occlusion field** to seed L2 on large edits. | §1.3/§1.4 | Low/Med | Edit perf; no regression |
| 6 | **Replace §23 skybox with Hillaire 2020 atmosphere**; feed sky-luminance into L2 + L0 ambient. | §23 §4 | Med | Physical day/night |
| 7 | **Build an SVDAG path-traced baker** (offline/async) — needed both to validate GI and to train NIV (§1.12) later. | — | High | Enables §1 + future NIV |
| 8 | **NIV as Phase-6 distant-only optional backend** behind a trait; don't let it block L3/L4. | §1.12 | High | Distant GI compression |
| 9 | **Store SH per cluster/probe**, drop single-vector `accumulated_irradiance` + `1/(1+d²)` normalize. | §1.5 | Low | Energy conservation |

---

## 10. Reference List (URLs)

- NIV 2026: https://arxiv.org/abs/2602.12949  (Eurographics 2026)
- NIV summary: https://www.emergentmind.com/topics/neural-irradiance-volume-niv
- ReSTIR GI: https://research.nvidia.com/publication/2021-06_restir-gi-path-resampling-real-time-path-tracing
- ReSTIR GI (RTXDI SDK): https://github.com/NVIDIA-RTX/RTXDI/blob/main/Doc/RestirGI.md
- ReSTIR PT Enhanced 2026: https://research.nvidia.com/labs/rtr/publication/lin2026restirptenhanced
- ReSTIR study index: https://alegruz.github.io/graphics/2025/04/22/restir-studies.html
- SVDAG 2013: https://dl.acm.org/doi/10.1145/2461912.2462024
- Transform-Aware SVDAG 2025: https://dl.acm.org/doi/10.1145/3728301
- Voxel Cone Tracing (Crassin GTC2012): https://developer.download.nvidia.com/GTC/PDF/GTC2012/PresentationPDF/SB134-Voxel-Cone-Tracing-Octree-Real-Time-Illumination.pdf
- Aokana 2025: https://arxiv.org/abs/2505.02017  (PACMCGIT 8(1))
- Clustered Voxel GI 2022: https://doi.org/10.1016/j.cag.2022.01.005  (code: github.com/AlejandroC1983/cvrtgi)
- LPV (Kaplanyan & Dachsbacher 2010): https://www.advances.realtimerendering.com/s2009/Light_Propagation_Volumes.pdf
- DDGI 2019 (JCGT): https://www.jcgt.org/published/0008/02/01/  (GLSL reference impl)
- IS-DDGI 2023: https://allenliuzihao.github.io/IS-DDGI
- EA GIBS / SEED DDGI ship: https://www.ea.com/seed/news/siggraph21-global-illumination-surfels
- Traverse Research ReSTIR DDGI: https://blog.traverseresearch.nl/dynamic-diffuse-global-illumination-b56dc0525a0a
- Voxel-world TAA (Ott 2025, TU Wien): https://www.cg.tuwien.ac.at/research/publications/2025/ott-rgi/
- SVGF (Schied et al. 2017): https://www.highperformancegraphics.org/wp-content/uploads/2017/Papers-Session1/HPG2017_SpatiotemporalVarianceGuidedFiltering.pdf
- NVIDIA NRD: https://github.com/NVIDIA-RTX/NRD
- JFA (Rong & Tan 2006): https://www.comp.nus.edu.sg/~tants/jfa/i3d06-submitted.pdf ; Wikipedia: https://en.wikipedia.org/wiki/Jump_flooding_algorithm
- Hillaire 2020 Sky/Atmosphere: https://sebh.github.io/publications/egsr2020.pdf ; code: https://github.com/sebh/UnrealEngineSkyAtmosphere
- Bruneton Precomputed Atmospheric Scattering (alt): https://ebruneton.github.io/precomputed_atmospheric_scattering/
- WebGPU Hillaire impl: https://github.com/JolifantoBambla/webgpu-sky-atmosphere

---

## 11. Consistency Note vs Strata Constitution

All recommendations above are **additive** to plans 01–12 (XBrickMap, SVDAG, streaming tiers, visibility
buffer). They do not require architectural changes to the render pipeline (plan 10) — they plug into the
existing L3/L4 passes and the visibility buffer. The only plan-13-internal change is replacing the L3
pairwise cluster gather (§1.5) and the fixed-cone L4 (§1.6) with DDGI + ReSTIR, which is within plan 13's
scope. Plan 23's skybox (§4) should reference Hillaire 2020. No conflict with 01–12.
