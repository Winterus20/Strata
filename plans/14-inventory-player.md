# 25 — Inventory & Player Controller

> **Durum:** Kesinleşmiş (anayasa `01`–`14`, `AGENTS.md` §2). Bu revizyon; Plan 14 denetim bulgularını
> (2026-07) ve Bevy ECS alternatiflerinden (shipyard/hecs/flecs/Unity DOTS) çıkarılan
> transfer edilebilir dersleri birleştirir. `01`–`14` anayasa ile çelişirse anayasa esas alınır.

## 1. Genel Bakış

Strata'nın envanter ve oyuncu kontrol sistemi **Bevy ECS 0.17+** üzerinedir. Oyuncu hareketi,
envanter yönetimi ve oyun modları bu sistemde tanımlanır.

### Temel Prensipler

- **Bevy ECS-based:** Tüm component'lar ve sistemler Bevy ECS üzerinden; simülasyon `FixedUpdate` + `Time<Fixed>` içinde.
- **Server-authoritative:** Envanter ve blok etkileşimi server'da doğrulanır; istemcide **prediction + reconcile** uygulanır.
- **Modüler:** Oyun modları (survival, creative, spectator) ayrı component'lar / ZST tag'ler.
- **Input-agnostic:** Klavye/fare/gamepad aynı input layer'ı (forward binding map) kullanır.
- **Data-Oriented:** Hot/cold veri ayrımı zorunlu; hot path'te `Vec`/`HashMap`/`String` ***yasak*** (`AGENTS.md` §A.2/B.3).

### 2026 Denetim Notları (gereken düzeltmeler)

| # | Bileşen | Sorun | Çözüm |
|---|---|---|---|
| D1 | Movement | Çarpışma yok, oyuncu zeminden/duvardan geçer | Voxel swept-AABB collide-and-slide |
| D2 | Movement | `grounded` hiç hesaplanmıyor → zıplama çalışmaz | Aşağı probe ile `grounded` yaz |
| D3 | Movement | `time.delta_secs()` FPS'ye bağlı | `FixedUpdate` + `Time<Fixed>` |
| D4 | Movement | Kamera tabanı hatalı (pitch dahil) | Yaw'a göre flatten |
| D5 | Inventory | `hotbar` ayrı array → divergent dead state | `main[0..9]` view |
| D6 | Inventory | `remove_item` veri kaybı | Gerçek stack'i döndür |
| D7 | Inventory | `ItemStack` inline `HashMap`/`Vec` (DOD ihlali) | Lean POD + cold `ItemDataStore` |
| D8 | Interaction | `world` tanımsız (compile error) | `Res<VoxelWorld>` enjekte |
| D9 | Interaction | `EventWriter` → Bevy 0.17 eski API | `MessageWriter`/`MessageReader` |
| D10 | Interaction | `player_intersects` çağrılmıyor | Placement öncesi overlap kontrolü |
| D11 | Input | `HashMap<KeyCode,Action>` ölü kod | Forward `Action → inputs` map |
| D12 | Input | Eski event API (`Events<MouseMotion>`) | `AccumulatedMouseMotion`/`Scroll` |

---

## 2. Player Controller

### 2.1 Component Mimarisi (hot/cold split)

`PlayerController` (her frame değişen, HOT) ile `PlayerState` (nadir değişen, COLD) ayrı
component'lar olarak kalır. Ek olarak `Velocity` ayrı component; `PlayerState` içine konmaz
(`AGENTS.md` §A.2). Yerel/uzak oyuncu ayrımı için ZST tag'ler (`IsLocalPlayer`/`IsRemotePlayer`)
kullanılır — sistemler dallanma (`if`) yerine `With<T>` filtresiyle partition edilir (flecs
entity-partitioning dersi).

```rust
use bevy::prelude::*;

/// HOT: Her-frame input intent'i (Copy, heap-free).
#[derive(Component, Default, Clone, Copy)]
pub struct PlayerController {
    pub move_input: Vec2,
    pub jump_pressed: bool,
    pub sprinting: bool,
    pub sneaking: bool,
    pub mouse_delta: Vec2,
    pub mouse_wheel: f32,
    pub left_click: bool,
    pub right_click: bool,
    pub middle_click: bool,
}

/// HOT: Hareket hızı (ayrı component, PlayerState içinde DEĞİL).
#[derive(Component, Default)]
pub struct Velocity(pub Vec3);

/// COLD: Oyun modu / durum (nadir değişir).
#[derive(Component)]
pub struct PlayerState {
    pub game_mode: GameMode,
    pub grounded: bool,   // D2: artık hareket sistemi tarafından YAZILIR
    pub flying: bool,     // D: creative/spectator uçuş flag'i
    pub hurt: bool,
    pub death_count: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GameMode { Survival, Creative, Adventure, Spectator }

/// ZST partition tag'leri (flecs dersi): dallanma yerine filtre.
#[derive(Component)]
pub struct IsLocalPlayer;
#[derive(Component)]
pub struct IsRemotePlayer;
```

### 2.2 Hareket Sistemi (FixedUpdate + collide-and-slide)

