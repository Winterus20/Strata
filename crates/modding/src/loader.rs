use std::collections::HashMap;
use std::path::{Path, PathBuf};
use anyhow::Context;
use serde::Deserialize;
use wasmtime::component::{Component, Linker};
use wasmtime::Store;

use crate::runtime::{ModState, StrataMod};
use crate::sandbox::{create_wasm_engine, SandboxLimits};

/// Mod manifest dosyası (mod.toml) yapısı.
#[derive(Deserialize, Debug, Clone)]
pub struct ModManifest {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub entrypoint: String, // .wasm dosya yolu (mod klasörüne göre bağıl)
}

/// Yüklenmiş ve çalışmakta olan bir Wasm modunun çalışma zamanı örneği.
pub struct WasmModInstance {
    pub manifest: ModManifest,
    pub store: Store<ModState>,
    pub bindings: StrataMod,
}

impl WasmModInstance {
    /// Wasm dosyasından yeni bir mod yükler ve instantiate eder.
    pub fn load<P: AsRef<Path>>(
        engine: &wasmtime::Engine,
        manifest: ModManifest,
        wasm_path: P,
    ) -> anyhow::Result<Self> {
        let wasm_bytes = std::fs::read(&wasm_path)
            .with_context(|| format!("Wasm dosyası okunamadı: {:?}", wasm_path.as_ref()))?;

        // Komponenti compile et
        let component = Component::new(engine, wasm_bytes)
            .context("Wasm component derleme hatası")?;

        // Linker ve API import'larını kur
        let mut linker = Linker::new(engine);
        crate::runtime::StrataMod::add_to_linker(&mut linker, |state| state)?;

        // Mod State & Store oluştur
        let limits = SandboxLimits::default();
        let state = ModState::new(limits);
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limits);

        // Yakıt limitini belirle (Örn: 10.000.000 instructions)
        store.set_fuel(10_000_000)?;

        // Komponenti instantiate et
        let bindings = StrataMod::instantiate(&mut store, &component, &linker)?;

        Ok(Self {
            manifest,
            store,
            bindings,
        })
    }

    /// Olay tetikleyici: Blok yerleştirildiğinde Wasm modundaki export hook'u çalıştırır.
    pub fn trigger_on_block_placed(&mut self, x: i32, y: i32, z: i32, block_id: u16) -> anyhow::Result<()> {
        // Her çağrıda yakıtı yenile
        let _ = self.store.set_fuel(5_000_000);
        let hooks = self.bindings.strata_engine_api_event_hooks();
        hooks.call_on_block_placed(&mut self.store, x, y, z, block_id)?;
        Ok(())
    }

    /// Olay tetikleyici: Blok kırıldığında Wasm modundaki export hook'u çalıştırır.
    pub fn trigger_on_block_broken(&mut self, x: i32, y: i32, z: i32, block_id: u16) -> anyhow::Result<()> {
        let _ = self.store.set_fuel(5_000_000);
        let hooks = self.bindings.strata_engine_api_event_hooks();
        hooks.call_on_block_broken(&mut self.store, x, y, z, block_id)?;
        Ok(())
    }

    /// Olay tetikleyici: Oyuncu her tick yaptığında Wasm modundaki export hook'u çalıştırır.
    pub fn trigger_on_player_tick(&mut self, player_id: u64, x: f32, y: f32, z: f32) -> anyhow::Result<()> {
        let _ = self.store.set_fuel(1_000_000);
        let hooks = self.bindings.strata_engine_api_event_hooks();
        hooks.call_on_player_tick(&mut self.store, player_id, x, y, z)?;
        Ok(())
    }
}

/// Tüm Wasm modlarının yüklenmesi, güncellenmesi ve tetiklenmesinden sorumlu yönetici.
pub struct ModManager {
    engine: wasmtime::Engine,
    mods_directory: PathBuf,
    loaded_mods: HashMap<String, WasmModInstance>,
}

