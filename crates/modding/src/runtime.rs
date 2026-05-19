use std::collections::HashMap;
use wasmtime::component::ResourceTable;

// WIT interface dosyasını oku ve otomatik Rust bağlayıcılarını üret
wasmtime::component::bindgen!({
    path: "wit/strata_api.wit",
    world: "strata-mod",
});

/// Host (motor) tarafında her Wasm mod instance'ı için tutulacak olan state yapısı.
pub struct ModState {
    /// Sandbox kaynak limit kontrolcüsü
    pub limits: crate::sandbox::SandboxLimits,
    /// Opaque resource tablosu (Wasmtime gereksinimi)
    pub table: ResourceTable,
    /// Mod tarafından kayıt edilen blokların yerel eşleşme tablosu
    pub block_names: HashMap<u16, String>,
}

impl ModState {
    /// Yeni bir ModState oluşturur.
    pub fn new(limits: crate::sandbox::SandboxLimits) -> Self {
        Self {
            limits,
            table: ResourceTable::new(),
            block_names: HashMap::new(),
        }
    }
}

// WIT içindeki import block-registry arayüzünün host tarafındaki implementasyonu
impl strata::engine_api::block_registry::Host for ModState {
    fn register_block(
        &mut self,
        properties: strata::engine_api::block_registry::BlockProperties,
    ) -> u16 {
        let name = properties.name.clone();
        // ID ataması yap (Gerçek senaryoda bu host'un block registry'sine gider)
        let next_id = (self.block_names.len() + 1000) as u16; // Mod blokları 1000'den başlasın
        self.block_names.insert(next_id, name.clone());

        tracing::info!(
            "Wasm Mod üzerinden yeni blok başarıyla kaydedildi: {} (ID: {})",
            name,
            next_id
        );
        next_id
    }

    fn get_block_name(&mut self, id: u16) -> String {
        if let Some(name) = self.block_names.get(&id) {
            name.clone()
        } else {
            "Hava / Bilinmeyen".to_string()
        }
    }
}

// WIT içindeki import entity-control arayüzünün host tarafındaki implementasyonu
impl strata::engine_api::entity_control::Host for ModState {
    fn spawn_entity(
        &mut self,
        entity_type: u16,
        position: strata::engine_api::entity_control::Vec3,
    ) -> u32 {
        tracing::info!(
            "Wasm Mod entity spawn tetikledi: Type {} @ ({:.2}, {:.2}, {:.2})",
            entity_type,
            position.x,
            position.y,
            position.z
        );
        // Örnek ID dön
        100 + entity_type as u32
    }

    fn set_entity_position(
        &mut self,
        entity_id: u32,
        position: strata::engine_api::entity_control::Vec3,
    ) {
        tracing::debug!(
            "Wasm Mod entity pozisyonunu güncelledi: ID {} -> ({:.2}, {:.2}, {:.2})",
            entity_id,
            position.x,
            position.y,
            position.z
        );
    }
}
