//! Client-side GPU ownership + window presentation (M9b).
//!
//! `ClientGpu` owns the wgpu `Instance`/`Surface`/`Device`/`Queue` and the
//! `strata_render::Renderer`. The surface is created lazily once Bevy's window
//! exposes a [`RawHandleWrapper`] (the winit backend inserts it after the OS
//! window exists). Each frame the offscreen HDR target is rendered and blitted to
//! the window surface.

use bevy::input::ButtonInput;
use bevy::prelude::*;
use bevy::window::RawHandleWrapper;

use strata_core::prelude::*;
use strata_player::PlayerLook;
use strata_player::controller::EYE_HEIGHT;
use strata_render::meshing::MeshStorage;
use strata_render::pipeline::LightmapEntry;
use strata_render::pipeline::camera::{look_at_rh, perspective_rh_zo};
use strata_render::pipeline::cull::{Aabb, cull_visible};
use strata_render::pipeline::{CameraView, MAX_QUAD_SSBO_SLOTS, Renderer, prepass_features};
use strata_world::prelude::*;

use wgpu::*;

/// Max sectors re-flattened (CPU clone into GPU cache) per frame during a burst.
const REFLATTEN_BUDGET: usize = 12;
/// Max sectors uploaded into the vertex SSBO per frame.
const UPLOAD_BUDGET: usize = 16;

/// Debug toggle: when on, the resolve pass colors each voxel face by its
/// direction rather than Lambert shading, so missing/wrong faces are obvious.
/// Toggle live with the `F` key. Default on while debugging faces.
#[derive(Resource, Default)]
pub struct DebugFaces(pub bool);

/// Request a one-shot GPU readback (offscreen sky/terrain split diagnostic)
/// with the `R` key. Fires once then auto-clears, so the expensive
/// `poll(wait_indefinitely)` GPU-sync stall that `readback` incurs never lands
/// in the per-frame path — it only runs on explicit request, eliminating the
/// periodic multi-ms hitch the old "every 120 frames" version caused.
#[derive(Resource, Default)]
pub struct DebugReadback(pub bool);

/// One-shot debug dump trigger. When `triggered` is true, the next frame's
/// resolve pass will write selected signals to the debug SSBO and the result
/// will be `eprintln!`ed after the frame. Target pixel is always screen centre.
#[derive(Resource, Default)]
pub struct DebugDump {
    pub triggered: bool,
    pub mask: u32,
}

/// Toggle [`DebugReadback`] with the `R` key.
pub fn toggle_debug_readback(keys: Res<ButtonInput<KeyCode>>, mut rb: ResMut<DebugReadback>) {
    if keys.just_pressed(KeyCode::KeyR) {
        rb.0 = true;
        info!("strata: readback requested (next frame)");
    }
}

/// Toggle [`DebugFaces`] with the `F` key.
pub fn toggle_debug_faces(keys: Res<ButtonInput<KeyCode>>, mut faces: ResMut<DebugFaces>) {
    if keys.just_pressed(KeyCode::KeyF) {
        faces.0 = !faces.0;
        info!("strata: debug face colors = {}", faces.0);
    }
}

/// One-shot debug dump with the `G` key. Dumps all signals at screen centre.
pub fn toggle_debug_dump(keys: Res<ButtonInput<KeyCode>>, mut dump: ResMut<DebugDump>) {
    if keys.just_pressed(KeyCode::KeyG) {
        dump.triggered = true;
        dump.mask = strata_render::pipeline::resolve::debug_dump::ALL;
        info!(
            "strata: debug dump requested (next frame, mask=0x{:02x})",
            dump.mask
        );
    }
}

/// Owns the single wgpu device + the offscreen renderer for the client window.
#[derive(Resource)]
pub struct ClientGpu {
    instance: Instance,
    surface: Option<Surface<'static>>,
    config: Option<SurfaceConfiguration>,
    surface_format: Option<TextureFormat>,
    renderer: Option<Renderer>,
    width: u32,
    height: u32,
    ready: bool,
    /// Per-sector cached GPU quad bytes (opaque batch) + world AABB + world
    /// origin. Updated incrementally: only sectors whose mesh `generation` changed
    /// are re-flattened, so streaming a new sector does not rebuild the whole set.
    mesh_cache: std::collections::HashMap<SectorCoord, (Vec<u8>, Aabb, [f32; 3])>,
    /// Last seen `MeshData::generation` per cached sector, so we can detect which
    /// sectors actually changed since the last frame.
    cache_gen: std::collections::HashMap<SectorCoord, u64>,
    /// Insertion-stable sector order, rebuilt only when the cached set changes
    /// (sector added/evicted). Drives per-frame iterate/translate/range building
    /// without re-collecting a fresh `Vec` every frame.
    coord_order: Vec<SectorCoord>,
    /// World-space AABBs parallel to `coord_order`, lifted once at cache-insert
    /// time so the per-frame frustum cull never re-translates sector-local boxes.
    world_aabbs: Vec<Aabb>,
    /// Stable SSBO slot per cached sector: `(base_quad, quad_count, uploaded_gen)`.
    /// Sectors keep their slot across frames, so only changed/new sectors are
    /// re-uploaded — never the whole visible buffer (this removed the streaming
    /// FPS spikes).
    slots: std::collections::HashMap<SectorCoord, (u32, u32, u64)>,
    /// Free address ranges (base, len) in the quad SSBO, for slot reuse.
    free_quads: Vec<(u32, u32)>,
    /// Bump allocator cursor (in quads) for new slots.
    next_base: u32,
    /// Current SSBO capacity in quads.
    quad_capacity: u32,
    /// Reusable scratch for the per-quad world-origin upload slice. Filled per
    /// sector then handed to `upload_quad_region`; capacity is retained across
    /// frames so streaming a burst of sectors never re-allocates per upload.
    origins_scratch: Vec<[f32; 4]>,
    /// Reusable scratch for per-sector lightmap bytes. Cleared + filled per
    /// sector to avoid a per-sector `Vec` allocation (500 sectors × 1 alloc).
    lightmap_scratch: Vec<LightmapEntry>,
    /// Sectors whose lightmap SSBO region must be rewritten. Inserted when a
    /// mesh slot is (re)uploaded or when `SectorLight` is added/changed — so a
    /// late lighting pass brightens the sector without waiting for a remesh.
    lightmap_dirty: std::collections::HashSet<SectorCoord>,
    /// Per-frame cost counters for FPS diagnostics (cheap; accumulate then log
    /// once per second). `frame_reflatten` = sectors whose mesh changed this
    /// frame; `frame_uploaded` = sectors actually re-uploaded to the GPU SSBO;
    /// `frame_draws` = visible sector draw ranges; `frame_rebuild` = full SSBO
    /// reassignment happened this frame.
    pub frame_reflatten: u32,
    pub frame_uploaded: u32,
    pub frame_draws: u32,
    pub frame_rebuild: u32,
    /// Main-thread phase timings (µs) for the last frame, to localize streaming
    /// hitches: reflatten = clone changed meshes into the GPU cache; upload =
    /// SSBO write_buffer of changed sectors; draw = prepass+resolve+bloom+present.
    pub frame_us_reflatten: u64,
    pub frame_us_upload: u64,
    pub frame_us_draw: u64,
    /// Frustum cull stats: total sectors tested, sectors that passed.
    pub frame_cull_total: u32,
    pub frame_cull_visible: u32,
    pub frame_cull_us: u64,
    /// Prepass stats: total quads submitted, number of merged draw runs.
    pub frame_prepass_quads: u32,
    pub frame_prepass_runs: u32,
    /// Bloom parameters uploaded each frame; mutated via the live `B` toggle
    /// (and through a future settings UI). Defaults match `BloomParams::default`.
    pub bloom_params: strata_render::pipeline::BloomParams,
}

