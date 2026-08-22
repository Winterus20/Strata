//! Strata client binary (M9b): wires every Strata plugin together, borrows
//! Bevy's `RenderDevice`/`RenderQueue` for the single wgpu device, and
//! PRESENTs the M4 offscreen renderer to a real window.
//!
//! Per plan 31 §1.5: Bevy's `RenderPlugin` is enabled (creates the single
//! wgpu device/surface). `ClientRenderPlugin` borrows Bevy's
//! `RenderDevice`/`RenderQueue` to construct `strata_render::Renderer`.
//! The GPU draw + present runs on the main app using Bevy's surface texture.

mod client_input;
mod client_render;

use bevy::input::InputPlugin;
use bevy::prelude::*;
use bevy::window::{Window, WindowPlugin, WindowResolution};
use bevy::{DefaultPlugins, MinimalPlugins};

use strata_core::component::SectorSnapshot;
use strata_core::prelude::*;
use strata_physics::plugin::PhysicsPlugin;
use strata_player::controller::EYE_HEIGHT;
use strata_player::prelude::*;
use strata_render::meshing::MeshingPlugin;
use strata_save::plugin::{DirtyQueue, InFlightSaves, SaveBackend, SavePlugin, SectorSave};
use strata_save::save_manager::SaveManager;
use strata_storage::backend::{AsyncStorageBackend, TokioBackend};
use strata_storage::metadata::{FjallMetadata, SectorMetadata};
use strata_world::prelude::*;

#[derive(Resource, Clone)]
pub struct SaveDirectory(pub std::path::PathBuf);

#[derive(Resource)]
pub struct TempSaveDir(pub tempfile::TempDir);

use client_input::{cursor_grab_system, mouse_look_system};
use client_render::ClientRenderPlugin;

/// Horizontal spawn column (world X/Z). The Y is computed at spawn time from the
/// terrain surface (see [`spawn_position`]).
const SPAWN_COLUMN: (i32, i32) = (16, 16);

/// The player spawn transform: the [`SPAWN_COLUMN`] at the terrain surface for
/// that column, plus a small clearance. Computing the surface Y from world-gen
/// (deterministic, no sector needed) fixes the player spawning high in the sky
/// and free-falling through not-yet-generated (async) terrain until it lands
/// buried below the surface.
fn spawn_position() -> Vec3 {
    let (sx, sz) = SPAWN_COLUMN;
    // Surface block top sits at `surface_y + 1`; drop the player ~2 blocks above
    // it. The tiny fall is caught by collide-and-slide once the sector generates.
    let center_y = surface_y(sx, sz) as f32 + 3.0;
    Vec3::new(sx as f32 + 0.5, center_y, sz as f32 + 0.5)
}

/// Optional init-time `client.toml` (view distance radius + window size). Missing
/// file -> defaults. Minimal `key = value` parser (no external TOML dependency).
#[derive(Clone, Copy)]
struct ClientConfig {
    radius: i32,
    width: u32,
    height: u32,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            radius: 2,
            width: 1280,
            height: 720,
        }
    }
}

fn load_config() -> ClientConfig {
    let mut cfg = ClientConfig::default();
    let text = match std::fs::read_to_string("client.toml") {
        Ok(t) => t,
        Err(_) => return cfg,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "radius" => {
                    if let Ok(r) = value.parse::<i32>() {
                        cfg.radius = r.clamp(1, 16);
                    }
                }
                "width" => {
                    if let Ok(w) = value.parse::<u32>() {
                        cfg.width = w.clamp(320, 7680);
                    }
                }
                "height" => {
                    if let Ok(h) = value.parse::<u32>() {
                        cfg.height = h.clamp(240, 4320);
                    }
                }
                _ => {}
            }
        }
    }
    cfg
}

