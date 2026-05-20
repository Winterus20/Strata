# 25 — Inventory & Player Controller

## 1. Genel Bakış

Strata'nın envanter ve oyuncu kontrol sistemi **ECS-based**'tir. Oyuncu hareketi, envanter yönetimi, ve oyun modları bu sistemde tanımlanır.

### Temel Prensipler

- **ECS-based:** Tüm component'lar ve sistemler Bevy ECS üzerinden
- **Server-authoritative:** Envanter server'da doğrulanır
- **Modüler:** Oyun modları (survival, creative, spectator) ayrı component'lar
- **Input-agnostic:** Klavye/fare/gamepad aynı input layer'ı kullanır

---

## 2. Player Controller

```rust
/// Player controller component.
#[derive(Component)]
pub struct PlayerController {
    /// Hareket input'u.
    pub move_input: Vec2,

    /// Zıplama input'u.
    pub jump_pressed: bool,

    /// Sprint input'u.
    pub sprinting: bool,

    /// Sneak input'u.
    pub sneaking: bool,

    /// Mouse input (kamera).
    pub mouse_delta: Vec2,

    /// Mouse wheel (hotbar).
    pub mouse_wheel: f32,

    /// Sol tık (blok kırma/saldırı).
    pub left_click: bool,

    /// Sağ tık (blok yerleştirme/kullanma).
    pub right_click: bool,

    /// Orta tık (blok seçme).
    pub middle_click: bool,
}

/// Player movement sistemi.
pub fn player_movement_system(
    time: Res<Time>,
    mut players: Query<(
        &PlayerController,
        &mut Transform,
        &mut Velocity,
        &PlayerState,
    )>,
    world: Res<World>,
) {
    let dt = time.delta_secs();

    for (input, mut transform, mut velocity, state) in players.iter_mut() {
        // Hareket vektörü
        let forward = transform.rotation * Vec3::NEG_Z;
        let right = transform.rotation * Vec3::X;

        let move_dir = (forward * input.move_input.y + right * input.move_input.x).normalize_or_zero();

        // Hız
        let base_speed = if state.game_mode == GameMode::Creative {
            10.0
        } else if input.sprinting {
            5.6
        } else {
            4.3
        };

        // Hareket uygula
        if state.grounded {
            velocity.x = move_dir.x * base_speed;
            velocity.z = move_dir.z * base_speed;
        } else {
            // Havada daha yavaş
            velocity.x = move_dir.x * base_speed * 0.7;
            velocity.z = move_dir.z * base_speed * 0.7;
        }

        // Zıplama
        if input.jump_pressed && state.grounded {
            velocity.y = 8.5;
        }

        // Yerçekimi
        if state.game_mode != GameMode::Creative && state.game_mode != GameMode::Spectator {
            velocity.y -= 28.0 * dt;

            // Terminal velocity
            if velocity.y < -50.0 {
                velocity.y = -50.0;
            }
        }

        // Sneak (yavaşlama)
        if input.sneaking {
            velocity.x *= 0.5;
            velocity.z *= 0.5;
        }

        // Pozisyon güncelle
        transform.translation += velocity.0 * dt;

        // Ground check
        state.grounded = check_ground(&world, transform.translation);
    }
}

/// Oyuncu durumu.
#[derive(Component)]
pub struct PlayerState {
    /// Oyun modu.
    pub game_mode: GameMode,

    /// Yerde mi?
    pub grounded: bool,

    /// Uçuyor mu?
    pub flying: bool,

    /// Hasar aldı mı?
    pub hurt: bool,

    /// Ölüm sayısı.
    pub death_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}
```

---

## 3. Envanter Sistemi

