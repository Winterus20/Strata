# Chunk-Boundary Bug Investigation

**Date:** 2026-07-19
**Scope:** Read-only root cause analysis. No code edits.
**Method:** Manual read-through of `crates/strata_render/src/pipeline/{visbuf,prepass,resolve,lightmap,mod}.rs`, `crates/strata_render/src/meshing/{greedy,packed_quad,ecs}.rs`, `crates/strata_player/src/interaction.rs`, `crates/strata_world/src/lighting/mod.rs`, `crates/strata_world/src/streaming/{mod.rs,tests.rs}`, and the integration wiring in `bin/client/src/client_render.rs`. The previous "fix" attempt referenced in the user's screenshots was a visbuf v2 layout bump (4-bit `block_id` + 4-bit `sector_id`) plus the M10a.2 scalar `face_tint` rewrite in the resolve shader.

## Bug 1: Komşu blok rengi aniden açılıyor (neighbour blocks lighten after a break)

### Root cause
The renderer only ever uploads a lightmap for **one** sector per frame — the "focus sector" picked by `client_render_system` in `bin/client/src/client_render.rs:525-553`. The focus sector is whichever `visible_idx[i]` happens to be iterated first, and its `SectorLight` is flattened into a 32 KB SSBO (`upload_lightmap` → `lightmap` at `crates/strata_render/src/pipeline/mod.rs:1383`).

The resolve shader (`crates/strata_render/src/pipeline/resolve.rs:350-365`) reads that single SSBO using `quad_id & lightmap_mask`. Quads from *other* sectors in the same frame point at the same SSBO but a different (and now-stale) range of `quad_id` bytes — they read whatever the focus sector last uploaded, which is byte-for-byte wrong for those sectors. So when a break happens, the focus sector's `SectorLight` is recomputed (`lighting_system` only re-runs the sector that was marked `ChunkDirty` — see `crates/strata_world/src/lighting/mod.rs:430-489`), uploaded, and the resolve pass picks up the *focus sector's* new light for every pixel, including those that belong to neighbouring sectors. The neighbour blocks "lighten" because they suddenly index into the focus sector's just-uploaded `lightmap[quad_id]` byte rather than their own.

There is no cross-sector light propagation in `LightEngine` either: `compute_sector` (`crates/strata_world/src/lighting/mod.rs:187-200`) and `apply_edit` (lines 202-213) are documented as self-contained — sky light is column-only inside one 32³ sector, block light propagation stops at sector boundaries, and the neighbour that was previously shadowed is never re-baked even if the cast shadow was removed.

### Why previous fix didn't work
The previous attempt touched the visbuf v2 bit layout (`VisBufEntry::pack` + `prepass.rs` WGSL) and replaced the per-channel `face_tint` with the scalar `0.85*up + 0.45*down + 0.95*side` formula (`crates/strata_render/src/pipeline/resolve.rs:339-343`, tested in `face_tint_scalar_directions`). Both are correct in isolation — the visbuf `block_id` mask and `face_tint` math were independently verified and have unit tests (`test_visbuf_field_boundaries_no_bleed`, `face_tint_scalar_directions`). But neither has anything to do with the per-sector lightmap model. The lightmap SSBO is only ever populated for one sector per frame, so the tint formula only modulates whatever stale byte is sitting in the lightmap at the neighbouring quad's `quad_id` offset — the bug is upstream of the tint. The previous "fix" was a red herring.