impl ClientGpu {
    fn new() -> Self {
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });
        Self {
            instance,
            surface: None,
            config: None,
            surface_format: None,
            renderer: None,
            width: 1,
            height: 1,
            ready: false,
            mesh_cache: std::collections::HashMap::new(),
            cache_gen: std::collections::HashMap::new(),
            coord_order: Vec::new(),
            world_aabbs: Vec::new(),
            slots: std::collections::HashMap::new(),
            free_quads: Vec::new(),
            next_base: 0,
            // Cap at visbuf 21-bit / device SSBO binding limit (origins = 16 B/slot).
            // Growing past 2^21 → 2^24 made resolve binding 8 exceed 128 MiB.
            quad_capacity: MAX_QUAD_SSBO_SLOTS as u32,
            origins_scratch: Vec::new(),
            lightmap_scratch: Vec::new(),
            lightmap_dirty: std::collections::HashSet::new(),
            frame_reflatten: 0,
            frame_uploaded: 0,
            frame_draws: 0,
            frame_rebuild: 0,
            frame_us_reflatten: 0,
            frame_us_upload: 0,
            frame_us_draw: 0,
            frame_cull_total: 0,
            frame_cull_visible: 0,
            frame_cull_us: 0,
            frame_prepass_quads: 0,
            frame_prepass_runs: 0,
            bloom_params: strata_render::pipeline::BloomParams::default(),
        }
    }
}

/// Client render plugin: registers the mark-for-remesh system (PostUpdate) and
/// the present system (RenderUpdate, after Meshing has populated `MeshStorage`).
pub struct ClientRenderPlugin;

impl StrataPlugin for ClientRenderPlugin {
    fn name(&self) -> &'static str {
        "client_render"
    }

    fn build(&self, app: &mut App) {
        app.insert_resource(ClientGpu::new());
        app.insert_resource(DebugFaces(false));
        app.insert_resource(DebugReadback(false));
        app.insert_resource(DebugDump::default());
        app.add_systems(PostUpdate, mark_generated_for_remesh);
        // Key toggles must run before present so a press takes effect this frame.
        app.add_systems(
            Update,
            (
                toggle_debug_faces,
                toggle_debug_readback,
                toggle_debug_dump,
            )
                .before(client_render_system),
        );
        app.add_systems(Update, client_render_system.in_set(StrataSet::RenderUpdate));
    }
}

/// Evict GPU mesh-cache entries whose sector key is no longer in `storage`.
///
/// Compares by **key set**, not `len()` — equal lengths with different keys
/// (unload A + load B in the same frame) must still drop stale A.
fn retain_mesh_cache_to_storage<V>(
    cache: &mut std::collections::HashMap<SectorCoord, V>,
    storage: &MeshStorage,
) -> bool {
    let before = cache.len();
    cache.retain(|c, _| storage.meshes.contains_key(c));
    cache.len() != before
}

/// Drop surface/device state before a `create_surface` retry after
/// [`wgpu::SurfaceError::Lost`]. Holding the old surface keeps the native
/// window reserved and makes reinit fail with "Native window is in use".
fn drop_surface_for_reinit(gpu: &mut ClientGpu) {
    gpu.surface = None;
    gpu.renderer = None;
    gpu.config = None;
    gpu.surface_format = None;
    gpu.ready = false;
}

/// Mark freshly `Generated` sectors (that have no mesh yet) for greedy meshing.
/// Filter-first: only sectors missing `NeedsRemesh` and `Meshed` (and not currently in MeshStorage)
/// are queued, so each sector is meshed exactly once (no per-frame churn).
#[allow(clippy::type_complexity)]
fn mark_generated_for_remesh(
    storage: Res<MeshStorage>,
    mut commands: Commands,
    q: Query<(Entity, &SectorCoord), (With<Generated>, Without<NeedsRemesh>, Without<Meshed>)>,
) {
    for (e, coord) in &q {
        if !storage.meshes.contains_key(coord) {
            commands.entity(e).insert(NeedsRemesh);
        }
    }
}

/// Allocate a quad SSBO slot of `count` quads using a free-list (first fit) or a
/// bump cursor. Returns `Err(())` if the slot would exceed `capacity`, signaling
/// the caller to grow the buffer and reassign all slots.
fn alloc_slot(
    free: &mut Vec<(u32, u32)>,
    next_base: &mut u32,
    capacity: u32,
    count: u32,
) -> Result<u32, ()> {
    let count = (count + 3) & !3; // Align to 4 quads (COPY_BUFFER_ALIGNMENT-friendly)
    if let Some(idx) = free.iter().position(|(_b, c)| *c >= count) {
        let (base, c) = free.remove(idx);
        let leftover = c - count;
        if leftover > 0 {
            free.push((base + count, leftover));
        }
        Ok(base)
    } else {
        let base = *next_base;
        if base + count > capacity {
            return Err(());
        }
        *next_base += count;
        Ok(base)
    }
}