```rust
/// Envanter component.
#[derive(Component)]
pub struct Inventory {
    /// Ana envanter (27 slot — 3×9).
    pub main: [Option<ItemStack>; 27],

    /// Zırh slot'ları (4 slot).
    pub armor: [Option<ItemStack>; 4],

    /// Hotbar (9 slot — ana envanterin ilk 9'u).
    pub hotbar: [Option<ItemStack>; 9],

    /// Seçili hotbar slot'u (0-8).
    pub selected_slot: u8,

    /// Off-hand (sol el).
    pub offhand: Option<ItemStack>,
}

impl Inventory {
    /// Boş envanter oluştur.
    pub fn empty() -> Self {
        Self {
            main: [None; 27],
            armor: [None; 4],
            hotbar: [None; 9],
            selected_slot: 0,
            offhand: None,
        }
    }

    /// Hotbar'dan slot al (main ile shared).
    pub fn get_hotbar(&self, index: u8) -> Option<&ItemStack> {
        self.main.get(index as usize).and_then(|s| s.as_ref())
    }

    /// Seçili slot'u al.
    pub fn selected(&self) -> Option<&ItemStack> {
        self.get_hotbar(self.selected_slot)
    }

    /// Slot'a item koy.
    pub fn set_slot(&mut self, index: u8, item: Option<ItemStack>) {
        if let Some(slot) = self.main.get_mut(index as usize) {
            *slot = item;
        }
    }

    /// Envantere item ekle (ilk uygun slot'a).
    pub fn add_item(&mut self, item: ItemStack) -> Result<(), ItemStack> {
        // Önce stack'lenebilir slot ara
        for slot in self.main.iter_mut() {
            if let Some(existing) = slot {
                if existing.item_id == item.item_id && existing.count < existing.max_stack {
                    let space = existing.max_stack - existing.count;
                    let add = item.count.min(space);
                    existing.count += add;

                    if add < item.count {
                        // Tam sığmadı, kalan ile devam et
                        let remaining = ItemStack {
                            count: item.count - add,
                            ..item
                        };
                        return self.add_item(remaining);
                    }
                    return Ok(());
                }
            }
        }

        // Boş slot ara
        for slot in self.main.iter_mut() {
            if slot.is_none() {
                *slot = Some(item);
                return Ok(());
            }
        }

        // Envanter dolu
        Err(item)
    }

    /// Envanterden item çıkar.
    pub fn remove_item(&mut self, item_id: u16, count: u8) -> Option<ItemStack> {
        let mut remaining = count;

        for slot in self.main.iter_mut().rev() {
            if let Some(stack) = slot {
                if stack.item_id == item_id {
                    let remove = remaining.min(stack.count);
                    stack.count -= remove;
                    remaining -= remove;

                    if stack.count == 0 {
                        *slot = None;
                    }

                    if remaining == 0 {
                        return Some(ItemStack {
                            item_id,
                            count,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        if remaining < count {
            Some(ItemStack {
                item_id,
                count: count - remaining,
                ..Default::default()
            })
        } else {
            None
        }
    }

    /// Envanteri temizle.
    pub fn clear(&mut self) {
        self.main = [None; 27];
        self.hotbar = [None; 9];
        self.armor = [None; 4];
        self.offhand = None;
    }
}

/// Item stack.
#[derive(Clone, Debug)]
pub struct ItemStack {
    /// Item ID.
    pub item_id: u16,

    /// Miktar.
    pub count: u8,

    /// Maksimum stack boyutu.
    pub max_stack: u8,

    /// Dayanıklılık (0 = kırık).
    pub durability: Option<u16>,

    /// Maksimum dayanıklılık.
    pub max_durability: Option<u16>,

    /// NBT data (opsiyonel).
    pub nbt: Option<ItemNbt>,

    /// Özel isim.
    pub custom_name: Option<String>,

    /// Lore (açıklama).
    pub lore: Vec<String>,

    /// Enchantment'lar.
    pub enchantments: Vec<Enchantment>,
}

impl Default for ItemStack {
    fn default() -> Self {
        Self {
            item_id: 0,
            count: 1,
            max_stack: 64,
            durability: None,
            max_durability: None,
            nbt: None,
            custom_name: None,
            lore: Vec::new(),
            enchantments: Vec::new(),
        }
    }
}

/// Item NBT data.
#[derive(Clone, Debug)]
pub struct ItemNbt {
    pub data: HashMap<String, NbtValue>,
}

#[derive(Clone, Debug)]
pub enum NbtValue {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    List(Vec<NbtValue>),
    Compound(HashMap<String, NbtValue>),
}

/// Enchantment.
#[derive(Clone, Debug)]
pub struct Enchantment {
    pub id: u16,
    pub level: u8,
}
```

---

## 4. Block Interaction