`player_movement_system` **`FixedUpdate`** schedule'ında çalışır (`Time<Fixed>`, varsayılan 64 Hz).
DDA yerine oyuncu AABB'si XBrickMap voxellerine karşı **swept-AABB collide-and-slide** ile
çözülür (Quake/Half-Life slide; voxel grid için endüstri standardı). `player_intersects` narrowphase
predicate olarak kullanılır.

```rust
/// Simülasyon: FixedUpdate + Time<Fixed>.
fn player_movement_system(
    time: Res<Time<Fixed>>,
    voxel_world: Res<VoxelWorld>,           // D8: authoritative CPU voxels
    mut query: Query<(&PlayerController, &mut Transform, &mut Velocity, &mut PlayerState), With<IsLocalPlayer>>,
) {
    let dt = time.delta_secs();

    for (input, mut transform, mut velocity, mut state) in query.iter_mut() {
        // D4: kamera yaw'ına göre flatten (pitch DAHİL DEĞİL)
        let yaw = transform.rotation.to_euler(EulerRot::YXZ).0;
        let forward = Vec3::new(yaw.sin(), 0.0, yaw.cos()).normalize();
        let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin()).normalize();
        let move_dir = (forward * input.move_input.y + right * input.move_input.x).normalize_or_zero();

        let base_speed = match state.game_mode {
            GameMode::Creative => 10.0,
            _ if input.sprinting => 5.6,
            _ => 4.3,
        };

        // Uçuş modu (creative/spectator): yerçekimi yok, dikey kontrol var
        if state.flying || state.game_mode == GameMode::Spectator {
            let vert = if input.jump_pressed { 1.0 } else if input.sneaking { -1.0 } else { 0.0 };
            velocity.0 = Vec3::new(move_dir.x * base_speed, vert * base_speed, move_dir.z * base_speed);
        } else {
            // Yatay: havada %70 kontrol
            let ctrl = if state.grounded { 1.0 } else { 0.7 };
            velocity.0.x = move_dir.x * base_speed * ctrl;
            velocity.0.z = move_dir.z * base_speed * ctrl;

            if input.jump_pressed && state.grounded {
                velocity.0.y = 8.5;
            }
            if state.game_mode != GameMode::Creative && state.game_mode != GameMode::Spectator {
                velocity.0.y -= 28.0 * dt;
                velocity.0.y = velocity.0.y.max(-50.0); // terminal velocity
            }
            if input.sneaking { velocity.0.x *= 0.5; velocity.0.z *= 0.5; }
        }

        // D1: collide-and-slide — XBrickMap voxellerine karşı per-axis swept AABB
        let desired = velocity.0 * dt;
        let (resolved, grounded) = collide_and_slide(
            transform.translation, desired, &voxel_world,
        );
        transform.translation = resolved;
        state.grounded = grounded;                 // D2
        if grounded && velocity.0.y < 0.0 { velocity.0.y = 0.0; }
    }
}
```

`collide_and_slide` yardımcı fonksiyonu: oyuncu AABB'sini (0.6×1.8×0.6) XBrickMap Sector→Brick→SubBrick
maskesine karşı çözer; Y, X, Z eksenlerinde ayrı çözüm + `player_intersects` ile overlap testi.
Tünel etkisini önlemek için `|desired| > 0.5 * voxel_size` ise alt-adım (sub-step) uygula (eşik voxel
boyutuna bağlı olmalı, sabit `0.5` değil).

**Denetim düzeltmeleri (2026-07, Movement):**
- **Skin width / epsilon:** AABB'yi hafif küçült (~1/16 voxel inset) ve her eksen clamp'inden sonra
  yüzeyi minik bir vektörle it — jitter, seam-catch ve yanlış overlap (touching face) önlenir.
  `player_intersects` katı `<`/`>` (eşikli değil) ya da inset kullanmalı; `Option<T>` benzeri false-positive yok.
- **Step-up / block-hopping:** Yatay clamp'de, üstte boşluk ve `max_step_height` (~0.5–0.6) içinde zemin
  varsa oyuncuyu blok üstüne snap et — aksi halde her 1-voxel eşiğe takılır (Minecraft/Veloren/Godot).
- **İlk-overlap depenetration:** Spawn-içinde-blok / chunk pop-in durumları için min-translation push-out
  (Veloren/PhysX MTD) — saf sweep başlangıç overlap'ini çözemez.
- **Broadphase:** Nested float döngü yerine swept kutunun `[floor(min), ceil(max)]` integer-cell
  enumerasyonu (DDA/`for_each_in`) ile yalnızca solid hücreleri tara — "AABB sweep = tick'in %70'i" patolojisi.
- **Çözüm iterasyonu:** `MAX_ATTEMPTS` (3–4) tavanı; yakınsamazsa ilk TOI'da dur (discrete fallback).
- **Grounded:** Hangi eksenin clamp'lendiğine göre türet (Veloren `resolve_dir.z > 0 ⇒ on_ground`) — tutarlılık.
- **İç-köşe takılması (internal-edge snagging):** Düz bir duvar boyunca her blok sınırı AABB'yi
  yakalayabilir ("her voxel kenarına takılma"). Çözüm: çarpışma normali boyunca komşu voxel de
  solid ise o teması yok say; VEYA katı epsilon-inset + depenetration ile AABB'yi yüzeye asla tam
  oturtma. Bu, "yapışkan duvar" tuzağının tek en olası kaynağıdır — açıkça ele alınmalı (DENETİM 2026-07).
