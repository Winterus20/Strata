# 19 — Particle & VFX Sistemi

## 1. Genel Bakış

Strata'nın particle sistemi **GPU compute-based** particle simulation kullanır. Düşen kum, patlama debris, yağmur, kar, ve blok kırma parçacıkları bu sistemle render edilir.

### Temel Prensipler

- **GPU compute:** Binlerce particle paralel simüle edilir
- **ECS entegrasyonu:** Particle emitter'lar ECS component'larıdır
- **Collidable:** Particle'lar dünya ile etkileşime girer
- **Sorted:** Translucent particle'lar doğru sıralanır

---

## 2. Particle System Architecture

```rust
/// Particle system — GPU compute tabanlı (wgpu).
pub struct ParticleSystem {
    /// Particle buffer (GPU).
    particle_buffer: wgpu::Buffer,

    /// Particle count buffer.
    count_buffer: wgpu::Buffer,

    /// Compute pipeline.
    compute_pipeline: wgpu::ComputePipeline,

    /// Render pipeline.
    render_pipeline: wgpu::RenderPipeline,

    /// Compute bind group.
    compute_bind_group: wgpu::BindGroup,

    /// Render bind group.
    render_bind_group: wgpu::BindGroup,

    /// Emitter'lar.
    emitters: Vec<ParticleEmitter>,

    /// Maksimum particle sayısı.
    max_particles: u32,
}

/// GPU particle verisi.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuParticle {
    /// Pozisyon.
    pub position: [f32; 3],

    /// Velocity.
    pub velocity: [f32; 3],

    /// Yaşam süresi (0-1).
    pub lifetime: f32,

    /// Max yaşam süresi (saniye).
    pub max_lifetime: f32,

    /// Boyut.
    pub size: f32,

    /// Renk (RGBA).
    pub color: [f32; 4],

    /// Gravity etkisi.
    pub gravity: f32,

    /// Drag (hava direnci).
    pub drag: f32,

    /// Particle tipi.
    pub particle_type: u32,

    /// Padding.
    pub _padding: [u32; 2],
}
```

---

## 3. Particle Emitter'ları

```rust
/// Particle emitter component.
#[derive(Component)]
pub struct ParticleEmitter {
    /// Emitter tipi.
    pub emitter_type: EmitterType,

    /// Spawn rate (particle/saniye).
    pub spawn_rate: f32,

    /// Spawn area.
    pub spawn_area: SpawnArea,

    /// Initial velocity.
    pub initial_velocity: VelocityDistribution,

    /// Particle lifetime.
    pub lifetime: std::ops::Range<f32>,

    /// Particle size.
    pub size: std::ops::Range<f32>,

    /// Particle color.
    pub color: ColorGradient,

    /// Gravity.
    pub gravity: f32,

    /// Drag.
    pub drag: f32,

    /// Collision (dünya ile etkileşim).
    pub collision: bool,

    /// Aktif mi?
    pub active: bool,
}

#[derive(Clone)]
pub enum EmitterType {
    /// Nokta emitter.
    Point,

    /// Alan emitter (dikdörtgen).
    Area { size: Vec2 },

    /// Küre emitter.
    Sphere { radius: f32 },

    /// Kutu emitter.
    Box { size: Vec3 },

    /// Yüzey emitter (blok yüzeyi).
    Surface { face: BlockFace },

    /// Hat emitter (çizgi).
    Line { start: Vec3, end: Vec3 },
}

pub enum SpawnArea {
    /// Belirli pozisyon.
    Fixed(Vec3),

    /// Oyuncu etrafında.
    AroundPlayer { radius: f32 },

    /// Entity pozisyonu.
    Entity(Entity),

    /// Dünya koordinatı.
    World(Vec3),
}

pub struct VelocityDistribution {
    /// Minimum velocity.
    pub min: Vec3,

    /// Maximum velocity.
    pub max: Vec3,

    /// Random spread.
    pub spread: f32,
}

pub struct ColorGradient {
    /// Renk keyframe'leri.
    pub keyframes: Vec<ColorKeyframe>,
}

pub struct ColorKeyframe {
    pub time: f32, // 0-1
    pub color: [f32; 4],
}
```

---

## 4. Compute Shader

