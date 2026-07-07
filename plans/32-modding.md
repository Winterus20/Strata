# 32 — Modding & Derin Modlama (Tiered)

> **İlişki:** Bu dosya modlamanın **detay/uygulama** planıdır. Mimari *ilke ve kararlar* (granülerlik × güven matrisi, L0–L4 katman modeli, dispatcher/registry deseni, bilinçli redler) **`04-plugin-api.md` anayasasındadır** ve çelişkide `04` önceliklidir. Bu dosya `04`'ün T1 (WASM) ve T2 (native) detaylarını, WIT sözleşmelerini ve fizik policy hook bus'ını somutlaştırır.

## 1. Genel Bakış ve Kademe Modeli

Strata, **wasmtime 45.0** ile Wasm modding desteği sunar (`04` ile aynı sürüm). Modlar **WIT (Wasm Interface Types) + Component Model** ile oyun API'sine erişir. Amaç: **motorun her katmanı (fizik, meshing, ışıklandırma, world-gen dahil) modlanabilir** olsun — ama hot inner loop her zaman native kalsın.

### Temel İlke: Granülerlik × Güven (özet — tam tanım `04` §12.1)

Bir mod'un nereye dokunabileceği *ne sıklıkta çalıştığına* (granülerlik) ve *ne kadar güvenildiğine* (kademe) göre belirlenir. **Hot inner loop'a (per-voxel / per-contact) mod kodu ASLA girmez.** Diğer her katman, doğru granülerlik + kademede açıktır.

### Üç Kademe (Tier)