/// Build the full Strata client app. `headless` swaps `DefaultPlugins` (windowed,
/// winit-driven) for `MinimalPlugins` (schedule-runner driven) so the pipeline can
/// be exercised without a display.
fn build_client_app(headless: bool, config: &ClientConfig) -> App {
    let mut app = App::new();

    if headless {
        // MinimalPlugins lacks the plugins Rapier's physics step depends on
        // (Asset/Transform/Mesh/Scene/Hierarchy). In windowed mode DefaultPlugins
        // provides them; here we add the minimal subset so the full plugin chain
        // (including PhysicsPlugin) can run headlessly for tests.
        app.add_plugins(MinimalPlugins);
        app.add_plugins(InputPlugin);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(bevy::mesh::MeshPlugin);
        app.add_plugins(bevy::scene::ScenePlugin);
    } else {
        let window = Window {
            resolution: WindowResolution::new(config.width, config.height),
            ..Default::default()
        };
        // Per plan 31 §1.5: Bevy's `RenderPlugin` is ENABLED — it creates the
        // single wgpu device/surface that `ClientGpu` borrows via
        // `RenderDevice`/`RenderQueue`. Previously it was disabled to avoid a
        // double-device conflict; that conflict is now resolved by having
        // `ClientGpu` use Bevy's device instead of creating its own.
        //
        // We register the `Shader` asset type ourselves *before* `DefaultPlugins`
        // builds: some plugins (e.g. CorePipelinePlugin's TonemappingPlugin)
        // `asset_server.load` a `Shader` at build time, which panics if the type
        // is unregistered. We add `AssetPlugin` + register `Shader` ourselves,
        // then disable `DefaultPlugins`' `AssetPlugin` to avoid a duplicate.
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::shader::Shader>()
            .init_asset_loader::<bevy::shader::ShaderLoader>();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(window),
                    ..Default::default()
                })
                .disable::<bevy::asset::AssetPlugin>(),
        );
    }

    // Core scheduling (orders the StrataSet chain) + the block registry the
    // world-gen and meshing systems read from.
    app.init_resource::<bevy::ecs::message::Messages<SectorSave>>();
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.add_strata_plugin(BlockRegistryPlugin);

    // Initialize storage backend & metadata store
    let (save_dir, temp_dir) = if headless {
        let temp = tempfile::tempdir().expect("failed to create temp dir");
        let path = temp.path().to_path_buf();
        (path, Some(temp))
    } else {
        let path = std::env::var("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap())
            .join("Strata")
            .join("saves");
        (path, None)
    };

    let backend =
        TokioBackend::new(save_dir.clone()).expect("failed to initialize storage backend");
    let meta_store =
        FjallMetadata::open(&save_dir.join("metadata")).expect("failed to open metadata store");
    let save_manager = SaveManager::new(
        std::sync::Arc::new(meta_store),
        std::time::Duration::from_secs(30),
    );

    app.insert_resource(SaveBackend(backend));
    app.insert_resource(save_manager);
    app.insert_resource(SaveDirectory(save_dir));
    if let Some(td) = temp_dir {
        app.insert_resource(TempSaveDir(td));
    }
    app.add_plugins(SavePlugin {
        auto_save_interval: std::time::Duration::from_secs(30),
    });

    // Streaming -> WorldGen -> Meshing -> Physics -> Lighting -> Player, plus our
    // client present plugin (registered last so the surface/resource exist).
    app.add_strata_plugin(StreamingPlugin::new(config.radius, DEFAULT_HYSTERESIS).with_ramp());
    app.add_strata_plugin(WorldGenPlugin);
    app.add_strata_plugin(MeshingPlugin);
    app.add_strata_plugin(PhysicsPlugin);
    app.add_strata_plugin(LightingPlugin);
    app.add_strata_plugin(PlayerPlugin);
    app.add_strata_plugin(ClientRenderPlugin);

    app.add_systems(Startup, spawn_player_system);
    // Periodic player save in Update; durable world flush in Last so AppExit is
    // visible and in-flight Update save handlers can finish enqueueing first.
    // After Input so hotbar/place Inventory mutations are observed consistently.
    app.add_systems(Update, save_player_system.after(StrataSet::Input));
    app.add_systems(Last, shutdown_system);

    // Window-only FPS input: cursor capture + mouse-look. Skipped headlessly
    // (no primary window / cursor to grab). Runs in the Input set so the updated
    // look is consumed by the controller and camera the same frame.
    if !headless {
        // Grab before look (CursorOptions); look before gameplay input so
        // break/place raycasts see this frame's yaw/pitch.
        app.add_systems(
            Update,
            (cursor_grab_system, mouse_look_system)
                .chain()
                .in_set(StrataSet::Input)
                .before(strata_player::input::input_mapper_system),
        );
        app.add_systems(
            Update,
            diagnostics_log_system
                .in_set(StrataSet::RenderUpdate)
                .after(crate::client_render::client_render_system),
        );
    }

    app
}

fn spawn_player_system(
    mut commands: Commands,
    save_manager: Option<Res<SaveManager>>,
    save_dir: Option<Res<SaveDirectory>>,
) {
    let mut spawn_pos = spawn_position();
    let mut spawn_look = PlayerLook::default();

    if let (Some(mgr), Some(dir)) = (save_manager, save_dir) {
        let path = dir.0.join("player.dat");
        #[allow(clippy::collapsible_if)]
        if path.exists() {
            if let Ok(player_data) = mgr.load_player(&path) {
                spawn_pos = Vec3::from_slice(&player_data.position);
                spawn_look.yaw = player_data.rotation[0];
                spawn_look.pitch = player_data.rotation[1];
                info!(
                    "strata: Loaded player position {:?} and rotation from disk",
                    spawn_pos
                );
            }
        }
    }

    commands.spawn((
        PlayerController::default(),
        PlayerState::default(),
        spawn_look,
        Inventory::default(),
        StreamingAnchor,
        Transform::from_translation(spawn_pos),
        GlobalTransform::from_translation(spawn_pos),
        Name::new("player"),
    ));
}

