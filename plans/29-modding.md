# 15 — Modding & Plugin Sistemi (Wasm)

## 1. Genel Bakış

Strata, **wasmtime 30+** ile Wasm modding desteği sunar. Modlar **WIT (Web Interface Types)** ile oyun API'sine erişir. Native .dll modlar da desteklenir (Windows x64).

### Temel Prensipler

- **Sandboxed:** Wasm modlar izole çalışır, doğrudan bellek erişimi yok
- **WIT-based:** Tip güvenli API tanımları
- **Lifecycle-managed:** Modlar yüklenme/kapatma hook'larına sahiptir
- **Native fallback:** Performans kritik modlar .dll olarak yüklenebilir

---

## 2. WIT Interface Tanımları

```wit
# wit/strata.wit
package strata:modding@0.1.0;

/// Block registry erişimi.
interface block-registry {
    /// Yeni blok tipi kaydet.
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

    /// Bölge düzenle (batch).
    set-region: func(min: vec3, max: vec3, blocks: list<u16>) -> result<(), error>;
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

/// Ana mod interface'i.
interface mod-api {
    use block-registry.{register-block, get-block, find-block};
    use world-read.{get-block as world-get-block, get-biome, get-height};
    use world-write.{set-block, break-block, set-region};
    use entities.{spawn-entity, despawn-entity, get-position, set-position};
    use network.{broadcast, send-to, register-event};
    use ui.{create-panel, add-button, show-panel, hide-panel};
    use events.{on-event, emit-event};
    use timers.{once, interval};
    use logging.{log-info, log-warn, log-error};

    /// Mod başlatıldığında çağrılır.
    on-init: func();

    /// Mod kapatıldığında çağrılır.
    on-shutdown: func();

    /// Her tick'te çağrılır (20 TPS).
    on-tick: func();
}
```

---

## 3. Wasm Runtime

```rust
/// Wasm mod runtime.
pub struct WasmRuntime {
    /// Wasmtime engine.
    engine: Engine,

    /// Yüklü modlar.
    mods: HashMap<ModId, WasmMod>,

    /// WIT binding'leri.
    wit_bindings: WitBindings,

    /// Mod izinleri.
    permissions: PermissionManager,
}

impl WasmRuntime {
    /// Yeni runtime oluştur.
    pub fn new() -> Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.async_support(true);
        config.cranelift_debug_verifies(false);

        let engine = Engine::new(&config)?;

        Ok(Self {
            engine,
            mods: HashMap::new(),
            wit_bindings: WitBindings::new(),
            permissions: PermissionManager::new(),
        })
    }

    /// Bir mod yükle.
    pub async fn load_mod(&mut self, path: &Path) -> Result<ModId> {
        let wasm_bytes = tokio::fs::read(path).await?;

        // Validate mod metadata
        let metadata = self.validate_mod(&wasm_bytes)?;

        // İzin kontrolü
        self.permissions.check_mod_permissions(&metadata)?;

        // Wasmtime linker oluştur
        let mut linker = Linker::new(&self.engine);
        self.wit_bindings.bind_all(&mut linker)?;

        // Mod instance oluştur
        let store = Store::new(&self.engine, ModState::new(&metadata));
        let instance = linker.instantiate_async(&mut store, &wasm_bytes).await?;

        let mod_id = ModId::new(&metadata.id);
        self.mods.insert(mod_id.clone(), WasmMod {
            metadata,
            instance,
            store,
            state: ModState::Running,
        });

        Ok(mod_id)
    }

    /// Mod'u başlat (on-init çağır).
    pub async fn init_mod(&mut self, mod_id: &ModId) -> Result<()> {
        let mod_ref = self.mods.get_mut(mod_id).ok_or(ModError::NotFound)?;

        // on-init fonksiyonunu çağır
        let on_init = mod_ref.instance.get_typed_func::<(), ()>(&mut mod_ref.store, "on-init")?;
        on_init.call_async(&mut mod_ref.store, ()).await?;

        Ok(())
    }

    /// Mod'u kapat (on-shutdown çağır).
    pub async fn unload_mod(&mut self, mod_id: &ModId) -> Result<()> {
        if let Some(mod_ref) = self.mods.get_mut(mod_id) {
            if let Ok(on_shutdown) = mod_ref.instance.get_typed_func::<(), ()>(&mut mod_ref.store, "on-shutdown") {
                on_shutdown.call_async(&mut mod_ref.store, ()).await.ok();
            }
        }

        self.mods.remove(mod_id);
        Ok(())
    }

    /// Tüm modlar için tick çağır (20 TPS).
    pub async fn tick_all(&mut self) {
        let mod_ids: Vec<_> = self.mods.keys().cloned().collect();

        for mod_id in mod_ids {
            if let Some(mod_ref) = self.mods.get_mut(&mod_id) {
                if mod_ref.state != ModState::Running {
                    continue;
                }

                if let Ok(on_tick) = mod_ref.instance.get_typed_func::<(), ()>(&mut mod_ref.store, "on-tick") {
                    let _ = on_tick.call_async(&mut mod_ref.store, ()).await;
                }
            }
        }
    }
}
```

