# 29 — Animation System

## 1. Genel Bakış

Strata'nın animasyon sistemi **ECS-based** ve **skeletal + keyframe** animasyonları destekler. Oyuncu, mob'lar ve entity'ler bu sistemle animasyonlanır.

### Temel Prenipler

- **ECS-based:** Her animasyon bir component
- **Skeletal:** Kemik tabanlı animasyon (mob'lar için)
- **Keyframe:** Basit animasyonlar için keyframe desteği
- **Blending:** Animasyonlar arası yumuşak geçiş
- **State Machine:** Animasyon state machine (idle → walk → run → jump)

---

## 2. Animation Component

```rust
#[derive(Component)]
pub struct Animator {
    /// Aktif animasyon state machine.
    pub state_machine: AnimationStateMachine,

    /// Kemik transform'ları.
    pub bones: Vec<BoneTransform>,

    /// Animasyon blending.
    pub blend_weights: Vec<f32>,
}

#[derive(Component)]
pub struct AnimationClip {
    /// Animasyon ismi.
    pub name: String,

    /// Keyframe'ler.
    pub keyframes: Vec<Keyframe>,

    /// Süre (saniye).
    pub duration: f32,

    /// Loop flag.
    pub looped: bool,
}
```

---

## 3. Animation State Machine

```rust
pub struct AnimationStateMachine {
    /// State'ler.
    pub states: HashMap<String, AnimationState>,

    /// Transition'lar.
    pub transitions: Vec<Transition>,

    /// Aktif state.
    pub current_state: String,
}

pub struct AnimationState {
    /// Animasyon clip.
    pub clip: AnimationClip,

    /// Blend time.
    pub blend_in: f32,
}

pub struct Transition {
    /// Kaynak state.
    pub from: String,

    /// Hedef state.
    pub to: String,

    /// Koşul.
    pub condition: TransitionCondition,
}
```

---

## 4. Crate Organizasyonu

```
crates/
  animation/
    ├── mod.rs
    ├── animator.rs
    ├── clip.rs
    ├── state_machine.rs
    ├── bone.rs
    └── blending.rs
```
