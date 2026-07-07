# 04 — Plugin API & Modüler Mimari Sistemi

## 1. Genel Bakış ve Felsefe

Strata, **Data-Oriented Design (DOD)** ve **Plugin-First** (Önce Eklenti) felsefesiyle inşa edilmiş ultra yüksek performanslı bir voxel oyun motorudur. Motorun hiçbir parçası monolitik değildir; Render, Fizik, XBrickMap ve Dünya Üretimi gibi tüm alt sistemler birer Bevy ECS eklentisidir (Plugin).

Rust doğrudan makine koduna derlendiği için JVM/CLR ekosistemlerindeki **runtime hook/mixin** (bytecode enjeksiyonu, sanal çağrı tabloları) yaklaşımı Strata'da yoktur ve hedeflenmez. Bunun yerine %100 tip güvenli, bellek dostu (cache-friendly) ve DOD uyumlu **Native Bevy Plugin**, **SubApp İzolasyonu**, **Bevy 0.18 Scheduler** ve **WASM Sandboxing (wasmtime 45.0)** kullanılır.

### Temel Prensipler
- **DOD-Plugin Hybrid:** Eklentiler kendi içlerinde nesne (OOP Object) barındırmaz. Veriler `GlobalBrickPool` gibi Strata'nın ortak bellek havuzlarında tutulur. Eklentiler sadece bu veriyi işleyecek **Sistemleri (Systems)** ECS'ye kaydeder.
- **Granülerlik × Güven İlkesi (En Kritik Modding Kuralı):** Bir mod'un nereye dokunabileceği, **ne sıklıkta çalıştığına** (granülerlik) ve **ne kadar güvenildiğine** (güven kademesi) göre belirlenir. Tek dokunulmaz kural: **mod kodu hot inner loop'a (per-voxel / per-contact / per-vertex SIMD döngüsü) ASLA girmez.** Bunun dışındaki her katman — fizik, meshing, ışıklandırma, world-gen dahil — *doğru granülerlikte* ve *doğru güven kademesinde* modlanabilir hale getirilir (bkz. §10.1 katman modeli ve matris).
- **Katman Ayrımı (L0–L4):** Ağır hesap (XBrickMap SIMD, GPU pipeline, solver/mesher kernel) **L0 native**'dedir ve mod kodu içermez. Üst katmanlar mod'a *veri* (L4 data pack), *batch policy* (L3 WASM hook) ve *strateji* (L2 native registry) sağlar; çekirdek sistemler bu üst katmanları okuyan **dispatcher**'lardır. "Her şey moddanabilir" bu modeli doldurmaktır, hot-inner'ı feda etmek değildir.
- **Veri-Önce Modding:** Blok tanımı, doku, loot, recipe gibi içerik çoğunlukla **TOML/asset** (`05-block-registry.md`) ile gelir; WASM yalnızca özel davranış gerektiğinde devreye girer.
- **SubApp İzolasyonu:** Render ve Fizik gibi kritik alt sistemler kendi `World`'lerinde (SubApp) izole çalışır. Main world'den SubApp'e sadece gerekli "hot data" `set_extract` ile kopyalanır. (Audio SubApp ileride değerlendirilebilir — şimdilik ana App'te.)
- **İki katmanlı plugin yükleme:** `StrataCorePlugins` / `StrataSubAppPlugin` = motor bootstrap (`04`); tam oyun plugin grafiği ve cross-plugin `SystemSet` zinciri = `StrataPlugin` (`03-ecs-architecture.md` §3, §5).
- **Derleme Zamanı Güvenliği:** Bağımlılıklar (Dependencies) ve sıralamalar string isimlerle çalışma zamanında (runtime) değil, Rust'ın tip sistemiyle derleme zamanında çözülür.
- **Filter-First Scheduling:** Sistemler arası bağımlılıklar `SystemSet` ve `.chain()` ile compile-time'da topolojik olarak garantilenir. Runtime ambiguity detection Bevy 0.18 `ScheduleBuildSettings` ile yönetilir.
- **Performans:** `Box<dyn>` / dynamic dispatch **hot inner loop'ta** yasaktır; L1 dispatcher'da tick/batch başına coarse strateji seçimi kabul edilir (§10.4).

---

## 2. SubApp Mimarisi (World İzolasyonu)