```wgsl
// WGSL compute shader — particle simulation
struct GpuParticle {
    position: vec3<f32>,
    velocity: vec3<f32>,
    lifetime: f32,
    max_lifetime: f32,
    size: f32,
    color: vec4<f32>,
    gravity: f32,
    drag: f32,
    particle_type: u32,
    _padding: array<u32, 2>,
};

struct SimulationParams {
    delta_time: f32,
    particle_count: u32,
};

@group(0) @binding(0)
var<storage, read_write> particles: array<GpuParticle>;
@group(0) @binding(1)
var<uniform> sim_params: SimulationParams;
@group(0) @binding(2)
var<storage, read> world_colliders: array<Collider>;

@compute @workgroup_size(256, 1, 1)
fn simulate_particles(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if (idx >= sim_params.particle_count) { return; }

    var particle = particles[idx];

    // Yaşam süresi kontrolü
    particle.lifetime += sim_params.delta_time;
    if (particle.lifetime >= particle.max_lifetime) {
        particle.lifetime = -1.0; // Ölü particle
        particles[idx] = particle;
        return;
    }

    // Yerçekimi
    particle.velocity.y -= particle.gravity * sim_params.delta_time;

    // Drag
    let drag_factor = 1.0 - particle.drag * sim_params.delta_time;
    particle.velocity *= drag_factor;

    // Pozisyon güncelle
    particle.position += particle.velocity * sim_params.delta_time;

    // Dünya collision
    if (particle.particle_type != 0u) {
        let grid_pos = floor(particle.position);
        if (world_is_solid(grid_pos)) {
            // Çarpma — bounce veya stop
            particle.velocity.y = -particle.velocity.y * 0.3;
            particle.position.y = grid_pos.y + 1.0;

            // Hız düşükse dur
            if (length(particle.velocity) < 0.5) {
                particle.lifetime = particle.max_lifetime; // Öldür
            }
        }
    }

    // Lifetime bazlı color fade
    let life_ratio = particle.lifetime / particle.max_lifetime;
    particle.color.a = 1.0 - life_ratio * life_ratio; // Quadratic fade

    // Size fade
    particle.size *= 1.0 - life_ratio * 0.5;

    particles[idx] = particle;
}
```

---

## 5. Preset Emitter'lar

### 5.1 Blok Kırma Particles

```rust
/// Blok kırma particle emitter'ı.
pub fn create_block_break_emitter(
    pos: Vec3,
    block_color: [f32; 3],
) -> ParticleEmitter {
    ParticleEmitter {
        emitter_type: EmitterType::Box {
            size: Vec3::splat(0.8),
        },
        spawn_rate: 20.0,
        spawn_area: SpawnArea::World(pos),
        initial_velocity: VelocityDistribution {
            min: Vec3::new(-1.0, 0.5, -1.0),
            max: Vec3::new(1.0, 2.0, 1.0),
            spread: 0.5,
        },
        lifetime: 0.5..1.5,
        size: 0.05..0.15,
        color: ColorGradient {
            keyframes: vec![
                ColorKeyframe { time: 0.0, color: [block_color[0], block_color[1], block_color[2], 1.0] },
                ColorKeyframe { time: 1.0, color: [block_color[0] * 0.5, block_color[1] * 0.5, block_color[2] * 0.5, 0.0] },
            ],
        },
        gravity: 15.0,
        drag: 2.0,
        collision: true,
        active: true,
    }
}
```

### 5.2 Patlama Particles

```rust
/// Patlama particle emitter'ı.
pub fn create_explosion_emitter(
    pos: Vec3,
    intensity: f32,
) -> ParticleEmitter {
    ParticleEmitter {
        emitter_type: EmitterType::Sphere {
            radius: 0.5 * intensity,
        },
        spawn_rate: 200.0 * intensity,
        spawn_area: SpawnArea::World(pos),
        initial_velocity: VelocityDistribution {
            min: Vec3::splat(-5.0 * intensity),
            max: Vec3::splat(5.0 * intensity),
            spread: 1.0,
        },
        lifetime: 0.3..2.0,
        size: 0.1..0.5,
        color: ColorGradient {
            keyframes: vec![
                ColorKeyframe { time: 0.0, color: [1.0, 1.0, 0.8, 1.0] },
                ColorKeyframe { time: 0.3, color: [1.0, 0.6, 0.1, 0.8] },
                ColorKeyframe { time: 0.7, color: [0.8, 0.2, 0.0, 0.4] },
                ColorKeyframe { time: 1.0, color: [0.2, 0.2, 0.2, 0.0] },
            ],
        },
        gravity: 8.0,
        drag: 1.0,
        collision: true,
        active: true,
    }
}
```

