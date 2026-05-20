# 21 — Security & Validation Sistemi

## 1. Genel Bakış

Strata'nın güvenlik sistemi **server-authoritative** modeline dayanır. Client tarafı sadece input gönderir, tüm validasyon server'da yapılır.

### Temel Prensipler

- **Server-authoritative:** Client hiçbir şeye güvenmez
- **Input validation:** Tüm client input'ları validate edilir
- **Rate limiting:** Spam ve exploit önleme
- **State verification:** Client state periyodik doğrulanır

---

## 2. Input Validation

```rust
/// Input validator — client input'larını doğrular.
pub struct InputValidator {
    /// Rate limiter'lar (per-client).
    rate_limiters: HashMap<ClientId, RateLimiter>,

    /// Maksimum hareket mesafesi (per-tick).
    max_movement: f32,

    /// Maksimum blok yerleştirme mesafesi.
    max_reach: f32,

    /// Maksimum blok yerleştirme hızı.
    max_block_place_rate: u32,

    /// Maksimum blok kırma hızı.
    max_block_break_rate: u32,
}

pub struct RateLimiter {
    /// Son action zamanı.
    last_action: Instant,

    /// Action sayacı (pencere içinde).
    action_count: u32,

    /// Pencere süresi.
    window: Duration,

    /// Maksimum action sayısı.
    max_actions: u32,
}

impl RateLimiter {
    pub fn new(window: Duration, max_actions: u32) -> Self {
        Self {
            last_action: Instant::now(),
            action_count: 0,
            window,
            max_actions,
        }
    }

    pub fn allow(&mut self) -> bool {
        let now = Instant::now();

        // Pencere sıfırla
        if now - self.last_action > self.window {
            self.action_count = 0;
            self.last_action = now;
        }

        if self.action_count < self.max_actions {
            self.action_count += 1;
            true
        } else {
            false
        }
    }
}

impl InputValidator {
    /// Blok yerleştirme input'unu validate et.
    pub fn validate_block_place(
        &mut self,
        client_id: ClientId,
        pos: IVec3,
        player_pos: Vec3,
    ) -> Result<(), ValidationError> {
        // Rate limit kontrolü
        let limiter = self.rate_limiters
            .entry(client_id)
            .or_insert_with(|| RateLimiter::new(Duration::from_secs(1), 10));

        if !limiter.allow() {
            return Err(ValidationError::RateLimited);
        }

        // Mesafe kontrolü (reach check)
        let dist = player_pos.distance(pos.as_vec3());
        if dist > self.max_reach {
            return Err(ValidationError::TooFar {
                distance: dist,
                max: self.max_reach,
            });
        }

        // Pozisyon geçerli mi?
        if !self.is_valid_position(pos) {
            return Err(ValidationError::InvalidPosition);
        }

        Ok(())
    }

    /// Hareket input'unu validate et.
    pub fn validate_movement(
        &mut self,
        client_id: ClientId,
        new_pos: Vec3,
        old_pos: Vec3,
        dt: f32,
    ) -> Result<(), ValidationError> {
        // Hız kontrolü
        let distance = new_pos.distance(old_pos);
        let speed = distance / dt;

        if speed > self.max_movement / dt {
            return Err(ValidationError::TooFast {
                speed,
                max: self.max_movement / dt,
            });
        }

        // Teleport check (ani pozisyon değişimi)
        if distance > self.max_movement * 2.0 {
            return Err(ValidationError::TeleportSuspected);
        }

        Ok(())
    }

    /// Pozisyon geçerli mi?
    fn is_valid_position(&self, pos: IVec3) -> bool {
        // World sınırları içinde mi?
        // Y pozisyonu geçerli mi (0-128)?
        pos.y >= 0 && pos.y < 128
    }
}

#[derive(Debug)]
pub enum ValidationError {
    RateLimited,
    TooFar { distance: f32, max: f32 },
    TooFast { speed: f32, max: f32 },
    TeleportSuspected,
    InvalidPosition,
    InvalidBlock,
    InsufficientPermissions,
}
```

---

## 3. State Verification