/// Cadence for periodic `player.dat` writes. Movement must not sync-write every
/// `Changed<Transform>` tick — that stalls the main thread on disk I/O.
const PLAYER_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Drive a storage future from a Bevy sync system without deadlocking the
/// multi-thread tokio runtime that owns [`TokioBackend`]'s worker.
///
/// `pollster::block_on` on a `current_thread` runtime deadlocks (worker shares
/// the blocked thread) — production uses `#[tokio::main]` (multi-thread);
/// unit tests must not call this path on `#[tokio::test]` default flavor.
fn block_on_storage<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(fut))
        }
        _ => pollster::block_on(fut),
    }
}

/// Returns true when `elapsed` has accumulated at least `interval` of `dt`.
fn player_save_due(
    elapsed: &mut std::time::Duration,
    dt: std::time::Duration,
    interval: std::time::Duration,
) -> bool {
    *elapsed = elapsed.saturating_add(dt);
    if *elapsed >= interval {
        *elapsed = std::time::Duration::ZERO;
        true
    } else {
        false
    }
}

/// Dirty coords to flush on shutdown: queued batch plus sticky loaded leftovers.
fn collect_shutdown_flush_coords(
    queued: Vec<SectorCoord>,
    loaded_dirty: impl IntoIterator<Item = SectorCoord>,
) -> Vec<SectorCoord> {
    let mut out = queued;
    for coord in loaded_dirty {
        if !out.contains(&coord) {
            out.push(coord);
        }
    }
    out
}

fn player_save_data_from(
    transform: &Transform,
    look: &PlayerLook,
    inventory: &Inventory,
) -> strata_save::player_save_data::PlayerSaveData {
    let hotbar_index = inventory.active as u8;
    let inventory: Vec<Option<strata_save::player_save_data::ItemStack>> = inventory
        .hotbar
        .iter()
        .map(|stack| {
            Some(strata_save::player_save_data::ItemStack {
                block_id: stack.block.0 as u32,
                count: stack.count,
            })
        })
        .collect();
    strata_save::player_save_data::PlayerSaveData {
        position: [
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        ],
        rotation: [look.yaw, look.pitch],
        health: 20.0,
        hunger: 20.0,
        xp: 0,
        hotbar_index,
        inventory,
    }
}

fn save_player_system(
    time: Res<Time>,
    mut elapsed: Local<std::time::Duration>,
    save_manager: Option<Res<SaveManager>>,
    save_dir: Option<Res<SaveDirectory>>,
    player_query: Query<(&Transform, &PlayerLook, &PlayerState, &Inventory)>,
) {
    if !player_save_due(&mut elapsed, time.delta(), PLAYER_SAVE_INTERVAL) {
        return;
    }
    if let (Some(mgr), Some(dir)) = (save_manager, save_dir) {
        #[allow(clippy::collapsible_if)]
        if let Ok((transform, look, _player_state, inventory)) = player_query.single() {
            let path = dir.0.join("player.dat");
            std::fs::create_dir_all(path.parent().unwrap()).ok();
            let player_data = player_save_data_from(transform, look, inventory);
            if let Err(e) = mgr.save_player(&path, &player_data) {
                error!("Failed to save player data: {e:?}");
            }
        }
    }
}