```rust
/// Blok etkileşim sistemi.
pub fn block_interaction_system(
    mut players: Query<(
        Entity,
        &PlayerController,
        &Transform,
        &Inventory,
        &PlayerState,
    )>,
    mut world: ResMut<World>,
    mut events: EventWriter<StrataEvent>,
    raycast: Res<RaycastSystem>,
) {
    for (entity, input, transform, inventory, state) in players.iter() {
        if state.game_mode == GameMode::Spectator {
            continue;
        }

        // Ray cast — oyuncunun baktığı yön
        let direction = transform.rotation * Vec3::NEG_Z;
        let origin = transform.translation + Vec3::new(0.0, 1.6, 0.0); // Göz yüksekliği

        let reach = if state.game_mode == GameMode::Creative {
            6.0
        } else {
            5.0
        };

        let hit = raycast.cast(origin, direction, reach, &world);

        // Sol tık — blok kırma
        if input.left_click {
            if let Some(hit) = &hit {
                if state.game_mode != GameMode::Adventure {
                    let block_id = world.get_block(hit.position);

                    if let Some(block_id) = block_id {
                        world.set_block(hit.position, None);

                        // Drop oluştur
                        if state.game_mode == GameMode::Survival {
                            if let Some(drop) = get_block_drop(block_id) {
                                // Drop entity spawn et
                                events.send(StrataEvent::ItemSpawned {
                                    item: drop,
                                    position: hit.position.as_vec3(),
                                });
                            }
                        }

                        events.send(StrataEvent::BlockBroken {
                            position: hit.position,
                            block_id,
                        });
                    }
                }
            }
        }

        // Sağ tık — blok yerleştirme
        if input.right_click {
            if let Some(hit) = &hit {
                if let Some(selected) = inventory.selected() {
                    let place_pos = hit.position + hit.face.direction();

                    // Yerleştirme geçerli mi?
                    if world.is_empty(place_pos)
                        && !player_intersects(place_pos, transform.translation)
                    {
                        world.set_block(place_pos, Some(selected.item_id));

                        // Stack'ten çıkar
                        // (server'da doğrulanır)

                        events.send(StrataEvent::BlockPlaced {
                            position: place_pos,
                            block_id: selected.item_id,
                        });
                    }
                }
            }
        }

        // Orta tık — blok seçme (creative)
        if input.middle_click && state.game_mode == GameMode::Creative {
            if let Some(hit) = &hit {
                if let Some(block_id) = world.get_block(hit.position) {
                    // Hotbar'daki seçili slot'a koy
                    // (client tarafı, server'da doğrulanır)
                }
            }
        }
    }
}

/// Oyuncu pozisyonu ile blok çakışıyor mu?
fn player_intersects(block_pos: IVec3, player_pos: Vec3) -> bool {
    let player_min = player_pos - Vec3::new(0.3, 0.0, 0.3);
    let player_max = player_pos + Vec3::new(0.3, 1.8, 0.3);

    let block_min = block_pos.as_vec3();
    let block_max = block_min + Vec3::ONE;

    player_min.x < block_max.x
        && player_max.x > block_min.x
        && player_min.y < block_max.y
        && player_max.y > block_min.y
        && player_min.z < block_max.z
        && player_max.z > block_min.z
}
```

---

## 5. Input Mapping