Bevy 0.18'in `SubApp` sistemi, her alt sistemin kendi `World`'ünü yönetmesine izin verir ([docs.rs/bevy/0.18.1/SubApp](https://docs.rs/bevy/0.18.1/bevy/prelude/struct.SubApp.html)). Bu, özellikle GPU iletişimi (Render) ve sabit timestep fizik (Physics) için kritiktir.

**Bevy 0.18 SubApp kuralları** ([docs.rs SubApp 0.18.1](https://docs.rs/bevy/0.18.1/bevy/prelude/struct.SubApp.html), kaynak: [sub_app.rs](https://github.com/bevyengine/bevy/blob/master/crates/bevy_app/src/sub_app.rs)):
- `update_schedule` **zorunlu** — atanmazsa `SubApp::update` schedule çalıştırmaz; sistemler hiç koşmaz ([bevy#15841](https://github.com/bevyengine/bevy/issues/15841), [PR #16160](https://github.com/bevyengine/bevy/pull/16160)).
- `set_extract` **isteğe bağlı** — tanımlanmazsa `extract()` no-op; main→sub veri aktarımı için **zorunlu** (Strata Render/Physics).
- `set_extract(main, sub)` yalnızca **main → sub** kopyalar; ters yön için ayrı köprü gerekir (write-back, §2.1).
- `bevy_render` kullanılıyorsa varsayılan extract vardır; özelleştirmek için `take_extract()` ile sarmalayıp varsayılanı çağırın.
- Frame sırası: `main.run_default_schedule()` → her SubApp için `extract(&mut main.world)` + `update()`.

```rust
use bevy::prelude::*;
use crossbeam::queue::SegQueue;
use std::sync::Arc;

#[derive(AppLabel, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum StrataSubApp {
    Render,
    Physics,
}

pub struct StrataSubAppPlugin;

impl Plugin for StrataSubAppPlugin {
    fn build(&self, app: &mut App) {
        // Paylaşılan write-back kanalı — main App'te bir kez kayıt edilir
        app.insert_resource(PhysicsWriteBackChannel(Arc::new(SegQueue::new())));

        // --- RENDER SUBAPP ---
        // Mesh akışı: Extract (main→render) → Prepare → Render (Bevy render graph)
        let mut render_app = SubApp::new();
        render_app.update_schedule = Some(Main.intern()); // ZORUNLU

        // bevy_render ile: let mut default_extract = render_app.take_extract();
        render_app.set_extract(|main_world, render_world| {
            // 1. Delta dirty listesi — tüm DirtySectors clone DEĞİL (sadece bu frame işaretleri)
            if let Some(delta) = main_world.get_resource::<DirtySectorDelta>() {
                render_world.insert_resource(delta.clone());
            }

            // 2. Filter-First: ChunkDirty ZST ile işaretli mesh entity'leri
            let mut query =
                main_world.query_filtered::<(Entity, &Handle<Mesh>), With<ChunkDirty>>();
            for (entity, mesh_handle) in query.iter(main_world) {
                // Render world'de aynı Entity id mirror (spawn sırasında reserved id ile)
                if render_world.get_entity(entity).is_ok() {
                    render_world.entity_mut(entity).insert(mesh_handle.clone());
                }
            }

            // if let Some(f) = default_extract.as_mut() { f(main_world, render_world); }
        });
        render_app.add_plugins(StrataRenderPlugin); // yalnızca SubApp world'üne
        app.insert_sub_app(StrataSubApp::Render, render_app);

        // --- PHYSICS SUBAPP ---
        // Bevy'de fizik schedule'ı: FixedUpdate (PhysicsFixedUpdate diye ayrı label yok)
        let mut physics_app = SubApp::new();
        physics_app.update_schedule = Some(FixedUpdate.intern()); // ZORUNLU

        physics_app.set_extract(|main_world, physics_world| {
            let map = main_world.resource::<PhysicsEntityMap>();
            physics_world.insert_resource(map.clone());
            // Hot component sync: With<PhysicsSync> filtreli entity'ler (Transform, Velocity)
            let mut q = main_world.query_filtered::<(Entity, &Transform, &Velocity), With<PhysicsSync>>();
            for (entity, transform, velocity) in q.iter(main_world) {
                if let Ok(mut e) = physics_world.get_entity_mut(entity) {
                    e.insert((*transform, *velocity));
                }
            }
        });
        physics_app.add_plugins(StrataPhysicsPlugin);
        physics_app.add_systems(
            FixedUpdate,
            physics_collect_write_back.after(StrataPhysicsSets::SolveConstraints),
        );
        app.insert_sub_app(StrataSubApp::Physics, physics_app);
    }
}

// Ana uygulamada (main.rs):
// fn main() {
//     App::new()
//         .add_plugins(StrataSubAppPlugin)
//         .add_plugins(StrataCorePlugins)
//         .run();
// }
```

### Extract Stratejisi (SoA Odaklı)

Extract fonksiyonu, main world'den SubApp'in world'üne veri kopyalar. Sadece **değişen** veriler kopyalanmalıdır.

### Physics SubApp Write-Back (Two-Phase Sync)

Physics SubApp veri *alır* (extract) ve hesaplama sonuçlarını main world'e geri *göndermelidir* (write-back). Bevy 0.18'de `set_extract()` yalnızca **main → sub** yönlüdür; SubApp sistemleri main `World`'e doğrudan erişemez.

**Çözüm: Paylaşılan kanal + main schedule apply**
1. **Phase 1 (Extract, frame N):** Main → Physics (`PhysicsEntityMap`, `With<PhysicsSync>` hot component'lar)
2. **Phase 2 (Physics FixedUpdate, frame N):** SubApp içinde `Changed<Transform>` topla → lock-free kanala push
3. **Phase 3 (Main Update, frame N+1):** Main App `physics_apply_write_back` ile kanalı tüket (SubApp'ler main schedule'dan **sonra** çalışır)

```rust
use crossbeam::queue::SegQueue;
use std::sync::Arc;

#[derive(Resource, Clone)]
pub struct PhysicsWriteBackChannel(pub Arc<SegQueue<PhysicsSyncItem>>);

#[derive(Clone)]
pub struct PhysicsSyncItem {
    pub entity: Entity,
    pub transform: Transform,
}

// Physics SubApp — FixedUpdate sonu (SubApp world'ünde çalışır)
fn physics_collect_write_back(
    query: Query<(Entity, &Transform), Changed<Transform>>,
    channel: Res<PhysicsWriteBackChannel>,
) {
    for (entity, transform) in &query {
        channel.0.push(PhysicsSyncItem {
            entity,
            transform: *transform,
        });
    }
}

// Main App — Update başı (StrataSets::Input öncesi veya hemen sonra)
fn physics_apply_write_back(
    mut transforms: Query<&mut Transform>,
    channel: Res<PhysicsWriteBackChannel>,
) {
    while let Some(item) = channel.0.pop() {
        if let Ok(mut t) = transforms.get_mut(item.entity) {
            if t.translation != item.transform.translation
                || t.rotation != item.transform.rotation
            {
                *t = item.transform; // guard: gereksiz Changed tetikleme yok
            }
        }
    }
}
// Kayıt (ana App): app.add_systems(Update, physics_apply_write_back.in_set(StrataSets::Input));
```

Bu desen SoA'yı korur: tüm physics state kopyalanmaz; yalnızca değişen `Transform`'lar kanal üzerinden akar. Çarpışma olayları için aynı kalıp (`CollisionEventQueue`) kullanılır.

```rust
// ÖNERİLEN EXTRACT PATTERN (set_extract closure içinde):
// 1. ZST marker (ChunkDirty / SectorDirty / PhysicsSync)
// 2. query_filtered::<_, With<Marker>> — per-entity if yok
// 3. SubApp'e yalnızca hot component batch

// set_extract(|main, render| { ... }) imzası: FnMut(&mut World, &mut World)
```

### ZST Dirty-Flag Lifecycle (3 Aşama)

Zero-Sized Type (ZST) component'lar (`ChunkDirty`, `SectorDirty`, `TransformChanged`) veri taşımaz — sadece *işaret* görevi görürler. Doğru kullanım için kesin bir yaşam döngüsü izlenmelidir:

| Aşama | Schedule | İşlem |
|-------|----------|-------|
| **1. Mark** | Oyun mantığı sistemleri | Değişiklik tespit edildiğinde `commands.entity(e).insert(ChunkDirty)` ekle |
| **2. Consume** | SubApp Extract / Render Prepare | `With<ChunkDirty>` filter ile sadece işaretli entity'leri oku ve veriyi SubApp'e kopyala |
| **3. Clear** | En son (Last sıralaması) | Tüm `ChunkDirty` component'larını temizle, bir sonraki frame'e hazırla |

```rust
// Aşama 1: Bir sistem değişiklik yaptığında ZST marker ekler
fn worldgen_system(
    mut commands: Commands,
    query: Query<Entity, Added<ChunkGenerated>>,
) {
    for entity in &query {
        commands.entity(entity).insert(ChunkDirty);
    }
}

// Aşama 2: SubApp extract veya render hazırlık sistemi marker'ı okur
fn render_extract(
    main_world: &mut World,
    render_world: &mut World,
) {
    let mut query = main_world.query_filtered::<(Entity, &Handle<Mesh>), With<ChunkDirty>>();
    for (entity, mesh) in query.iter(main_world) {
        render_world.entity_mut(entity).insert(mesh.clone());
    }
}

// Aşama 3: Clear — Extract'ten sonra, aynı frame'in en sonunda çalışır
fn clear_dirty_flags(
    mut commands: Commands,
    dirty_query: Query<Entity, With<ChunkDirty>>,
) {
    for entity in &dirty_query {
        commands.entity(entity).remove::<ChunkDirty>();
    }
}
// Kayıt: .in_set(StrataSets::RenderPrepare).last() ile en sona alınır
```

**Neden ZST?** ZST component'lar (örn: `struct ChunkDirty;`) sıfır byte yer kaplar. Bir entity'de bulunup bulunmaması archetype'ı değiştirir ancak bellek tüketmez. Bevy 0.18, `With<ChunkDirty>` filtrelemesini archetype bitmask'ları üzerinde O(1) işlemle yapar — per-entity `if` kontrolüne gerek kalmaz.

---

## 3. SubApp'siz Ana App (Game World) - Plugin Sistemi

Motor eklentileri, özel yapılar yerine doğrudan Bevy'nin `Plugin` trait'ini kullanır ([Plugin trait](https://docs.rs/bevy/0.18.1/bevy/app/trait.Plugin.html)). Ana oyun dünyası (world, entity'ler, oyun mantığı) SubApp'siz ana App'te çalışır.

| Yapı | Rol | Plan |
|------|-----|------|
| `StrataSubAppPlugin` + `StrataCorePlugins` | Motor bootstrap (voxel, meshing, SubApp kurulumu) | `04` |
| `StrataPlugin` | Tam oyun (Player, Network, Lighting, …) + cross-plugin `SystemSet` | `03` §3, §5 |
| Plugin-içi setler (`PlayerSystems`, `PhysicsSystems`, …) | Her crate kendi `configure_sets` | `03` §5.1 |
| `StrataSets` | Yalnızca **çekirdek Update** bootstrap zinciri | `04` §4.B |

```rust
use bevy::prelude::*;

pub struct StrataMeshingPlugin;

impl Plugin for StrataMeshingPlugin {
    fn build(&self, app: &mut App) {
        // Eklenti kendi sistemlerini ve eventlerini motora kaydeder
        app.add_systems(Update, mesh_generation_system.in_set(StrataSets::Meshing));
    }

    fn finish(&self, app: &mut App) {
        // finish() phase'inde bağımlılık kontrolü:
        // StrataMeshingPlugin, XBrickMapPlugin olmadan çalışamaz.
        assert!(
            app.is_plugin_added::<XBrickMapPlugin>(),
            "StrataMeshingPlugin requires XBrickMapPlugin!"
        );
    }
}
```

### Strata PluginGroups (Toplu Yükleme)

Çekirdek sistemler `PluginGroup` ile yüklenir. **Render/Physics plugin'leri ana grupta değil** — yalnızca `StrataSubAppPlugin` içindeki ilgili SubApp'e eklenir (çift kayıt ve world karışıklığını önler).

```rust
use bevy::app::PluginGroupBuilder;

pub struct StrataCorePlugins;

impl PluginGroup for StrataCorePlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(StrataSubAppPlugin)       // Render + Physics SubApp'leri kurar
            .add(StrataSchedulingPlugin) // StrataSets / StrataPhysicsSets
            .add(BlockRegistryPlugin)      // 05 — init-only registry
            .add(XBrickMapPlugin)
            .add(StrataMeshingPlugin)
            // StrataRenderPlugin / StrataPhysicsPlugin → SubApp içinde (§2)
    }
}

// client / server entry (main.rs):
// App::new()
//     .add_plugins(StrataCorePlugins)
//     .add_plugins(StrataPlugin)   // 03 — oyun katmanı (Network, Player, …)
//     .add_plugins(ModdingPlugin)  // 32 — StrataCorePlugins SONRASI
```

---

## 4. Bağımlılık ve Sıralama Yönetimi (SystemSets & Schedules)

Bir eklentinin (örneğin Render) çalışabilmesi için başka bir eklentinin (örneğin Meshing) veriyi hazırlaması gerekir. Bu topolojik sıralama, çalışma zamanı hata riskini sıfıra indirmek için **SystemSet**'ler kullanılarak tasarlanır.

Bevy 0.18'de birden çok schedule vardır. Strata, her fiziksel süreci kendi schedule'ında yönetir:

### A. FixedUpdate (Fizik - Sabit Timestep)

Fizik hesapları 60-100 kez/sn sabit aralıklarla çalışmalıdır. `FixedUpdate` schedule'ı bunun için idealdir:

```rust
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum StrataPhysicsSets {
    BroadPhase,
    NarrowPhase,
    SolveConstraints,
}

pub struct StrataPhysicsSchedulingPlugin;

impl Plugin for StrataPhysicsSchedulingPlugin {
    fn build(&self, app: &mut App) {
        // FixedUpdate: Fizik için sabit timestep (60-100 kez/sn)
        app.configure_sets(FixedUpdate, (
            StrataPhysicsSets::BroadPhase,
            StrataPhysicsSets::NarrowPhase,
            StrataPhysicsSets::SolveConstraints,
        ).chain());
    }
}
```

### B. Update (Oyun Mantığı - Variable Timestep)

`StrataSets` yalnızca **voxel çekirdek bootstrap** zinciridir. Streaming, Lighting, Network, Player vb. plugin setleri `03` içinde `StrataPlugin::build` altında cross-plugin `.before()` / `.after()` ile bağlanır — burada tekrarlanmaz.

```rust
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub enum StrataSets {
    Input,          // WASM drain, physics_apply_write_back
    WorldGen,
    ChunkLoad,
    Meshing,
    RenderPrepare,  // clear_dirty_flags .last()
}

pub struct StrataSchedulingPlugin;

impl Plugin for StrataSchedulingPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, (
            StrataSets::Input,
            StrataSets::WorldGen,
            StrataSets::ChunkLoad,
            StrataSets::Meshing,
            StrataSets::RenderPrepare,
        ).chain());

        // Tüm mevcut schedule'lara uygulanır (Bevy 0.18 App::configure_schedules)
        app.configure_schedules(ScheduleBuildSettings {
            ambiguity_detection: LogLevel::Warn,   // dev; prod: Ignore
            auto_insert_apply_deferred: true,
            hierarchy_detection: LogLevel::Warn,   // default zaten Warn
            ..default()
        });

        // Tek schedule için override (opsiyonel):
        // app.edit_schedule(Update, |s| {
        //     s.set_build_settings(ScheduleBuildSettings {
        //         ambiguity_detection: LogLevel::Error,
        //         ..default()
        //     });
        // });
    }
}
```

### C. Executor Seçimi

Bevy 0.18 varsayılanı native'de `ExecutorKind::MultiThreaded`. Strata, **yalnızca gerekli schedule'larda** executor değiştirir (tüm `Update`'i SingleThreaded yapmak paralelliği öldürür):

```rust
// Örnek: belirli bir custom schedule (ör. GpuUpload) tek thread
app.edit_schedule(GpuUpload, |schedule| {
    schedule.set_executor_kind(ExecutorKind::SingleThreaded);
});
```

---

## 5. Olay Yönetimi ve Etkileşim Kancaları (Event Hooks)

Eklentilerin veya modların birbirleriyle konuşması ve birbirlerinin verilerine müdahale etmesi (Hooking) işlemi **frekansına göre** ikiye ayrılır:

### KRİTİK KARAR ÇİZGİSİ

| Kriter | EventReader (Sıcak) | Observer (Soğuk) |
|--------|--------------------|--------------------|
| Frekans | Her frame 10+ kez | Saniyede 1-2 kez |
| Mekanizma | Poll-based (batch) | Push-based (anında) |
| Bellek | Event queue'de buffer | Allocation-free |
| Entity hedefi | Global event (varsayılan) | Global veya `commands.trigger(...).target(entity)` |
| Batch işleme | Evet (`EventReader::read`) | Hayır (tekil tetikleme) |
| WASM mod erişimi | **Hayır** — native motor | **Evet** — `trigger_event` → drain → `commands.trigger()` / `World::trigger()` |

WASM modları **sıcak** `EventReader` zincirine doğrudan bağlanmaz; her blok değişiminde guest callback çağırmak, yüksek frekanslı host köprüsü patlamasına yol açar (bkz. `32-modding.md` §3.7).

### A. Yüksek Frekanslı Olaylar (EventReader / Poll-Based)

Saniyede yüzlerce kez olabilen durumlar (Örn: Chunk mesh güncellemeleri, blok kırılmaları, ağ paketleri) DOD mantığına uygun olarak **yığın işleme (batch processing)** ile çözülür:

```rust
#[derive(Event)]
pub struct ChunkMeshReady {
    pub sector_entity: Entity, // sektör chunk entity (IVec3 değil!)
    pub mesh_handle: Handle<Mesh>,
}

fn process_ready_meshes(
    mut events: EventReader<ChunkMeshReady>,
    mut commands: Commands,
) {
    for ev in events.read() {
        commands.entity(ev.sector_entity).insert(ev.mesh_handle.clone());
    }
}
```

### B. Düşük Frekanslı / Anlık Olaylar (Observer / Push-Based)

"Oyuncu Öldü", "Özel bir kapı açıldı" veya ECS Bileşenlerinin eklendiği an gibi seyrek ama anında müdahale gerektiren durumlarda **Bevy 0.18 Observers** kullanılır.

```rust
#[derive(Event)]
pub struct PlayerDied {
    pub player_id: Entity,
    pub killed_by: Entity,
}

// Bevy 0.18: Observer ilk parametresi On<E> (Trigger değil)
fn on_player_death_hook(event: On<PlayerDied>, mut commands: Commands) {
    commands.spawn(ItemDropBundle {
        owner: event.player_id,
    });
}

impl Plugin for GameplayModPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_player_death_hook);
    }
}

// Tetikleme:
// commands.trigger(PlayerDied { player_id, killed_by }); // schedule sync noktasında
// world.trigger(...) // anında (test / tek thread senaryoları)
```

---

## 6. BlockRegistry Entegrasyonu (kaynak: `05-block-registry.md`)

**Tek gerçek kaynak:** Blok tipi `u16`, SoA `BlockRegistryInner`, `Arc` immutable runtime, TOML yükleme, `BlockRegistryBuilder` init fazı — tümü **`05-block-registry.md`** içindedir. Bu bölüm yalnızca **plugin yaşam döngüsü** bağlantısını tanımlar.

| Aşama | Ne olur | Plugin |
|-------|---------|--------|
| Startup / PreStartup | `BlockRegistryBuilder` + TOML + vanilla pack | `BlockRegistryPlugin` |
| Init sonu | `builder.build()` → `Res<BlockRegistry>` (`Arc`, salt-okunur) | aynı |
| Runtime | Okuma lock-free; **yeni blok tipi eklenmez** | — |
| WASM mod Init | `register_block` → builder'a yazar | `ModdingPlugin` (`32` §3) |

```rust
// Tam implementasyon: 05-block-registry.md §15
pub struct BlockRegistryPlugin {
    pub blocks_dir: String,
}

impl Plugin for BlockRegistryPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, self.load_blocks);
    }

    fn finish(&self, app: &mut App) {
        assert!(
            app.world().get_resource::<BlockRegistry>().is_some(),
            "BlockRegistry must exist after Startup!"
        );
    }
}
// load_blocks: BlockRegistryBuilder::new() → TOML → builder.build() → insert_resource
```

### WASM Modları İçin Block Kaydı

WASM modları `register_block` host fonksiyonu ile **yalnızca Init fazında** builder'a yazar. Host implementasyonu → `32-modding.md` §3.1; veri şeması → `05` §3.

---

## 7. Dış Modlama Köprüsü (Özet — detay `32-modding.md`)

Strata, güvensiz dış modlar (T1) için **wasmtime 45.0** ve dar host yüzeyi kullanır. Linker, `WasmHostState`, hot-reload, manifest, bütçe ve drain sistemlerinin tam implementasyonu **`32-modding.md` §3–5** dosyasındadır.

### 7.1 Anayasal Kurallar (değişmez)

| Kural | Özet |
|-------|------|
| Host yüzeyi | MVP: 8 fonksiyon (`register_block`, `get/set_block`, `get_blocks_region`, `push_block_commands`, `trigger_event`, `log`, `get_time`). Geniş API → `32` §2 WIT |
| Köprü | Okuma: `RwLock`; yazma/event: `SegQueue` + `drain_*` (`StrataSets::Input`) |
| Block kayıt | Init-only; runtime registry immutable (`05`) |
| Bütçe | Frame başına host-call / komut / event limiti; `on_tick` **opsiyonel** |
| Red | GPU, network, tam ECS WASM'a kapalı (§10.3) |

### 7.2 Motor Entegrasyonu

- `modding` crate → `ModdingPlugin`, `StrataCorePlugins` sonrası yüklenir.
- Çoklu oyuncu: hook'lar server-authoritative + deterministik (`32` §12).

---

## 8. Plugin Config Yönetimi

Her plugin, kendine ait yapılandırma dosyasına sahip olabilir. Varsayılan değerler compile-time'da belirlenir, runtime'da RON dosyasından override edilir:

```rust
use serde::{Serialize, Deserialize};
use bevy::prelude::*;

#[derive(Resource, Serialize, Deserialize, Reflect)]
#[reflect(Resource)]
pub struct StrataRenderConfig {
    pub render_distance: u32,
    pub enable_raytracing: bool,
    pub shadow_resolution: u32,
    pub bloom_enabled: bool,
    pub vsync: bool,
}

impl Default for StrataRenderConfig {
    fn default() -> Self {
        Self {
            render_distance: 12,
            enable_raytracing: false,
            shadow_resolution: 1024,
            bloom_enabled: true,
            vsync: true,
        }
    }
}

pub struct StrataRenderPlugin {
    /// Config dosyasının yolu. None ise varsayılan değerler kullanılır.
    pub config_path: Option<PathBuf>,
}

impl Plugin for StrataRenderPlugin {
    fn build(&self, app: &mut App) {
        // Config'i yükle (varsa dosyadan, yoksa default)
        let config = self.config_path.as_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|s| ron::from_str::<StrataRenderConfig>(&s).ok())
            .unwrap_or_default();
        app.insert_resource(config);
    }
}

// Config dosyası örneği (strata_render.config.ron):
// (
//     render_distance: 16,
//     enable_raytracing: true,
//     shadow_resolution: 2048,
//     bloom_enabled: true,
//     vsync: false,
// )
```

---

## 9. Native Plugin Evrimi (Bevy Ekosistemi)

Motor **içi** eklentiler için Bevy'nin gidişatı izlenir; WASM mod API'sinden bağımsızdır.

| Bevy yönü | Strata karşılığı (şimdi / sonra) |
|-----------|----------------------------------|
| `PluginGroup` + `finish()` assert | `StrataCorePlugins` — mevcut |
| `PluginSet` + manifest grafiği ([PR #11228](https://github.com/bevyengine/bevy/pull/11228)) | Büyüdükçe `TypeId` bağımlılık sırası |
| Declarative plugins ([HackMD](https://hackmd.io/CBXLcBgZSRSQCEdaqZC86A)) | Editor / dry-run — ileride |
| `build_async` ([PR #18187](https://github.com/bevyengine/bevy/pull/18187)) | Async asset yükleme plugin'leri — ileride |

WASM modları bu lifecycle'a **dahil değildir**; yalnızca `manifest.toml` + semver + `WasmModManager` ile yönetilir.

---

## 10. Modding Tasarım Dersleri ve Bilinçli Redler

Bu bölüm, alternatif modding yaklaşımlarından (WASM sandbox, gömülü script, geniş plugin SDK'ları) alınan ve Strata'ya uyarlanan kuralları özetler. Amaç: performans, güvenlik ve DOD uyumu.

### 10.1 Katman Modeli (L0–L4) ve Granülerlik × Güven Matrisi

Eski 3 katmanlı (HOT/WARM/COLD) model "granülerlik" ve "güven" eksenlerini karıştırdığı için yetersizdi. Strata bunları ayrıştıran **5 katmanlı (L0–L4)** modeli kullanır. Akış tek yönlüdür: alt katmanlar veriyi/stratejiyi besler, **L0 native her zaman çalıştırır**. Mod hiçbir zaman L0'a *kod* enjekte etmez.

```text
┌────────────────────────────────────────────────────────────────┐
│ L0  ÇEKİRDEK (native, mod YOK)                                   │
│     XBrickMap SIMD, GPU pipeline, solver/mesher inner loop       │
│     → Hiçbir mod giremez; yalnızca registry/komut kuyruğu okur   │
├────────────────────────────────────────────────────────────────┤
│ L1  DISPATCHER (native, registry okur)                           │
│     movement_dispatch, collision_dispatch, fluid_dispatch ...    │
│     → Strateji listesini iterate eder; batch'i native işler      │
├────────────────────────────────────────────────────────────────┤
│ L2  STRATEJİ REGISTRY (T2 native plugin doldurur)                │
│     Box<dyn FluidSolver> vb. — yalnızca COARSE dispatch          │
│     → "Alt sistemi komple değiştir" buradan (compile-time)       │
├────────────────────────────────────────────────────────────────┤
│ L3  POLICY HOOK BUS (T1 WASM, bütçeli, batch)                    │
│     pre/post-solve hook, on_block_change(batch), cold observer   │
│     → SegQueue + zero-copy linear memory                         │
├────────────────────────────────────────────────────────────────┤
│ L4  DATA PACK (T0, TOML/RON, kod YOK)                            │
│     PhysicsMaterial, blok trait flag, recipe, loot — hot-reload  │
└────────────────────────────────────────────────────────────────┘
```

**Granülerlik × Güven matrisi** — bir mod'un hangi hücreye girebileceği:

```text
              GÜVENSİZ (runtime .wasm)      GÜVENİLİR (compile-time native)
            ┌──────────────────────────┬──────────────────────────────────┐
 COARSE     │ T1: batch policy hook    │ T2: strateji registry            │
 (tick/     │ (pre/post-solve, observer│ (solver/mesher/lighting komple   │
  batch)    │  bütçeli, zero-copy)     │  değiştir)                       │
            ├──────────────────────────┼──────────────────────────────────┤
 HOT INNER  │ ❌ ASLA                  │ ❌ ASLA (registry içi native SIMD)│
 (per-voxel/│ (sınır geçişi öldürür)   │ (Box<dyn> iç döngüde yasak)       │
  contact)  │                          │                                  │
            └──────────────────────────┴──────────────────────────────────┘
 STATIC     │ T0: data pack (TOML/RON) — her iki güven seviyesinde de OK   │
 (load-time)│ fizik parametreleri, blok trait flag'leri, recipe, loot      │
            └──────────────────────────────────────────────────────────────┘
```

Sol-alt hücre (güvensiz × hot-inner) **kalıcı olarak boştur** — performansın tek dokunulmaz kuralı budur. Diğer her hücre açıktır.

**Güven kademeleri (Tier):**

| Kademe | Ne çalıştırır | Erişim | Yükleme |
|--------|---------------|--------|---------|
| **T0 — Data Pack** | TOML/RON (kod yok) | Fizik *parametreleri*, blok davranış flag'leri, recipe, loot | Hot-reload, güvensiz OK |
| **T1 — Sandboxed WASM** | Güvensiz `.wasm` | Cold observer + batch I/O + **fizik/world-gen batch policy hook'ları** (bütçeli, capability'ye bağlı) | Runtime hot-reload |
| **T2 — Native Plugin** | Motorla **birlikte derlenen** Rust crate | Tam ECS, herhangi bir `SystemSet`'e sistem ekleme, L2 registry üzerinden alt sistem *değiştirme* | Compile-time |

**Neden T2 derin değişim için compile-time + native?** Rust'ın **stabil ABI'si yoktur** ve `dlclose` güvenli değildir ([Bevy issue #4843](https://github.com/bevyengine/bevy/issues/4843)); managed olmayan dilde TLS/thread/logger pointer'ları dylib'i canlı tutar. Yani "motorun derinine runtime'da güvenli dylib enjekte etmek" pratik değildir. Strata'nın dürüst kararı: derin motor değişimi = motorla birlikte derlenen native plugin (modpack = yeniden derlenmiş client). Bu, planın "derleme-zamanı güvenliği" felsefesinin (Bölüm 1) doğal uzantısıdır. (`stabby`/`abi_stable` ile ayrı derleme ileride değerlendirilebilir; boilerplate ağır ve hot-reload kısıtlıdır — §10.4.)

### 10.2 Alınan Dersler

| Ders | Uygulama (`04`) |
|------|-----------------|
| Az host call, çok iş | `get_blocks_region`, `push_block_commands`, `SegQueue` drain |
| Frekansa göre kanal | Sıcak: native `EventReader`; soğuk: WASM → `trigger_event` → Observer |
| Init vs runtime | `ModPhase`, `register_block` init-only |
| Dar sandbox | 8 host fonksiyon + `ModCapabilities` |
| Mod başına izolasyon | Store başına `WasmHostState`, hot-reload tek mod |
| Frame bütçesi | `WasmModBudgetConfig` (`32-modding.md` §3.7) |
| Veri-önce | `05-block-registry` TOML; WASM davranış için |
| semver | `StrataApiVersion` + manifest `api_version` |

### 10.3 Bilinçli Redler (Strata'da hedeflenmez)

| Yaklaşım | Neden red |
|----------|-----------|
| Runtime mixin / bytecode hook | Rust'ta yok; ECS bütünlüğünü bozar |
| **Hot inner loop'a mod kodu** (per-voxel / per-contact / per-vertex) | Sınır geçişi + dynamic dispatch SIMD'i öldürür — matrisin kalıcı boş hücresi (§10.1) |
| GPU / network / tam-ECS yeteneğini **WASM**'a açmak | Cache, filter-first, güvenlik — bunlar yalnızca T2 native'de |
| Her frame WASM `on_tick` zorunlu | Host call patlaması; opsiyonel + bütçeli |
| Güvenilmeyen runtime native `.dll` / `dlopen` | Rust stabil ABI yok, `dlclose` güvensiz; T2 native = compile-time birlikte derleme |
| Out-of-process mod (gRPC/subprocess) | Tick latency; voxel streaming için uyumsuz |
| Sıcak yolda WASM event callback | §5 tablosu — native batch event |

**Önemli nüans (eski mutlak reddin revizyonu):** "Fizik / meshing / world-gen WASM'a *tamamen* kapalı" mutlak kuralı artık **granülerliğe ve güven kademesine göre açık** ilkesiyle değiştirilmiştir (§10.1). Korunan: hot-inner native kalır, GPU/network WASM'a kapalıdır. Açılan: bu alt sistemler T0 data + T1 batch policy hook + T2 native registry üçlüsüyle, hot loop native kalarak modlanabilir hale gelir.

### 10.4 Dispatcher + Strateji Registry Deseni (L1/L2 — Native Genişleme)

Çekirdek sistemleri *monolitik* yazmak yerine, bir **strateji registry'sini okuyan dispatcher** olarak yazmak, bir alt sistemin (solver, mesher, ışıklandırma, world-gen aşaması) komple değiştirilebilmesini sağlar. Kritik detay: **dispatch COARSE (tick/batch başına birkaç kez), gerçek iş native SoA batch içinde akar.** Böylece `AGENTS.md`'deki "hot loop'ta `Box<dyn>` yok" kuralı ihlal edilmez.

```rust
// --- L1: çekirdek dispatcher (motorda sabit, native) ---
fn fluid_dispatch(
    registry: Res<FluidSolverRegistry>,
    mut field: FluidFieldBatch,   // SoA: tüm sıvı voxel'leri tek slice
) {
    // COARSE: aktif strateji seçimi (genelde 1; mod override edebilir)
    registry.active().step(&mut field);  // iç döngü: native SIMD, branchless, mod kodu YOK
}

// --- L2: trait + registry (native plugin doldurur) ---
pub trait FluidSolver: Send + Sync {
    fn id(&self) -> &str;
    fn step(&self, field: &mut FluidFieldBatch);  // batch — per-voxel DEĞİL
}

#[derive(Resource)]
pub struct FluidSolverRegistry {
    strategies: Vec<Box<dyn FluidSolver>>,
    active: usize,
}
impl FluidSolverRegistry {
    pub fn override_with(&mut self, s: Box<dyn FluidSolver>) { /* yenisini aktif yap */ }
    pub fn active(&self) -> &dyn FluidSolver { self.strategies[self.active].as_ref() }
}

// --- T2 native plugin: çekirdek çözücüyü komple değiştirir ---
impl Plugin for MyNavierStokesPlugin {
    fn build(&self, app: &mut App) {
        app.world_mut()
           .resource_mut::<FluidSolverRegistry>()
           .override_with(Box::new(NavierStokesFluid::new()));
    }
}
```

Aynı kalıp motorun her katmanına uygulanır: `MovementBehaviorRegistry`, `CollisionResponseRegistry`, `FluidSolverRegistry`, `MeshingStrategyRegistry`, `LightingModelRegistry`, `WorldGenStageRegistry`. Her registry = o katmanın T2 native modlarca değiştirilebilir noktası. (Detay, WIT sözleşmeleri ve fizik policy hook bus implementasyonu: `32-modding.md`.)

### 10.5 İleride Değerlendirilebilir (şimdilik kapsam dışı)

- **WASM Component Model + WIT geçişi:** API yüzeyi büyüdükçe (fizik hook'ları, geniş world API) elle yazılan ham-pointer `func_wrap`'lar `unsafe` cehennemine döner. Tipli `record`/`list`/`resource` ve `wit-bindgen` ile sürdürülebilir SDK için Component Model hedeflenir (referans: Wasvy, Zed, Typst). WIT sözleşme tasarımı `32-modding.md`'de tutulur.
- T2 native modlarda ayrı derleme (`stabby`/`abi_stable` ile stabil C-ABI) — boilerplate ağır, hot-reload kısıtlı; varsayılan birlikte-derleme.
- Asset hot-reload (Bevy `file_watcher`) WASM hot-reload'dan ayrı — data pack iterasyonu için.
- Sunucu tarafında mod allowlist + manifest imzası (multiplayer, `16-network` ile hizalanır). **Fizik/world-gen hook'ları determinizm + server-authoritative gerektirir** (bkz. `32-modding.md`).

---

## 11. Özet: Revize Edilmiş Modül Hiyerarşisi

```text
APP (Ana Uygulama)
│
├── StrataCorePlugins (bootstrap — §3)
│   ├── StrataSubAppPlugin
│   │   ├── SubApp Render → StrataRenderPlugin (Main schedule + extract)
│   │   └── SubApp Physics → StrataPhysicsPlugin (FixedUpdate + write-back kanalı)
│   ├── StrataSchedulingPlugin (StrataSets / StrataPhysicsSets)
│   ├── BlockRegistryPlugin → 05 (SoA Arc registry)
│   ├── XBrickMapPlugin → 06
│   └── StrataMeshingPlugin
│
├── StrataPlugin → 03 (Player, Network, Lighting, Storage, …)
│
├── SCHEDULE (bootstrap Update)
│   Input → WorldGen → ChunkLoad → Meshing → RenderPrepare
│   (+ 03 cross-plugin: NetworkSystems, PlayerSystems, …)
│
├── SCHEDULE (Physics SubApp FixedUpdate)
│   BroadPhase → NarrowPhase → SolveConstraints → collect_write_back
│
├── EVENT
│   Sıcak: EventReader (ChunkMeshReady, BlockChanged, …)
│   Soğuk: Observer + On<E> (PlayerDied, …)
│
├── MODLAMA (§10; detay 32)
│   T0 data pack | T1 WASM | T2 native registry
│
└── ModdingPlugin (StrataCorePlugins sonrası)
```

---

## Ek: Kod Referansları ve Kaynaklar

| Konu | Referans |
|------|----------|
| Bevy Plugin / `finish` / `is_plugin_added` | [Plugin](https://docs.rs/bevy/0.18.1/bevy/app/trait.Plugin.html), [App](https://docs.rs/bevy/0.18.1/bevy/prelude/struct.App.html) |
| Bevy PluginSet | [PR #11228](https://github.com/bevyengine/bevy/pull/11228) |
| Bevy SubApp | [SubApp 0.18.1](https://docs.rs/bevy/0.18.1/bevy/prelude/struct.SubApp.html), `take_extract`, [issue #15841](https://github.com/bevyengine/bevy/issues/15841) |
| Bevy Schedule | [ScheduleBuildSettings](https://docs.rs/bevy_ecs/latest/bevy_ecs/schedule/struct.ScheduleBuildSettings.html), [Cheatbook: ambiguity](https://bevy-cheatbook.github.io/programming/schedules.html) |
| Bevy Event / Observer | [Event trait](https://docs.rs/bevy/0.18.1/bevy/prelude/trait.Event.html), [Observer](https://docs.rs/bevy/0.18.1/bevy/prelude/struct.Observer.html), `On<E>` |
| Bevy FixedUpdate | `FixedUpdate` schedule, `Time<Fixed>` |
| Rapier schedule | `RapierPhysicsPlugin::in_fixed_schedule()` → `FixedUpdate` |
| ECS çapraz plugin | `03-ecs-architecture.md` §3, §5 |
| Block registry (anayasa) | `05-block-registry.md` |
| XBrickMap | `06-xbrickmap.md` |
| wasmtime 45.0 | [Linker](https://docs.rs/wasmtime/45.0/wasmtime/struct.Linker.html), `32-modding.md` |
| crossbeam SegQueue | Physics write-back + mod drain (`§2`, `32` §3) |
| semver / toml | Mod manifest |
| RON Config | `StrataRenderConfig` (§8) |

---

## 12. Araştırma Doğrulamaları ve Öneriler (2026-06)

> **Kaynak:** 5 worker ile 40+ WebSearch sorgusu, WASM ekosistem, Bevy modding pattern'leri, cyubeVR karşılaştırma.

### 12.1 Doğrulanan Kararlar

| Karar | Doğrulama |
|-------|-----------|
| SubApp izolasyonu | Bevy 0.18 best practice, render/physics world separation |
| L0-L4 katman modeli | Granülerlik × güvenlik matrisi — hot inner loop dokunulmazlığı |
| T0/T1/T2 modding tiers | cyubeVR 3-tier mod sistemi ile validated |
| 8 host fonksiyon MVP | Minimal API surface, güvenlik + performans dengesi |
| Observer + EventReader ayrımı | Sıcak/soğuk frekans bazlı kanal seçimi |

### 12.2 P3 — WIT Component Model Geçişi (Phase 5+)

**Problem:** Mevcut raw pointer ABI (`func_wrap`) uzun vadede unsustainable. API yüzeyi büyüdükçe (fizik hook'ları, geniş world API) `unsafe` cehennemine döner.

**Çözüm:** Phase 5'te WIT interface spec hazır olmalı, Phase 6'da geçiş.

**Avantajlar:**
- Tipi `record`/`list`/`resource` ile sürdürülebilir SDK
- `wit-bindgen` ile otomatik Rust/WASM binding generation
- Wasvy, Zed, Typst projeleri tarafından validated

**Aksiyon:** `32-modding.md`'de WIT world tanımı **şimdi tasarlanmalı** (MVP implementasyonu etkilemez).

### 12.3 P3 — WASM Production Hardening (Phase 5+)

Mevcut MVP 8 host fonksiyon ile devam eder, ama production'da aşağıdaki güvenlik katmanları **zorunlu**:

| Katman | Açıklama | Phase |
|--------|----------|-------|
| **Fuel metering** | WASM instruction başına fuel tüketimi, limit aşımında trap | 5 |
| **Epoch interrupt** | Zaman aşımı koruması (infinite loop prevention) | 5 |
| **Memory cap** | Mod başına bellek limiti (default 16MB, configurable) | 5 |
| **Import-level access control** | Her host fonksiyonu için ayrı capability flag | 5 |

```rust
// wasmtime fuel metering örneği
let mut config = wasmtime::Config::new();
config.consume_fuel(true);
config.epoch_interruption(true);

let engine = wasmtime::Engine::new(&config)?;
let mut store = wasmtime::Store::new(&engine, host_state);
store.set_fuel(1_000_000)?; // 1M instruction budget per tick
store.set_epoch_deadline(1); // 1 epoch = ~16ms (60 FPS)
```

**Not:** Bu katmanlar `32-modding.md`'de detaylandırılacak, bu dosyada sadece özet.

### 12.4 P1 — Plugin Loading Phase Config

Multiplayer sunucu mod sırası RON config'ten configurable olmalı:

```ron
// mod_loading_order.ron
(
    load_order: [
        "strata:core",
        "strata:physics",
        "mymod:custom_blocks",
    ],
    hot_reload_enabled: true,
    max_wasm_memory_bytes: 16777216, // 16MB
)
```