/// Durable AppExit flush: cut new work, drain backend (`sync`), fsync regions
/// (`flush`), then clear dirty bits only after commit.
#[allow(clippy::too_many_arguments)]
fn shutdown_system(
    mut exit: MessageReader<AppExit>,
    save_manager: Option<Res<SaveManager>>,
    save_dir: Option<Res<SaveDirectory>>,
    player_query: Query<(&Transform, &PlayerLook, &PlayerState, &Inventory)>,
    sectors: Query<(&SectorCoord, &SectorSnapshot)>,
    dirty_queue: Option<Res<DirtyQueue>>,
    backend: Option<Res<SaveBackend>>,
    saved_receiver: Option<Res<SavedReceiver>>,
) {
    if exit.read().next().is_none() {
        return;
    }
    info!("strata: AppExit received — durable-flushing player and world data");

    // Drain any already-completed async saves (post-commit clear only).
    if let Some(ref rx_res) = saved_receiver {
        if let (Ok(mut rx), Some(ref dq)) = (rx_res.rx.lock(), dirty_queue.as_ref()) {
            while let Ok(coord) = rx.try_recv() {
                dq.tracker.clear(coord);
            }
        }
    }

    if let (Some(dq), Some(bk), Some(mgr)) = (
        dirty_queue.as_ref(),
        backend.as_ref(),
        save_manager.as_ref(),
    ) {
        let sector_map: std::collections::HashMap<SectorCoord, &SectorSnapshot> = sectors
            .iter()
            .map(|(coord, snapshot)| (*coord, snapshot))
            .collect();

        // Remaining queued dirty + any sticky in-flight loaded sectors.
        let queued = dq.tracker.consume_dirty_batch(dq.tracker.pending());
        let loaded_dirty: Vec<SectorCoord> = sector_map
            .keys()
            .copied()
            .filter(|c| dq.tracker.is_dirty(*c))
            .collect();
        let to_flush = collect_shutdown_flush_coords(queued, loaded_dirty);

        let mut committed = Vec::new();
        for coord in to_flush {
            let Some(snapshot) = sector_map.get(&coord) else {
                warn!("strata: shutdown skip dirty {coord:?} — no SectorSnapshot resident");
                continue;
            };
            let payload = match postcard::to_allocvec(&*snapshot.0) {
                Ok(bytes) => bytes,
                Err(e) => {
                    error!("Failed to serialize sector {coord:?} on shutdown: {e}");
                    continue;
                }
            };
            let payload_hash = blake3::hash(&payload).into();
            let payload_size = payload.len() as u64;
            let write_ok = block_on_storage(async {
                if let Err(e) =
                    bk.0.write_sector_with_priority(
                        coord,
                        payload,
                        strata_storage::backend::priority::ACTIVE,
                    )
                    .await
                {
                    error!("Failed to write sector {coord:?} on shutdown: {e}");
                    return false;
                }
                let mtime = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64;
                let meta = SectorMetadata {
                    coord,
                    hash: payload_hash,
                    size: payload_size,
                    mtime,
                    tier: 0,
                    version: 1,
                    dirty: false,
                };
                if let Err(e) = mgr.metadata.put(meta).await {
                    error!("Failed to write sector {coord:?} metadata on shutdown: {e}");
                    return false;
                }
                true
            });
            if write_ok {
                committed.push(coord);
            }
        }

        // Barrier: drain any earlier in-flight I/O, then fsync region files.
        if let Err(e) = block_on_storage(AsyncStorageBackend::sync(&bk.0)) {
            error!("strata: shutdown backend sync failed: {e}");
        }
        if let Err(e) = block_on_storage(AsyncStorageBackend::flush(&bk.0)) {
            error!("strata: shutdown backend flush failed: {e}");
        }

        // Post-commit dirty clear only.
        for coord in committed {
            dq.tracker.clear(coord);
        }
        if let Some(ref rx_res) = saved_receiver {
            if let Ok(mut rx) = rx_res.rx.lock() {
                while let Ok(coord) = rx.try_recv() {
                    dq.tracker.clear(coord);
                }
            }
        }
        info!(
            "strata: Durable shutdown flush done (pending_queue={})",
            dq.tracker.pending()
        );
    }

    // Player position — always on exit (not subject to the periodic debounce).
    if let (Some(mgr), Some(dir)) = (save_manager, save_dir.as_ref()) {
        let path = dir.0.join("player.dat");
        std::fs::create_dir_all(path.parent().unwrap()).ok();

        if let Ok((transform, look, _player_state, inventory)) = player_query.single() {
            let player_data = player_save_data_from(transform, look, inventory);
            if let Err(e) = mgr.save_player(&path, &player_data) {
                error!("Failed to save player data: {e:?}");
            } else {
                info!("strata: Successfully saved player data to disk");
            }
        }
    }

    info!("strata: AppExit — releasing GPU device + brick pool on teardown");
}

/// Wall-clock interval between periodic DIAG lines.
const DIAG_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// Minimum gap between two SPIKE lines (avoids burst spam during streaming ramps).
const DIAG_SPIKE_COOLDOWN: std::time::Duration = std::time::Duration::from_millis(300);
/// Absolute hitch thresholds — only log SPIKE on real stalls, not 100+ FPS noise.
const DIAG_SPIKE_FRAME_MS: f32 = 25.0;
const DIAG_SPIKE_FPS: f32 = 80.0;

#[derive(Default)]
struct DiagLogState {
    last_wall_log: Option<std::time::Instant>,
    last_spike_log: Option<std::time::Instant>,
    last_frame_t: Option<std::time::Instant>,
    fps_ema: f32,
    frame_ms_ema: f32,
}

