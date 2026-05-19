#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::Result;
use glam::Vec3;
use std::sync::Arc;
use std::time::{Duration, Instant};
use strata_core::{BlockId, BlockPos, BlockProperties, BlockRegistry, ChunkPos};
use strata_render::engine::RenderEngine;
use tracing::info;
use winit::event::{ElementState, MouseButton};
use winit::keyboard::Key;
use winit::window::CursorGrabMode;

const PLAYER_GRAVITY: f32 = -20.0;
const PLAYER_WALK_SPEED: f32 = 10.0;
const PLAYER_SPRINT_SPEED: f32 = 15.0;
const PLAYER_JUMP_VELOCITY: f32 = 8.0;
const PLAYER_HALF_WIDTH: f32 = 0.3;
const PLAYER_HEIGHT: f32 = 1.8;

mod camera;
mod chunk_gen_worker;
mod debug_overlay;
mod dirty_manager;
mod input;
mod lazy_loader;
mod mesh_worker;
mod render;
mod world;

use camera::{Camera, CameraController};
use chunk_gen_worker::ChunkGenWorker;
use debug_overlay::DebugOverlay;
use dirty_manager::DirtyChunkManager;
use input::InputState;
use lazy_loader::LazyChunkLoader;
use world::WorldManager;

struct App {
    registry: BlockRegistry,
    window: Option<Arc<winit::window::Window>>,
    render: Option<RenderEngine>,
    camera: Camera,
    camera_controller: CameraController,
    world: WorldManager,
    input: InputState,
    lazy_loader: LazyChunkLoader,
    chunk_gen_worker: ChunkGenWorker,
    dirty_manager: DirtyChunkManager,
    debug: DebugOverlay,
    frame_count: u32,
    fps_timer: Instant,
    current_fps: f32,
    last_frame_time: Instant,
    render_distance: u32,
    last_player_chunk: Option<ChunkPos>,
    last_update_pos: Option<glam::Vec3>,
    player_velocity: Vec3,
    grounded: bool,
}

impl App {
    fn new() -> Self {
        let mut registry = BlockRegistry::new();
        registry.register(BlockProperties {
            id: BlockId::STONE,
            name: "stone",
            transparent: false,
            solid: true,
            hardness: 2.0,
            light_emission: 0,
            face_textures: [1, 1, 1, 1, 1, 1],
        });
        registry.register(BlockProperties {
            id: BlockId::DIRT,
            name: "dirt",
            transparent: false,
            solid: true,
            hardness: 0.6,
            light_emission: 0,
            face_textures: [2, 2, 2, 2, 2, 2],
        });
        registry.register(BlockProperties {
            id: BlockId::GRASS,
            name: "grass",
            transparent: false,
            solid: true,
            hardness: 0.6,
            light_emission: 0,
            face_textures: [3, 3, 2, 4, 3, 3],
        });
        registry.register(BlockProperties {
            id: BlockId::BEDROCK,
            name: "bedrock",
            transparent: false,
            solid: true,
            hardness: 999.0,
            light_emission: 0,
            face_textures: [5, 5, 5, 5, 5, 5],
        });
        registry.register(BlockProperties {
            id: BlockId::WOOD,
            name: "wood",
            transparent: false,
            solid: true,
            hardness: 1.5,
            light_emission: 0,
            face_textures: [6, 6, 6, 6, 6, 6],
        });
        registry.register(BlockProperties {
            id: BlockId::LEAVES,
            name: "leaves",
            transparent: true,
            solid: true,
            hardness: 0.3,
            light_emission: 0,
            face_textures: [7, 7, 7, 7, 7, 7],
        });

        let mut world = WorldManager::new(42);

        // Generate initial chunks synchronously (needed for spawn height)
        for x in -2..=2 {
            for z in -2..=2 {
                let pos = ChunkPos(glam::IVec2::new(x, z));
                world.get_or_generate(pos);
            }
        }

        // Create background worker pool (4 threads for chunk gen + meshing)
        let chunk_gen_worker = ChunkGenWorker::new(42, "world_data", 4);

        let terrain_floor = world.terrain_height_at(0, 0).floor();
        let spawn_height = terrain_floor + 2.8; // surface + eye height (1.8)
        let mut camera = Camera::new(1280.0 / 720.0);
        camera.position.y = spawn_height;
        camera.pitch = -0.3;

        Self {
            registry,
            window: None,
            render: None,
            camera,
            camera_controller: CameraController::default(),
            world,
            input: InputState::default(),
            lazy_loader: LazyChunkLoader::new(),
            chunk_gen_worker,
            dirty_manager: DirtyChunkManager::new(),
            debug: DebugOverlay::new(),
            frame_count: 0,
            fps_timer: Instant::now(),
            current_fps: 0.0,
            last_frame_time: Instant::now(),
            render_distance: 8,
            last_player_chunk: None,
            last_update_pos: None,
            player_velocity: Vec3::ZERO,
            grounded: false,
        }
    }
}