---

## 4. Permission Sistemi

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

/// Mod izinleri.
pub struct ModPermissions {
    /// World yazma izni.
    pub world_write: bool,

    /// Entity spawn izni.
    pub entity_spawn: bool,

    /// Network broadcast izni.
    pub network_broadcast: bool,

    /// UI oluşturma izni.
    pub ui_create: bool,

    /// File system erişimi.
    pub file_access: bool,

    /// Native library yükleme izni.
    pub native_load: bool,

    /// Maksimum bellek kullanımı (MB).
    pub max_memory_mb: u32,

    /// Maksimum CPU süresi (ms/tick).
    pub max_cpu_ms: u32,
}

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

## 5. Mod Metadata

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

## 6. Native Mod Desteği

```rust
/// Native (.dll) mod loader.
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
    pub on_init: unsafe extern "C" fn(*mut ModContext),
    pub on_shutdown: unsafe extern "C" fn(*mut ModContext),
    pub on_tick: unsafe extern "C" fn(*mut ModContext),
}

/// Mod context — native mod'ların oyun API'sine erişimi.
#[repr(C)]
pub struct ModContext {
    pub registry: *mut BlockRegistry,
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

## 7. Mod Lifecycle

```
Mod Yükleme:
  1. mod.toml parse et (metadata + permissions)
  2. Wasm bytecode validate et
  3. İzin kontrolü
  4. Wasmtime instance oluştur
  5. WIT binding'leri bağla
  6. on-init çağır
  7. Mod'ı running state'e al

Mod Tick (20 TPS):
  1. Tüm running modlar için on-tick çağır
  2. CPU süresi kontrol et (max_cpu_ms)
  3. Hata durumunda mod'u paused state'e al

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

## 8. Crate Organizasyonu

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
    │   ├── mod.rs          ← WIT binding'leri
    │   ├── block_registry.rs ← Block registry WIT
    │   ├── world.rs        ← World read/write WIT
    │   ├── entities.rs     ← Entity WIT
    │   ├── network.rs      ← Network WIT
    │   ├── ui.rs           ← UI WIT
    │   ├── events.rs       ← Events WIT
    │   ├── timers.rs       ← Timers WIT
    │   └── logging.rs      ← Logging WIT
    ├── permissions/
    │   ├── mod.rs          ← PermissionManager
    │   ├── policy.rs       ← PermissionPolicy
    │   └── limits.rs       ← Resource limits
    ├── metadata/
    │   ├── mod.rs          ← ModMetadata
    │   └── manifest.rs     ← mod.toml parsing
    └── lifecycle/
        ├── mod.rs          ← Mod lifecycle yönetimi
        ├── tick.rs         ← Mod tick sistemi
        └── error.rs        ← Mod hata yönetimi
```
