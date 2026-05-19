use wasmtime::{Config, Engine, ResourceLimiter};

/// Wasm modlarının hafıza ve tablo boyutu sınırlarını yöneten sınırlayıcı.
pub struct SandboxLimits {
    /// Modun kullanabileceği maksimum linear hafıza boyutu (byte cinsinden)
    pub max_memory: usize,
    /// Modun tablosunda barındırabileceği maksimum eleman sayısı
    pub max_table_elements: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_memory: 64 * 1024 * 1024, // 64 MB varsayılan sınır
            max_table_elements: 10000,
        }
    }
}

impl ResourceLimiter for SandboxLimits {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if desired > self.max_memory {
            tracing::warn!(
                "Wasm Mod hafıza sınırı aşılmaya çalışıldı! İstenen: {} MB, Sınır: {} MB",
                desired / (1024 * 1024),
                self.max_memory / (1024 * 1024)
            );
            return Ok(false);
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(desired <= self.max_table_elements)
    }
}

/// Modlama sistemi için optimize edilmiş Wasmtime Engine oluşturur.
pub fn create_wasm_engine() -> Engine {
    let mut config = Config::new();
    // Component modelini aktif et
    config.wasm_component_model(true);
    // Güvenlik ve kaynak yönetimi için fuel (yakıt) tüketimini aktif et
    config.consume_fuel(true);
    // Asenkron mod yürütme için epoch interruption'ı aktif et
    config.epoch_interruption(true);

    Engine::new(&config).expect("Wasmtime motoru oluşturulamadı")
}