impl winit::application::ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_none() {
            let window = Arc::new(
                event_loop
                    .create_window(
                        winit::window::WindowAttributes::default()
                            .with_title("Strata - Phase 2")
                            .with_inner_size(winit::dpi::PhysicalSize::new(1280u32, 720u32)),
                    )
                    .unwrap(),
            );
            info!("Window created");
            let render = pollster::block_on(RenderEngine::new(Arc::clone(&window), &self.registry));
            match render {
                Ok(mut engine) => {
                    self.world.init_mesh_worker();
                    self.debug.init(&engine.device, engine.config.format);
                    // Upload initial CPU-built meshes
                    for x in -2..=2 {
                        for z in -2..=2 {
                            let pos = ChunkPos(glam::IVec2::new(x, z));
                            if let Some(mesh) = self.world.get_mesh(pos) {
                                engine.chunk_renderer.upload_mesh(&engine.device, pos, mesh);
                            }
                        }
                    }
                    self.render = Some(engine);
                    self.window = Some(window);
                    let w = self.window.as_ref().unwrap();
                    w.set_cursor_visible(false);
                    let _ = w
                        .set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined));
                }
                Err(e) => {
                    tracing::error!("Failed to create RenderEngine: {}", e);
                }
            }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: winit::event::DeviceEvent,
    ) {
        if let winit::event::DeviceEvent::MouseMotion { delta, .. } = event {
            self.input
                .handle_mouse_motion(winit::dpi::PhysicalPosition::new(delta.0, delta.1));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: winit::event::WindowEvent,
    ) {
        match event {
            winit::event::WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                self.input
                    .handle_keyboard_input(&event.logical_key, event.state);

                if event.state == ElementState::Pressed
                    && let Key::Character(ch) = &event.logical_key
                    && ch.as_str() == "q"
                {
                    event_loop.exit();
                }
            }
            winit::event::WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed
                    && let Some(window) = &self.window
                {
                    window.set_cursor_visible(false);
                    let _ = window
                        .set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
                }

                if state == ElementState::Pressed && self.render.is_some() {
                    let (origin, forward) = {
                        let (s, c) = self.camera.pitch.sin_cos();
                        let (sy, cy) = self.camera.yaw.sin_cos();
                        let fwd = glam::Vec3::new(cy * c, s, sy * c);
                        (self.camera.position, fwd)
                    };

                    match button {
                        MouseButton::Left => {
                            if let Some((pos, _)) = self
                                .world
                                .raycast(origin, forward, 6.0)
                                && let Some((chunk_pos, _, _, _)) = pos.to_chunk_local()
                            {
                                self.world.break_block(pos);
                                self.dirty_manager.mark_dirty(chunk_pos);
                            }
                        }
                        MouseButton::Right => {
                            if let Some((pos, normal)) = self
                                .world
                                .raycast(origin, forward, 6.0)
                            {
                                let adjacent = BlockPos(pos.0 - normal);
                                // Don't place block inside player
                                let block_min = adjacent.0.as_vec3();
                                let block_max = adjacent.0.as_vec3() + glam::Vec3::ONE;
                                let player_min = glam::Vec3::new(
                                    self.camera.position.x - PLAYER_HALF_WIDTH,
                                    self.camera.position.y - PLAYER_HEIGHT,
                                    self.camera.position.z - PLAYER_HALF_WIDTH,
                                );
                                let player_max = glam::Vec3::new(
                                    self.camera.position.x + PLAYER_HALF_WIDTH,
                                    self.camera.position.y,
                                    self.camera.position.z + PLAYER_HALF_WIDTH,
                                );
                                let overlaps = block_min.x < player_max.x
                                    && block_max.x > player_min.x
                                    && block_min.y < player_max.y
                                    && block_max.y > player_min.y
                                    && block_min.z < player_max.z
                                    && block_max.z > player_min.z;
                                if !overlaps
                                    && let Some((chunk_pos, _, _, _)) = adjacent.to_chunk_local()
                                {
                                    self.world.place_block(adjacent, BlockId::STONE);
                                    self.dirty_manager.mark_dirty(chunk_pos);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            winit::event::WindowEvent::Resized(size) => {
                if let Some(render) = &mut self.render {
                    render.resize(size.width, size.height);
                }
            }
            winit::event::WindowEvent::CursorEntered { .. } => {
                if let Some(window) = &self.window {
                    window.set_cursor_visible(false);
                    let _ = window
                        .set_cursor_grab(CursorGrabMode::Locked)
                        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
                }
            }
            winit::event::WindowEvent::RedrawRequested => {
                // 7. Render
                if let Some(render) = &mut self.render {
                    // Sync camera
                    render.camera = self.camera.clone();
                    render.update_camera();

                    // Frustum culling (uses cached chunk keys)
                    render.chunk_renderer.cull(&render.frustum, self.world.get_chunk_keys());

                    // Update debug stats
                    self.debug.visible_chunks = render.chunk_renderer.visible_count();
                    self.debug.chunk_count = render.chunk_renderer.total_count();
                    self.debug.player_position = (
                        self.camera.position.x,
                        self.camera.position.y,
                        self.camera.position.z,
                    );

                    let output = render.render_frame();
                    if let Some(mut output) = output {
                        self.debug.render(
                            &render.device,
                            &render.queue,
                            &mut output.encoder,
                            render.config.format,
                            &output.view,
                            render.config.width,
                            render.config.height,
                        );
                        render
                            .queue
                            .submit(std::iter::once(output.encoder.finish()));
                        output.frame.present();
                    }
                }

                // 8. FPS update + window title
                if let Some(window) = &self.window {
                    self.frame_count += 1;
                    if self.fps_timer.elapsed() >= Duration::from_secs(1) {
                        self.current_fps = self.frame_count as f32 / self.fps_timer.elapsed().as_secs_f32();
                        self.debug.fps = self.current_fps;
                        window.set_title(&self.debug.debug_string());
                        self.frame_count = 0;
                        self.fps_timer = Instant::now();
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame_time).as_secs_f32();
        self.last_frame_time = now;

        // 1. Camera rotation (yaw/pitch from mouse)
        let (yaw_delta, pitch_delta) = self.input.cursor_delta();
        self.camera.yaw -= yaw_delta * self.camera_controller.mouse_sensitivity;
        self.camera.pitch -= pitch_delta * self.camera_controller.mouse_sensitivity;
        self.camera.pitch = self.camera.pitch.clamp(-1.55, 1.55);
        self.input.update();

        // 2. Player physics: movement + gravity + collision
        let (sin_yaw, cos_yaw) = self.camera.yaw.sin_cos();
        let forward_dir = Vec3::new(cos_yaw, 0.0, sin_yaw).normalize();
        let right_dir = Vec3::new(sin_yaw, 0.0, -cos_yaw).normalize();

        let move_input = Vec3::new(
            self.input.strafe() as f32,
            0.0,
            self.input.forward() as f32,
        );
        let move_speed = if self.input.sprint {
            PLAYER_SPRINT_SPEED
        } else {
            PLAYER_WALK_SPEED
        };

        // Direct velocity from input (instant response, no acceleration lag)
        if move_input.length_squared() > 0.0 {
            let wish_dir = (forward_dir * move_input.z + right_dir * move_input.x).normalize();
            self.player_velocity.x = wish_dir.x * move_speed;
            self.player_velocity.z = wish_dir.z * move_speed;
        } else {
            self.player_velocity.x = 0.0;
            self.player_velocity.z = 0.0;
        }

        // Ground check: test if AABB shifted slightly downward collides with blocks
        let aabb_min = |pos: Vec3| -> Vec3 {
            Vec3::new(
                pos.x - PLAYER_HALF_WIDTH,
                pos.y - PLAYER_HEIGHT,
                pos.z - PLAYER_HALF_WIDTH,
            )
        };
        let aabb_max = |pos: Vec3| -> Vec3 {
            Vec3::new(
                pos.x + PLAYER_HALF_WIDTH,
                pos.y,
                pos.z + PLAYER_HALF_WIDTH,
            )
        };
        let cam = self.camera.position;
        let below_min = Vec3::new(cam.x - PLAYER_HALF_WIDTH, cam.y - PLAYER_HEIGHT - 0.01, cam.z - PLAYER_HALF_WIDTH);
        let below_max = Vec3::new(cam.x + PLAYER_HALF_WIDTH, cam.y - 0.01, cam.z + PLAYER_HALF_WIDTH);
        self.grounded = self.world.is_colliding(below_min, below_max);
        self.debug.grounded = self.grounded;
        self.debug.space_pressed = self.input.jump;

        // Jump (before gravity so constant jump height regardless of frame rate)
        if self.input.jump && self.grounded {
            self.player_velocity.y = PLAYER_JUMP_VELOCITY;
        }

        // Gravity (applied to velocity, not position)
        self.player_velocity.y += PLAYER_GRAVITY * dt;

        // Axis-by-axis collision resolution
        let dt = dt.min(0.05); // cap to avoid tunneling through blocks
        let mut new_pos = self.camera.position;

        // X axis
        new_pos.x += self.player_velocity.x * dt;
        if self.world.is_colliding(aabb_min(new_pos), aabb_max(new_pos)) {
            new_pos.x = self.camera.position.x;
            self.player_velocity.x = 0.0;
        }

        // Y axis
        new_pos.y += self.player_velocity.y * dt;
        if self.world.is_colliding(aabb_min(new_pos), aabb_max(new_pos)) {
            new_pos.y = self.camera.position.y;
            self.player_velocity.y = 0.0;
        }

        // Z axis
        new_pos.z += self.player_velocity.z * dt;
        if self.world.is_colliding(aabb_min(new_pos), aabb_max(new_pos)) {
            new_pos.z = self.camera.position.z;
            self.player_velocity.z = 0.0;
        }

        self.camera.position = new_pos;

        // 2. Lazy chunk loading and unloading based on player position (gated on chunk boundary crossing + hysteresis)
        let player_pos = self.camera.position;
        let should_update = if let Some(last_pos) = self.last_update_pos {
            let dx = player_pos.x - last_pos.x;
            let dz = player_pos.z - last_pos.z;
            (dx * dx + dz * dz) > 64.0 // 8.0 blocks squared (half a chunk distance threshold)
        } else {
            true
        };

        if should_update {
            let player_chunk =
                ChunkPos::from_world(player_pos.x as i32, player_pos.z as i32);
            let required = self
                .world
                .get_required_chunks(player_chunk, self.render_distance);
            self.lazy_loader.request_chunks(&required);
            self.lazy_loader.prioritize(player_chunk);

            // 4. Unload distant chunks and clean up GPU buffers
            let removed = self.world
                .unload_distant_chunks(player_chunk, self.render_distance);
            if let Some(render) = &mut self.render {
                for pos in &removed {
                    render.chunk_renderer.remove_mesh(*pos);
                }
            }

            self.last_player_chunk = Some(player_chunk);
            self.last_update_pos = Some(player_pos);
        }

        // Submit chunk gen requests to background workers (non-blocking, throttled per frame)
        self.lazy_loader.process(&self.chunk_gen_worker);

        // 3. Poll for completed chunks from background workers and upload to GPU
        {
            let completed = self.chunk_gen_worker.poll();
            for result in completed {
                self.lazy_loader.mark_completed(result.pos);
                self.world.insert_generated_chunk(result.pos, result.chunk);
                if let Some(render) = &mut self.render {
                    render.chunk_renderer.upload_mesh(&render.device, result.pos, &result.mesh);
                }
            }
        }

        // 5. Propagate light for light-dirty chunks (throttled: 4/frame)
        self.world.propagate_light();

        // 6. Process dirty chunks (submits mesh rebuilds to background thread)
        let _rebuilt = self.dirty_manager.process(&mut self.world);
        // Poll for any completed dirty rebuilds (from dirty chunk or other mesh updates)
        if let Some(render) = &mut self.render {
            let completed = self.world.poll_completed_meshes();
            for (pos, mesh) in completed {
                render.chunk_renderer.upload_mesh(&render.device, pos, &mesh);
            }
        }

        // 7. Request redraw
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Strata Phase 2 client starting...");

    let event_loop = winit::event_loop::EventLoop::new()?;
    let mut app = App::new();
    event_loop.run_app(&mut app)?;

    Ok(())
}
