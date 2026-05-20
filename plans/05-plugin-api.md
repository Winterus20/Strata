# 24 — Plugin-API Sistemi

## 1. Genel Bakış

Strata'nın plugin sistemi **her alt sistemi bir plugin** olarak tanımlar. Plugin'lar bağımsız yüklenir, başlatılır ve kapatılır. Bu, modülerlik ve test edilebilirlik sağlar.

### Temel Prensipler

- **Plugin-first:** Her subsystem bir plugin
- **Lifecycle-managed:** init → run → shutdown
- **Dependency-aware:** Plugin'lar birbirine bağımlılık tanımlayabilir
- **Hot-reload:** Bazı plugin'lar runtime'da yeniden yüklenebilir

---

## 2. Plugin Trait

```rust
/// Plugin trait — tüm plugin'lar bunu uygular.
pub trait Plugin: Send + Sync {
    /// Plugin ismi (benzersiz).
    fn name(&self) -> &str;

    /// Bağımlılıklar.
    fn dependencies(&self) -> &[&str] {
        &[]
    }

    /// Plugin'i başlat (App oluşturulurken).
    fn build(&self, app: &mut App);

    /// Plugin'i hazırla (tüm plugin'lar build edildikten sonra).
    fn finish(&self, _app: &mut App) {}

    /// Plugin'i temizle (kapatma).
    fn cleanup(&self, _app: &mut App) {}

    /// Hot-reload destekli mi?
    fn hot_reloadable(&self) -> bool {
        false
    }

    /// Plugin'i yeniden yükle.
    fn reload(&self, _app: &mut App) -> Result<(), PluginError> {
        Err(PluginError::NotReloadable)
    }
}

#[derive(Debug)]
pub enum PluginError {
    NotReloadable,
    DependencyNotFound(String),
    CircularDependency(String),
    InitFailed(String),
}
```

---

## 3. Plugin Registry

```rust
/// Plugin registry — plugin'ları yönetir.
pub struct PluginRegistry {
    /// Kayıtlı plugin'lar.
    plugins: HashMap<String, Box<dyn Plugin>>,

    /// Yükleme sırası (topolojik sıralama).
    load_order: Vec<String>,

    /// Yüklü plugin'lar.
    loaded: HashSet<String>,

    /// Plugin state.
    states: HashMap<String, PluginState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Unloaded,
    Loading,
    Loaded,
    Running,
    Error,
}

impl PluginRegistry {
    /// Plugin kaydet.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<(), PluginError> {
        let name = plugin.name().to_string();

        // Bağımlılık kontrolü
        for dep in plugin.dependencies() {
            if !self.plugins.contains_key(*dep) {
                return Err(PluginError::DependencyNotFound(dep.to_string()));
            }
        }

        self.plugins.insert(name, plugin);
        Ok(())
    }

    /// Tüm plugin'ları yükle (topolojik sıralama).
    pub fn load_all(&mut self, app: &mut App) -> Result<(), PluginError> {
        // Topolojik sıralama (bağımlılık grafiği)
        self.load_order = self.topological_sort()?;

        // Sırayla yükle
        for name in &self.load_order {
            self.load_plugin(name, app)?;
        }

        // Finish çağır
        for name in &self.load_order {
            if let Some(plugin) = self.plugins.get(name) {
                plugin.finish(app);
                self.states.insert(name.clone(), PluginState::Running);
            }
        }

        Ok(())
    }

    /// Tek plugin yükle.
    fn load_plugin(&mut self, name: &str, app: &mut App) -> Result<(), PluginError> {
        // Bağımlılıkları önce yükle
        if let Some(plugin) = self.plugins.get(name) {
            for dep in plugin.dependencies() {
                if !self.loaded.contains(*dep) {
                    self.load_plugin(dep, app)?;
                }
            }
        }

        // Plugin'i yükle
        if let Some(plugin) = self.plugins.get(name) {
            self.states.insert(name.to_string(), PluginState::Loading);
            plugin.build(app);
            self.states.insert(name.to_string(), PluginState::Loaded);
            self.loaded.insert(name.to_string());

            tracing::info!(plugin = name, "Plugin loaded");
        }

        Ok(())
    }

    /// Topolojik sıralama.
    fn topological_sort(&self) -> Result<Vec<String>, PluginError> {
        let mut order = Vec::new();
        let mut visited = HashSet::new();
        let mut visiting = HashSet::new();

        for name in self.plugins.keys() {
            if !visited.contains(name) {
                self.visit(name, &mut visited, &mut visiting, &mut order)?;
            }
        }

        Ok(order)
    }

    fn visit(
        &self,
        name: &str,
        visited: &mut HashSet<String>,
        visiting: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) -> Result<(), PluginError> {
        if visiting.contains(name) {
            return Err(PluginError::CircularDependency(name.to_string()));
        }

        if visited.contains(name) {
            return Ok(());
        }

        visiting.insert(name.to_string());

        if let Some(plugin) = self.plugins.get(name) {
            for dep in plugin.dependencies() {
                self.visit(dep, visited, visiting, order)?;
            }
        }

        visiting.remove(name);
        visited.insert(name.to_string());
        order.push(name.to_string());

        Ok(())
    }

    /// Tüm plugin'ları kapat.
    pub fn unload_all(&mut self, app: &mut App) {
        // Ters sırayla kapat
        for name in self.load_order.iter().rev() {
            if let Some(plugin) = self.plugins.get(name) {
                plugin.cleanup(app);
                self.states.insert(name.clone(), PluginState::Unloaded);
                tracing::info!(plugin = name, "Plugin unloaded");
            }
        }

        self.loaded.clear();
    }

    /// Plugin durumunu al.
    pub fn get_state(&self, name: &str) -> PluginState {
        self.states.get(name).copied().unwrap_or(PluginState::Unloaded)
    }

    /// Tüm plugin durumlarını raporla.
    pub fn report(&self) -> String {
        let mut output = String::from("=== Plugin Status ===\n\n");

        for name in &self.load_order {
            let state = self.get_state(name);
            let deps = self.plugins.get(name)
                .map(|p| p.dependencies().join(", "))
                .unwrap_or_default();

            output.push_str(&format!(
                "{}: {:?} (deps: {})\n",
                name, state, deps
            ));
        }

        output
    }
}
```

