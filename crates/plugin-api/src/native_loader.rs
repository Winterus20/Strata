use std::path::Path;
use libloading::{Library, Symbol};
use crate::r#trait::GamePlugin;

/// Dinamik kütüphaneleri (.dll / .so) çalışma zamanında (runtime) yükleyen
/// ve strata-plugin-api arayüzünü çıkaran yükleyici.
pub struct NativePluginLoader {
    loaded_libraries: Vec<Library>,
}

impl Default for NativePluginLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl NativePluginLoader {
    /// Yeni bir NativePluginLoader oluşturur.
    pub fn new() -> Self {
        Self {
            loaded_libraries: Vec::new(),
        }
    }

    /// Belirtilen dosya yolundaki dinamik kütüphaneyi yükler ve
    /// GamePlugin arayüzünü döner.
    ///
    /// # Güvenlik
    ///
    /// Yüklenen dynamic library güvenilmeyen kod içerebilir. Rust'ın dinamik
    /// kütüphane yükleme mekanizması (libloading) unsafe olup, ABI uyumsuzlukları
    /// durumunda tanımsız davranışa (undefined behavior) yol açabilir.
    pub unsafe fn load_plugin<P: AsRef<Path>>(&mut self, path: P) -> anyhow::Result<Box<dyn GamePlugin>> {
        let path_ref = path.as_ref();
        tracing::info!("Native dynamic plugin yükleniyor: {:?}", path_ref);

        if !path_ref.exists() {
            return Err(anyhow::anyhow!("Eklenti dosyası bulunamadı: {:?}", path_ref));
        }

        // Kütüphaneyi hafızaya yükle
        let library = unsafe { Library::new(path_ref) }
            .map_err(|e| anyhow::anyhow!("Dinamik kütüphane yüklenemedi ({:?}): {:?}", path_ref, e))?;

        // Entry-point fonksiyonunu al.
        // Bu fonksiyon eklenti tarafında `#[no_mangle] pub extern "C" fn _strata_plugin_create() -> *mut dyn GamePlugin` şeklinde tanımlanmalıdır.
        let constructor: Symbol<unsafe extern "C" fn() -> *mut dyn GamePlugin> = unsafe { library.get(b"_strata_plugin_create") }
            .map_err(|e| anyhow::anyhow!("Dinamik entry-point '_strata_plugin_create' bulunamadı: {:?}", e))?;

        // Eklenti nesnesini oluştur
        let raw_plugin_ptr = unsafe { constructor() };
        if raw_plugin_ptr.is_null() {
            return Err(anyhow::anyhow!("Dinamik eklenti oluşturucu null pointer döndürdü"));
        }

        // Raw pointer'ı Box'a dönüştür
        let plugin = unsafe { Box::from_raw(raw_plugin_ptr) };

        // Kütüphane referansını sakla (böylece eklenti yaşadığı sürece kütüphane hafızadan atılmaz)
        self.loaded_libraries.push(library);

        tracing::info!(
            "Native eklenti başarıyla yüklendi: {} (v{})",
            plugin.name(),
            plugin.version()
        );

        Ok(plugin)
    }
}