```rust
/// State verifier — client state'i doğrular.
pub struct StateVerifier {
    /// Son doğrulama zamanı (per-client).
    last_verification: HashMap<ClientId, Instant>,

    /// Doğrulama aralığı.
    verification_interval: Duration,

    /// Tolerans (float hataları için).
    tolerance: f32,
}

impl StateVerifier {
    /// Client state'i doğrula.
    pub fn verify(
        &mut self,
        client_id: ClientId,
        client_state: &ClientState,
        server_state: &ServerState,
    ) -> Result<(), VerificationError> {
        let now = Instant::now();

        // Periyodik doğrulama
        if let Some(last) = self.last_verification.get(&client_id) {
            if now - *last < self.verification_interval {
                return Ok(()); // Henüz zamanı değil
            }
        }

        self.last_verification.insert(client_id, now);

        // Pozisyon doğrulama
        let pos_diff = client_state.position.distance(server_state.position);
        if pos_diff > self.tolerance {
            return Err(VerificationError::PositionMismatch {
                client: client_state.position,
                server: server_state.position,
                diff: pos_diff,
            });
        }

        // Velocity doğrulama
        let vel_diff = client_state.velocity.distance(server_state.velocity);
        if vel_diff > self.tolerance * 2.0 {
            return Err(VerificationError::VelocityMismatch {
                client: client_state.velocity,
                server: server_state.velocity,
                diff: vel_diff,
            });
        }

        // Inventory doğrulama
        if !self.verify_inventory(&client_state.inventory, &server_state.inventory) {
            return Err(VerificationError::InventoryMismatch);
        }

        Ok(())
    }

    /// Inventory doğrulama.
    fn verify_inventory(
        &self,
        client: &Inventory,
        server: &Inventory,
    ) -> bool {
        // Toplam item sayısı aynı mı?
        let client_total: u32 = client.slots.iter()
            .filter_map(|s| s.as_ref().map(|s| s.count as u32))
            .sum();

        let server_total: u32 = server.slots.iter()
            .filter_map(|s| s.as_ref().map(|s| s.count as u32))
            .sum();

        client_total == server_total
    }
}

#[derive(Debug)]
pub enum VerificationError {
    PositionMismatch {
        client: Vec3,
        server: Vec3,
        diff: f32,
    },
    VelocityMismatch {
        client: Vec3,
        server: Vec3,
        diff: f32,
    },
    InventoryMismatch,
}
```

---

## 4. Anti-Cheat

```rust
/// Anti-cheat sistemi.
pub struct AntiCheat {
    /// Şüpheli client'lar.
    suspicious: HashMap<ClientId, SuspicionReport>,

    /// Ban listesi.
    ban_list: HashSet<ClientId>,

    /// Aksiyon limitleri.
    limits: AntiCheatLimits,
}

pub struct SuspicionReport {
    /// Şüpheli aksiyonlar.
    pub actions: Vec<SuspiciousAction>,

    /// Şüpheli skoru (yüksek = daha şüpheli).
    pub score: f32,

    /// İlk şüpheli aksiyon zamanı.
    pub first_suspicion: Instant,
}

#[derive(Clone)]
pub enum SuspiciousAction {
    /// Hız ihlali.
    SpeedViolation { speed: f32, max: f32 },

    /// Reach ihlali.
    ReachViolation { distance: f32, max: f32 },

    /// Fly şüphesi.
    FlightSuspected,

    /// NoClip şüphesi.
    NoClipSuspected,

    /// Hızlı blok kırma.
    FastBreaking { rate: f32, max: f32 },

    /// Hızlı blok yerleştirme.
    FastPlacing { rate: f32, max: f32 },

    /// Invalid inventory.
    InvalidInventory,

    /// Packet spam.
    PacketSpam { rate: f32, max: f32 },
}

pub struct AntiCheatLimits {
    /// Maksimum şüphe skoru (auto-ban threshold).
    pub auto_ban_threshold: f32,

    /// Şüphe skoru zamanla azalır (half-life).
    pub suspicion_half_life: Duration,

    /// Aksiyon ağırlıkları.
    pub action_weights: HashMap<SuspiciousActionType, f32>,
}

impl AntiCheat {
    /// Şüpheli aksiyon kaydet.
    pub fn report_suspicion(
        &mut self,
        client_id: ClientId,
        action: SuspiciousAction,
    ) {
        let weight = self.get_action_weight(&action);

        let report = self.suspicious
            .entry(client_id)
            .or_insert_with(|| SuspicionReport {
                actions: Vec::new(),
                score: 0.0,
                first_suspicion: Instant::now(),
            });

        report.actions.push(action);
        report.score += weight;

        // Auto-ban kontrolü
        if report.score >= self.limits.auto_ban_threshold {
            self.ban_client(client_id, "Anti-cheat: suspicion threshold exceeded");
        }
    }

    /// Aksiyon ağırlığı.
    fn get_action_weight(&self, action: &SuspiciousAction) -> f32 {
        match action {
            SuspiciousAction::SpeedViolation { .. } => 5.0,
            SuspiciousAction::ReachViolation { .. } => 3.0,
            SuspiciousAction::FlightSuspected => 10.0,
            SuspiciousAction::NoClipSuspected => 10.0,
            SuspiciousAction::FastBreaking { .. } => 2.0,
            SuspiciousAction::FastPlacing { .. } => 2.0,
            SuspiciousAction::InvalidInventory => 5.0,
            SuspiciousAction::PacketSpam { .. } => 1.0,
        }
    }

    /// Client'ı banla.
    pub fn ban_client(&mut self, client_id: ClientId, reason: &str) {
        self.ban_list.insert(client_id);
        tracing::warn!(client_id = %client_id, reason, "Client banned");
    }

    /// Şüphe skorunu zamanla azalt.
    pub fn decay_suspicion(&mut self, dt: f32) {
        let decay_rate = 1.0 / self.limits.suspicion_half_life.as_secs_f32();

        for report in self.suspicious.values_mut() {
            report.score *= 1.0 - decay_rate * dt;

            if report.score < 0.1 {
                report.actions.clear();
            }
        }
    }
}
```