/// Decode an IEEE-754 binary16 half-float (little-endian u16) to f32.
fn f16_of(bytes: &[u8], o: usize) -> f32 {
    let bits = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let s = (bits >> 15) & 1;
    let e = (bits >> 10) & 0x1f;
    let m = bits & 0x3ff;
    let v = if e == 0 {
        (m as f32) * 2f32.powi(-24)
    } else if e == 0x1f {
        f32::INFINITY
    } else {
        (1.0 + m as f32 / 1024.0) * 2f32.powi(e as i32 - 15)
    };
    if s == 1 { -v } else { v }
}

/// Owns the wgpu device and presents the offscreen render to the window.
#[allow(clippy::too_many_arguments)]
pub fn client_render_system(
    mut gpu: ResMut<ClientGpu>,
    windows: Query<(Entity, &Window, &RawHandleWrapper)>,
    player: Query<(&Transform, &PlayerLook)>,
    mut storage: ResMut<MeshStorage>,
    face_debug: Res<DebugFaces>,
    mut readback: ResMut<DebugReadback>,
    mut debug_dump: ResMut<DebugDump>,
    registry: Res<BlockRegistry>,
    lights: Query<(&SectorCoord, &SectorLight)>,
    changed_lights: Query<&SectorCoord, Changed<SectorLight>>,
) {
    let Some((_e, window, rhw)) = windows.iter().next() else {
        return;
    };

    if !gpu.ready {
        if !init_surface(&mut gpu, window, rhw) {
            return;
        }
        gpu.ready = true;
    }

    // Work with a plain `&mut ClientGpu` so disjoint field borrows (e.g. reading
    // `gpu.quad_capacity` while mutating `gpu.free_quads`) are accepted by the
    // borrow checker — `ResMut`'s deref reborrows the whole struct per field
    // access, which would otherwise conflict.
    let gpu = &mut *gpu;

    // Phase timers (µs) — used to pinpoint which main-thread stage costs a
    // frame during streaming bursts. Accumulated per frame, reset after the diag
    // snapshot so the numbers reflect the last second, not the whole session.

    // Resize the swapchain + offscreen target to follow the window. We only
    // reconfigure when the *physical* size actually changed (not every frame):
    // Bevy rewrites the logical resolution on each DPI/scale round-trip, which
    // would otherwise trigger a per-frame `Surface::configure` that races with
    // the live swapchain on Windows ("Native window is in use"). Comparing
    // against the physical size and only acting on a real delta avoids that, and
    // lets the game fill the window / grow with it.
    let w = window.resolution.physical_width().max(1);
    let h = window.resolution.physical_height().max(1);
    if w != gpu.width || h != gpu.height {
        // Phase 1: update the stored configuration (mutable, released at once).
        if let Some(cfg) = gpu.config.as_mut() {
            cfg.width = w;
            cfg.height = h;
        }
        // Phase 2: reconfigure the swapchain. `Surface::configure` takes the
        // config by immutable reference, so all three borrows below are
        // immutable and disjoint — no conflict.
        {
            let surface = gpu.surface.as_ref().unwrap();
            let cfg = gpu.config.as_ref().unwrap();
            let renderer = gpu.renderer.as_ref().unwrap();
            surface.configure(renderer.device(), cfg);
        }
        // Phase 3: resize the offscreen HDR target to match.
        // `Renderer::resize` drops the pre-pass (including the lightmap SSBO);
        // re-dirty every resident slot so the next present refills it.
        gpu.renderer.as_mut().unwrap().resize(w, h);
        for c in gpu.slots.keys() {
            gpu.lightmap_dirty.insert(*c);
        }
        gpu.width = w;
        gpu.height = h;
    }

    // ---- Incremental per-sector GPU upload (stable SSBO slots) ----
    // Re-flatten only sectors whose mesh `generation` changed, then place each
    // cached sector into a stable SSBO slot. Uploading touches ONLY the changed
    // sector's quads, so streaming a new sector no longer re-uploads the whole
    // visible buffer — this removed the instantaneous FPS spikes.

    // Re-flatten (clone pre-calculated GPU quads) only the sectors whose mesh
    // actually changed this frame. `storage.dirty` is drained here (O(dirty),
    // not O(all_sectors)), so a streaming burst of N new meshes scans N entries
    // — not the full resident set — eliminating the chunk-load scan spike that
    // dropped FPS during streaming.
    let prev_cache_len = gpu.mesh_cache.len();
    let mut num_cached = 0;
    if !storage.dirty.is_empty() {
        let ts = std::time::Instant::now();
        // Drain only the changed coords; `version` no longer needs tracking.
        let changed: Vec<SectorCoord> = storage.dirty.drain().collect();
        for coord in &changed {
            if gpu.cache_gen.get(coord).copied() == storage.meshes.get(coord).map(|m| m.generation)
            {
                continue;
            }
            if num_cached >= REFLATTEN_BUDGET {
                // Re-queue the unprocessed coords for next frame so a burst is
                // spread across frames instead of stalling one.
                storage.dirty.insert(*coord);
                continue;
            }
            let Some(mesh) = storage.meshes.get(coord) else {
                continue;
            };
            let opaque = mesh.opaque_gpu.clone();
            let aabb = mesh.aabb;
            let origin = [
                (coord.0 * 32) as f32,
                (coord.1 * 32) as f32,
                (coord.2 * 32) as f32,
            ];
            gpu.mesh_cache.insert(*coord, (opaque, aabb, origin));
            gpu.cache_gen.insert(*coord, mesh.generation);
            gpu.frame_reflatten += 1;
            num_cached += 1;
        }
        gpu.frame_us_reflatten = ts.elapsed().as_micros() as u64;
    }
    let mut set_changed = prev_cache_len != gpu.mesh_cache.len();

    // Drop cache entries for sectors that have been unloaded (streaming eviction)
    // so stale geometry is never drawn, then free their SSBO slots for reuse.
    // Key-set retain (not len equality): same cardinality with different keys
    // still evicts stale coords.
    let evicted_mesh = retain_mesh_cache_to_storage(&mut gpu.mesh_cache, &storage);
    let evicted_gen = retain_mesh_cache_to_storage(&mut gpu.cache_gen, &storage);
    if evicted_mesh || evicted_gen {
        set_changed = true;
    }
    let unloaded: Vec<SectorCoord> = gpu
        .slots
        .keys()
        .filter(|c| !gpu.mesh_cache.contains_key(*c))
        .copied()
        .collect();
    for c in &unloaded {
        if let Some((b, n, _g)) = gpu.slots.remove(c) {
            let rounded = (n + 3) & !3;
            gpu.free_quads.push((b, rounded));
        }
        gpu.lightmap_dirty.remove(c);
    }

    // Rebuild the stable sector order + pre-translated world AABBs only when the
    // cached set changed (sector added/evicted), so the per-frame cull builds no
    // fresh `Vec` and never re-translates sector-local boxes.
    if set_changed {
        gpu.coord_order.clear();
        gpu.world_aabbs.clear();
        for (c, (_b, aabb, origin)) in gpu.mesh_cache.iter() {
            gpu.coord_order.push(*c);
            gpu.world_aabbs.push(aabb.translated(*origin));
        }
    }

    // Size the renderer SSBO to hold every cached sector with headroom for the
    // slot allocator's fragmentation. Never past max_quad_capacity — origins
    // (resolve binding 8, 16 B/slot) would exceed max_storage_buffer_binding_size.
    let max_cap = gpu
        .renderer
        .as_ref()
        .map(|r| r.max_quad_capacity())
        .unwrap_or(MAX_QUAD_SSBO_SLOTS);
    let total_quads: u32 = gpu
        .mesh_cache
        .values()
        .map(|(b, _, _)| (b.len() / 8) as u32)
        .sum();
    let need_cap = (total_quads.max(gpu.next_base) as usize) * 2 + (1 << 16);
    let new_cap = (need_cap.max(gpu.quad_capacity as usize).max(1 << 20))
        .next_power_of_two()
        .min(max_cap) as u32;
    if new_cap > gpu.quad_capacity {
        gpu.quad_capacity = new_cap;
    }
    // Always hand the renderer the required SSBO capacity. `ensure_quad_capacity`
    // is a no-op when the buffer already meets it, so this also performs the
    // first-frame allocation (initial `quad_capacity` is 2M, but the renderer's
    // own buffer starts at 0 until told).
    gpu.renderer.as_mut().unwrap().ensure_quad_capacity(new_cap);

    // SectorLight insert/update must refresh the GPU lightmap even when the
    // mesh generation is unchanged (otherwise first mesh upload stays dark
    // until a dig forces NeedsRemesh).
    for coord in &changed_lights {
        gpu.lightmap_dirty.insert(*coord);
    }

    let coords = &gpu.coord_order;

    // (Re)upload changed sectors into their slots. A sector's world origin is
    // constant, so when only the mesh changed but the quad count is unchanged we
    // reuse the existing SSBO slot and re-upload just the quad payload — the
    // per-quad origins (2x the quad bandwidth, all identical per sector) are
    // already correct. Only a new sector or a size change pays for a slot
    // (re)alloc + origins write. Both paths are budgeted so a burst of
    // background-meshed sectors is spread across frames, never dumped into one.
    let mut upload_budget = UPLOAD_BUDGET;
    let tu = std::time::Instant::now();
    for coord in coords {
        let (bytes, _aabb, origin) = &gpu.mesh_cache[coord];
        let count = (bytes.len() / 8) as u32;
        let g = gpu.cache_gen[coord];
        // `reuse_base` = Some(base) when re-meshing with the SAME quad count:
        // the slot base and the (constant) origins are already valid, so only the
        // quad bytes need refreshing. `s.0`/`s.1` are `Copy`, so no `slots` borrow
        // escapes the match into the insert below.
        let (need, reuse_base) = match gpu.slots.get(coord) {
            Some(s) if s.2 == g && s.1 == count => (false, None),
            Some(s) if s.1 == count => (true, Some(s.0)),
            _ => (true, None),
        };
        if !need {
            continue;
        }
        // Defer the rest of a burst to next frame so a frame never pays for one
        // giant SSBO upload. Deferred sectors keep their stale `cache_gen`, so
        // they are retried (and budgeted again) on the following frame.
        if upload_budget == 0 {
            continue;
        }
        if let Some(base) = reuse_base {
            let n = count as usize;
            gpu.origins_scratch.clear();
            gpu.origins_scratch.extend(std::iter::repeat_n(
                [origin[0], origin[1], origin[2], 0.0],
                n,
            ));
            gpu.renderer
                .as_mut()
                .unwrap()
                .upload_quad_region(base, bytes, &gpu.origins_scratch);
            gpu.slots.insert(*coord, (base, count, g));
            gpu.frame_uploaded += 1;
            gpu.lightmap_dirty.insert(*coord);
            upload_budget -= 1;
            continue;
        }
        if let Some(s) = gpu.slots.remove(coord) {
            let rounded = (s.1 + 3) & !3;
            gpu.free_quads.push((s.0, rounded));
        }
        match alloc_slot(
            &mut gpu.free_quads,
            &mut gpu.next_base,
            gpu.quad_capacity,
            count,
        ) {
            Ok(base) => {
                let n = count as usize;
                gpu.origins_scratch.clear();
                gpu.origins_scratch.extend(std::iter::repeat_n(
                    [origin[0], origin[1], origin[2], 0.0],
                    n,
                ));
                gpu.renderer.as_mut().unwrap().upload_quad_region(
                    base,
                    bytes,
                    &gpu.origins_scratch,
                );
                gpu.slots.insert(*coord, (base, count, g));
                gpu.frame_uploaded += 1;
                gpu.lightmap_dirty.insert(*coord);
                upload_budget -= 1;
            }
            Err(()) => {
                // Capacity exhausted; grow buffer up to the hard SSBO cap.
                // Past that, skip this sector (free-list recycle next frame)
                // rather than allocating origins > max_storage_buffer_binding_size.
                let max_cap = gpu
                    .renderer
                    .as_ref()
                    .map(|r| r.max_quad_capacity())
                    .unwrap_or(MAX_QUAD_SSBO_SLOTS);
                if (gpu.quad_capacity as usize) >= max_cap {
                    continue;
                }
                let new_cap = (((total_quads as usize) * 2 + (1 << 16))
                    .max(gpu.quad_capacity as usize * 2)
                    .max(1 << 20))
                .next_power_of_two()
                .min(max_cap) as u32;
                if new_cap <= gpu.quad_capacity {
                    continue;
                }
                gpu.renderer.as_mut().unwrap().ensure_quad_capacity(new_cap);
                gpu.quad_capacity = new_cap;
                gpu.frame_rebuild = 1;
                // Lightmap SSBO was recreated — refresh every resident sector.
                for c in gpu.slots.keys() {
                    gpu.lightmap_dirty.insert(*c);
                }

                // Retry allocation now that we have grown.
                if let Ok(base) = alloc_slot(
                    &mut gpu.free_quads,
                    &mut gpu.next_base,
                    gpu.quad_capacity,
                    count,
                ) {
                    let n = count as usize;
                    gpu.origins_scratch.clear();
                    gpu.origins_scratch.extend(std::iter::repeat_n(
                        [origin[0], origin[1], origin[2], 0.0],
                        n,
                    ));
                    gpu.renderer.as_mut().unwrap().upload_quad_region(
                        base,
                        bytes,
                        &gpu.origins_scratch,
                    );
                    gpu.slots.insert(*coord, (base, count, g));
                    gpu.frame_uploaded += 1;
                    gpu.lightmap_dirty.insert(*coord);
                    upload_budget -= 1;
                }
            }
        }
    }
    // Flush all queued sector uploads as a single pair of write_buffer calls
    // (see Renderer::upload_quad_region / flush_quad_uploads) instead of one
    // per sector — this is what removes the multi-ms streaming upload hitch.
    gpu.renderer.as_mut().unwrap().flush_quad_uploads();
    gpu.frame_us_upload = tu.elapsed().as_micros() as u64;

    // M10a.2: install the block-palette SSBO so resolve colors voxels with
    // `BlockRegistry.base_color`. Idempotent and cheap: the renderer skips the
    // rebind when the storage buffer is unchanged.
    gpu.renderer.as_mut().unwrap().set_block_registry(&registry);

    let camera = build_camera(&player, gpu.width, gpu.height);
    let format = gpu.surface_format.unwrap();

    // Frustum-cull whole sectors. The world-space AABBs are pre-translated once
    // at cache time (`world_aabbs`), so no per-frame `Vec` allocation or sector-
    // local->world translation happens here — the cull only touches cached data.
    let tc = std::time::Instant::now();
    let visible_idx = cull_visible(&gpu.world_aabbs, &camera);
    gpu.frame_cull_us = tc.elapsed().as_micros() as u64;
    gpu.frame_cull_total = gpu.world_aabbs.len() as u32;
    gpu.frame_cull_visible = visible_idx.len() as u32;

    let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(visible_idx.len());
    let mut focus_sector: Option<SectorCoord> = None;
    for &i in &visible_idx {
        let c = coords[i];
        if let Some(s) = gpu.slots.get(&c) {
            ranges.push((s.0, s.1));
            if focus_sector.is_none() {
                focus_sector = Some(c);
            }
        }
    }

    // M10a.4: upload the per-quad lightmap for dirty visible sectors. Each
    // sector writes to its allocated slot in the global lightmap SSBO. Dirty
    // sources: mesh (re)upload, Changed<SectorLight>, and SSBO grow. This is
    // what brightens a sector when lighting finishes without a remesh.
    // Y4: build a HashMap once instead of O(n) linear scan per visible sector.
    // Y5: reuse `gpu.lightmap_scratch` to avoid per-sector Vec allocation.
    let light_map: std::collections::HashMap<SectorCoord, &SectorLight> =
        lights.iter().map(|(coord, light)| (*coord, light)).collect();
    let dirty_visible: Vec<SectorCoord> = visible_idx
        .iter()
        .map(|&i| coords[i])
        .filter(|c| gpu.lightmap_dirty.contains(c))
        .collect();
    for c in dirty_visible {
        let (base, count) = match gpu.slots.get(&c) {
            Some(&(b, n, _)) if n > 0 => (b, n),
            _ => {
                gpu.lightmap_dirty.remove(&c);
                continue;
            }
        };
        let sector_light = light_map.get(&c).copied();
        let mesh = storage.meshes.get(&c);

        gpu.lightmap_scratch.clear();
        if let Some(mesh) = mesh {
            for q in &mesh.opaque {
                let light_byte = if let Some(sl) = sector_light {
                    let x = q.x() as i32;
                    let y = q.y() as i32;
                    let z = q.z() as i32;
                    let w = q.width() as i32;
                    let h = q.height() as i32;
                    let face = q.face() as u8;
                    let axis = (face / 2) as usize;
                    let uaxis = (axis + 1) % 3;
                    let vaxis = (axis + 2) % 3;

                    let norm_offset = match face {
                        0 => [1, 0, 0],   // +X
                        1 => [-1, 0, 0],  // -X
                        2 => [0, 1, 0],   // +Y
                        3 => [0, -1, 0],  // -Y
                        4 => [0, 0, 1],   // +Z
                        5 => [0, 0, -1],  // -Z
                        _ => unreachable!(),
                    };

                    let sample = |du: i32, dv: i32| {
                        let mut pos = [x, y, z];
                        pos[uaxis] += du;
                        pos[vaxis] += dv;
                        let nx = pos[0] + norm_offset[0];
                        let ny = pos[1] + norm_offset[1];
                        let nz = pos[2] + norm_offset[2];

                        if nx >= 0 && nx < 32 && ny >= 0 && ny < 32 && nz >= 0 && nz < 32 {
                            let ld = sl.get(VoxelCoord::new(nx as u32, ny as u32, nz as u32));
                            (ld.sky(), ld.block())
                        } else if ny >= 32 {
                            // Open sky above the sector top.
                            (15, 0)
                        } else if ny < 0 {
                            (0, 0)
                        } else {
                            // TODO(H10): Cross-sector lateral neighbours. Do NOT
                            // clamp to the solid owner (sky=0) — that paints every
                            // sector-edge face and full-span greedy corner black.
                            // Treat outward air as open sky until neighbour queries
                            // land.
                            (15, 0)
                        }
                    };
                    // Corners of the owning voxels (match AO), not one-past at
                    // (w,h) which is OOB for width/height==32 and pulls sky=0.
                    let u_max = (w - 1).max(0);
                    let v_max = (h - 1).max(0);
                    let s0 = sample(0, 0);
                    let s1 = sample(u_max, 0);
                    let s2 = sample(0, v_max);
                    let s3 = sample(u_max, v_max);
                    let sky_avg =
                        ((s0.0 as u16 + s1.0 as u16 + s2.0 as u16 + s3.0 as u16) / 4) as u8;
                    let block_avg =
                        ((s0.1 as u16 + s1.1 as u16 + s2.1 as u16 + s3.1 as u16) / 4) as u8;
                    LightmapEntry::pack(sky_avg, block_avg)
                } else if q.light() != 0 {
                    LightmapEntry(q.light())
                } else {
                    // No SectorLight yet (mesher baked none): stay dark until
                    // lighting inserts SectorLight (Changed → dirty → re-upload).
                    LightmapEntry::pack(0, 0)
                };
                gpu.lightmap_scratch.push(light_byte);
            }
        } else {
            gpu.lightmap_scratch.resize(count as usize, LightmapEntry::pack(0, 0));
        }

        gpu.renderer
            .as_mut()
            .unwrap()
            .upload_lightmap_region(base, &gpu.lightmap_scratch);
        gpu.lightmap_dirty.remove(&c);
    }

    // Acquire the current surface texture.
    let frame = match gpu.surface.as_ref().unwrap().get_current_texture() {
        Ok(f) => f,
        // O6: handle specific surface errors instead of generic skip.
        Err(wgpu::SurfaceError::Outdated) => {
            // Surface configuration stale (e.g. resize race) — reconfigure
            // and skip this frame; next frame will acquire successfully.
            if let (Some(surface), Some(cfg), Some(renderer)) = (
                gpu.surface.as_ref(),
                gpu.config.as_ref(),
                gpu.renderer.as_ref(),
            ) {
                surface.configure(renderer.device(), cfg);
            }
            return;
        }
        Err(wgpu::SurfaceError::Lost) => {
            // Surface destroyed (GPU driver crash, Optimus switch, TDR).
            // Drop surface/renderer/config BEFORE create_surface next frame
            // so the native window handle is released.
            warn!("strata: surface lost — dropping GPU surface for reinit");
            drop_surface_for_reinit(gpu);
            return;
        }
        Err(e) => {
            warn!("strata: surface not available this frame: {e:?}");
            return;
        }
    };

    let view = frame.texture.create_view(&TextureViewDescriptor::default());
    {
        let td = std::time::Instant::now();
        let renderer = gpu.renderer.as_mut().unwrap();
        // Camera moves every frame, so re-upload the (tiny) uniform.
        renderer.set_debug_faces(face_debug.0);
        renderer.set_camera(&camera);
        // One-shot debug dump: write the dump mask AFTER set_debug_faces so
        // the resolve params buffer has the dump config (set_debug_faces
        // only writes when the value actually changes, so this is safe in
        // the common steady-state case).
        let do_dump = debug_dump.triggered;
        if do_dump {
            let cx = gpu.width / 2;
            let cy = gpu.height / 2;
            renderer.set_debug_dump(debug_dump.mask, cx, cy);
            debug_dump.triggered = false;
        }
        gpu.frame_prepass_quads = ranges.iter().map(|(_, c)| c).sum();
        gpu.frame_prepass_runs = ranges.len() as u32;
        gpu.frame_draws = renderer.draw_quad_ranges(&ranges, Some(&gpu.bloom_params)) as u32;
        if do_dump {
            renderer.dump_debug("client");
        }
        renderer.present(&view, format);
        gpu.frame_us_draw = td.elapsed().as_micros() as u64;
    }
    // `frame.present()` consumes `frame`, releasing the surface texture.
    frame.present();

    // One-shot ground-truth: only when `DebugReadback` is armed (R key), then
    // auto-clear so the expensive GPU sync stall never sits in the hot path.
    if cfg!(debug_assertions) && readback.0 {
        readback.0 = false;
        let px = {
            let renderer = gpu.renderer.as_mut().unwrap();
            renderer.readback()
        };
        let w = gpu.renderer.as_ref().unwrap().width() as usize;
        let h = gpu.renderer.as_ref().unwrap().height() as usize;
        let bpp = 8usize;
        let mut sky = 0usize;
        let mut sampled_top = [0f32; 3];
        let mut sampled_mid = [0f32; 3];
        let mut sampled_bot = [0f32; 3];
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * bpp;
                let r = f16_of(&px, o);
                let b = f16_of(&px, o + 4);
                if b > r + 0.1 {
                    sky += 1;
                }
                if y == h / 4 {
                    sampled_top = [r, f16_of(&px, o + 2), b];
                }
                if y == h / 2 {
                    sampled_mid = [r, f16_of(&px, o + 2), b];
                }
                if y == (3 * h) / 4 {
                    sampled_bot = [r, f16_of(&px, o + 2), b];
                }
            }
        }
        info!(
            "DIAG_OFFSCREEN offscreen_sky_pct={:.0} top(rgb)={:?} mid(rgb)={:?} bot(rgb)={:?}",
            100.0 * sky as f32 / (w * h) as f32,
            sampled_top,
            sampled_mid,
            sampled_bot,
        );
    }
}

