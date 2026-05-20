# 44 — HDR Rendering

## 1. Genel Bakış

Strata'nın HDR rendering sistemi geniş dinamik aralıkta renk ve ışık hesaplaması yapar.

### Temel Prensipler

- **FP16 swapchain:** Yüksek hassasiyetli renk buffer'ları
- **Tone mapping:** HDR → LDR dönüşüm (ACES, Reinhard, etc.)
- **Bloom:** Parlak alanlardan ışık yayılımı
- **Exposure control:** Otomatik/manuel pozlama

---

## 2. HDR Pipeline

```rust
pub struct HdrPipeline {
    pub hdr_texture: wgpu::Texture,
    pub bloom_texture: wgpu::Texture,
    pub exposure: f32,
    pub tone_mapper: ToneMappingMode,
}

pub enum ToneMappingMode {
    Aces,
    Reinhard,
    Uncharted2,
    Linear,
}

pub struct ExposureController {
    pub mode: ExposureMode,
    pub manual_exposure: f32,
    pub auto_min: f32,
    pub auto_max: f32,
    pub auto_speed: f32,
    pub current_exposure: f32,
}

pub enum ExposureMode {
    Manual,
    Auto,
}
```

---

## 3. Bloom Effect

```rust
pub struct BloomPass {
    pub threshold: f32,
    pub intensity: f32,
    pub blur_iterations: u32,
    pub blur_radius: f32,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  render/
    └── hdr/
        ├── mod.rs
        ├── pipeline.rs
        ├── tone_mapping.rs
        ├── bloom.rs
        └── exposure.rs
```