- **Bump / clip-plane cap:** Quake gibi `numbumps = 4`, `MAX_CLIP_PLANES = 5` ile sonsuz clip
  döngüsü köşelerde engellenir; plan bir bump/iteration cap belirtmeli.
- **Maksimum sub-step cap:** `|desired| > 0.5 * voxel_size` eşiğiyle birlikte bir üst sınır
  (max-substeps) konmalı; kötü senaryoda yakınsama ilk TOI'da durur (discrete fallback).

```rust
/// Tek blok AABB overlap testi (narrowphase predicate).
fn player_intersects(block_pos: IVec3, player_pos: Vec3) -> bool {
    let p_min = player_pos - Vec3::new(0.3, 0.0, 0.3);
    let p_max = player_pos + Vec3::new(0.3, 1.8, 0.3);
    let b_min = block_pos.as_vec3();
    let b_max = b_min + Vec3::ONE;
    p_min.x < b_max.x && p_max.x > b_min.x &&
    p_min.y < b_max.y && p_max.y > b_min.y &&
    p_min.z < b_max.z && p_max.z > b_min.z
}
```

### 2.3 Render İnterpolasyonu

`FixedUpdate` simülasyonu ile `Update` render'ı ayrılır; kamera render'da `Transform`'u son iki
fixed state arasında interpolasyon yapar (Unity DOTS / Bevy fixed-timestep dersi) → jitter yok.

**Denetim netleştirmesi (ZORUNLU):** `FixedUpdate` frame başına 0/1/çok kez çalışabileceğinden ham
`Transform` yazımı yüksek-refresh'te takılır. Kanonik Bevy deseni:
- `PreviousPhysicalTranslation` + `PhysicalTranslation` (current) component'leri; `FixedUpdate`'te
  `previous = current; current += vel * dt`.
- `RunFixedMainLoopSystems::AfterFixedMainLoop`'ta `transform.translation = previous.lerp(current,
  time.overstep_fraction())`. **Öneri (denetim 2026-07):** Elle yazmak yerine
  `bevy_transform_interpolation` crate'ini (v0.4, Bevy 0.18 uyumlu; Hermite, teleport-aware, doğru
  schedule kablolaması `FixedFirst`/`FixedLast` + lerp `RunFixedMainLoop`) benimse — hata yüzeyini
  azaltır ve planın niyetiyle birebir uyumludur. Sıfır-bağımlılık isteniyorsa exact schedule
  sıralaması kopyalanır.
- **Sıralama:** kamera **yaw**'ı `BeforeFixedMainLoop`'ta (fizik kamera-bağımlı hareket için okur — D4
  ile uyumlu), kamera **pozisyonu** `AfterFixedMainLoop`'ta interpolate edilmiş transform'u kullanır.
- Ham input `FixedUpdate` içinde örneklenmez; frame başına accumulate edilip fixed step'te tüketilir.
- 64 Hz interpolasyon gecikmesi ~15.6 ms; şikayet olursa 100–128 Hz'e çıkarılabilir (maliyet lineer).

---

## 3. Envanter Sistemi

### 3.1 DOD İlkesi (D7)

`ItemStack` **lean POD** (~12 B, `Copy`) olur; nadir kullanılan veri (isim, lore, enchant, NBT)
`ItemDataStore` adlı **cold arena**'ya taşınır. Bu, Plan 06 `GlobalBrickPool` (**`SlotMap` + `SparseSecondaryMap`**)
deseniyle birebir uyumludur ve `AGENTS.md` §A.2/B.3'ü karşılar. Cold handle **tip-güvenli `ColdHandle`
newtype**'ıdır (raw `NonZeroU32` değil — ABA/dangling koruması ve tip güvenliği için, aynı 8 B).

```rust
/// Tip-güvenli cold handle.
///
/// **DENETİM DÜZELTMESİ (2026-07):** `Option<ColdHandle>` yalnızca `ColdHandle` bir
/// `NonZeroU32` (niche'li) newtype ise 8 B'dir. `slotmap::DefaultKey` niche'siz olduğundan
/// `Option<DefaultKey>` = 16 B olur ve `ItemStack` 12 B değil **24 B** olur. Bu nedenle
/// `ColdHandle`, index + version'ı tek bir `NonZeroU32`'e paketleyen özel bir
/// `slotmap::Key` implementasyonu olmalıdır (önceki `DefaultKey` newtype'ı 8 B Option
/// vermez). Aksi halde 24 B kabul edilip "12 B" iddiası düşürülür.
///
/// `new_key_type!` (slotmap) yalnızca `DefaultKey` tabanlı 8 B bir key üretir (niche'siz) —
/// 8 B `Option` için elle `impl slotmap::Key` (pack/unpack `NonZeroU32`) gerekir.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ColdHandle(NonZeroU32); // impl slotmap::Key: index+version -> NonZeroU32

/// Boyut regresyon koruması: `ColdHandle` düzgün paketlenmişse `ItemStack` 12 B olmalı.
/// `#[cfg(test)] assert_eq!(size_of::<ItemStack>(), 12);` (paketleme yapılmazsa 24 beklenir).