A second contributing factor: when a neighbour sector hasn't been marked `ChunkDirty` by the player's break (`crates/strata_player/src/interaction.rs:151` only inserts `ChunkDirty` + `NeedsRemesh` for the edited sector's entity, and `lighting_system` only runs the recompute on `Added<ChunkDirty>`), its `SectorLight` is stale even for the moment of the break. The current focus sector is fine; the *other* visible sectors are the ones displaying wrong light.

### Minimal fix sketch
- **Make the lightmap cover the whole visible set, not a single focus sector.** Either:
  1. Allocate `SECTOR_LIGHTMAP_QUADS * visible_sector_count` bytes and append each sector's `SectorLight` bytes (in `quad_id` order, which already matches because the prepass writes `quad_id` as a per-quad index in the sector-local mesh order) — and compute a per-sector base offset in the WGSL `quad_id + sector_base_offset`, or
  2. Maintain a per-sector lightmap SSBO array (one binding per slot, up to the AOI cap) and index via `sector_id` decoded from the visbuf.
- **Mark neighbour sectors dirty on the edit.** After a `PlayerBreak`/`PlayerPlace`, also insert `ChunkDirty` (and `NeedsRemesh`) on the 6 neighbours of `hit.sector_coord` (if resident) so `lighting_system` re-bakes them and they appear in the lightmap upload. Care needed for the case where the broken block is *not* on a sector boundary (no-op) — gate by `hit.voxel` sitting on the local boundary plane.
- **Cross-sector light propagation (real fix, longer term).** `LightEngine::compute_sky/compute_block` need a second pass that bleeds sky/block light across sector boundaries the way `plans/13-lighting.md` already describes (one-step extrapolation at the seam, then mark the affected neighbour dirty). Until that lands, even re-baking neighbours with self-contained `compute_sector` is only a partial fix — boundary voxels still won't see the cast-shadow removal of a block on the other side.
- **Stop using the focus-sector model entirely** in `client_render` once the per-sector lightmap is in place, or at least stop sampling the focus sector's bytes for non-focus quads.

## Bug 2: Kırılan bloğun arkası renderlanmıyor (hole behind the broken block)

### Root cause
`player_break_system` (`crates/strata_player/src/interaction.rs:105-155`) only inserts `ChunkDirty` + `NeedsRemesh` on the entity whose `SectorCoord` matches the player's sector. When the broken block sits on a *chunk boundary* (i.e. the broken voxel is at the local `0` or `31` plane), the **neighbouring** sector owns the now-exposed face. The meshing system never re-runs on the neighbour, so the neighbour's mesh still culls the face that previously faced a solid voxel (see `face_visible` in `crates/strata_render/src/meshing/greedy.rs:341-357` — both opaque → `false`, face is dropped). The result is a one-voxel-thick hole at the chunk boundary.

The remesh-on-load pass in `crates/strata_render/src/meshing/ecs.rs:381-407` is the only mechanism in the codebase that ever marks a *neighbour* dirty because of an edit in the current sector, but it is gated on `mt.present_mask` having a missing back-bit — i.e. it only fires for neighbours that were *unloaded* at the time the current sector was meshed, not for the edit-time direction. It does not help when the current sector is edited.

The constitution plan `plans/09-meshing.md` explicitly calls this out: "`NeedsRemesh` komşu bleed | Edit sonrası 6 komşu sektör de işaretlenir." The plan mandates the bleed; the implementation in `player_break_system`/`player_place_system` does not do it.

### Why previous fix didn't work
Same as Bug 1 — the previous patch only changed the visbuf bit layout and the resolve-shader tint. The actual hole is the missing neighbour remesh, which is owned by the player-side edit path. The mesher itself is correct given the right inputs: `GreedyMesher::mesh_sector_planes` (lines 274-300) builds neighbour boundary planes, calls `face_visible` correctly, and the WGSL atomicMax in the prepass is a strict nearest-wins (so the neighbour's would-be face would be drawn if the neighbour were re-meshed). None of that code is wrong; the data feeding it is stale.

The "dark spots" visible in image 1 (the `1-voxel gaps` interpretation) are consistent with neighbour-face culling staying on for boundary voxels while the focus-sector's own face is gone, leaving a single-voxel-wide band of unrendered space — exactly what would happen if the neighbour mesh is one edit behind.

### Minimal fix sketch
- **Insert `ChunkDirty` + `NeedsRemesh` on the 6 neighbours** of `hit.sector_coord` whenever the broken/placed voxel is on a sector boundary plane (`hit.voxel.x ∈ {0, 31}` → mark `±X` neighbours; same for Y and Z). Use `commands.entity(ne).insert(NeedsRemesh)` for each resident neighbour — `chunk_storage` already exposes the entity lookup via `StreamingManager::entity_for`. Skip neighbours that aren't resident (streaming will rebuild them with the right masks when they load back).
- **Add a meshing test that asserts neighbour re-mesh** when a boundary voxel is broken — see Verifiability below.
- (Optional) **Trigger the same neighbour bleed inside `apply_mesh_tasks`'s remesh-on-load path** so that if the neighbour is missing, the current sector at least re-runs on edit-time, not just load-time.

## Verifiability

The investigation found two specific tests that would catch a regression of each bug once a real fix lands:

1. **Neighbour remesh on boundary break (Bug 2).** Add a headless test in `crates/strata_world/src/streaming/tests.rs` (or a new `crates/strata_render/src/meshing/tests.rs` test) that:
   - spawns two adjacent sectors with one shared boundary plane filled with opaque blocks,
   - injects a `PlayerBreak` whose hit voxel lies on the boundary (`voxel.x == 0`),
   - runs the player + streaming + meshing systems for two frames,
   - asserts the *neighbour* entity has had `NeedsRemesh` inserted and the new mesh contains a quad facing into the broken space. Today this test would fail because `player_break_system` only marks the edited sector.

2. **Neighbour lightmap re-bake (Bug 1).** Add a test that:
   - spawns two adjacent sectors, one with a tall opaque column at the boundary that casts a vertical shadow column in the neighbour's `SectorLight`,
   - triggers a `PlayerBreak` that removes the topmost block of the column,
   - asserts that the neighbour's `SectorLight` at the now-illuminated column voxels is brighter (sky light increased) than before the break. Today this would fail because `lighting_system` skips the neighbour — `Added<ChunkDirty>` is not set on it.

3. **Multi-sector lightmap upload.** A renderer-level test in `bin/client/src/client_render.rs` (or a unit test in `crates/strata_render/src/pipeline/mod.rs`) that:
   - builds two visible sectors with distinct quads and lights,
   - drives one frame of `client_render_system`,
   - asserts the lightmap SSBO contains the bytes from *both* sectors, not just `focus_sector`. Today the only upload is the focus sector's bytes, so the neighbour bytes are stale.

4. **`face_tint` regression sentinel.** The `face_tint_scalar_directions` test in `crates/strata_render/src/pipeline/resolve.rs:596` already pins the previous fix; leave it as-is. It will not catch Bug 1 or Bug 2, but it should not regress when the real fix lands.

## Risk assessment

- **Performance:** Marking the 6 neighbours dirty on every boundary edit will trigger up to 6 extra `lighting_system` recomputes and 6 extra `spawn_mesh_tasks` per break/place. `compute_sector` is a full per-sector pass (`crates/strata_world/src/lighting/mod.rs:187-200`) and `mesh_sector_planes` is greedy O(sector_volume) with a 64 KB snapshot. With a budget of `MESH_BUDGET` / `LIGHTING_BUDGET` per frame, a 6× fan-out is acceptable for single-block edits but would need to be deferred (rate-limited or batched) if a future "explode N blocks" tool is added. The existing rate-limits already in `spawn_mesh_tasks` (lines 200-240) and `lighting_system` (lines 471-486) will absorb the fan-out.
- **Multi-sector lightmap memory:** Going from a 32 KB focus-sector lightmap to a per-AOI lightmap grows upload to `~32 KB * visible_sector_count` per frame. The current 4-shell AOI is 729 sectors in the full worst case, but realistic culling keeps it under ~20-40; ~1.3 MB worst case vs the current 32 KB. PCIe bandwidth impact is dominated by the 8-byte `PackedQuadGpu` upload, not the lightmap, so this is acceptable.
- **Cross-sector light propagation:** Adding a true propagation pass (sky/block light bleeding across sector boundaries) is a much larger change with its own consistency story. Recommend deferring past the immediate fix — re-baking the neighbour sector with its self-contained `compute_sector` is enough to clear the visible neighbour light; the boundary-voxel seam still loses one step of bleed, but that is a pre-existing M7 limitation, not a regression.
- **Stale focus-sector model:** Replacing the single `upload_lightmap(focus_sector)` with a per-AOI lightmap means the renderer can no longer rely on "the bytes at `quad_id`" being a single coherent sector. Anything else in the codebase that still reads `lightmap[quad_id]` as a sector-local lookup (search shows only `resolve.rs:357`) must be updated to either pass a base offset or use the visbuf's `sector_id` field as an index.
- **No additional dependencies needed.** All required primitives (`ChunkDirty`, `NeedsRemesh`, `SectorCoord`, `streaming.entity_for`) are already wired in.
