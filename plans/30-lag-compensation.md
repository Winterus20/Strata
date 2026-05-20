# 40 — Lag Compensation & Client Reconciliation

## 1. Genel Bakış

Strata'nın lag compensation sistemi **client-side prediction** ve **server reconciliation** kullanarak ağ gecikmesini gizler. Oyuncu hareketi ve blok etkileşimleri anında client'ta uygulanır, server doğrulaması asenkron yapılır.

### Temel Prensipler

- **Client-side prediction:** Client kendi input'larını hemen uygular (server cevabı beklemez)
- **Server reconciliation:** Server cevabı geldiğinde client state'i düzeltir
- **Entity interpolation:** Diğer oyuncuların pozisyonları interpolate edilir
- **Input buffering:** Server tick rate'i ile client input rate'i eşlenir

---

## 2. Client-Side Prediction

```rust
pub struct PredictionState {
    /// Son onaylanmış server state.
    pub confirmed_state: PlayerState,

    /// Onaylanmamış input'lar (sıralı).
    pub pending_inputs: Vec<InputSequence>,

    /// Client'ın tahmini state'i.
    pub predicted_state: PlayerState,
}

impl PredictionState {
    /// Input uygula ve tahmini state'i güncelle.
    pub fn apply_input(&mut self, input: InputSequence) {
        self.pending_inputs.push(input.clone());
        self.predicted_state = self.simulate(&self.pending_inputs);
    }

    /// Server onayı geldiğinde reconciliation yap.
    pub fn reconcile(&mut self, server_state: PlayerState, server_seq: u32) {
        self.confirmed_state = server_state;

        // Server'ın onayladığı input'ları kaldır
        self.pending_inputs.retain(|i| i.sequence > server_seq);

        // Kalan input'ları tekrar uygula
        self.predicted_state = self.simulate(&self.pending_inputs);
    }
}
```

---

## 3. Entity Interpolation

```rust
pub struct InterpolatedEntity {
    /// Önceki state.
    pub previous: EntityState,

    /// Sonraki state.
    pub current: EntityState,

    /// Interpolation faktörü (0.0 - 1.0).
    pub alpha: f32,
}

impl InterpolatedEntity {
    pub fn interpolated_position(&self) -> Vec3 {
        self.previous.position.lerp(self.current.position, self.alpha)
    }

    pub fn interpolated_rotation(&self) -> Quat {
        self.previous.rotation.slerp(self.current.rotation, self.alpha)
    }
}
```

---

## 4. Input Buffering

```rust
pub struct InputBuffer {
    /// Input'lar (max buffer size).
    pub inputs: [InputSequence; 64],

    /// Başlangıç index'i (ring buffer).
    pub head: u8,

    /// Buffer'daki input sayısı.
    pub count: u8,
}

impl InputBuffer {
    pub fn push(&mut self, input: InputSequence) {
        let idx = (self.head + self.count) % 64;
        self.inputs[idx as usize] = input;
        self.count = self.count.min(63) + 1;
    }

    pub fn pop(&mut self) -> Option<InputSequence> {
        if self.count == 0 {
            return None;
        }
        let input = self.inputs[self.head as usize];
        self.head = (self.head + 1) % 64;
        self.count -= 1;
        Some(input)
    }
}
```

---

## 5. Server Reconciliation Flow

```
Client:  Input[1] → Input[2] → Input[3] → (predict) → Render
         ↓           ↓           ↓
Server:  ──────────── Input[1] ── Input[2] ── Input[3] → Validate
         ↓
Client:  ←── ServerState(seq=3) ── Reconcile pending inputs → Render
```

---

## 6. Crate Organizasyonu

```
crates/
  network/
    ├── prediction/
    │   ├── mod.rs          ← Client-side prediction
    │   ├── state.rs        ← PredictionState
    │   └── simulate.rs     ← Input simulation
    ├── reconciliation/
    │   ├── mod.rs          ← Server reconciliation
    │   ├── diff.rs         ← State diff hesaplama
    │   └── correct.rs      ← State düzeltme
    ├── interpolation/
    │   ├── mod.rs          ← Entity interpolation
    │   ├── entity.rs       ← InterpolatedEntity
    │   └── timeline.rs     ← Interpolation timeline
    └── input_buffer/
        ├── mod.rs          ← InputBuffer
        ├── sequence.rs     ← InputSequence
        └── ring.rs         ← Ring buffer
```