| Kademe | Ne | Erişim | Yükleme | Güven |
|--------|----|--------|---------|-------|
| **T0 — Data Pack** | TOML/RON (kod yok) | Fizik *parametreleri*, blok trait flag'leri, recipe, loot | Hot-reload | Güvensiz OK |
| **T1 — Sandboxed WASM** | `.wasm` (WIT/Component) | Cold observer + batch I/O + **batch policy hook** (fizik/world-gen, capability'ye bağlı) | Runtime hot-reload | İzole sandbox |
| **T2 — Native Plugin** | Motorla **birlikte derlenen** Rust crate | Tam ECS + L2 strateji registry (alt sistem komple değiştirme) | Compile-time | Tam güven |

### Temel Prensipler

- **Sandboxed (T1):** Wasm modlar izole çalışır, host belleğine doğrudan erişim yok; veri zero-copy linear memory + `SegQueue` ile batch akar.
- **WIT-based + Component Model:** Tipli (`record`/`list`/`resource`) API; `wit-bindgen` ile binding. Ham `i32 ptr,len` + `unsafe` minimuma iner.
- **Lifecycle-managed:** Init vs Runtime fazları (§3.6); `register_block` init-only.
- **Bütçeli:** Her T1 mod frame başına host-call / komut / event bütçesine tabidir (§3.7). `on_tick` **zorunlu değil**, opsiyonel + bütçeli.
- **Native = compile-time:** Performans-kritik / derin modlar **T2 native** olarak motorla birlikte derlenir. Runtime `.dll`/`dlopen` varsayılan **değildir** (Rust stabil ABI yok, `dlclose` güvensiz — bkz. §6).
- **Server-authoritative + deterministik:** Fizik/world-gen hook'ları determinizm gerektirir; otorite sunucudadır (bkz. §12).

---

## 2. WIT Interface Tanımları

```wit
# wit/strata.wit
package strata:modding@0.1.0;

/// Block registry erişimi.
/// register-block YALNIZCA on-init fazında çağrılabilir (init sonrası registry immutable).
/// Bkz. 05-block-registry.md §3, §15
interface block-registry {
    /// Yeni blok tipi kaydet (init-only).
    register-block: func(name: string, definition: block-definition) -> result<u16, error>;

    /// Blok tanımını getir.
    get-block: func(id: u16) -> option<block-definition>;

    /// İsme göre blok ID'si bul.
    find-block: func(name: string) -> option<u16>;
}

/// World erişimi (read-only).
interface world-read {
    /// Blok ID'sini getir.
    get-block: func(x: s32, y: s32, z: s32) -> option<u16>;

    /// Biome'i getir.
    get-biome: func(x: s32, z: s32) -> biome-info;

    /// Yükseklik haritasını getir.
    get-height: func(x: s32, z: s32) -> s32;
}

/// World düzenleme (write, izinli modlar için).
interface world-write {
    /// Blok yerleştir.
    set-block: func(x: s32, y: s32, z: s32, block-id: u16) -> result<(), error>;

    /// Blok kır.
    break-block: func(x: s32, y: s32, z: s32) -> result<(), error>;

    /// Bölge düzenle (batch — tek host call, tercih edilen yol).
    set-region: func(min: vec3, max: vec3, blocks: list<u16>) -> result<(), error>;
}

/// FİZİK BATCH POLICY HOOK (T1 — capability: physics-hooks).
/// Mod, solver'ın İÇİNE girmez. FixedUpdate sınırında, tick başına TEK call ile
/// tüm contact batch'ini alır ve impulse override batch'i döner (L3 hook bus, `04` §10.1).
/// Determinizm: impulse'lar fixed-point (q16.16) — f32 non-determinizmi lockstep'i bozar (§9).
interface physics-hooks {
    record contact {
        entity-a: u32,
        entity-b: u32,
        normal: vec3,        // birim normal (q16.16 packed)
        depth: s32,          // penetrasyon (q16.16)
        block-id: u16,       // çarpılan blok (0 = entity-entity)
    }
    record impulse-override {
        entity: u32,
        impulse: vec3,       // q16.16
    }
    /// SolveConstraints ÖNCESİ — tüm batch tek call. Native solver override'ları uygular.
    on-pre-solve: func(contacts: list<contact>) -> list<impulse-override>;
    /// SolveConstraints SONRASI — sonuç batch'i (opsiyonel; salt okuma).
    on-post-solve: func(results: list<contact>);
}

/// WORLD-GEN BATCH HOOK (T1 — capability: worldgen-hooks).
/// Chunk üretildikten SONRA, mod tek call ile bir bölgeyi post-process eder (mağara, yapı).
/// Deterministik olmalı: yalnızca (seed, chunk-pos) girdisine bağlı.
interface worldgen-hooks {
    /// Üretilen chunk için batch düzenleme komutları döner.
    on-chunk-generated: func(chunk-pos: vec3, seed: u64) -> list<u16>;
}

/// Entity yönetimi.
interface entities {
    /// Yeni entity spawn et.
    spawn-entity: func(entity-type: string, position: vec3) -> result<entity-id, error>;

    /// Entity'yi despawn et.
    despawn-entity: func(id: entity-id) -> result<(), error>;

    /// Entity pozisyonunu getir.
    get-position: func(id: entity-id) -> option<vec3>;

    /// Entity pozisyonunu ayarla.
    set-position: func(id: entity-id, position: vec3) -> result<(), error>;
}

/// Network erişimi.
interface network {
    /// Mesaj gönder (tüm client'lara).
    broadcast: func(message: string) -> result<(), error>;

    /// Mesaj gönder (belirli client'a).
    send-to: func(client-id: client-id, message: string) -> result<(), error>;

    /// Custom network event kaydet.
    register-event: func(name: string) -> result<event-id, error>;
}

/// UI erişimi.
interface ui {
    /// Yeni UI panel oluştur.
    create-panel: func(title: string, size: vec2) -> result<panel-id, error>;

    /// Panel'e buton ekle.
    add-button: func(panel: panel-id, label: string) -> result<button-id, error>;

    /// Panel'i göster.
    show-panel: func(panel: panel-id) -> result<(), error>;

    /// Panel'i gizle.
    hide-panel: func(panel: panel-id) -> result<(), error>;
}

/// Event sistemi.
interface events {
    /// Event dinleyicisi kaydet.
    on-event: func(event: string, handler: func(event-data: string>);

    /// Event yayınla.
    emit-event: func(event: string, data: string) -> result<(), error>;
}

/// Zamanlayıcı.
interface timers {
    /// Tek seferlik zamanlayıcı.
    once: func(delay-ms: u64, callback: func()>);

    /// Periyodik zamanlayıcı.
    interval: func(interval-ms: u64, callback: func()>);
}

/// Logging.
interface logging {
    log-info: func(message: string>);
    log-warn: func(message: string>);
    log-error: func(message: string>);
}

/// Ana mod world'ü (Component Model `world`).
/// NOT: import'lar capability'ye bağlıdır — manifest'te verilmeyen interface
/// linker'da bağlanmaz (Trap). Bkz. §7 capability/kademe tablosu.
world strata-mod {
    // Daima izinli
    import block-registry;   // register-block init-only
    import world-read;
    import events;
    import timers;
    import logging;

    // Capability gerektirir
    import world-write;      // cap: world-write
    import entities;         // cap: entity-spawn
    import network;          // cap: network (yalnızca sunucu allowlist)
    import ui;               // cap: ui-create (yalnızca client)

    /// Mod başlatıldığında çağrılır (Init fazı — tek yazma penceresi).
    export on-init: func();
    /// Mod kapatıldığında çağrılır.
    export on-shutdown: func();

    /// OPSİYONEL exports — motor yalnızca mod export ediyorsa ve bütçe varsa çağırır.
    /// Zorunlu DEĞİL; ağır mantık native + data-driven ile çözülmelidir (32 §3.7).
    export on-tick: func();                 // opsiyonel, bütçeli (sabit 20 TPS zorunluluğu YOK)
    export physics-hooks;                   // cap: physics-hooks (batch pre/post-solve)
    export worldgen-hooks;                  // cap: worldgen-hooks (batch chunk post-process)
}
```

## 3. WASM Bridge (wasmtime 45.0 ile Host-WASM İletişimi)

Strata, WASM modları için `wasmtime 45.0` runtime'ını kullanır. `wasmtime::Linker<T>` ile tip güvenli host fonksiyonları export edilir. Anayasal özet → `04-plugin-api.md` §7.

### 3.0 WASM Block Kaydı (Host)

WASM modülleri `strata::register_block` ile kayıt yapar. **Init-only:** `ModPhase::Init` dışında `Trap`. Capability: `can_register_blocks`. Zero-copy isim okuma `caller.get_memory()`. Host handler: `wasm_register_block` (§3.1 `build_strata_linker`); `05-block-registry.md` ile uyumlu.

### 3.1 Host Fonksiyonları (Minimal Attack Surface)

Güvenlik için WASM modlarına **dar bir host yüzeyi** export edilir: **6 temel** fonksiyon + **2 batch** fonksiyon (toplam 8; her biri bütçede tek host call sayılır). Tüm ECS'yi WASM'a açmak GÜVENLİK RİSKİDİR.

| # | Host fonksiyon | Faz | Bütçe |
|---|----------------|-----|-------|
| 1 | `register_block` | Init-only | Init |
| 2 | `get_block` | Runtime (okuma) | 1 call |
| 3 | `set_block` | Runtime (yazma, tek voxel) | 1 call |
| 4 | `get_blocks_region` | Runtime (okuma, batch) | 1 call |
| 5 | `push_block_commands` | Runtime (yazma, batch) | 1 call |
| 6 | `trigger_event` | Runtime (soğuk event) | 1 call |
| 7 | `log` | Her zaman | Init + runtime (throttle) |
| 8 | `get_time` | Runtime | 1 call |

**Thread-Safety Mimarisi (Lock-Free Bridge):**

wasmtime 45.0'da her `Store<T>` kendi `T`'sine sahiptir. Birden çok Store aynı `Linker`'ı paylaşabilir, ancak her Store'un verisi diğerlerinden bağımsızdır. Aşağıdaki tasarımda:

1. **`WasmHostState` her Store'a özeldir** — Store'lar arasında paylaşılmaz, `Mutex` gerekmez.
2. **`get_block` (okuma):** `Arc<parking_lot::RwLock<XBrickMap>>` üzerinden **read lock** alır. `parking_lot::RwLock` okuma kilidi çok hafiftir (CAS atomic), birden çok WASM thread'i aynı anda okuyabilir.
3. **`set_block` (yazma):** Lock-free bir `crossbeam::SegQueue<BlockCommand>` kanalına `BlockCommand` gönderir. Main thread, `Update` schedule'ında bu kuyruğu tüketirken XBrickMap'e **write lock** alır. Böylece WASM thread'leri hiçbir zaman write lock için beklemek zorunda kalmaz.
4. **`trigger_event` (event tetikleme):** `crossbeam::SegQueue<WasmEvent>` kanalına event data gönderir. Main thread'deki `drain_wasm_events` sistemi bu event'leri `World::trigger()` ile observer'a iletir.
5. **`block_registry` (okuma/yazma):** `Arc<parking_lot::Mutex<BlockRegistry>>` ile lazy-init edilir. Block kaydı yalnızca mod yükleme zamanında (hot path dışında) yapıldığı için buradaki lock kabul edilebilir.

```rust
use wasmtime::*;
use crossbeam::queue::SegQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// ========================
// LOCK-FREE COMMAND CHANNELS
// ========================

// --- Block yazma komutları ---
#[derive(Clone)]
pub struct BlockCommandQueue(pub Arc<SegQueue<BlockCommand>>);

#[derive(Copy, Clone)]
pub struct BlockCommand {
    pub x: i32, pub y: i32, pub z: i32,
    pub block_id: BlockId,
}

// --- Event tetikleme komutları ---
#[derive(Clone)]
pub struct WasmEventQueue(pub Arc<SegQueue<WasmEvent>>);

/// WASM modülünden tetiklenen tip-güvenli event.
/// event_type: u32 → string lookup tablosuyla çözülür
/// payload: raw bytes (ör: blok koordinatları, oyuncu ID'leri)
pub struct WasmEvent {
    pub event_type: u32,
    pub payload: Vec<u8>,
}

// ========================
// PER-STORE STATE (Mutex'siz + RwLock)
// ========================
pub struct WasmHostState {
    /// Her Store kendi instance verisini tutar (Store'a özel, lock gerekmez).
    pub instance_local_data: WasmInstanceData,

    /// Shared: XBrickMap — tüm Store'lar aynı haritayı okur.
    /// get_block → read lock (concurrent readers), set_block → SegQueue
    pub xbrickmap: Arc<parking_lot::RwLock<XBrickMap>>,

    /// Lazy-init: BlockRegistry tüm Store'lar tarafından paylaşılır.
    /// Lock sadece mod yükleme zamanında alınır (hot path dışı).
    pub block_registry: Arc<parking_lot::Mutex<BlockRegistry>>,

    /// Lock-free command channel — set_block istekleri buraya düşer.
    /// Main thread bu kuyruğu Update schedule'ında tüketir.
    pub block_command_queue: BlockCommandQueue,

    /// Lock-free event channel — trigger_event istekleri buraya düşer.
    pub event_queue: WasmEventQueue,

    /// Oyun zamanı (atomic, lock-free paylaşım)
    pub game_time: Arc<AtomicU64>,
}

// Per-instance metadata (Store'a özel, asla paylaşılmaz)
pub struct WasmInstanceData {
    pub instance_id: u32,
    pub mod_name: String,
    pub registered_events: Vec<String>,
    /// manifest.toml'den yüklenen yetenekler (bkz. §3.5)
    pub capabilities: ModCapabilities,
    /// Init tamamlandıysa register_block reddedilir
    pub phase: ModPhase,
    /// Frame başına host/kuyruk sayaçları (§3.7)
    pub budget: WasmInstanceBudget,
}

#[derive(Clone, Copy, Default)]
pub struct ModCapabilities {
    pub can_read_world: bool,
    pub can_write_world: bool,
    pub can_register_blocks: bool,
    pub can_trigger_events: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ModPhase {
    Init,
    Runtime,
}

pub fn build_strata_linker(engine: &Engine) -> Result<Linker<WasmHostState>> {
    let mut linker = Linker::new(engine);
    linker.allow_shadowing(true);

    // 1. Block kaydı (cold path — lock kabul edilebilir)
    linker.func_wrap("strata", "register_block", wasm_register_block)?;

    // 2. Voxel okuma (hot path — parking_lot::RwLock read lock)
    //    Read lock: birden çok WASM thread'i aynı anda okuyabilir.
    //    Write lock: sadece main thread'deki drain_block_commands alır.
    linker.func_wrap("strata", "get_block", |caller: Caller<'_, WasmHostState>, x: i32, y: i32, z: i32| -> Result<u32, Trap> {
        if !caller.data().instance_local_data.capabilities.can_read_world {
            return Err(Trap::new("get_block: missing capability"));
        }
        let map = caller.data().xbrickmap.read();
        Ok(map.get_block(IVec3::new(x, y, z)).map(|b| b.0).unwrap_or(0))
    })?;

    // 3. Voxel yazma (hot path — lock-free SegQueue; tek voxel)
    linker.func_wrap("strata", "set_block", |caller: Caller<'_, WasmHostState>, x: i32, y: i32, z: i32, block_id: u32| -> Result<(), Trap> {
        let caps = caller.data().instance_local_data.capabilities;
        if !caps.can_write_world {
            return Err(Trap::new("set_block: missing can_write_world capability"));
        }
        caller.data().block_command_queue.0.push(BlockCommand {
            x, y, z,
            block_id: BlockId(block_id),
        });
        Ok(())
    })?;

    // 4. Bölgesel okuma (batch — tek host call; zero-copy WASM buffer'a yazım mod tarafında)
    linker.func_wrap("strata", "get_blocks_region", wasm_get_blocks_region)?;

    // 5. Toplu yazma (batch — tek host call; WASM belleğinden BlockCommand dizisi)
    linker.func_wrap("strata", "push_block_commands", wasm_push_block_commands)?;

    // 6. Observer tetikleme (hot path — lock-free SegQueue)
    //    WASM modülü event_type (önceden kayıtlı) ve payload gönderir.
    //    Main thread'deki drain_wasm_events sistemi bunu World::trigger()'a çevirir.
    linker.func_wrap("strata", "trigger_event", |caller: Caller<'_, WasmHostState>, event_type: i32, data_ptr: i32, data_len: i32| -> Result<(), Trap> {
        if !caller.data().instance_local_data.capabilities.can_trigger_events {
            return Err(Trap::new("trigger_event: missing capability"));
        }
        let memory = caller.get_memory("memory")
            .ok_or_else(|| Trap::new("no memory exported"))?;
        let payload = if data_len > 0 {
            unsafe {
                let base = memory.data_ptr().add(data_ptr as usize);
                std::slice::from_raw_parts(base, data_len as usize).to_vec()
            }
        } else {
            Vec::new()
        };
        caller.data().event_queue.0.push(WasmEvent {
            event_type: event_type as u32,
            payload,
        });
        Ok(())
    })?;

    // 7. Debug log
    linker.func_wrap("strata", "log", |caller: Caller<'_, WasmHostState>, ptr: i32, len: i32| {
        let memory = caller.get_memory("memory").unwrap();
        let bytes = unsafe { std::slice::from_raw_parts(memory.data_ptr().add(ptr as usize), len as usize) };
        if let Ok(msg) = std::str::from_utf8(bytes) {
            bevy::log::info!("[WASM: {}] {}", caller.data().instance_local_data.mod_name, msg);
        }
    })?;

    // 8. Oyun zamanı (lock-free atomic read)
    linker.func_wrap("strata", "get_time", |caller: Caller<'_, WasmHostState>| -> f64 {
        let ticks = caller.data().game_time.load(Ordering::Relaxed);
        ticks as f64 / 1000.0
    })?;

    Ok(linker)
}

// ========================
// MAIN THREAD TÜKETİCİ SİSTEMLERİ
// ========================

// --- Block command tüketici ---
// Her frame'de BlockCommandQueue'daki birikmiş komutları işler.
// NOT: Write lock sadece BURADA alınır — WASM thread'leri asla write lock beklemez.
fn drain_block_commands(
    queue: Res<BlockCommandQueue>,
    mut xbrickmap: ResMut<XBrickMap>,
) {
    while let Some(cmd) = queue.0.pop() {
        xbrickmap.set_block(IVec3::new(cmd.x, cmd.y, cmd.z), cmd.block_id);
    }
}
// Kayıt: app.add_systems(Update, drain_block_commands.in_set(StrataSets::Input));

// --- Event tüketici ---
// WASM kuyruğundaki event'leri alır ve Bevy World::trigger() ile observer'lara iletir.
fn drain_wasm_events(
    queue: Res<WasmEventQueue>,
    mut commands: Commands,
    registry: Res<WasmEventRegistry>,
) {
    while let Some(wasm_event) = queue.0.pop() {
        // Event type string lookup → hangi Rust event tipine karşılık geldiğini bul
        if let Some(event_type) = registry.lookup(wasm_event.event_type) {
            match event_type {
                "player_died" => {
                    // payload: [player_id: u32, killer_id: u32]
                    if wasm_event.payload.len() >= 8 {
                        let player_id = u32::from_le_bytes(
                            wasm_event.payload[0..4].try_into().unwrap());
                        let killer_id = u32::from_le_bytes(
                            wasm_event.payload[4..8].try_into().unwrap());
                        commands.trigger(PlayerDied {
                            player_id: Entity::from_raw(player_id),
                            killed_by: Entity::from_raw(killer_id),
                        });
                    }
                }
                "block_broken" => {
                    // payload: [x: i32, y: i32, z: i32]
                    if wasm_event.payload.len() >= 12 {
                        let x = i32::from_le_bytes(
                            wasm_event.payload[0..4].try_into().unwrap());
                        let y = i32::from_le_bytes(
                            wasm_event.payload[4..8].try_into().unwrap());
                        let z = i32::from_le_bytes(
                            wasm_event.payload[8..12].try_into().unwrap());
                        commands.trigger(BlockBrokenEvent {
                            position: IVec3::new(x, y, z),
                        });
                    }
                }
                _ => bevy::log::warn!("Unknown WASM event type: {}", event_type),
            }
        }
    }
}
// Kayıt: app.add_systems(Update, drain_wasm_events.in_set(StrataSets::Input));
```

### 3.2 Zero-Copy Veri İletişimi

WASM modülü ile host arasında büyük veri transferleri (block listesi, mesh data) zero-copy pattern ile yapılır:

```rust
// WASM modülü kendi belleğinde bir buffer oluşturur:
//   (memory (data (; block list ;) "..."))
//
// Host, caller.get_memory() ile doğrudan WASM belleğini okur:
pub fn read_wasm_buffer<T: bytemuck::Pod>(
    caller: &Caller<'_, WasmHostState>,
    ptr: i32,
    len: i32,
) -> Result<&'static [T], Trap> {
    let memory = caller.get_memory("memory")
        .ok_or_else(|| Trap::new("no memory"))?;
    let slice = unsafe {
        let base = memory.data_ptr().add(ptr as usize);
        std::slice::from_raw_parts(base as *const T, len as usize)
    };
    Ok(slice)
}

// Kullanım: 1000 blokluk toplu kayıt
let blocks: &[WasmBlockDef] = read_wasm_buffer(&caller, ptr, count)?;
for block in blocks {
    registry.register_raw(block.id, block.name_id);
}
```

### 3.3 WASM Performans Kuralları

1. **Host call bütçesi:** Varsayılan `WasmModBudget::max_host_calls_per_frame = 64` (bkz. §3.7). Tek voxel yerine `push_block_commands` / `get_blocks_region` tercih edin.
2. **GPU / mesh / tam ECS WASM'da yok:** Sadece oyun mantığı ve seyrek event.
3. **Zero-copy:** `read_wasm_buffer` / `caller.get_memory()` ile batch transfer; host tarafında `Vec` kopyası yalnızca zorunlu payload'larda.
4. **Linker + Store:** `func_wrap` tanımları paylaşılır; `WasmHostState` Store başına — hot path'te `Mutex` yok (§3.1).
5. **JIT overhead:** wasmtime JIT native'e yakın olsa da köprü maliyeti çağrı başınadır; amortize etmek batch ve kuyruk tüketimidir.

### 3.4 Batch Host API (Zero-Copy)

Tekil `get_block` / `set_block` hâlâ desteklenir; yüksek frekanslı modlar batch kullanmalıdır.

```rust
/// WASM belleğinde: [count: u32][(x,y,z,block_id): 16 byte] * count
pub fn wasm_push_block_commands(
    mut caller: Caller<'_, WasmHostState>,
    ptr: i32,
    count: i32,
) -> Result<(), Trap> {
    if !caller.data().instance_local_data.capabilities.can_write_world {
        return Err(Trap::new("push_block_commands: missing capability"));
    }
    let cmds = read_wasm_buffer::<BlockCommandPod>(&caller, ptr, count as usize)?;
    let queue = &caller.data().block_command_queue.0;
    for cmd in cmds {
        queue.push(BlockCommand {
            x: cmd.x, y: cmd.y, z: cmd.z,
            block_id: BlockId(cmd.block_id),
        });
    }
    Ok(())
}

/// Host, okunan blokları modun export ettiği `memory` içindeki `out_ptr` bölgesine yazar.
/// Layout: düz u32[block_id] — boyut = (max-min+1) hacmi; mod tarafında önceden allocate edilir.
pub fn wasm_get_blocks_region(
    caller: Caller<'_, WasmHostState>,
    min_x: i32, min_y: i32, min_z: i32,
    max_x: i32, max_y: i32, max_z: i32,
    out_ptr: i32,
) -> Result<(), Trap> {
    if !caller.data().instance_local_data.capabilities.can_read_world {
        return Err(Trap::new("get_blocks_region: missing capability"));
    }
    // ... bounds check, map.read(), zero-copy write into WASM memory ...
    Ok(())
}
```

`drain_block_commands` her frame'de kuyruğu **tek write lock** altında boşaltır; binlerce tekil `set_block` host call yerine tek batch push + tek drain tercih edilir.

### 3.5 Mod Manifest ve Capabilities

Her WASM modu paket kökünde `manifest.toml` taşır. Host yüklemeden önce doğrular; yetkisiz import'lar linker'da `Trap` üretir.

```toml
# mods/example_mod/manifest.toml
id = "example_mod"
name = "Example Mod"
api_version = "1.0.0"
engine_version = ">=0.1.0,<0.2.0"

[capabilities]
can_read_world = true
can_write_world = false
can_register_blocks = true
can_trigger_events = true

[dependencies]
# opsiyonel: diğer mod id'leri
```

```rust
#[derive(Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub api_version: Version,
    pub capabilities: ModCapabilities,
}
```

**Bilinçli redler (manifest ile de verilmez):** `can_access_ecs`, `can_gpu`, `can_network` — bunlar motor içi native plugin veya sunucu yetkisindedir; güvenilmeyen WASM'a açılmaz.

### 3.6 Mod Yaşam Döngüsü: Init vs Runtime

| Faz | Ne olur | İzinli host API |
|-----|---------|-----------------|
| **Init** | `_start`, `register_block`, asset referansları | `register_block`, `log` |
| **Runtime** | Mod tick yok; yalnızca motor `on_tick` export'unu **isteğe bağlı** çağırır (bütçe dahilinde) | `get_*`, `push_*`, `set_block`, `trigger_event`, `get_time` |
| **Hot-reload** | Store drop → yeniden Init | Önceki runtime state geri yüklenmez (deterministik sunucu için) |

Init sonunda host `instance_local_data.phase = ModPhase::Runtime` yapar ve registry'yi mühürler.

```rust
fn seal_mod_after_init(store: &mut Store<WasmHostState>) {
    let data = store.data_mut();
    data.instance_local_data.phase = ModPhase::Runtime;
}
```

### 3.7 Mod Tick Bütçesi (Host Call & Kuyruk Limitleri)

WASM modlarının her frame'de sınırsız host çağrısı yapması engellenir. Sayaçlar **Store başına** tutulur; frame sonunda sıfırlanır.

```rust
#[derive(Resource)]
pub struct WasmModBudgetConfig {
    /// Frame başına maksimum host fonksiyon girişi (tüm modlar toplamı veya mod başına — prod'da mod başına önerilir)
    pub max_host_calls_per_mod_per_frame: u32,
    /// Frame başına maksimum BlockCommand kuyruk girişi (push_block_commands içindeki her komut sayılır)
    pub max_block_commands_per_mod_per_frame: u32,
    /// Frame başına maksimum WasmEvent
    pub max_events_per_mod_per_frame: u32,
}

impl Default for WasmModBudgetConfig {
    fn default() -> Self {
        Self {
            max_host_calls_per_mod_per_frame: 64,
            max_block_commands_per_mod_per_frame: 4096,
            max_events_per_mod_per_frame: 8,
        }
    }
}

pub struct WasmInstanceBudget {
    pub host_calls_this_frame: u32,
    pub block_cmds_this_frame: u32,
    pub events_this_frame: u32,
}

/// Linker wrapper: her host call öncesi budget check; aşımda Trap veya sessiz drop + warn (prod: Trap)
fn with_budget<F>(caller: &mut Caller<'_, WasmHostState>, cost: u32, f: F) -> Result<(), Trap>
where
    F: FnOnce(&mut Caller<'_, WasmHostState>) -> Result<(), Trap>,
{
    let budget = &mut caller.data_mut().instance_local_data.budget;
    let cfg = caller.data().budget_config; // Arc clone veya Store'da kopya
    if budget.host_calls_this_frame + cost > cfg.max_host_calls_per_mod_per_frame {
        return Err(Trap::new("mod host call budget exceeded"));
    }
    budget.host_calls_this_frame += cost;
    f(caller)
}
```

```rust
fn reset_wasm_budgets(mut manager: ResMut<WasmModManager>) {
    for store in manager.stores.values_mut() {
        store.data_mut().instance_local_data.budget = WasmInstanceBudget::default();
    }
}
// Kayıt: app.add_systems(Last, reset_wasm_budgets);
```

İsteğe bağlı `on_tick` export'u: motor her frame çağırmak zorunda değildir; çağrılırsa bir host call bütçeye dahildir. Ağır mod mantığı native sistem + data-driven içerik ile çözülmelidir.

---

## 4. Hot-Reload Mimarisi (WASM Modülleri)

WASM modülleri, oyun çalışırken hot-reload edilebilir. Bu işlem şu adımlarla gerçekleşir:

### 4.1 Hot-Reload Mekanizması

```rust
use bevy::prelude::*;
use std::time::SystemTime;
use std::collections::HashMap;

/// WASM modül yöneticisi.
/// Her modül KENDİ Store'una sahiptir (per-mod Store).
/// Bu sayede:
///   - Her modülün `WasmHostState`'i diğerlerinden izoledir (bkz. §3.1)
///   - Bir modül çökerse (trap) diğer modüller etkilenmez
///   - Hot-reload sırasında sadece ilgili Store drop edilip yeniden oluşturulur
#[derive(Resource)]
pub struct WasmModManager {
    engine: Engine,
    linker: Linker<WasmHostState>,

    /// Her modül için ayrı Store (manifest.id → Store mapping).
    /// Store'lar asla paylaşılmaz — wasmtime 45.0'da her Store kendi
    /// Instance'larına ve T'sine sahiptir.
    stores: HashMap<String, Store<WasmHostState>>,

    /// Hot-reload takibi (mod dizini → son `mod.wasm` mtime)
    mod_dirs: HashMap<PathBuf, SystemTime>,
    last_check: f64,

    /// Tüm Store'ların paylaştığı ortak kaynaklar
    shared_xbrickmap: Arc<parking_lot::RwLock<XBrickMap>>,
    shared_block_registry: Arc<parking_lot::Mutex<BlockRegistry>>,
    block_command_queue: BlockCommandQueue,
    event_queue: WasmEventQueue,
    game_time: Arc<AtomicU64>,
}

impl WasmModManager {
    pub fn new(
        engine: Engine,
        linker: Linker<WasmHostState>,
        xbrickmap: Arc<parking_lot::RwLock<XBrickMap>>,
        block_registry: Arc<parking_lot::Mutex<BlockRegistry>>,
    ) -> Self {
        Self {
            engine,
            linker,
            stores: HashMap::new(),
            mod_dirs: HashMap::new(),
            last_check: 0.0,
            shared_xbrickmap: xbrickmap,
            shared_block_registry: block_registry,
            block_command_queue: BlockCommandQueue(Default::default()),
            event_queue: WasmEventQueue(Default::default()),
            game_time: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn load_or_reload_mod(&mut self, mod_dir: &Path) -> Result<(), ModLoadError> {
        let manifest: ModManifest = toml::from_str(&fs::read_to_string(mod_dir.join("manifest.toml"))?)?;
        let wasm_path = mod_dir.join("mod.wasm");
        let wasm_bytes = std::fs::read(&wasm_path)?;
        validate_wasm_module(&Module::new(&self.engine, &wasm_bytes)?, &manifest.api_version)?;
        let new_module = Module::new(&self.engine, &wasm_bytes)?;

        // Her modül için yeni bir Store oluştur (kendi WasmHostState'i ile)
        let host_state = WasmHostState {
            instance_local_data: WasmInstanceData {
                instance_id: self.stores.len() as u32,
                mod_name: manifest.id.clone(),
                registered_events: Vec::new(),
                capabilities: manifest.capabilities,
                phase: ModPhase::Init,
                budget: WasmInstanceBudget::default(),
            },
            xbrickmap: self.shared_xbrickmap.clone(),
            block_registry: self.shared_block_registry.clone(),
            block_command_queue: self.block_command_queue.clone(),
            event_queue: self.event_queue.clone(),
            game_time: self.game_time.clone(),
        };

        let mut store = Store::new(&self.engine, host_state);

        // Eski Store'u temizle (Drop edilince tüm Instance'ları otomatik temizlenir)
        if let Some(old_store) = self.stores.remove(&manifest.id) {
            drop(old_store);
        }

        // Yeni modülü yeni Store'a instantiate et
        let instance_pre = self.linker.instantiate_pre(&new_module)?;
        let instance = instance_pre.instantiate(&mut store)?;

        // Modülün init fonksiyonunu çağır (block kayıtları vs.)
        if let Some(run) = instance.get_export(&mut store, "_start") {
            run.into_func()
                .ok_or("_start not a func")?
                .call(&mut store, &[], &mut [])?;
        }
        seal_mod_after_init(&mut store);

        // Instance'ı store içinde tutmak zorunda değiliz çünkü instance
        // store'a aittir. Store drop edilince instance da temizlenir.
        self.stores.insert(manifest.id.clone(), store);
        self.mod_dirs.insert(mod_dir.to_path_buf(), SystemTime::now());
        bevy::log::info!("WASM mod loaded: {} ({})", manifest.id, mod_dir.display());
        Ok(())
    }
}

// Periyodik kontrol sistemi (saniyede 1 kez polling; `mod.wasm` mtime)
fn check_wasm_mod_changes(
    mut manager: ResMut<WasmModManager>,
    time: Res<Time>,
) {
    if time.elapsed() - manager.last_check < 1.0 {
        return;
    }
    manager.last_check = time.elapsed();

    for mod_dir in manager.mod_dirs.keys().cloned().collect::<Vec<_>>() {
        let wasm_path = mod_dir.join("mod.wasm");
        if let Ok(metadata) = std::fs::metadata(&wasm_path) {
            if let Ok(modified) = metadata.modified() {
                let cached = manager.mod_dirs.get(&mod_dir).copied()
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if modified > cached {
                    if let Err(e) = manager.load_or_reload_mod(&mod_dir) {
                        bevy::log::error!("Failed to hot-reload {}: {:?}", mod_dir.display(), e);
                    }
                }
            }
        }
    }
}
```

**Mod dizin düzeni:**

```text
mods/
  example_mod/
    manifest.toml
    mod.wasm
    blocks/          # opsiyonel data pack (TOML, 05 ile)
      custom_ore.toml
```

WASM hot-reload yalnızca `mod.wasm` değişimini izler; TOML/data pack yenilemesi ayrı asset pipeline ile yapılır (`04` §10.5).

---

## 5. API Versioning & Uyumluluk

Her WASM modülü, hangi Strata API sürümüyle yazıldığını belirtmek zorundadır:

```rust
use semver::{Version, VersionReq};

#[derive(Resource)]
pub struct StrataApiVersion {
    /// Motor versiyonu (örn: 0.1.0)
    pub engine_version: Version,
    /// Plugin API versiyonu (örn: 1.0.0)
    pub api_version: Version,
}

impl Default for StrataApiVersion {
    fn default() -> Self {
        Self {
            engine_version: Version::new(0, 1, 0),
            api_version: Version::new(1, 0, 0),
        }
    }
}

// WASM modülü doğrulama:
pub fn validate_wasm_module(
    module: &Module,
    engine_api: &Version,
) -> Result<(), String> {
    for import in module.imports() {
        if import.module() == "strata" {
            match import.name() {
                // "strata" namespace'indeki tüm import'lar kontrol edilir
                name if name.starts_with("api_v") => {
                    let ver = name.strip_prefix("api_v")
                        .and_then(|v| Version::parse(v).ok())
                        .ok_or_else(|| "Invalid API version in import".to_string())?;
                    if ver.major != engine_api.major {
                        return Err(format!(
                            "WASM mod requires Strata API v{}, engine has v{}",
                            ver.major, engine_api.major
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// Native plugin'ler için compile-time kontrol:
pub struct MyPlugin {
    pub required_api_version: VersionReq,
}

impl Plugin for MyPlugin {
    fn build(&self, app: &mut App) {
        let engine_ver = app.world().resource::<StrataApiVersion>();
        assert!(
            self.required_api_version.matches(&engine_ver.api_version),
            "Plugin requires Strata API {} but engine has v{}",
            self.required_api_version,
            engine_ver.api_version
        );
    }
}
```

---

---

## 6. Mod Yöneticisi (Özet)

`WasmModManager` (§4) ve `WasmRuntime` (WIT/Component yolu) aynı `modding` crate'te birleşir. Yükleme sırası: `manifest.toml` → semver doğrulama → linker/instantiate → `_start` / `on-init` → `seal_mod_after_init`. Per-mod `Store` izolasyonu — bkz. §4.1.

Detaylı linker/state machine: **§3–5**. Eski yüksek-seviye async `WasmRuntime` stub'ı implementasyonda `WasmModManager` ile birleştirilir.

---

## 7. Permission Sistemi

```rust
/// Mod izin yöneticisi.
pub struct PermissionManager {
    /// Mod bazlı izinler.
    mod_permissions: HashMap<ModId, ModPermissions>,

    /// Global izin politikası.
    default_policy: PermissionPolicy,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// Tüm izinler reddedilir (whitelist).
    DenyAll,

    /// Tüm izinler verilir (blacklist).
    AllowAll,

    /// Sadece okuma izinleri verilir.
    ReadOnly,
}

/// Mod izinleri (capability). Kademe-farkında: bazı yetenekler yalnızca
/// imzalı/allowlist modlara veya sunucuya verilir (aşağıdaki tablo).
pub struct ModPermissions {
    /// World yazma izni (world-write interface).
    pub world_write: bool,

    /// Entity spawn izni.
    pub entity_spawn: bool,

    /// Network broadcast izni (YALNIZCA sunucu allowlist).
    pub network_broadcast: bool,

    /// UI oluşturma izni (YALNIZCA client).
    pub ui_create: bool,

    /// Fizik batch policy hook izni (pre/post-solve). YALNIZCA imzalı/allowlist.
    pub physics_hooks: bool,

    /// World-gen batch hook izni. Deterministik + imzalı/allowlist.
    pub worldgen_hooks: bool,

    /// File system erişimi.
    pub file_access: bool,

    /// Maksimum bellek kullanımı (MB).
    pub max_memory_mb: u32,

    /// Maksimum CPU süresi (ms/tick).
    pub max_cpu_ms: u32,
}
```

### Capability → Kademe / Güven Eşlemesi

| Capability | T0 data | T1 WASM | T2 native | Not |
|------------|:------:|:-------:|:---------:|-----|
| world-read | — | ✅ | ✅ | serbest |
| world-write | — | ✅ | ✅ | batch (`set-region`) tercih |
| `physics_hooks` | — | ⚠️ imzalı | ✅ | batch pre/post-solve, deterministik |
| `worldgen_hooks` | — | ⚠️ imzalı | ✅ | deterministik (seed, chunk-pos) |
| L2 strateji registry (solver/mesher değiştir) | — | ❌ | ✅ | yalnızca compile-time native |
| GPU / network internals / tam ECS | — | ❌ | ✅ | WASM'a **asla** (04 §10.3) |

`⚠️ imzalı`: yalnızca sunucu mod-allowlist'inde + manifest imzalı modlara verilir (multiplayer için zorunlu, `16-network` ile hizalanır).

```rust
// (devam — PermissionManager)

impl PermissionManager {
    /// Mod manifest'inden izinleri parse et.
    pub fn check_mod_permissions(&self, metadata: &ModMetadata) -> Result<()> {
        let perms = &metadata.permissions;

        // Politika kontrolü
        match self.default_policy {
            PermissionPolicy::DenyAll => {
                // Sadece manifest'te açıkça istenen izinler
            }
            PermissionPolicy::ReadOnly => {
                if perms.world_write || perms.entity_spawn || perms.network_broadcast {
                    return Err(ModError::PermissionDenied("read-only policy".into()));
                }
            }
            PermissionPolicy::AllowAll => {
                // Tüm izinler kabul
            }
        }

        // Resource limit kontrolü
        if perms.max_memory_mb > 256 {
            return Err(ModError::PermissionDenied("memory limit exceeded".into()));
        }

        if perms.max_cpu_ms > 50 {
            return Err(ModError::PermissionDenied("CPU limit exceeded".into()));
        }

        Ok(())
    }
}
```

---

## 8. Mod Metadata

```rust
/// Mod metadata (mod.toml'den yüklenir).
pub struct ModMetadata {
    /// Mod ID (benzersiz).
    pub id: String,

    /// Mod ismi.
    pub name: String,

    /// Versiyon.
    pub version: String,

    /// Yazar.
    pub author: String,

    /// Açıklama.
    pub description: String,

    /// Bağımlılıklar.
    pub dependencies: Vec<ModDependency>,

    /// İzinler.
    pub permissions: ModPermissions,

    /// Entry point (Wasm dosya yolu).
    pub entry_point: PathBuf,

    /// Minimum Strata versiyonu.
    pub min_strata_version: String,
}

/// Mod bağımlılığı.
pub struct ModDependency {
    pub mod_id: String,
    pub version_requirement: String,
    pub optional: bool,
}
```

---

## 9. Native Mod Desteği (T2)

**Varsayılan T2 yolu = motorla birlikte derleme.** Derin modlama (L2 strateji registry ile solver/mesher/lighting değiştirme, tam ECS) `04` §12.5'teki gibi **compile-time** native plugin olarak yapılır. Modpack pratikte yeniden derlenmiş bir client'tır. Bu, Rust'ın stabil ABI'sinin olmaması ve `dlclose`'un güvensizliği gerçeğiyle ([Bevy #4843](https://github.com/bevyengine/bevy/issues/4843)) uyumludur.

Aşağıdaki **runtime `.dll`** loader yalnızca **opsiyonel/ileri** bir yoldur ve şu kısıtlara tabidir:

- **ABI riski:** host ve mod **aynı rustc sürümü + aynı derleme flag'leriyle** derlenmek zorundadır; aksi halde UB. (Alternatif: `stabby`/`abi_stable` ile `extern "C"` stabil ABI — boilerplate ağır.)
- **Hot-reload yok:** `dlclose` güvenli unload yapamaz; reload = süreç yeniden başlatma.
- **Sandbox yok:** native mod tüm sürece erişir → yalnızca **imzalı + allowlist** (asla güvensiz kaynak).
- **Yalnızca single-player / kendi sunucun:** multiplayer'da imzalı sunucu allowlist'i zorunlu.

```rust
/// Native (.dll) mod loader — OPSİYONEL/İLERİ yol (yukarıdaki kısıtlara tabi).
/// Birincil T2 yolu compile-time birlikte derlemedir (`04` §10.4).
pub struct NativeModLoader {
    /// Yüklü native modlar.
    loaded: HashMap<ModId, NativeMod>,
}

pub struct NativeMod {
    pub metadata: ModMetadata,
    pub library: libloading::Library,
    pub vtable: ModVTable,
}

/// Mod virtual function table.
pub struct ModVTable {
    /// Init: registry'ye blok ekleme (builder).
    pub on_init: unsafe extern "C" fn(*mut ModInitContext),
    pub on_shutdown: unsafe extern "C" fn(*mut ModRuntimeContext),
    /// Tick: salt okunur registry + dünya.
    pub on_tick: unsafe extern "C" fn(*mut ModRuntimeContext),
}

/// Init fazı — BlockRegistryBuilder üzerinden kayıt (05 §3.3).
/// Init tamamlanınca builder `build()` → `Arc<BlockRegistryInner>`; pointer geçersiz olur.
#[repr(C)]
pub struct ModInitContext {
    pub registry_builder: *mut BlockRegistryBuilder,
    pub world: *mut WorldState,
    pub logging: *mut LogManager,
}

/// Runtime — registry immutable (05: init sonrası mutation yok).
#[repr(C)]
pub struct ModRuntimeContext {
    pub registry: *const BlockRegistryInner,
    pub world: *mut WorldState,
    pub entities: *mut EntityManager,
    pub network: *mut NetworkManager,
    pub logging: *mut LogManager,
}

impl NativeModLoader {
    /// Native mod yükle.
    pub unsafe fn load_native_mod(&mut self, path: &Path) -> Result<ModId> {
        // Sadece izinli native modlar yüklenebilir
        if !self.is_native_allowed(path) {
            return Err(ModError::NativeNotAllowed);
        }

        let library = libloading::Library::new(path)?;

        // VTable sembollerini resolve et
        let on_init = *library.get(b"mod_on_init")?;
        let on_shutdown = *library.get(b"mod_on_shutdown")?;
        let on_tick = *library.get(b"mod_on_tick")?;

        // Metadata yükle
        let metadata_fn = *library.get(b"mod_metadata")?;
        let metadata = metadata_fn();

        let mod_id = ModId::new(&metadata.id);
        self.loaded.insert(mod_id.clone(), NativeMod {
            metadata,
            library,
            vtable: ModVTable { on_init, on_shutdown, on_tick },
        });

        Ok(mod_id)
    }
}
```

---

## 10. Mod Lifecycle

```
Mod Yükleme:
  1. mod.toml parse et (metadata + permissions)
  2. Wasm bytecode validate et
  3. İzin kontrolü
  4. Wasmtime instance oluştur
  5. WIT binding'leri bağla
  6. on-init çağır (register-block / native builder — tek yazma penceresi)
  7. Tüm modlar init bitince BlockRegistryBuilder.build() → Arc (immutable)
  8. Mod'ı running state'e al

Mod Tick (OPSİYONEL — sabit TPS zorunluluğu YOK):
  1. Yalnızca on-tick EXPORT eden + bütçesi olan modlar için on-tick çağrılır
  2. CPU süresi + host-call bütçesi kontrol et (max_cpu_ms, host_calls_per_tick)
  3. Bütçe aşımı → çağrıyı atla; Trap → mod paused, log error
  4. NOT: Fizik/world-gen hook'ları on-tick'ten AYRIDIR — FixedUpdate sınırında, batch
     halinde, capability'ye bağlı çağrılır (§11 hook bus)

Mod Kapatma:
  1. on-shutdown çağır
  2. Wasmtime instance'ı temizle
  3. Event listener'ları kaldır
  4. Timer'ları iptal et
  5. Mod'ı unloaded state'e al

Mod Hata Yönetimi:
  - Trap (Wasm exception) → mod paused, log error
  - CPU timeout → mod paused, log warning
  - Memory limit → mod killed, log error
  - Permission violation → mod killed, log error
```

---

## 11. Fizik Policy Hook Bus (L3 — Batch, Solver Dışı)

Bu, "fiziği modlamak"ın T1 (WASM) yolunun kalbidir. WASM solver'ın **içine girmez**; `FixedUpdate` sınırında, **tick başına tek call** ile tüm contact batch'ini alır ve override batch'i döner. Solver SIMD iç döngüsü native kalır.

```text
FixedUpdate (her tick 1 kez, capability: physics-hooks):
  BroadPhase → NarrowPhase
    → [pre-solve hook]   host: contacts[] slice'ı zero-copy linear mem'e yazar →
                         wasm on-pre-solve(contacts) tek call → impulse_overrides[]
                         SegQueue'ya batch push
    → SolveConstraints   (L0 native; override kuyruğunu uygular)
    → [post-solve hook]  sonuç batch'i wasm'a (opsiyonel, salt okuma)
```

```rust
/// Pre-solve hook'u tüm physics-hooks capability'li modlar için BATCH çağırır.
/// Maliyet: contact başına DEĞİL, tick başına mod başına 1 host call.
fn run_pre_solve_hooks(
    contacts: &[Contact],                 // NarrowPhase çıktısı (SoA)
    manager: &mut WasmModManager,
    override_queue: &SegQueue<ImpulseOverride>,
) {
    for store in manager.physics_hook_stores_mut() {
        // Bütçe: aşımda bu mod atlanır (Trap değil — opsiyonel hook)
        if !store.data().budget.try_consume_host_call() { continue; }

        // Zero-copy: contacts'ı modun linear memory'sindeki in-buffer'a yaz,
        // on-pre-solve'u çağır, dönen override slice'ını kuyruğa batch push.
        let overrides = call_on_pre_solve(store, contacts);
        for ov in overrides {
            override_queue.push(ov);      // lock-free; solver tek drain ile uygular
        }
    }
}
// Kayıt: FixedUpdate, run_pre_solve_hooks
//   .after(StrataPhysicsSets::NarrowPhase).before(StrataPhysicsSets::SolveConstraints)
```

**Native (T2) fizik değişimi** ise bambaşka bir mekanizmadır: solver'ı komple değiştirmek `04` §12.5'teki `CollisionResponseRegistry` / `FluidSolverRegistry` ile compile-time yapılır (WASM'a açılmaz).

---

## 12. Determinizm ve Çoğul Oyuncu

Fizik/world-gen hook'ları açılırken en kritik kısıt (CLAUDE.md: server-authoritative + deterministic):

1. **Server-authoritative:** Fizik hook'u istemcide yalnızca *tahmin/görsel*tir; otorite **sunucudadır**. Aksi halde desync + cheat. İstemci tahmini ile sunucu sonucu çeliştiğinde sunucu kazanır (reconciliation, `16-network`).
2. **Determinizm zorunlu:** WASM `f32` non-determinizmi (NaN bit-pattern, transcendental) lockstep'i bozar. Bu yüzden hook arayüzü **fixed-point (q16.16) impulse/normal** kullanır; `on-chunk-generated` yalnızca `(seed, chunk-pos)` girdisine bağlı saf fonksiyon olmalıdır.
3. **Mod set uyumu:** İstemci ↔ sunucu **aynı mod set + aynı WIT `api_version`**'a sahip olmalı; uyuşmazlık → bağlantı reddi (`validate_wasm_module`, `04` §9). Fizik etkileyen modlarda bu zorunludur.
4. **Yan etki yasağı:** Hook'lar global state'e (zaman, RNG seed) host'un verdiği deterministik kanal dışında erişemez; `get_time` gibi non-deterministik kaynaklar fizik hook bütçesinde değildir.

---

## 13. Crate Organizasyonu

```
crates/
  modding/
    ├── mod.rs              ← Modding plugin entry point
    ├── runtime/
    │   ├── mod.rs          ← WasmRuntime
    │   ├── loader.rs       ← Wasm mod yükleme
    │   ├── instance.rs     ← Mod instance yönetimi
    │   └── native.rs       ← Native mod loader
    ├── wit/
    │   ├── mod.rs          ← WIT binding'leri (Component Model)
    │   ├── block_registry.rs ← Block registry WIT
    │   ├── world.rs        ← World read/write WIT
    │   ├── physics_hooks.rs ← Fizik batch policy hook (L3, §11)
    │   ├── worldgen_hooks.rs ← World-gen batch hook
    │   ├── entities.rs     ← Entity WIT
    │   ├── network.rs      ← Network WIT
    │   ├── ui.rs           ← UI WIT
    │   ├── events.rs       ← Events WIT
    │   ├── timers.rs       ← Timers WIT
    │   └── logging.rs      ← Logging WIT
    ├── hooks/
    │   ├── mod.rs          ← Hook bus (FixedUpdate entegrasyonu)
    │   ├── pre_solve.rs    ← run_pre_solve_hooks + override SegQueue
    │   └── worldgen.rs     ← on-chunk-generated batch
    ├── permissions/
    │   ├── mod.rs          ← PermissionManager (capability→kademe, §4)
    │   ├── policy.rs       ← PermissionPolicy
    │   ├── allowlist.rs    ← Sunucu mod allowlist + imza doğrulama
    │   └── limits.rs       ← Resource + frame bütçe limitleri
    ├── metadata/
    │   ├── mod.rs          ← ModMetadata
    │   └── manifest.rs     ← mod.toml parsing
    └── lifecycle/
        ├── mod.rs          ← Mod lifecycle yönetimi
        ├── tick.rs         ← Mod tick sistemi
        └── error.rs        ← Mod hata yönetimi
```