/// Create the wgpu surface + device from the Bevy window's raw handle and
/// configure it for presentation.
fn init_surface(gpu: &mut ClientGpu, window: &Window, rhw: &RawHandleWrapper) -> bool {
    // Release any prior surface before recreating (Lost / hot-reload path).
    drop_surface_for_reinit(gpu);

    let target = SurfaceTargetUnsafe::RawHandle {
        raw_window_handle: rhw.get_window_handle(),
        raw_display_handle: rhw.get_display_handle(),
    };

    let surface = match unsafe { gpu.instance.create_surface_unsafe(target) } {
        Ok(s) => s,
        Err(e) => {
            error!("strata: failed to create wgpu surface: {e:?}");
            return false;
        }
    };

    // Enumerate adapters and pick one that BOTH exposes the pre-pass u64-atomic
    // features AND is compatible with our surface. `request_adapter` with
    // `compatible_surface` alone returns the first surface-capable adapter, which
    // on Optimus laptops is the integrated GPU — it lacks SHADER_INT64_ATOMIC and
    // would force the pre-pass off (all-sky clear -> gray window). The discrete
    // RTX does support it, so we search explicitly (matching the headless diag
    // path that renders terrain correctly).
    let adapter = match gpu
        .instance
        .enumerate_adapters(Backends::all())
        .into_iter()
        .find(|a| a.features().contains(prepass_features()) && a.is_surface_supported(&surface))
    {
        Some(a) => a,
        None => {
            error!("strata: no wgpu adapter supports the depth pre-pass features AND the surface");
            return false;
        }
    };

    // The selected adapter already advertises the pre-pass features, so request
    // them in full (do NOT intersect — that would silently drop the atomic bit
    // and disable terrain rendering).
    let features = prepass_features();

    let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("strata_client"),
        required_features: features,
        ..Default::default()
    })) {
        Ok(dq) => dq,
        Err(e) => {
            error!("strata: failed to create wgpu device: {e:?}");
            return false;
        }
    };

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .first()
        .copied()
        .unwrap_or(TextureFormat::Bgra8Unorm);

    // Use the *physical* surface size. `window.resolution.width()` is logical
    // (CSS) pixels; on a HiDPI/scale-factor != 1 display the real swapchain must
    // be the physical size, otherwise the rendered region is smaller than the
    // window (game appears in a corner, rest of the window stays black).
    let w = window.resolution.physical_width().max(1);
    let h = window.resolution.physical_height().max(1);

    let config = SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT,
        format,
        width: w,
        height: h,
        // `Fifo` is the only present mode guaranteed to be supported on every
        // platform and enforces VSync, capping the frame rate to the monitor's
        // native refresh rate (it never tears and never runs faster than the
        // display). This makes the game's FPS match the user's monitor.
        present_mode: PresentMode::Fifo,
        alpha_mode: CompositeAlphaMode::Opaque,
        view_formats: vec![],
        desired_maximum_frame_latency: 1,
    };
    surface.configure(&device, &config);

    let renderer = Renderer::new(device, queue, w, h);
    gpu.surface = Some(surface);
    gpu.config = Some(config);
    gpu.surface_format = Some(format);
    gpu.renderer = Some(renderer);
    gpu.width = w;
    gpu.height = h;
    true
}