/// Windowed diagnostic: every 500 ms, or on a large FPS/frame-time spike, log player
/// pose, streaming stage timings, and GPU counters. Avoids per-frame spam while
/// still catching hitch frames.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn diagnostics_log_system(
    mut state: Local<DiagLogState>,
    pool: Res<GlobalBrickPool>,
    player: Query<(&Transform, &PlayerLook)>,
    sectors: Query<(&SectorCoord, &XBrickMap)>,
    storage: Res<strata_render::meshing::MeshStorage>,
    pending: Res<strata_render::meshing::ecs::PendingMesh>,
    wg_timers: Res<strata_world::plugin::WorldGenTimers>,
    mesh_timers: Res<strata_render::meshing::ecs::MeshingTimers>,
    phys_timers: Res<strata_physics::voxel_collider::PhysicsTimers>,
    light_timers: Res<strata_world::lighting::LightingTimers>,
    stream_timers: Res<strata_world::streaming::StreamingTimers>,
    mut gpu: ResMut<crate::client_render::ClientGpu>,
) {
    let now = std::time::Instant::now();
    let dt = match state.last_frame_t {
        Some(p) => now.saturating_duration_since(p).as_secs_f32(),
        None => 0.0,
    };
    state.last_frame_t = Some(now);
    let fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
    let frame_ms = dt * 1000.0;
    if dt > 0.0 {
        const EMA: f32 = 0.08;
        state.fps_ema = if state.fps_ema <= 0.0 {
            fps
        } else {
            state.fps_ema * (1.0 - EMA) + fps * EMA
        };
        state.frame_ms_ema = if state.frame_ms_ema <= 0.0 {
            frame_ms
        } else {
            state.frame_ms_ema * (1.0 - EMA) + frame_ms * EMA
        };
    }

    let periodic = state
        .last_wall_log
        .is_none_or(|t| now.saturating_duration_since(t) >= DIAG_LOG_INTERVAL);
    let spike_raw = dt > 0.0 && (frame_ms >= DIAG_SPIKE_FRAME_MS || fps < DIAG_SPIKE_FPS);
    let spike = spike_raw
        && state
            .last_spike_log
            .is_none_or(|t| now.saturating_duration_since(t) >= DIAG_SPIKE_COOLDOWN);

    if !periodic && !spike {
        return;
    }
    if spike {
        state.last_spike_log = Some(now);
    }
    state.last_wall_log = Some(now);

    let Some((tf, look)) = player.iter().next() else {
        info!(
            "DIAG PLAYER_QUERY_NONE -> build_camera falls back to eye=(0,0,0) which is INSIDE terrain => gray screen"
        );
        return;
    };
    let eye = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let sc = SectorCoord(
        (eye.x / 32.0).floor() as i32,
        (eye.y / 32.0).floor() as i32,
        (eye.z / 32.0).floor() as i32,
    );
    let eye_solid = sectors.iter().find(|(c, _)| **c == sc).map(|(_, m)| {
        let lx = (eye.x.floor() as i32 - sc.0 * 32) as u32;
        let ly = (eye.y.floor() as i32 - sc.1 * 32) as u32;
        let lz = (eye.z.floor() as i32 - sc.2 * 32) as u32;
        m.is_occupied(&pool, VoxelCoord::new(lx, ly, lz))
    });
    // Mirror client_render::build_camera forward vector (yaw/pitch -> direction).
    let cp = look.pitch.cos();
    let sp = look.pitch.sin();
    let forward = Vec3::new(-cp * look.yaw.sin(), sp, -cp * look.yaw.cos());
    let quads: usize = storage.meshes.values().map(|m| m.opaque.len()).sum();
    let reflatten = gpu.frame_reflatten;
    let uploaded = gpu.frame_uploaded;
    let draws = gpu.frame_draws;
    let rebuild = gpu.frame_rebuild;
    let pending_cnt = pending.len();
    let us_reflatten = gpu.frame_us_reflatten;
    let us_upload = gpu.frame_us_upload;
    let us_draw = gpu.frame_us_draw;
    let cull_total = gpu.frame_cull_total;
    let cull_visible = gpu.frame_cull_visible;
    let cull_us = gpu.frame_cull_us;
    let prepass_quads = gpu.frame_prepass_quads;
    let prepass_runs = gpu.frame_prepass_runs;
    // Reset per-second counters (they accumulate per frame in client_render_system).
    gpu.frame_reflatten = 0;
    gpu.frame_uploaded = 0;
    gpu.frame_draws = 0;
    gpu.frame_rebuild = 0;
    gpu.frame_us_reflatten = 0;
    gpu.frame_us_upload = 0;
    gpu.frame_us_draw = 0;
    gpu.frame_cull_total = 0;
    gpu.frame_cull_visible = 0;
    gpu.frame_cull_us = 0;
    gpu.frame_prepass_quads = 0;
    gpu.frame_prepass_runs = 0;
    let tag = if spike && !periodic {
        "SPIKE"
    } else if spike {
        "PERIOD+SPIKE"
    } else {
        "PERIOD"
    };
    info!(
        "DIAG[{tag}] fps={:.1} ema_fps={:.1} frame_ms={:.1} pos=({:.1},{:.1},{:.1}) eye=({:.1},{:.1},{:.1}) yaw={:.2} pitch={:.2} forward=({:.2},{:.2},{:.2}) eye_solid={:?} sectors={} quads={} reflatten={} uploaded={} draws={} rebuild={} pending={} wg_apply={} wg_n={} mesh_spawn={} mesh_n={} mesh_apply={} mesh_an={} phys_build={} phys_sort={} phys_queue={} phys_n={} phys_col={} phys_rap={} phys_apply={} phys_pend={} phys_sync={} phys_sn={} light={} light_n={} light_sky_us={} light_block_us={} light_sky_bfs={} light_block_bfs={} light_sources={} cull_total={} cull_vis={} cull_us={} prepass_quads={} prepass_runs={} stream={} stream_sp={} stream_un={} us_reflatten={} us_upload={} us_draw={}",
        fps,
        state.fps_ema,
        frame_ms,
        tf.translation.x,
        tf.translation.y,
        tf.translation.z,
        eye.x,
        eye.y,
        eye.z,
        look.yaw,
        look.pitch,
        forward.x,
        forward.y,
        forward.z,
        eye_solid,
        storage.meshes.len(),
        quads,
        reflatten,
        uploaded,
        draws,
        rebuild,
        pending_cnt,
        wg_timers.apply_us,
        wg_timers.applied,
        mesh_timers.spawn_us,
        mesh_timers.spawned,
        mesh_timers.apply_us,
        mesh_timers.applied,
        phys_timers.build_us,
        phys_timers.sort_us,
        phys_timers.queue_us,
        phys_timers.built,
        phys_timers.collect_us,
        phys_timers.rapier_us,
        phys_timers.apply_us,
        phys_timers.pending,
        phys_timers.sync_us,
        phys_timers.synced,
        light_timers.apply_us,
        light_timers.applied,
        light_timers.sky_us,
        light_timers.block_us,
        light_timers.sky_bfs_pushed,
        light_timers.block_bfs_pushed,
        light_timers.light_sources,
        cull_total,
        cull_visible,
        cull_us,
        prepass_quads,
        prepass_runs,
        stream_timers.us,
        stream_timers.spawned,
        stream_timers.unloaded,
        us_reflatten,
        us_upload,
        us_draw,
    );
}