---

## 5. Server-Side Authority

```rust
/// Server-authoritative world update.
pub fn server_world_update(
    mut world: ResMut<World>,
    mut events: EventReader<StrataEvent>,
    mut validator: ResMut<InputValidator>,
    mut anti_cheat: ResMut<AntiCheat>,
    clients: Query<(Entity, &RepliconClient, &PlayerPosition)>,
) {
    for event in events.read() {
        match event {
            StrataEvent::BlockPlaced { pos, block_id } => {
                // Client pozisyonunu bul
                if let Some((_, client, player_pos)) = clients.iter()
                    .find(|(_, c, _)| c.peer_id == event.client_id)
                {
                    // Validate
                    match validator.validate_block_place(
                        client.peer_id,
                        *pos,
                        player_pos.0,
                    ) {
                        Ok(()) => {
                            // Geçerli — dünyayı güncelle
                            world.set_block(*pos, *block_id);
                        }
                        Err(e) => {
                            // Geçersiz — client'a geri bildir
                            tracing::warn!(
                                client_id = %client.peer_id,
                                error = ?e,
                                "Invalid block place rejected"
                            );

                            anti_cheat.report_suspicion(
                                client.peer_id,
                                SuspiciousAction::ReachViolation {
                                    distance: player_pos.0.distance(pos.as_vec3()),
                                    max: validator.max_reach,
                                },
                            );

                            // Client state'i geri al
                            // (server doğru state'i gönderir)
                        }
                    }
                }
            }

            StrataEvent::BlockBroken { pos } => {
                // Benzer validasyon
            }
        }
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  security/
    ├── mod.rs              ← Security plugin entry point
    ├── validation/
    │   ├── mod.rs          ← InputValidator
    │   ├── movement.rs     ← Hareket validasyonu
    │   ├── block.rs        ← Blok yerleştirme/kırma validasyonu
    │   └── rate_limit.rs   ← RateLimiter
    ├── verification/
    │   ├── mod.rs          ← StateVerifier
    │   ├── position.rs     ← Pozisyon doğrulama
    │   └── inventory.rs    ← Inventory doğrulama
    ├── anti_cheat/
    │   ├── mod.rs          ← AntiCheat
    │   ├── report.rs       ← SuspicionReport
    │   ├── actions.rs      ← SuspiciousAction enum
    │   └── limits.rs       ← AntiCheatLimits
    └── ban/
        ├── mod.rs          ← Ban sistemi
        └── list.rs         ← Ban listesi
```
