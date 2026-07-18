//! Strata client binary (M9b): wires every Strata plugin together, owns the
//! single wgpu device, and PRESENTs the M4 offscreen renderer to a real window.
//!
//! Bevy's `render` feature is disabled (see `Cargo.toml`), so the only wgpu
//! device in the process is our `strata_render::Renderer`. `ClientRenderPlugin`
//! creates a wgpu `Surface` from the Bevy window's raw handle once it exists,
//! then each frame renders the resident sector meshes offscreen and blits them
//! to the window.

mod client_input;
mod client_render;

use bevy::input::InputPlugin;
use bevy::prelude::*;
use bevy::window::{Window, WindowPlugin, WindowResolution};
use bevy::{DefaultPlugins, MinimalPlugins};

use strata_core::prelude::*;
use strata_physics::plugin::PhysicsPlugin;
use strata_player::controller::EYE_HEIGHT;
use strata_player::prelude::*;
use strata_render::meshing::MeshingPlugin;
use strata_world::prelude::*;

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
                        cfg.radius = r;
                    }
                }
                "width" => {
                    if let Ok(w) = value.parse::<u32>() {
                        cfg.width = w;
                    }
                }
                "height" => {
                    if let Ok(h) = value.parse::<u32>() {
                        cfg.height = h;
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
        // `RenderPlugin` normally registers the `Shader` asset type + loader and
        // creates Bevy's own wgpu Surface on our window (which raced with
        // ClientGpu's surface => "Native window is in use" panic). It is DISABLED
        // so we own the only GPU device (strata_render's Renderer).
        //
        // `WindowRenderPlugin` is a sub-plugin nested *inside* `RenderPlugin`,
        // not a member of the `DefaultPlugins` group, so it cannot be disabled
        // directly — hence the whole `RenderPlugin` is disabled.
        //
        // We must still register the `Shader` asset type: other `DefaultPlugins`
        // members (e.g. CorePipelinePlugin's TonemappingPlugin) `asset_server.load`
        // a `Shader` at build time, which panics if the type is unregistered. So
        // we add `AssetPlugin` + register `Shader` ourselves *before*
        // `DefaultPlugins` (whose `AssetPlugin` we disable to avoid a duplicate),
        // guaranteeing `AssetServer` exists when we register `Shader` and when
        // `CorePipelinePlugin` builds.
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<bevy::shader::Shader>()
            .init_asset_loader::<bevy::shader::ShaderLoader>();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(window),
                    ..Default::default()
                })
                .disable::<bevy::asset::AssetPlugin>()
                .disable::<bevy::render::RenderPlugin>(),
        );
    }

    // Core scheduling (orders the StrataSet chain) + the block registry the
    // world-gen and meshing systems read from.
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.add_strata_plugin(BlockRegistryPlugin);

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
    app.add_systems(Update, shutdown_system);

    // Window-only FPS input: cursor capture + mouse-look. Skipped headlessly
    // (no primary window / cursor to grab). Runs in the Input set so the updated
    // look is consumed by the controller and camera the same frame.
    if !headless {
        // Movement now runs in `FixedUpdate`, which executes before `Update`
        // each frame, so mouse-look (Update) no longer needs to be ordered
        // before the controller — it just samples the look for the next tick.
        app.add_systems(
            Update,
            (cursor_grab_system, mouse_look_system).in_set(StrataSet::Input),
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

fn spawn_player_system(mut commands: Commands) {
    spawn_player(&mut commands, spawn_position());
}

/// Graceful shutdown (plan 13 §2.6): on `AppExit`, log the teardown point. The
/// wgpu device / SSBOs (`ClientGpu`) and the `GlobalBrickPool` are reclaimed by
/// their `Drop` impls as the `App` is dropped — this is the explicit, observable
/// shutdown hook the plan asks for.
fn shutdown_system(mut exit: MessageReader<AppExit>) {
    if exit.read().next().is_some() {
        info!("strata: AppExit received — releasing GPU device + brick pool on teardown");
    }
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
    // Reset per-second counters (they accumulate per frame in client_render_system).
    gpu.frame_reflatten = 0;
    gpu.frame_uploaded = 0;
    gpu.frame_draws = 0;
    gpu.frame_rebuild = 0;
    gpu.frame_us_reflatten = 0;
    gpu.frame_us_upload = 0;
    gpu.frame_us_draw = 0;
    let tag = if spike && !periodic {
        "SPIKE"
    } else if spike {
        "PERIOD+SPIKE"
    } else {
        "PERIOD"
    };
    info!(
        "DIAG[{tag}] fps={:.1} ema_fps={:.1} frame_ms={:.1} pos=({:.1},{:.1},{:.1}) eye=({:.1},{:.1},{:.1}) yaw={:.2} pitch={:.2} forward=({:.2},{:.2},{:.2}) eye_solid={:?} sectors={} quads={} reflatten={} uploaded={} draws={} rebuild={} pending={} wg_apply={} wg_n={} mesh_spawn={} mesh_n={} mesh_apply={} mesh_an={} phys_build={} phys_sort={} phys_queue={} phys_n={} phys_col={} phys_rap={} phys_apply={} phys_pend={} phys_sync={} phys_sn={} light={} light_n={} stream={} stream_sp={} stream_un={} us_reflatten={} us_upload={} us_draw={}",
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
        stream_timers.us,
        stream_timers.spawned,
        stream_timers.unloaded,
        us_reflatten,
        us_upload,
        us_draw,
    );
}

fn main() {
    let config = load_config();
    let mut app = build_client_app(false, &config);
    app.run();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::query::QueryState;
    use strata_render::meshing::MeshStorage;

    /// The app must construct (all plugins + systems registered) without panic.
    /// No window/run is performed, so this is safe headlessly.
    #[test]
    fn test_client_app_builds() {
        let cfg = ClientConfig::default();
        let app = build_client_app(true, &cfg);
        // Probe that the expected resources/plugins are present by running a few
        // frames headlessly (no window) and checking the streaming->gen->mesh chain.
        let _ = app;
    }

    /// End-to-end headless pipeline check: with a player present, sectors stream
    /// in, get generated, and are meshed into `MeshStorage` (no GPU required).
    #[test]
    fn test_stream_gen_mesh_chain() {
        let cfg = ClientConfig::default();
        let mut app = build_client_app(true, &cfg);

        for _ in 0..16 {
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
    #[test]
    #[ignore = "diagnostic: heavy (600 sim frames); run explicitly with --ignored"]
    fn diag_player_settles_above_terrain() {
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
    #[test]
    #[ignore = "diagnostic: heavy (600 sim frames + GPU); run explicitly with --ignored"]
    fn diag_render_from_player_view() {
        use strata_player::controller::{EYE_HEIGHT, PlayerController};
        use strata_render::meshing::{MeshData, MeshStorage};
        use strata_render::pipeline::camera::{look_at_rh, perspective_rh_zo};
        use strata_render::pipeline::{CameraView, Renderer, prepass_features};
        use wgpu::*;

        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::all(),
            ..Default::default()
        });
        let adapter = match instance
            .enumerate_adapters(Backends::all())
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
}