### 5.3 Yağmur Particles

```rust
/// Yağmur particle emitter'ı.
pub fn create_rain_emitter(
    camera_pos: Vec3,
    intensity: f32,
) -> ParticleEmitter {
    let area_size = 80.0;

    ParticleEmitter {
        emitter_type: EmitterType::Area {
            size: Vec2::new(area_size, area_size),
        },
        spawn_rate: 5000.0 * intensity,
        spawn_area: SpawnArea::AroundPlayer { radius: area_size / 2.0 },
        initial_velocity: VelocityDistribution {
            min: Vec3::new(-0.5, -15.0, -0.5),
            max: Vec3::new(0.5, -25.0, 0.5),
            spread: 0.1,
        },
        lifetime: 2.0..4.0,
        size: 0.02..0.04,
        color: ColorGradient {
            keyframes: vec![
                ColorKeyframe { time: 0.0, color: [0.5, 0.6, 0.8, 0.3] },
                ColorKeyframe { time: 1.0, color: [0.5, 0.6, 0.8, 0.1] },
            ],
        },
        gravity: 20.0,
        drag: 0.5,
        collision: true,
        active: true,
    }
}
```

### 5.4 Duman Particles

```rust
/// Duman particle emitter'ı.
pub fn create_smoke_emitter(
    pos: Vec3,
) -> ParticleEmitter {
    ParticleEmitter {
        emitter_type: EmitterType::Point,
        spawn_rate: 10.0,
        spawn_area: SpawnArea::World(pos),
        initial_velocity: VelocityDistribution {
            min: Vec3::new(-0.2, 0.5, -0.2),
            max: Vec3::new(0.2, 1.5, 0.2),
            spread: 0.3,
        },
        lifetime: 2.0..4.0,
        size: 0.2..0.8,
        color: ColorGradient {
            keyframes: vec![
                ColorKeyframe { time: 0.0, color: [0.3, 0.3, 0.3, 0.6] },
                ColorKeyframe { time: 0.5, color: [0.4, 0.4, 0.4, 0.3] },
                ColorKeyframe { time: 1.0, color: [0.5, 0.5, 0.5, 0.0] },
            ],
        },
        gravity: -0.5, // Yukarı doğru
        drag: 1.5,
        collision: false,
        active: true,
    }
}
```

---

## 6. Particle Render

```wgsl
// WGSL vertex shader — particle render (point sprite)
struct CameraUniform {
    view_matrix: mat4x4<f32>,
    projection_matrix: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) size: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) size: f32,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    // Screen-space size
    let view_pos = camera.view_matrix * vec4<f32>(in.position, 1.0);
    let screen_pos = camera.projection_matrix * view_pos;

    out.clip_position = screen_pos;
    out.color = in.color;
    out.size = in.size / screen_pos.w; // Perspective size

    return out;
}
```

```wgsl
// WGSL fragment shader — particle render
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Circular particle
    let center = vec2<f32>(0.5, 0.5);
    let dist = length(in.color.xy - center); // Using point_coord approximation

    if (dist > 0.5) {
        discard;
    }

    // Soft edge
    let alpha = smoothstep(0.5, 0.3, dist) * in.color.a;

    return vec4<f32>(in.color.rgb, alpha);
}
```

---

## 7. Crate Organizasyonu

```
crates/
  particles/
    ├── mod.rs              ← Particle plugin entry point
    ├── system.rs           ← ParticleSystem (GPU compute)
    ├── emitter/
    │   ├── mod.rs          ← ParticleEmitter
    │   ├── types.rs        ← EmitterType
    │   └── presets.rs      ← Preset emitter'lar
    ├── compute/
    │   ├── mod.rs          ← Compute shader (wgpu)
    │   └── simulation.wgsl ← Simulation compute shader (WGSL)
    ├── render/
    │   ├── mod.rs          ← Particle render (wgpu)
    │   ├── pipeline.rs     ← Render pipeline
    │   ├── particle.wgsl   ← Vertex + Fragment shader (WGSL)
    └── events.rs           ← Particle event handler'ları
```