impl ModManager {
    /// Yeni bir ModManager oluşturur.
    pub fn new<P: AsRef<Path>>(mods_dir: P) -> Self {
        Self {
            engine: create_wasm_engine(),
            mods_directory: mods_dir.as_ref().to_path_buf(),
            loaded_mods: HashMap::new(),
        }
    }

    /// Belirlenen modlar dizinindeki tüm geçerli modları tarar ve yükler.
    pub fn load_all_mods(&mut self) -> anyhow::Result<()> {
        tracing::info!("Modlar taranıyor: {:?}", self.mods_directory);

        if !self.mods_directory.exists() {
            std::fs::create_dir_all(&self.mods_directory)?;
        }

        for entry in std::fs::read_dir(&self.mods_directory)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                let manifest_path = path.join("mod.toml");
                if manifest_path.exists() {
                    match self.load_single_mod(&path) {
                        Ok(mod_instance) => {
                            tracing::info!(
                                "Mod başarıyla yüklendi: {} (v{})",
                                mod_instance.manifest.name,
                                mod_instance.manifest.version
                            );
                            self.loaded_mods.insert(mod_instance.manifest.name.clone(), mod_instance);
                        }
                        Err(e) => {
                            tracing::error!("Mod yüklenirken hata oluştu ({:?}): {:?}", path, e);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Tek bir modu diskten yükler.
    pub fn load_single_mod(&self, mod_dir: &Path) -> anyhow::Result<WasmModInstance> {
        let manifest_path = mod_dir.join("mod.toml");
        let manifest_content = std::fs::read_to_string(&manifest_path)
            .context("mod.toml okunamadı")?;
        
        let manifest: ModManifest = toml::from_str(&manifest_content)
            .context("mod.toml parse edilemedi")?;

        let entrypoint_path = mod_dir.join(&manifest.entrypoint);
        WasmModInstance::load(&self.engine, manifest, entrypoint_path)
    }

    /// Belirli bir modu ismiyle yeniden yükler (Hot-Reload).
    pub fn reload_mod(&mut self, mod_name: &str) -> anyhow::Result<()> {
        tracing::info!("Mod yeniden yükleniyor (Hot-Reload): {}", mod_name);
        
        let mod_dir = self.mods_directory.join(mod_name);
        if !mod_dir.exists() {
            return Err(anyhow::anyhow!("Yeniden yüklenecek mod dizini bulunamadı: {:?}", mod_dir));
        }

        let new_instance = self.load_single_mod(&mod_dir)?;
        self.loaded_mods.insert(mod_name.to_string(), new_instance);
        
        tracing::info!("Mod başarıyla güncellendi (Hot-Reload): {}", mod_name);
        Ok(())
    }

    /// Tüm modlarda 'on-block-placed' olayını tetikler.
    pub fn broadcast_block_placed(&mut self, x: i32, y: i32, z: i32, block_id: u16) {
        for (name, instance) in self.loaded_mods.iter_mut() {
            if let Err(e) = instance.trigger_on_block_placed(x, y, z, block_id) {
                tracing::error!("Mod '{}' on-block-placed hatası: {:?}", name, e);
            }
        }
    }

    /// Tüm modlarda 'on-block-broken' olayını tetikler.
    pub fn broadcast_block_broken(&mut self, x: i32, y: i32, z: i32, block_id: u16) {
        for (name, instance) in self.loaded_mods.iter_mut() {
            if let Err(e) = instance.trigger_on_block_broken(x, y, z, block_id) {
                tracing::error!("Mod '{}' on-block-broken hatası: {:?}", name, e);
            }
        }
    }

    /// Tüm modlarda 'on-player-tick' olayını tetikler.
    pub fn broadcast_player_tick(&mut self, player_id: u64, x: f32, y: f32, z: f32) {
        for (name, instance) in self.loaded_mods.iter_mut() {
            if let Err(e) = instance.trigger_on_player_tick(player_id, x, y, z) {
                tracing::error!("Mod '{}' on-player-tick hatası: {:?}", name, e);
            }
        }
    }

    /// Yüklü modların listesini ve durumlarını döner.
    pub fn loaded_mods(&self) -> &HashMap<String, WasmModInstance> {
        &self.loaded_mods
    }
}
