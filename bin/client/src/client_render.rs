//! Client-side GPU ownership + window presentation (M9b).
//!
//! `ClientGpu` owns the wgpu `Instance`/`Surface`/`Device`/`Queue` and the
//! `strata_render::Renderer`. The surface is created lazily once Bevy's window
//! exposes a [`RawHandleWrapper`] (the winit backend inserts it after the OS
//! window exists). Each frame the offscreen HDR target is rendered and blitted to
//! the window surface.

use bevy::prelude::*;
use bevy::window::RawHandleWrapper;

use strata_core::prelude::*;
use strata_player::controller::EYE_HEIGHT;
use strata_player::PlayerLook;
use strata_world::prelude::*;
use strata_render::meshing::{MeshData, MeshStorage};
use strata_render::pipeline::camera::{look_at_rh, perspective_rh_zo};
use strata_render::pipeline::{CameraView, Renderer};

use wgpu::*;

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
        app.add_systems(PostUpdate, mark_generated_for_remesh);
        app.add_systems(
            Update,
            client_render_system.in_set(StrataSet::RenderUpdate),
        );
    }
}

/// Mark freshly `Generated` sectors (that have no mesh yet) for greedy meshing.
/// Filter-first: only sectors missing both `NeedsRemesh` and a `MeshStorage`
/// entry are queued, so each sector is meshed exactly once (no per-frame churn).
#[allow(clippy::type_complexity)]
fn mark_generated_for_remesh(
    storage: Res<MeshStorage>,
    mut commands: Commands,
    q: Query<(Entity, &SectorCoord), (With<Generated>, Without<NeedsRemesh>)>,
) {
    for (e, coord) in &q {
        if !storage.meshes.contains_key(coord) {
            commands.entity(e).insert(NeedsRemesh);
        }
    }
}

/// Owns the wgpu device and presents the offscreen render to the window.
fn client_render_system(
    mut gpu: ResMut<ClientGpu>,
    windows: Query<(Entity, &Window, &RawHandleWrapper)>,
    player: Query<(&Transform, &PlayerLook)>,
    storage: Res<MeshStorage>,
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

    // Keep the surface + offscreen target sized to the window.
    let w = window.resolution.width().max(1.0) as u32;
    let h = window.resolution.height().max(1.0) as u32;
    if w != gpu.width || h != gpu.height {
        let cfg = gpu.config.as_ref().unwrap();
        let mut new_cfg = cfg.clone();
        new_cfg.width = w;
        new_cfg.height = h;
        {
            let device = gpu.renderer.as_ref().unwrap().device();
            gpu.surface.as_ref().unwrap().configure(device, &new_cfg);
        }
        gpu.renderer.as_mut().unwrap().resize(w, h);
        gpu.config = Some(new_cfg);
        gpu.width = w;
        gpu.height = h;
    }

    let meshes: Vec<MeshData> = storage.meshes.values().cloned().collect();
    let camera = build_camera(&player, gpu.width, gpu.height);
    let format = gpu.surface_format.unwrap();

    // Acquire the current surface texture (ends the immutable gpu.surface borrow
    // immediately, so the renderer can be borrowed mutably next).
    let frame = match gpu.surface.as_ref().unwrap().get_current_texture() {
        Ok(f) => f,
        Err(e) => {
            warn!("strata: surface not available this frame: {e:?}");
            return;
        }
    };

    let view = frame.texture.create_view(&TextureViewDescriptor::default());
    let renderer = gpu.renderer.as_mut().unwrap();
    renderer.render_frame(&meshes, &camera);
    renderer.present(&view, format);
    frame.present();
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

    let adapter = match pollster::block_on(gpu.instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::default(),
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    })) {
        Ok(a) => a,
        Err(e) => {
            error!("strata: no wgpu adapter available: {e:?}");
            return false;
        }
    };

    let (device, queue) = match pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("strata_client"),
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

    let w = window.resolution.width().max(1.0) as u32;
    let h = window.resolution.height().max(1.0) as u32;

    let config = SurfaceConfiguration {
        usage: TextureUsages::RENDER_ATTACHMENT,
        format,
        width: w,
        height: h,
        present_mode: PresentMode::AutoVsync,
        alpha_mode: CompositeAlphaMode::Opaque,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
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
fn build_camera(
    player: &Query<(&Transform, &PlayerLook)>,
    width: u32,
    height: u32,
) -> CameraView {
    let (eye, yaw, pitch) = match player.iter().next() {
        Some((tf, look)) => {
            let e = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
            (e, look.yaw, look.pitch)
        }
        None => (Vec3::ZERO, 0.0, 0.0),
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