#[tokio::main]
async fn main() {
    let config = load_config();
    let mut app = build_client_app(false, &config);
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::query::QueryState;
    use strata_render::meshing::MeshStorage;

    /// Regression: Update schedule must not report ambiguous conflicting pairs
    /// among Strata + client systems (including window-only Input/Render edges).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_update_schedule_has_no_ambiguities() {
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);
        app.add_systems(
            Update,
            (cursor_grab_system, mouse_look_system)
                .chain()
                .in_set(StrataSet::Input)
                .before(strata_player::input::input_mapper_system),
        );
        app.add_systems(
            Update,
            diagnostics_log_system
                .in_set(StrataSet::RenderUpdate)
                .after(crate::client_render::client_render_system),
        );
        app.update();
        let world = app.world();
        let schedules = world.resource::<bevy::ecs::schedule::Schedules>();
        let schedule = schedules.get(Update).expect("Update");
        let conflicts = schedule.graph().conflicting_systems();
        assert_eq!(
            conflicts.len(),
            0,
            "Update schedule has {} ambiguous pairs",
            conflicts.len()
        );
    }

    /// The app must construct (all plugins + systems registered) without panic.
    /// No window/run is performed, so this is safe headlessly.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_client_app_builds() {
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);
        for _ in 0..4 {
            app.update();
        }
        let world = app.world();
        assert!(
            world.contains_resource::<SaveManager>(),
            "SaveManager resource must be registered"
        );
        assert!(
            world.contains_resource::<SaveBackend>(),
            "SaveBackend resource must be registered"
        );
        assert!(
            world.contains_resource::<DirtyQueue>(),
            "DirtyQueue resource must be registered"
        );
        assert!(
            world.contains_resource::<strata_save::plugin::FlushScheduler>(),
            "FlushScheduler resource must be registered"
        );
        assert!(
            world.contains_resource::<GlobalBrickPool>(),
            "GlobalBrickPool resource must be registered"
        );
        assert!(
            world.contains_resource::<BlockRegistry>(),
            "BlockRegistry resource must be registered"
        );
        assert!(
            world.contains_resource::<PlayerInput>(),
            "PlayerInput resource must be registered"
        );
        let world_mut = app.world_mut();
        let mut pq = QueryState::<(&PlayerController, &PlayerLook), ()>::new(world_mut);
        let player_count = pq.iter(world_mut).count();
        assert_eq!(player_count, 1, "exactly one player entity must exist");
    }

    /// End-to-end headless pipeline check: with a player present, sectors stream
    /// in, get generated, and are meshed into `MeshStorage` (no GPU required).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_stream_gen_mesh_chain() {
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);

        for _ in 0..8 {
            app.update();
        }

        let world = app.world_mut();
        let mut state = QueryState::<&Generated, ()>::new(world);
        let generated = state.iter(world).count();
        assert!(generated > 0, "at least one sector must be generated");

        let storage = world.resource::<MeshStorage>();
        assert!(
            !storage.meshes.is_empty(),
            "generated sectors must be meshed into MeshStorage"
        );
    }

    /// Diagnostic: simulate the full fall from the spawn point and report where
    /// the player ends up relative to the terrain surface, and whether its eye
    /// voxel is solid (buried => gray screen). Prints with `--nocapture`.
    #[tokio::test]
    #[ignore = "diagnostic: heavy (600 sim frames); run explicitly with --ignored"]
    async fn diag_player_settles_above_terrain() {
        use strata_player::controller::{EYE_HEIGHT, PlayerController};

        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);

        // ~10 s at the controller's fixed 1/60 dt: well past the ~2 s free-fall.
        for _ in 0..600 {
            app.update();
        }

        let world = app.world_mut();

        // Build query states up front (each mutably borrows the world only briefly).
        let mut pq = QueryState::<&Transform, With<PlayerController>>::new(world);
        let mut sq = QueryState::<(&SectorCoord, &XBrickMap)>::new(world);

        // Final player position.
        let pos = pq.iter(world).next().expect("player exists").translation;

        // Terrain surface height at the spawn column.
        let biome = strata_world::biome::biome_at(16, 16);
        let surface = strata_world::generator::height_at(16, 16, biome);

        // Is the eye voxel solid? Look up the sector containing the eye.
        let eye = pos + Vec3::new(0.0, EYE_HEIGHT, 0.0);
        let sc = SectorCoord(
            (eye.x / 32.0).floor() as i32,
            (eye.y / 32.0).floor() as i32,
            (eye.z / 32.0).floor() as i32,
        );
        let pool = world.resource::<GlobalBrickPool>();
        let eye_solid = sq.iter(world).find(|(c, _)| **c == sc).map(|(_, m)| {
            let lx = (eye.x.floor() as i32 - sc.0 * 32) as u32;
            let ly = (eye.y.floor() as i32 - sc.1 * 32) as u32;
            let lz = (eye.z.floor() as i32 - sc.2 * 32) as u32;
            m.is_occupied(pool, VoxelCoord::new(lx, ly, lz))
        });

        eprintln!(
            "DIAG: spawn.y={}, final pos={:?}, terrain_surface_y={}, eye.y={}, eye_sector={:?}, eye_solid={:?}",
            spawn_position().y,
            pos,
            surface,
            eye.y,
            sc,
            eye_solid
        );
    }

    /// Render the REAL streamed+meshed scene from the REAL player camera on the
    /// GPU and report the vertical sky/terrain composition (top/middle/bottom
    /// center-column colors). Reveals whether the "gray screen" is terrain, sky,
    /// or a broken frame. Skipped when no capable GPU is present.
    #[tokio::test]
    #[ignore = "diagnostic: heavy (600 sim frames + GPU); run explicitly with --ignored"]
    async fn diag_render_from_player_view() {
        use strata_player::controller::{EYE_HEIGHT, PlayerController};
        use strata_render::meshing::{MeshData, MeshStorage};
        use strata_render::pipeline::camera::{look_at_rh, perspective_rh_zo};
        use strata_render::pipeline::{CameraView, Renderer, prepass_features};
        use wgpu::*;

        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let adapter = match pollster::block_on(instance.enumerate_adapters(Backends::all()))
            .into_iter()
            .find(|a| a.features().contains(prepass_features()))
        {
            Some(a) => a,
            None => {
                eprintln!("diag_render_from_player_view IGNORED: no capable GPU");
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("diag_device"),
            required_features: prepass_features(),
            ..Default::default()
        }))
        .expect("device");

        // Run the real pipeline until the player settles and sectors are meshed.
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);
        for _ in 0..600 {
            app.update();
        }

        let world = app.world_mut();
        let mut pq = QueryState::<&Transform, With<PlayerController>>::new(world);
        let pos = pq.iter(world).next().expect("player").translation;

        // Mirror client_render::build_camera exactly.
        const W: u32 = 256;
        const H: u32 = 256;
        let eye = pos + Vec3::new(0.0, EYE_HEIGHT, 0.0);
        let (yaw, pitch) = (0.0f32, 0.0f32);
        let forward = Vec3::new(
            -pitch.cos() * yaw.sin(),
            pitch.sin(),
            -pitch.cos() * yaw.cos(),
        );
        let center = eye + forward;
        let proj = perspective_rh_zo(
            std::f32::consts::FRAC_PI_4,
            W as f32 / H as f32,
            0.1,
            2000.0,
        );
        let view = look_at_rh(
            [eye.x, eye.y, eye.z],
            [center.x, center.y, center.z],
            [0.0, 1.0, 0.0],
        );
        let cam = CameraView::new([eye.x, eye.y, eye.z], view, proj, W, H);

        // Pull the real meshes + world origins out of MeshStorage.
        let storage = world.resource::<MeshStorage>();
        let mut meshes: Vec<MeshData> = Vec::new();
        let mut origins: Vec<[f32; 3]> = Vec::new();
        for (coord, mesh) in storage.meshes.iter() {
            meshes.push(mesh.clone());
            origins.push([
                (coord.0 * 32) as f32,
                (coord.1 * 32) as f32,
                (coord.2 * 32) as f32,
            ]);
        }
        let total_quads: usize = meshes.iter().map(|m| m.opaque.len()).sum();

        let mut renderer = Renderer::new(device, queue, W, H);
        renderer.render_frame(&meshes, &origins, &cam);
        let px = renderer.readback();

        let f16 = |b: &[u8], o: usize| {
            let bits = u16::from_le_bytes([b[o], b[o + 1]]);
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
        };
        let row = (W * 8) as usize;
        let sample = |y: usize| {
            let o = y * row + (W as usize / 2) * 8;
            (f16(&px, o), f16(&px, o + 2), f16(&px, o + 4))
        };

        // Classify every pixel: "sky" if noticeably blue (b > r + 0.1).
        let mut sky = 0usize;
        for y in 0..H as usize {
            for x in 0..W as usize {
                let o = y * row + x * 8;
                if f16(&px, o + 4) > f16(&px, o) + 0.1 {
                    sky += 1;
                }
            }
        }
        eprintln!(
            "DIAG-RENDER: sectors={}, total_opaque_quads={}",
            meshes.len(),
            total_quads
        );
        eprintln!(
            "DIAG-RENDER: eye={:?} sky_pixels={}/{} ({:.0}%)",
            eye,
            sky,
            W * H,
            100.0 * sky as f32 / (W * H) as f32
        );
        eprintln!("DIAG-RENDER: top(y=16)={:?}", sample(16));
        eprintln!("DIAG-RENDER: middle(y=128)={:?}", sample(128));
        eprintln!("DIAG-RENDER: bottom(y=240)={:?}", sample(240));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_save_and_shutdown_handle_missing_player() {
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);
        for _ in 0..4 {
            app.update();
        }
        let world = app.world_mut();
        let mut pq = QueryState::<Entity, With<PlayerController>>::new(world);
        let entities: Vec<_> = pq.iter(world).collect();
        for entity in entities {
            world.despawn(entity);
        }
        app.update();
    }

    #[test]
    fn test_load_config_clamps_values() {
        use std::env;
        use std::fs;
        let tmp = env::temp_dir().join("strata_test_cfg");
        let _ = fs::create_dir_all(&tmp);
        let cfg_path = tmp.join("client.toml");
        fs::write(&cfg_path, "radius = 9999\nwidth = 99999\nheight = 99999\n")
            .expect("write config");
        let orig = env::current_dir().expect("cwd");
        env::set_current_dir(&tmp).expect("chdir");
        let cfg = load_config();
        env::set_current_dir(orig).expect("chdir back");
        assert_eq!(cfg.radius, 16, "radius should be clamped to 16");
        assert_eq!(cfg.width, 7680, "width should be clamped to 7680");
        assert_eq!(cfg.height, 4320, "height should be clamped to 4320");
    }

    #[test]
    fn test_player_save_due_debounces_until_interval() {
        let mut elapsed = std::time::Duration::ZERO;
        let interval = std::time::Duration::from_secs(30);
        assert!(
            !player_save_due(&mut elapsed, std::time::Duration::from_millis(16), interval),
            "first frame must not save"
        );
        assert!(
            !player_save_due(&mut elapsed, std::time::Duration::from_secs(29), interval),
            "under interval must not save"
        );
        assert!(
            player_save_due(&mut elapsed, std::time::Duration::from_secs(1), interval),
            "at interval must save"
        );
        assert_eq!(elapsed, std::time::Duration::ZERO, "timer resets after due");
        assert!(
            !player_save_due(&mut elapsed, std::time::Duration::from_millis(16), interval),
            "after reset must wait again"
        );
    }

    /// Inventory encoding for player.dat — no Bevy app / AppExit (avoids
    /// current_thread + backend sync deadlock).
    #[test]
    fn test_player_save_data_encodes_inventory() {
        let transform = Transform::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let look = PlayerLook {
            yaw: 0.5,
            pitch: -0.25,
        };
        let mut inventory = Inventory::default();
        inventory.hotbar[0] = ItemStack {
            block: BlockId(42),
            count: 99,
        };
        inventory.active = 3;
        let data = player_save_data_from(&transform, &look, &inventory);
        assert_eq!(data.hotbar_index, 3);
        assert_eq!(data.inventory.len(), 9);
        assert_eq!(
            data.inventory[0],
            Some(strata_save::player_save_data::ItemStack {
                block_id: 42,
                count: 99,
            })
        );
        assert_eq!(data.position, [1.0, 2.0, 3.0]);
        assert_eq!(data.rotation, [0.5, -0.25]);
    }

    /// Sync envelope write of player.dat — no AppExit / TokioBackend.sync.
    #[test]
    fn test_save_player_envelope_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("player.dat");
        let meta = std::sync::Arc::new(strata_storage::metadata::InMemoryMetadata::new());
        let mgr = SaveManager::new(meta, PLAYER_SAVE_INTERVAL);
        let mut inventory = Inventory::default();
        inventory.hotbar[0] = ItemStack {
            block: BlockId(42),
            count: 99,
        };
        inventory.active = 3;
        let data = player_save_data_from(
            &Transform::from_translation(Vec3::ZERO),
            &PlayerLook::default(),
            &inventory,
        );
        mgr.save_player(&path, &data).expect("save");
        let loaded = mgr.load_player(&path).expect("load");
        assert_eq!(loaded.hotbar_index, 3);
        assert_eq!(
            loaded.inventory[0],
            Some(strata_save::player_save_data::ItemStack {
                block_id: 42,
                count: 99,
            })
        );
    }

    #[test]
    fn test_shutdown_flush_coords_merges_queued_and_loaded() {
        let a = SectorCoord(0, 0, 0);
        let b = SectorCoord(1, 0, 0);
        let c = SectorCoord(2, 0, 0);
        let merged = collect_shutdown_flush_coords(vec![a, b], [b, c]);
        assert_eq!(merged, vec![a, b, c]);
    }
}