/// HOT POD çekirdek. `ColdHandle` `NonZeroU32` paketli ise **12 B** (Copy, heap-free);
/// aksi halde (niche'siz `DefaultKey`) 24 B. `size_of` testi ile doğrula (DENETİM 2026-07).
#[derive(Clone, Copy, Debug, Default)]
pub struct ItemStack {
    pub item_id: u16,
    pub count: u8,
    pub max_stack: u8,
    pub durability: u16,            // sentinel 0xFFFF = yok
    pub extra: Option<ColdHandle>,  // ItemDataStore handle; None = düz stack (D7 düzeltmesi)
}

/// COLD veri — yalnızca extra.is_some() olan item'lar için allocate edilir (seyrek).
/// Ek seyrek sütunlar (sahip/ışık gibi) gerektiğinde `SparseSecondaryMap<ColdHandle, _>` ile eklenir
/// (tam `SecondaryMap`'a tercih — çoğu item düz olduğundan bellek israfı önlenir).
#[derive(Default)]
pub struct ItemDataStore {
    cold: SlotMap<ColdHandle, ItemColdData>,
}
pub struct ItemColdData {
    pub custom_name: Option<String>,
    pub lore: SmallVec<[String; 2]>,
    pub enchantments: [Enchantment; 4],  // boundsiz Vec YERİNE fixed
    pub enchant_count: u8,
    /// D7: binary NBT (simdnbt `borrow`), lazy-parse. Ham bytes cold arena tarafından sahiplenilir;
    /// yalnızca ihtiyaçta (tooltip) deserialize edilir — hot path'te HashMap<String,_> YOK.
    pub nbt: Option<SimdBuf>,            // simdnbt borrow::read ile lazy-parse
}
```

- NBT **binary `simdnbt`** olarak saklanır, yalnızca gerektiğinde `borrow::read` ile parse edilir
  (HashMap<String,_> inline YASAĞI). Plan 11 zaten hızlı binary format kullanıyor → tutarlılık.
- `Enchantment` fixed array + count; boundsiz `Vec` yok.

### 3.2 Inventory Component (inline array — Bevy idiomatik)

Envanter, **inline sabit boyutlu dizi** component olarak modellendi (Bevy ECS yalnızca `Table` /
`SparseSet` storage sunar; Unity `DynamicBuffer`, `ComponentSparseArray`, `FixedComponent` gibi
Bevy'de **mevcut olmayan** tiplerle KARIŞTIRILMAMALI — "alternatifler" çerçevesi yanıltıcıdır).
36 slot gövdeye gömülü (cache-local, heap-free, `Copy` uyumlu), büyük sandıklar `GlobalItemStore`'a spill eder.

```rust
/// Ana envanter: 27 slot inline + armor + offhand. Hotbar = main[0..9] VIEW (D5).
#[derive(Component, Default)]
pub struct Inventory {
    pub main: [Option<ItemStack>; 27],
    pub armor: [Option<ItemStack>; 4],
    pub selected_slot: u8,          // 0..8
    pub offhand: Option<ItemStack>,
}

impl Inventory {
    pub fn hotbar(&self) -> &[Option<ItemStack>] { &self.main[0..9] }   // D5: view
    pub fn selected(&self) -> Option<&ItemStack> { self.main.get(self.selected_slot as usize).and_then(|s| s.as_ref()) }

    /// Envantere ekle (ilk uygun slot). Recursive Clone YERİNE in-place (D6/D7).
    pub fn add_item(&mut self, mut item: ItemStack) -> Result<(), ItemStack> {
        for slot in self.main.iter_mut() {
            if let Some(existing) = slot {
                if existing.item_id == item.item_id && existing.count < existing.max_stack {
                    let space = existing.max_stack - existing.count;
                    let add = item.count.min(space);
                    existing.count += add;
                    if add >= item.count { return Ok(()); }
                    item.count -= add;
                }
            }
        }
        for slot in self.main.iter_mut() {
            if slot.is_none() { *slot = Some(item); return Ok(()); }
        }
        Err(item)
    }