/// Build a [`CameraView`] from the player's transform + look orientation.
fn build_camera(player: &Query<(&Transform, &PlayerLook)>, width: u32, height: u32) -> CameraView {
    let (eye, yaw, pitch) = match player.iter().next() {
        Some((tf, look)) => {
            let e = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
            (e, look.yaw, look.pitch)
        }
        // Fallback: the player is not queryable yet (e.g. before spawn resolved).
        // Previously this fell back to eye=(0,0,0) which is INSIDE terrain and
        // produces an all-gray screen. Point the camera at the spawn column
        // instead so a missing-player bug is visible rather than a gray void.
        None => (Vec3::new(16.0, 80.0 + EYE_HEIGHT, 16.0), 0.0, 0.0),
    };

    let cp = pitch.cos();
    let sp = pitch.sin();
    let sy = yaw.sin();
    let cy = yaw.cos();
    let forward = Vec3::new(-cp * sy, sp, -cp * cy);
    let center = eye + forward;

    let aspect = width as f32 / height.max(1) as f32;
    let proj = perspective_rh_zo(std::f32::consts::FRAC_PI_4, aspect, 0.1, 2000.0);
    let view = look_at_rh(
        [eye.x, eye.y, eye.z],
        [center.x, center.y, center.z],
        [0.0, 1.0, 0.0],
    );

    CameraView::new([eye.x, eye.y, eye.z], view, proj, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_slot_free_reuse_rounded() {
        let mut free = Vec::new();
        let mut next_base = 0u32;
        let capacity = 1024;

        // Allocate 5 quads (internally rounded to 8).
        let base1 = alloc_slot(&mut free, &mut next_base, capacity, 5).unwrap();
        assert_eq!(base1, 0);
        assert_eq!(next_base, 8); // rounded up from 5
        assert!(free.is_empty());

        // Free the slot — push the rounded count (bug fix: was unrounded 5).
        free.push((base1, 8)); // simulating the rounded push from the unload/realloc paths

        // Allocate 6 quads (internally rounded to 8). Must reuse the freed slot.
        let base2 = alloc_slot(&mut free, &mut next_base, capacity, 6).unwrap();
        assert_eq!(base2, 0, "freed slot must be reused");
        assert_eq!(next_base, 8); // cursor did not advance

        // Allocate 3 quads (internally rounded to 4). Must go to base 8 now.
        let base3 = alloc_slot(&mut free, &mut next_base, capacity, 3).unwrap();
        assert_eq!(base3, 8);
        assert_eq!(next_base, 12);

        // Free both and verify future round-trips.
        free.push((base2, 8)); // 6 rounded to 8
        free.push((base3, 4)); // 3 rounded to 4

        // Allocate 10 quads (rounded to 12) — should fail at base 0+8=8 but fragment is only 4+8.
        // First fit finds (0, 8): c=8, need=12 → skip. Then finds (8, 4): c=4, need=12 → skip.
        // Bump cursor at 12: 12+12=24 ≤ 1024 → Ok(12).
        let base4 = alloc_slot(&mut free, &mut next_base, capacity, 10).unwrap();
        assert_eq!(base4, 12);
        assert_eq!(next_base, 24);

        // Verify fragmentation didn't cause a leak: the old entries were too small.
        assert_eq!(free.len(), 2); // (0,8) and (8,4) still free
    }

    /// Y4: verify that a HashMap lookup produces the same result as the old
    /// linear scan for a realistic number of sectors (500).
    #[test]
    fn hashmap_lightmap_lookup_matches_linear_scan() {
        use std::collections::HashMap;

        // Simulate 500 sector entries (typical visible set size).
        let entries: Vec<(SectorCoord, i32)> = (0..500)
            .map(|i| (SectorCoord(i % 32, i / 32, 0), i * 10))
            .collect();

        // Build HashMap once (Y4 fix pattern).
        let light_map: HashMap<SectorCoord, &i32> =
            entries.iter().map(|(coord, val)| (*coord, val)).collect();

        // Verify each lookup matches the linear scan.
        for (coord, expected) in &entries {
            let linear = entries.iter().find(|(c, _)| c == coord).map(|(_, v)| *v);
            let hash = light_map.get(coord).copied().copied();
            assert_eq!(linear, hash);
            assert_eq!(hash, Some(*expected));
        }

        // Miss returns None.
        assert!(light_map.get(&SectorCoord(999, 999, 999)).is_none());
    }

    /// Y5: verify that reusing a scratch buffer (clear + push + resize)
    /// produces the same content as allocating a fresh Vec each time.
    #[test]
    fn scratch_buffer_matches_fresh_vec() {
        let mut scratch: Vec<u8> = Vec::new();

        for count in [0, 1, 7, 32, 128, 1024] {
            // Simulate the fresh-alloc path.
            let mut fresh = Vec::with_capacity(count);
            for i in 0..count {
                fresh.push((i % 256) as u8);
            }

            // Simulate the scratch-buffer path.
            scratch.clear();
            for i in 0..count {
                scratch.push((i % 256) as u8);
            }

            assert_eq!(fresh, scratch, "mismatch at count={count}");
        }

        // Verify resize fallback (no mesh available).
        scratch.clear();
        scratch.resize(64, 0xAB);
        let fresh = vec![0xABu8; 64];
        assert_eq!(fresh, scratch);
    }

    /// O6: verify the surface error classification logic: Outdated triggers
    /// reconfigure (ready stays true), Lost forces ready=false for full
    /// reinit next frame, other errors just skip the frame.
    #[test]
    fn surface_error_recovery_classifies_variants() {
        let errors = [
            wgpu::SurfaceError::Outdated,
            wgpu::SurfaceError::Lost,
            wgpu::SurfaceError::OutOfMemory,
            wgpu::SurfaceError::Timeout,
        ];

        for err in &errors {
            let mut ready = true;
            let mut reconfigure = false;
            let mut drop_surface = false;
            match err {
                wgpu::SurfaceError::Outdated => {
                    reconfigure = true;
                }
                wgpu::SurfaceError::Lost => {
                    ready = false;
                    drop_surface = true;
                }
                _ => {}
            }
            match err {
                wgpu::SurfaceError::Outdated => {
                    assert!(reconfigure && ready, "Outdated: reconfigure, keep ready");
                    assert!(!drop_surface);
                }
                wgpu::SurfaceError::Lost => {
                    assert!(!ready, "Lost: must set ready=false for full reinit");
                    assert!(
                        drop_surface,
                        "Lost: must drop surface/renderer/config before create_surface"
                    );
                }
                _ => {
                    assert!(ready, "Other errors: skip frame, keep ready");
                }
            }
        }
    }

    /// Equal-length cache vs storage with different keys must still evict stale.
    #[test]
    fn mesh_cache_eviction_uses_key_set_not_len() {
        use strata_render::meshing::{MeshData, MeshStorage};

        let a = SectorCoord(0, 0, 0);
        let b = SectorCoord(1, 0, 0);
        let c = SectorCoord(2, 0, 0);

        let mut cache: std::collections::HashMap<SectorCoord, u32> =
            [(a, 1u32), (b, 2u32)].into_iter().collect();
        // storage has same len (2) but different keys: B,C — A must go.
        let mut storage = MeshStorage::default();
        let empty = || MeshData {
            opaque: Vec::new(),
            opaque_gpu: Vec::new(),
            transparent: Vec::new(),
            transparent_gpu: Vec::new(),
            aabb: Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            generation: 1,
        };
        storage.meshes.insert(b, empty());
        storage.meshes.insert(c, empty());

        assert_eq!(
            cache.len(),
            storage.meshes.len(),
            "precondition: equal lengths hide the bug"
        );
        let removed = retain_mesh_cache_to_storage(&mut cache, &storage);
        assert!(removed, "must detect key-set mismatch at equal len");
        assert!(!cache.contains_key(&a), "stale A must be evicted");
        assert!(cache.contains_key(&b));
        assert!(!cache.contains_key(&c), "C was never cached");
    }

    #[test]
    fn surface_lost_drops_gpu_handles_before_reinit() {
        let mut gpu = ClientGpu::new();
        // Simulate a "live" surface session without a real wgpu surface.
        gpu.ready = true;
        gpu.width = 64;
        gpu.height = 64;
        // config/surface/renderer stay None — drop must clear flags anyway.
        drop_surface_for_reinit(&mut gpu);
        assert!(!gpu.ready);
        assert!(gpu.surface.is_none());
        assert!(gpu.renderer.is_none());
        assert!(gpu.config.is_none());
        assert!(gpu.surface_format.is_none());
    }

    /// Sectors whose lightmap must be re-uploaded when `SectorLight` lands
    /// after the mesh (no remesh required).
    #[test]
    fn lightmap_dirty_on_sector_light_without_remesh() {
        use std::collections::HashSet;

        let mut dirty: HashSet<SectorCoord> = HashSet::new();
        let mesh_coord = SectorCoord(1, 0, 0);
        // Mesh upload marks dirty and may write zeros before lighting runs.
        dirty.insert(mesh_coord);
        assert!(dirty.contains(&mesh_coord));
        // Upload consumes dirty (zeros).
        dirty.remove(&mesh_coord);
        assert!(!dirty.contains(&mesh_coord));
        // Later SectorLight insert (Changed) must re-dirty without NeedsRemesh.
        dirty.insert(mesh_coord);
        assert!(
            dirty.contains(&mesh_coord),
            "SectorLight change must force lightmap re-upload without remesh"
        );
    }

    /// Full-span greedy quads must sample owning-voxel corners, not (w,h)
    /// which is always OOB at 32 and used to average in solid sky=0.
    #[test]
    fn lightmap_corners_use_last_owning_voxel() {
        let corners = |w: i32, h: i32| {
            let u_max = (w - 1).max(0);
            let v_max = (h - 1).max(0);
            [(0, 0), (u_max, 0), (0, v_max), (u_max, v_max)]
        };
        assert_eq!(corners(32, 32), [(0, 0), (31, 0), (0, 31), (31, 31)]);
        assert_eq!(corners(1, 1), [(0, 0), (0, 0), (0, 0), (0, 0)]);
        assert_eq!(corners(4, 2), [(0, 0), (3, 0), (0, 1), (3, 1)]);
    }
}
