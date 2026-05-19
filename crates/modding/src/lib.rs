use bevy_app::prelude::*;
use bevy_ecs::observer::On;
use bevy_ecs::prelude::*;
use strata_ecs::components::interaction::{BlockBreakEvent, BlockPlaceEvent};
use strata_ecs::components::player::Player;
use strata_ecs::components::position::Position;

pub mod loader;
pub mod runtime;
pub mod sandbox;

pub use loader::{ModManager, WasmModInstance};

/// Bevy Resource wrapper for ModManager.
#[derive(Resource)]
pub struct ModManagerResource {
    pub manager: ModManager,
}

// Wasmtime Store ve dynamic objects aren't Sync, but Bevy sequential systems borrowing ResMut guarantees thread safety.
unsafe impl Send for ModManagerResource {}
unsafe impl Sync for ModManagerResource {}

/// Strata Modlama sistemini Bevy motoruna entegre eden Bevy Plugin'i.
pub struct ModdingPlugin {
    pub mods_dir: std::path::PathBuf,
}

impl Default for ModdingPlugin {
    fn default() -> Self {
        Self {
            mods_dir: std::path::PathBuf::from("assets/mods"),
        }
    }
}

impl Plugin for ModdingPlugin {
    fn build(&self, app: &mut App) {
        // ModManager'ı başlat ve Bevy Resource olarak ekle
        let mut manager = ModManager::new(&self.mods_dir);
        if let Err(e) = manager.load_all_mods() {
            tracing::error!("Modlar yüklenirken kritik hata: {:?}", e);
        }

        app.insert_resource(ModManagerResource { manager })
            // Blok kırma ve yerleştirme olayları için observer'ları (dinleyicileri) ekle
            .add_observer(on_block_place_event)
            .add_observer(on_block_break_event)
            // Her tick'te çalışan mod güncelleme sistemini ekle
            .add_systems(Update, player_tick_system);
    }
}

/// Observer: Blok yerleştirildiğinde Wasm modlarına duyur.
fn on_block_place_event(
    trigger: On<BlockPlaceEvent>,
    mut mod_manager: ResMut<ModManagerResource>,
) {
    let event = trigger.event();
    let pos = event.position.0;
    mod_manager.manager.broadcast_block_placed(pos.x, pos.y, pos.z, event.block_id);
}

/// Observer: Blok kırıldığında Wasm modlarına duyur.
fn on_block_break_event(
    trigger: On<BlockBreakEvent>,
    mut mod_manager: ResMut<ModManagerResource>,
) {
    let event = trigger.event();
    let pos = event.0.0;
    // Blok kırıldığında ID'si 0 (Air) olarak modlara iletilir
    mod_manager.manager.broadcast_block_broken(pos.x, pos.y, pos.z, 0);
}

/// System: Her tick'te tüm oyuncuların pozisyonlarını modlara bildirir.
fn player_tick_system(
    mut mod_manager: ResMut<ModManagerResource>,
    query: Query<(Entity, &Position), With<Player>>,
) {
    for (entity, pos) in query.iter() {
        let player_id = entity.to_bits();
        let p_pos = pos.0;
        mod_manager.manager.broadcast_player_tick(player_id, p_pos.x, p_pos.y, p_pos.z);
    }
}