    /// Çıkar — GERÇEK stack'i döndür (durability/nbt korunur) (D6).
    pub fn remove_item(&mut self, item_id: u16, count: u8) -> Option<ItemStack> {
        let mut remaining = count;
        let mut removed: Option<ItemStack> = None;
        for slot in self.main.iter_mut().rev() {
            if let Some(stack) = slot {
                if stack.item_id == item_id {
                    let take = remaining.min(stack.count);
                    stack.count -= take;
                    remaining -= take;
                    let mut out = *stack;        // POD copy, cold veri handle ile taşınır
                    out.count = take;
                    removed = Some(out);
                    if stack.count == 0 { *slot = None; }
                    if remaining == 0 { break; }
                }
            }
        }
        removed
    }
}
```

### 3.3 Büyük Ölçek: GlobalItemStore (opsiyonel, Plan 06 deseni)

Sandık/düşen eşya/mob envanterleri için `Inventory = [Option<StackHandle>; N]`; tek bir
`GlobalItemStore` (SoA sütunlar + cold blob arena, SlotMap) tüm stack'leri O(1) allocate/free
ile sahiplenir → heap fragmentation yasağı karşılanır (`AGENTS.md` §B.3).

### 3.4 Ownership Relationship (Bevy 0.17 Relationship)

"Hangi oyuncunun envanterinde elmas kazma var?" gibi çapraz sorgular için Bevy 0.17'in **Relationship**
API'si (`Related` / `Relationship` target) kullanılır — elle `(Inventory, OwningEntity)` tuple'ı yerine
first-class, lifecycle-safe ve sorgulanabilir. Block-entity hibrit (Plan 03/05) için de owner link cold
component'a; 2×2×2 voxel verisi `GlobalBrickPool`'da kalır.

---

## 4. Block Interaction

### 4.1 Raycast — Amanatides & Woo 3D-DDA (CPU XBrickMap)

Voxel picking için **DDA** SOTA'dır (~19 adım, 6 m reach). Raycast **yalnızca CPU XBrickMap**
üzerinden (ACTIVE tier, ≤6 m her zaman mevcut); GPU visibility buffer / SVDAG **kullanılmaz**
(non-deterministik, render-only). `world` tanımsız hatası (D8) `Res<VoxelWorld>` enjeksiyonu ile giderilir.

```rust
/// DDA hit sonucu. DDA ziyaret edilen hücreden `u8` palet indeksini *ücretsiz* döndürür;
/// global `block_id: u16`'a çözüm tek `SectorPalette` lookup ile olur (ikinci *traversal* yok).
/// `block_id` ya çözülmüş `u16` taşır ya da `palette_index: u8 + sector_id` — plan bunu sabitlemelidir
/// (Plan 05/06/07 ile tutarlılık; aksi halde veri modeli çelişir).
pub struct VoxelHit {
    pub position: IVec3,
    pub normal: Dir3,       // -step * last_axis
    pub block_id: u16,
    pub t: f32,
}

/// Res<VoxelWorld> üzerinden 3-level XBrickMap descent.
///
/// **DENETİM 2026-07:** Düz (per-voxel) Amanatides & Woo yerine **Hiyerarşik DDA (HDDA,
/// OpenVDB/NanoVDB `LevelSetHDDA`)** kullan — Sector(32³)→Brick(8³)→SubBrick(2³) maskeleri
/// boşlukları O(log N) sıçrar; seyrek arazide ~19 adım yerine <5 etkin adım. Yalnızca dolu
/// SubBrick içinde per-voxel DDA (≤8 adım) yapılır. A&W determinizmi `tMax`/epsilon
/// başlatmasına bağlıdır: grazing/axis-aligned ışınlar ve 6 m reach sınırı için birim testleri
/// yaz (sınır aliasing'i picking flicker'ına yol açar).
fn voxel_raycast(origin: Vec3, dir: Vec3, reach: f32, world: &VoxelWorld) -> Option<VoxelHit> {
    // Hierarchical DDA: Sector→Brick→SubBrick mask descent; per-voxel yalnızca dolu SubBrick'te.
    // ...
}
```

### 4.2 Interaction System (prediction + reconcile, Bevy 0.17 API)

`EventWriter` → **`MessageWriter`** (D9, `AGENTS.md` §5). Tek otorite `apply_block_change`
sistemi hem client prediction hem server broadcast'i funneller → double-write divergence yok.

```rust
fn block_interaction_system(
    voxel_world: Res<VoxelWorld>,
    camera: Query<&Transform, With<Camera>>,
    mut controller: Query<&PlayerController, With<IsLocalPlayer>>,
    mut requests: MessageWriter<ClientBlockBreakRequest>,   // D9: plan 03 modeli
    mut places: MessageWriter<ClientBlockPlaceRequest>,
) {
    let input = controller.single();
    let cam = camera.single();
    let origin = cam.translation + Vec3::new(0.0, 0.0, 0.0); // göz = kamera origin
    let dir = cam.forward();
    let reach = if /* creative */ true { 6.0 } else { 5.0 };

    if let Some(hit) = voxel_raycast(origin, dir, reach, &voxel_world) {
        // Sol tık — kır (adventure hariç)
        if input.left_click {
            requests.send(ClientBlockBreakRequest { position: hit.position });
        }
        // Sağ tık — yerleştir (D10: overlap + air kontrolü)
        if input.right_click {
            let place_pos = hit.position + hit.normal.as_ivec3();
            let player_pos = cam.translation;
            if voxel_world.get_block(place_pos).is_none()
               && !player_intersects(place_pos, player_pos) {
                places.send(ClientBlockPlaceRequest {
                    position: place_pos,
                    block_id: /* selected.item_id */ 0,
                });
            }
        }
    }
}
```

**Client prediction + reconcile:**
1. Tıklamada yerel XBrickMap'e hemen uygula → `NeedsRemesh` + `NeedsSvdagBake` ZST ekle.
2. `ClientBlockBreakRequest` / `ClientBlockPlaceRequest` gönder (her isteğe `client_id` + `client_seq` eklenir).
3. Server doğrular (reach, game mode, `player_intersects`, hedef air, sector ACTIVE tier'da yüklü) → broadcast.
4. **`apply_block_change` idempotent ve reversible olmalı:** rollback önceki `block_id` (ve palet indeksi/
   sub-brick mask) değerini saklar, "air'e set" değil. Reconcile granularity = **per-edit** (`(pos, client_seq)`
   map); uyuşmazlıkta yalnızca o editi geri al, hâlâ pending sonraki local editleri koru (clobber yok).

**Tek otorite:** `apply_block_change(pos, block_id)` → XBrickMap mutate + `NeedsRemesh`/`NeedsSvdagBake`
ZST set (meshing sistemi `Added<NeedsRemesh>` filtresiyle alır, polling yok — flecs observer dersi).
**Kritik (denetim D-notes):** `Added<T>` yalnızca ilk insert frame'inde tetiklenir; meshing/SVDAG bake
işi **dispatch anında** (sector snapshot alındığında) ZST'i `remove` etmeli, aksi halde aynı sektörün
sonraki editi sessizce kaçar. Birden çok tüketici (mesh + bake) için ayrı ZST'ler (`NeedsRemesh`,
`NeedsSvdagBake`) korunur. Hızlı ardışık edit patlamasında `Commands::insert` zaten no-op → tek `Added`
(yeterli; bir remesh o frame'deki tüm editleri kapsar).

### 4.3 Bevy 0.17 Event Modeli

`StrataEvent` yerine Plan 03'ün ağ-güvenli modeli (hepsi `Serialize/Deserialize` + `#[derive(Message)]`,
`bincode2`/`bitcode` ile serialize; `MessageWriter`/`MessageReader` in-app dispatch katmanıdır, **wire
transport değildir** — üstüne `bevy_renet` veya `naia-bevy` gerekir):