```rust
/// Input mapping sistemi.
pub struct InputMapper {
    /// Tuş mapping'leri.
    key_bindings: HashMap<KeyCode, InputAction>,

    /// Mouse mapping'leri.
    mouse_bindings: HashMap<MouseButton, InputAction>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputAction {
    MoveForward,
    MoveBackward,
    MoveLeft,
    MoveRight,
    Jump,
    Sprint,
    Sneak,
    Attack,
    Use,
    PickBlock,
    Hotbar1,
    Hotbar2,
    Hotbar3,
    Hotbar4,
    Hotbar5,
    Hotbar6,
    Hotbar7,
    Hotbar8,
    Hotbar9,
    Inventory,
    DropItem,
    SwapHands,
    DebugToggle,
    Chat,
}

impl InputMapper {
    /// Varsayılan mapping'ler.
    pub fn default_bindings() -> Self {
        let mut key_bindings = HashMap::new();
        let mut mouse_bindings = HashMap::new();

        // WASD
        key_bindings.insert(KeyCode::W, InputAction::MoveForward);
        key_bindings.insert(KeyCode::S, InputAction::MoveBackward);
        key_bindings.insert(KeyCode::A, InputAction::MoveLeft);
        key_bindings.insert(KeyCode::D, InputAction::MoveRight);

        // Aksiyonlar
        key_bindings.insert(KeyCode::Space, InputAction::Jump);
        key_bindings.insert(KeyCode::LShift, InputAction::Sneak);
        key_bindings.insert(KeyCode::LControl, InputAction::Sprint);

        // Mouse
        mouse_bindings.insert(MouseButton::Left, InputAction::Attack);
        mouse_bindings.insert(MouseButton::Right, InputAction::Use);
        mouse_bindings.insert(MouseButton::Middle, InputAction::PickBlock);

        // Hotbar
        key_bindings.insert(KeyCode::Digit1, InputAction::Hotbar1);
        key_bindings.insert(KeyCode::Digit2, InputAction::Hotbar2);
        key_bindings.insert(KeyCode::Digit3, InputAction::Hotbar3);
        key_bindings.insert(KeyCode::Digit4, InputAction::Hotbar4);
        key_bindings.insert(KeyCode::Digit5, InputAction::Hotbar5);
        key_bindings.insert(KeyCode::Digit6, InputAction::Hotbar6);
        key_bindings.insert(KeyCode::Digit7, InputAction::Hotbar7);
        key_bindings.insert(KeyCode::Digit8, InputAction::Hotbar8);
        key_bindings.insert(KeyCode::Digit9, InputAction::Hotbar9);

        // Diğer
        key_bindings.insert(KeyCode::KeyE, InputAction::Inventory);
        key_bindings.insert(KeyCode::KeyQ, InputAction::DropItem);
        key_bindings.insert(KeyCode::KeyF, InputAction::SwapHands);
        key_bindings.insert(KeyCode::F3, InputAction::DebugToggle);
        key_bindings.insert(KeyCode::KeyT, InputAction::Chat);

        Self {
            key_bindings,
            mouse_bindings,
        }
    }

    /// Input event'lerini PlayerController'a çevir.
    pub fn process_input(
        &self,
        keyboard_input: &Input<KeyCode>,
        mouse_input: &Input<MouseButton>,
        mouse_motion: &Events<MouseMotion>,
        mouse_wheel: &Events<MouseWheel>,
    ) -> PlayerController {
        let mut controller = PlayerController::default();

        // Hareket
        if keyboard_input.pressed(KeyCode::W) { controller.move_input.y += 1.0; }
        if keyboard_input.pressed(KeyCode::S) { controller.move_input.y -= 1.0; }
        if keyboard_input.pressed(KeyCode::A) { controller.move_input.x -= 1.0; }
        if keyboard_input.pressed(KeyCode::D) { controller.move_input.x += 1.0; }

        controller.move_input = controller.move_input.normalize_or_zero();

        // Aksiyonlar
        controller.jump_pressed = keyboard_input.just_pressed(KeyCode::Space);
        controller.sprinting = keyboard_input.pressed(KeyCode::LControl);
        controller.sneaking = keyboard_input.pressed(KeyCode::LShift);

        // Mouse
        controller.left_click = mouse_input.just_pressed(MouseButton::Left);
        controller.right_click = mouse_input.just_pressed(MouseButton::Right);
        controller.middle_click = mouse_input.just_pressed(MouseButton::Middle);

        // Mouse motion (kamera)
        for event in mouse_motion.iter() {
            controller.mouse_delta += event.delta;
        }

        // Mouse wheel (hotbar scroll)
        for event in mouse_wheel.iter() {
            controller.mouse_wheel += event.y;
        }

        controller
    }
}
```

---

## 6. Crate Organizasyonu

```
crates/
  player/
    ├── mod.rs              ← Player plugin entry point
    ├── controller.rs       ← PlayerController, input mapping
    ├── movement.rs         ← Movement sistemi
    ├── interaction.rs      ← Block interaction
    ├── inventory/
    │   ├── mod.rs          ← Envanter sistemi
    │   ├── inventory.rs    ← Inventory component
    │   ├── item_stack.rs   ← ItemStack
    │   ├── nbt.rs          ← NBT data
    │   └── enchantments.rs ← Enchantment sistemi
    ├── state.rs            ← PlayerState, GameMode
    └── input/
        ├── mod.rs          ← InputMapper
        └── bindings.rs     ← Key bindings
```