---

## 4. Hook Sistemi

```rust
/// Hook registry — plugin'lar event hook'ları kaydedebilir.
pub struct HookRegistry {
    hooks: HashMap<String, Vec<Box<dyn Hook>>>,
}

pub trait Hook: Send + Sync {
    /// Hook çalıştır.
    fn execute(&self, ctx: &HookContext) -> HookResult;

    /// Hook önceliği (düşük = önce çalışır).
    fn priority(&self) -> i32 {
        0
    }
}

pub enum HookResult {
    /// Devam et.
    Continue,

    /// İptal et (sonraki hook'lar çalışmaz).
    Cancel,

    /// Sonucu değiştir.
    Modify(Box<dyn Any>),
}

pub struct HookContext {
    pub event_name: String,
    pub data: Box<dyn Any>,
    pub cancelled: bool,
}

impl HookRegistry {
    /// Hook kaydet.
    pub fn register(&mut self, event: &str, hook: Box<dyn Hook>) {
        let hooks = self.hooks.entry(event.to_string()).or_default();
        hooks.push(hook);

        // Önceliğe göre sırala
        hooks.sort_by_key(|h| h.priority());
    }

    /// Hook'ları çalıştır.
    pub fn fire(&self, event: &str, ctx: &mut HookContext) -> HookResult {
        if let Some(hooks) = self.hooks.get(event) {
            for hook in hooks {
                match hook.execute(ctx) {
                    HookResult::Continue => {}
                    result => return result,
                }
            }
        }

        HookResult::Continue
    }
}
```

---

## 5. Built-in Plugin'lar

```rust
/// Strata built-in plugin'ları.
pub fn register_builtin_plugins(registry: &mut PluginRegistry) {
    registry.register(Box::new(BlockRegistryPlugin)).unwrap();
    registry.register(Box::new(EcsPlugin)).unwrap();
    registry.register(Box::new(WorldGenPlugin)).unwrap();
    registry.register(Box::new(MeshingPlugin::new(MeshType::Opaque))).unwrap();
    registry.register(Box::new(MeshingPlugin::new(MeshType::Transparent))).unwrap();
    registry.register(Box::new(RenderPlugin)).unwrap();
    registry.register(Box::new(PhysicsPlugin)).unwrap();
    registry.register(Box::new(LightingPlugin)).unwrap();
    registry.register(Box::new(StreamingPlugin)).unwrap();
    registry.register(Box::new(NetworkPlugin)).unwrap();
    registry.register(Box::new(StoragePlugin)).unwrap();
    registry.register(Box::new(AudioPlugin)).unwrap();
    registry.register(Box::new(UiPlugin)).unwrap();
    registry.register(Box::new(ParticlePlugin)).unwrap();
    registry.register(Box::new(AiPlugin)).unwrap();
    registry.register(Box::new(SecurityPlugin)).unwrap();
    registry.register(Box::new(DebugPlugin)).unwrap();
    registry.register(Box::new(ModdingPlugin)).unwrap();
}
```

---

## 6. Plugin Lifecycle

```
Plugin Yükleme Sırası:
  1. BlockRegistryPlugin    (bağımlılık yok)
  2. EcsPlugin              (bağımlılık yok)
  3. WorldGenPlugin         (BlockRegistry, Ecs)
  4. MeshingPlugin          (BlockRegistry)
  5. RenderPlugin           (Meshing, Ecs)
  6. PhysicsPlugin          (Ecs, BlockRegistry)
  7. LightingPlugin         (BlockRegistry, Meshing)
  8. StreamingPlugin        (WorldGen, Meshing, Render)
  9. NetworkPlugin          (Ecs, Streaming)
  10. StoragePlugin         (Streaming)
  11. AudioPlugin           (Ecs)
  12. UiPlugin              (Render, Ecs)
  13. ParticlePlugin        (Render, Ecs)
  14. AiPlugin              (Ecs, Physics)
  15. SecurityPlugin        (Network, Ecs)
  16. DebugPlugin           (Render, Ecs)
  17. ModdingPlugin         (BlockRegistry, Ecs)

Kapatma Sırası (ters):
  17 → 16 → 15 → ... → 1
```

---

## 7. Crate Organizasyonu

```
crates/
  plugin-api/
    ├── mod.rs              ← Plugin API entry point
    ├── trait.rs            ← Plugin trait
    ├── registry.rs         ← PluginRegistry
    ├── hooks/
    │   ├── mod.rs          ← Hook sistemi
    │   ├── registry.rs     ← HookRegistry
    │   └── types.rs        ← HookResult, HookContext
    ├── error.rs            ← PluginError
    └── builtin.rs          ← Built-in plugin listesi
```
