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
use strata_render::pipeline::camera::{look_at_rh, perspective_rh_zo};
use strata_render::pipeline::cull::{Aabb, cull_visible};
use strata_render::pipeline::{CameraView, Renderer, prepass_features};
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
    /// SSBO write_buffer of changed sectors; draw = prepass+resolve+present.
    pub frame_us_reflatten: u64,
    pub frame_us_upload: u64,
    pub frame_us_draw: u64,
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
            // Generous initial SSBO so normal streaming never triggers a full
            // re-upload regrow (one-time ~20-40ms spike). 2M quads ≈ 16 MB.
            quad_capacity: 1 << 21,
            origins_scratch: Vec::new(),
            frame_reflatten: 0,
            frame_uploaded: 0,
            frame_draws: 0,
            frame_rebuild: 0,
            frame_us_reflatten: 0,
            frame_us_upload: 0,
            frame_us_draw: 0,
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
        app.insert_resource(DebugFaces(true));
        app.insert_resource(DebugReadback(false));
        app.add_systems(PostUpdate, mark_generated_for_remesh);
        app.add_systems(Update, (toggle_debug_faces, toggle_debug_readback));
        app.add_systems(Update, client_render_system.in_set(StrataSet::RenderUpdate));
    }
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
pub fn client_render_system(
    mut gpu: ResMut<ClientGpu>,
    windows: Query<(Entity, &Window, &RawHandleWrapper)>,
    player: Query<(&Transform, &PlayerLook)>,
    mut storage: ResMut<MeshStorage>,
    face_debug: Res<DebugFaces>,
    mut diag_frame: Local<u32>,
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
        gpu.renderer.as_mut().unwrap().resize(w, h);
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
    if gpu.mesh_cache.len() != storage.meshes.len() {
        gpu.mesh_cache.retain(|c, _| storage.meshes.contains_key(c));
        gpu.cache_gen.retain(|c, _| storage.meshes.contains_key(c));
        let _ = &mut set_changed;
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
            gpu.free_quads.push((b, n));
        }
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
    // slot allocator's fragmentation.
    let total_quads: u32 = gpu
        .mesh_cache
        .values()
        .map(|(b, _, _)| (b.len() / 8) as u32)
        .sum();
    let need_cap = (total_quads.max(gpu.next_base) as usize) * 2 + (1 << 16);
    let new_cap =
        (need_cap.max(gpu.quad_capacity as usize).max(1 << 20)).next_power_of_two() as u32;
    if new_cap > gpu.quad_capacity {
        gpu.quad_capacity = new_cap;
    }
    // Always hand the renderer the required SSBO capacity. `ensure_quad_capacity`
    // is a no-op when the buffer already meets it, so this also performs the
    // first-frame allocation (initial `quad_capacity` is 2M, but the renderer's
    // own buffer starts at 0 until told).
    gpu.renderer.as_mut().unwrap().ensure_quad_capacity(new_cap);

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
            upload_budget -= 1;
            continue;
        }
        if let Some(s) = gpu.slots.remove(coord) {
            gpu.free_quads.push((s.0, s.1));
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
                upload_budget -= 1;
            }
            Err(()) => {
                // Capacity exhausted; grow buffer. Old slots are copied and preserved!
                let new_cap = (((total_quads as usize) * 2 + (1 << 16))
                    .max(gpu.quad_capacity as usize * 2)
                    .max(1 << 20))
                .next_power_of_two() as u32;
                gpu.renderer.as_mut().unwrap().ensure_quad_capacity(new_cap);
                gpu.quad_capacity = new_cap;
                gpu.frame_rebuild = 1;

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

    let camera = build_camera(&player, gpu.width, gpu.height);
    let format = gpu.surface_format.unwrap();

    // Frustum-cull whole sectors. The world-space AABBs are pre-translated once
    // at cache time (`world_aabbs`), so no per-frame `Vec` allocation or sector-
    // local->world translation happens here — the cull only touches cached data.
    let visible_idx = cull_visible(&gpu.world_aabbs, &camera);

    let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(visible_idx.len());
    for &i in &visible_idx {
        let c = coords[i];
        if let Some(s) = gpu.slots.get(&c) {
            ranges.push((s.0, s.1));
        }
    }

    // Acquire the current surface texture.
    let frame = match gpu.surface.as_ref().unwrap().get_current_texture() {
        Ok(f) => f,
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
        gpu.frame_draws = renderer.draw_quad_ranges(&ranges) as u32;
        renderer.present(&view, format);
        gpu.frame_us_draw = td.elapsed().as_micros() as u64;
    }
    // `frame.present()` consumes `frame`, releasing the surface texture.
    frame.present();

    // One-shot ground-truth: after presenting, read the offscreen HDR back to
    // the CPU and measure its sky/terrain split. Held in its own scope so the
    // `gpu.renderer` borrow ends before anything else touches `gpu`.
    *diag_frame += 1;
    if cfg!(debug_assertions) && *diag_frame == 120 {
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