- `ClientBlockBreakRequest { position, client_id, client_seq }`
- `ClientBlockPlaceRequest { position, block_id, client_id, client_seq }`
- `ServerBlockBrokenBroadcast { position, new_block_id, accepted, client_id, client_seq }`
- `ServerBlockPlacedBroadcast { position, new_block_id, accepted, client_id, client_seq }`
- **Eklenen (denetim 2026-07):** `client_seq` ile ACK bağlantısı (paket kaybında pending prediction takılmaz);
  `accepted: bool` / `ServerBlockChangeReject { pos, expected_block_id }` ile rollback sinyali;
  eşzamanlı edit çakışması için **Last-Write-Wins** (`server_timestamp`, tie-break `client_id`);
  anti-echo (`seq` eşleşmesinde kaynağın kendi prediction'ı override edilmez);
  delta batch (birden çok edit tek broadcast'te).

**Serialleştirme notu (denetim 2026-07):** `bitcode` benchmark'ta en hızlısı olabilir ancak
"format kararlılığı" ve "kendi kendine açıklayıcı" non-goal'idir — uzun ömürlü MMO wire
protokolü için tehlikelidir. `bincode2` de "major versiyonlar arası format değişebilir" uyarısı
verir. Bu nedenle edit mesaj şeması **versioned bir envelope** içinde sarılmalı
(`magic + version + varint/packed fields`: `pos: i64 packed`, `client_id: u32`, `client_seq: u32`,
`prev_block: u16`, `new_block: u16`, `server_ts: u64`) ve `ReliableOrdered` kanalından gönderilmeli.
`bitcode` pin'lenir (hızlı/sıkıştırılabilir) ama format kararlılığına bağımlı kalınmaz; stabil,
`no_std`, kompakt fallback olarak `postcard` korunur.

---

## 5. Input Mapping

### 5.1 Forward Binding Map (D11)

Reverse `HashMap<KeyCode, Action>` **silindi** (ölü kod). Yerine **forward map**:
`Action → SmallVec<[InputBinding; 4]>` (`EnumMap` ile, sabit dizi arka planı). Rebindable; **`EnumMap` +
`SmallVec` formu gerçekten heap-free ve cache-friendly** (eski `HashMap<InputAction, Vec<InputBinding>>`
formu bucket array + `Vec` değerleri heap'te allocate ettiği için "heap-free" iddiasıyla çelişiyordu —
D11 düzeltmesi).

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputAction {
    MoveForward, MoveBackward, MoveLeft, MoveRight, Jump, Sprint, Sneak,
    Attack, Use, PickBlock, Hotbar1..=Hotbar9, Inventory, DropItem, SwapHands, DebugToggle, Chat,
}

/// Rebindable config (cold Resource, RON/TOML).
/// `HashMap` YERİNE `EnumMap` (sabit dizi, O(1), hashing yok, heap-free) + `SmallVec` (bağlama listesi
/// stack-resident, yaygın durumda heap allocate etmez). D11 düzeltmesi.
#[derive(Resource)]
pub struct Bindings { map: EnumMap<InputAction, SmallVec<[InputBinding; 4]>> }

impl Bindings {
    pub fn pressed(&self, a: InputAction, kb: &ButtonInput<KeyCode>, mb: &ButtonInput<MouseButton>) -> bool {
        self.map[a].iter().any(|x| match x {
            InputBinding::Key(k) => kb.pressed(*k),
            InputBinding::Mouse(m) => mb.pressed(*m),
        })
    }
    pub fn just_pressed(&self, a: InputAction, kb: &ButtonInput<KeyCode>, mb: &ButtonInput<MouseButton>) -> bool {
        // aynısı just_pressed ile
    }
}
```

### 5.2 Input Sampling (Bevy 0.17 + Accumulated, D12)

Eski `Events<MouseMotion>` / `iter_current_update_events` yerine **`AccumulatedMouseMotion`** /
**`AccumulatedMouseScroll`** (Bevy 0.17 native, per-frame auto-reset). `PlayerController` bir
`Resource` olarak **in-place** güncellenir (`set_if_neq()` metodu / `bypass_change_detection`, DOD P0-3).

```rust
fn sample_input(
    bindings: Res<Bindings>,
    kb: Res<ButtonInput<KeyCode>>,
    mb: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,     // D12
    scroll: Res<AccumulatedMouseScroll>,
    mut controller: ResMut<PlayerController>, // in-place, heap-free
) {
    let mut c = PlayerController::default();
    c.move_input = Vec2::new(
        (bindings.pressed(MoveRight, &kb, &mb) as i32 - bindings.pressed(MoveLeft, &kb, &mb) as i32) as f32,
        (bindings.pressed(MoveForward, &kb, &mb) as i32 - bindings.pressed(MoveBackward, &kb, &mb) as i32) as f32,
    ).normalize_or_zero();
    c.jump_pressed = bindings.just_pressed(Jump, &kb, &mb);
    c.sprinting    = bindings.pressed(Sprint, &kb, &mb);
    c.sneaking     = bindings.pressed(Sneak, &kb, &mb);
    c.left_click   = bindings.just_pressed(Attack, &kb, &mb);
    c.right_click  = bindings.just_pressed(Use, &kb, &mb);
    c.middle_click = bindings.just_pressed(PickBlock, &kb, &mb);
    c.mouse_delta  = motion.delta;   // D12
    c.mouse_wheel  = scroll.delta;
    *controller = c;                  // in-place; yalnızca değiştiğinde ata: set_if_neq()
}
```

### 5.3 leafwing-input-manager (opsiyonel, feature-flag'li)

Gamepad/analog/chord/contextual bağlama isteniyorsa `leafwing-input-manager` (Bevy 0.17 uyumlu,
`ActionState`) adoption edilebilir. **DENETİM 2026-07:** leafwing'in `InputMap`'i dahili `HashMap`
multimap + per-entity `ActionState` component'tir → Strata'nın "heap-free, kendi içimizde" felsefesine
aykırı (D11/D12'in kaçındığı reverse-map + heap'i geri getirir). Bu nedenle leafwing **yalnızca**
`#[cfg(feature = "gamepad")]` arkasında, **input source** (ek cihaz/gamepad/analog) katmanı olarak
tutulmalı; core klavye/fare `InputAction` sampling path'i Strata'nın `EnumMap` resource'undan
okumaya devam etmeli. leafwing yalnızca rebinding/ek cihaz kaynağı besler, çekirdek değildir.

---

## 6. ECS Mimari Dersleri (Alternatiflerden Transfer Edilen)

Bu bölüm shipyard/hecs/flecs/Unity DOTS araştırmasından çıkarılan, Bevy ECS üzerinde uygulanabilir
pratikleri özetler (motoru değiştirmeden).

1. **SystemSet'ler + açık sıralama.** `StreamingSet → MeshingSet → SimulationSet → RenderExtractSet`
   arası `.before/.after` ile `ApplyDeferred` sınırları deterministik olur. Sıralamasız sistemler
   `Commands`'ı ancak sonraki frame görür → streaming→mesh→collider zinciri kırılır.
2. **Observers (`On<Add, SectorLoaded>`)** ile manuel event fan-out yerine reaktif tetikleme
   (Plan 08 §6): physics/lighting/network her biri observe eder, merkezi dispatcher yok (flecs observer dersi).
   **Netleştirme:** `SectorLoaded` bir **component** ise `On<Add, SectorLoaded>`; tek-seferlik sinyal ise
   `#[derive(Event)]` + `On<SectorLoaded>` (yalnız `Add` değil). Binlerce sektör için N observer tetiklemesi
   pahalı olabilir — yüksek hacimli fan-out'ta `Message` + `MessageReader::par_read()` (batch, paralel) daha ucuz.
3. **Hot/cold granularity.** Bir component başına bir değişim frekansı. `PlayerState` (her frame) ile
   `InventoryContents` (nadir) ayrı component; yoksa `Changed<PlayerState>` envanter cache'ini yanlış geçersiz kılar.
4. **ZST tag filtreleri.** `NeedsRemesh`, `ChunkDirty`, `IsLocalPlayer` — archetype-seviyesi, paralel,
   sıfır maliyetli filtre. `Option<&T>` **yalnızca presence filter olarak** yasak; gerçekten isteğe bağlı
   alanı okumak için `Option<&T>` meşrudur. Filtreleme için `With<T>`/`Without<T>`/ZST tercih edilir
   (`AGENTS.md` §A.1 — abartılı "FORBIDDEN" ifadesi düzeltildi).
5. **`set_if_neq()` (metot) / `bypass_change_detection()`.** `&mut T` alan (atama yapmasa da) `Changed`
   işaretler. Bevy'ye özgü bu tuzak flecs/Unity'nin read/write ayrımıyla önlenir; Strata'da `set_if_neq()`
   metodu zorunludur (**NOT:** `set_if_neq!` makrosu Bevy'de MEVCUT DEĞİLDİR ve derlenmez — metot olarak
   çağrılır: `comp.set_if_neq(yeni_deger)`, `PartialEq` gerektirir; `replace_if_neq`/`clone_from_if_neq` de var).
6. **Buffered structural edits.** Hot parallel sistemde `commands.spawn/despawn` YOK; `Commands`/`Deferred`
   ile queue'la, açık sınırda tek flush. Streaming manager sektör spawn/despawn'ı frame başına batch'ler.
7. **Determinism.** `FixedUpdate` + açık `.before/.after` (lockstep/netcode için). `PCG32+wyhash`
   (Plan 11) per-system seed; sim sistemde global `rand` YOK. **Ek gotcha'lar (Plan 11'e taşınmalı):**
   float nondeterminism (`glam` `libm`, x87'yi kapat, SIMD/AVX tutarlılığı); entity iteration order stabil
   değil → oyun anahtarıyla sort; sim-side map'lerde `IndexMap`/fixed-seed `hashbrown`; Bevy 0.18'de
   `SimpleExecutor` kaldırıldı → `SingleThreadedExecutor` veya sıfır-ambiguity `MultiThreadedExecutor`;
   `FixedUpdate` tek sabit schedule (çoklu rate yok).
   **Determinism sağlamlaştırma (denetim 2026-07):** "glam libm + x87 kapat" YETERSİZ. Lockstep için:
   sim crate'te `glam`/`bevy_math` `libm` feature'ı ZORUNLU; `.cargo/config.toml` →
   `rustflags = ["-C", "target-feature=+sse2,-fma"]` (FMA contract'i a*b+c rounding drift'ini söker,
   sık unutulur); opsiyonel olarak sim crate'e `scalar-math` (bit-exact, SIMD'siz). Tüm netcode
   peer'ları aynı `rustc` + `target-cpu` paylaşmalı. Ayrıca: her sim sistemi
   `query.iter_mut().sort::<&GameKey>()` ile sıralı iterate etmeli; stabil `SectorKey`/`RollbackId`
   + `MapEntities` zorunlu; rollback için tüm sim state snapshot'a kayıtlı + CI'da checksum plugin.
8. **CI lint.** `ScheduleGraph::conflicting_systems()` bir **runtime** API'dir (build edilmiş schedule
   üzerinde) ama **entegrasyon testi ile CI gate'i YAPILABİLİR**: `App` kur, `Schedules` kaynağından
   schedule'ı al, `schedule.graph().conflicting_systems().check_if_not_empty().is_ok()` assertion'ı
   (`FixedUpdate`, `Update` ve tüm Strata özel schedule'ları için). Gerçek mekanizma: schedule build
   ayarında `ambiguity_detection = Error` + bu test. **NOT:** "`&mut` alan ama `set_if_neq` kullanmayan
   sistemler için clippy lint" diye bir lint MEVCUT DEĞİLDİR — iddia kaldırıldı. `set_if_neq!` makrosu
   da MEVCUT DEĞİLDİR; metot `set_if_neq()` (eski değer için `replace_if_neq()`, alan için
   `map_unchanged()`) kullanılır.
   `cargo hakari` (workspace-hack, 2026'da hâlâ geçerli) build hızı (Plan 02 P0-2); ek araçlar: `sccache`,
   `lld`/`rust-lld` linker, `cargo nextest`.
9. **`par_iter` (Bevy 0.18 `ComputeTaskPool`)** büyük bağımsız işler için (block-interaction sweep,
   inventory iter); içeride `Commands`/shared mut yok. **NOT:** `AsyncComputeTaskPool`/`IoTaskPool`
   Bevy 0.14'te kaldırıldı ve `ComputeTaskPool`'a katıldı (Plan 11 referansı güncellenmeli) — bloklayıcı
   iş için `ComputeTaskPool::get().spawn_blocking`, async için `spawn` kullanılır.

---

## 7. Crate Organizasyonu

```
crates/
  player/
    ├── mod.rs              ← Player plugin entry point (SystemSet kayıtları)
    ├── controller.rs       ← PlayerController, Velocity, PlayerState, ZST tag'ler
    ├── movement.rs         ← FixedUpdate hareket + collide_and_slide + grounded probe
    ├── interaction.rs      ← DDA raycast, prediction/reconcile, apply_block_change
    ├── inventory/
    │   ├── mod.rs          ← Inventory component (inline array — Bevy idiomatik)
    │   ├── item_stack.rs   ← Lean POD ItemStack + ItemDataStore (cold arena)
    │   ├── nbt.rs          ← simdnbt binary lazy-parse
    │   └── enchantments.rs ← fixed-array Enchantment
    ├── state.rs            ← PlayerState, GameMode
    └── input/
        ├── mod.rs          ← InputMapper, sample_input (AccumulatedMouse*)
        └── bindings.rs     ← Forward Action → Vec<InputBinding>, Bindings Resource
```
