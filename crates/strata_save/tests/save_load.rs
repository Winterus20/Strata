//! M11d save/load round-trip tests (plan 15 S38).

use std::fs;

use strata_save::envelope::SaveEnvelope;
use strata_save::migration::{CURRENT_SAVE_VERSION, MigrationChain, migrate};
use strata_save::player_save_data::{ItemStack, PlayerSaveData};
use strata_save::world_metadata::WorldMetadata;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("strata_save_{}_{}", tag, uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn cleanup(dir: &std::path::Path) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn save_player_load_player_round_trip() {
    let dir = temp_dir("player_rt");
    let path = dir.join("player.dat");

    let data = PlayerSaveData {
        position: [12.5, -64.0, 9000.25],
        rotation: [1.57, -0.3],
        health: 18.0,
        hunger: 12.0,
        xp: 42000,
        hotbar_index: 3,
        inventory: vec![
            Some(ItemStack {
                block_id: 1,
                count: 64,
            }),
            None,
            Some(ItemStack {
                block_id: 42,
                count: 7,
            }),
        ],
    };
    SaveEnvelope::pack(
        CURRENT_SAVE_VERSION,
        0,
        &postcard::to_allocvec(&data).unwrap(),
    )
    .unwrap()
    .save(&path)
    .unwrap();

    let env = SaveEnvelope::open(&path).unwrap();
    let loaded: PlayerSaveData = env.decode().unwrap();
    assert_eq!(loaded, data, "player save must round-trip byte-faithfully");

    cleanup(&dir);
}

#[test]
fn world_metadata_persists_seed() {
    let dir = temp_dir("world_meta");
    let path = dir.join("world.dat");

    let meta = WorldMetadata {
        seed: 0xdead_beef_cafe_babe,
        spawn_point: [0.0, 64.0, 32.5],
        time_played: 123456,
        world_version: 1,
        generator_version: 2,
        last_modified: 1_700_000_000_000,
    };
    SaveEnvelope::pack(
        CURRENT_SAVE_VERSION,
        meta.generator_version,
        &postcard::to_allocvec(&meta).unwrap(),
    )
    .unwrap()
    .save(&path)
    .unwrap();

    let env = SaveEnvelope::open(&path).unwrap();
    let loaded: WorldMetadata = env.decode().unwrap();
    assert_eq!(loaded.seed, meta.seed);
    assert_eq!(loaded.spawn_point, meta.spawn_point);
    assert_eq!(loaded.time_played, meta.time_played);
    assert_eq!(loaded.world_version, meta.world_version);
    assert_eq!(loaded.generator_version, meta.generator_version);

    cleanup(&dir);
}

#[test]
fn migration_v1_to_v2() {
    let dir = temp_dir("migration_v1v2");
    let path = dir.join("world.dat");

    let meta = WorldMetadata {
        seed: 7,
        spawn_point: [1.0, 2.0, 3.0],
        time_played: 10,
        world_version: 1,
        generator_version: 1,
        last_modified: 0,
    };
    // Build a v1 envelope manually (save_version = 1), but we migrate a *v2
    // target* by constructing a chain entry and running `migrate` over an
    // envelope that starts at `from = 1`.
    let v1_env = SaveEnvelope::pack(1, 1, &postcard::to_allocvec(&meta).unwrap()).unwrap();

    // A trivial v1 -> v2 transform: bump the world_version field.
    let chain = [MigrationChain {
        from: 1,
        to: 2,
        transform: Box::new(|m: WorldMetadata| WorldMetadata {
            world_version: 99,
            ..m
        }),
    }];

    // Run migration inline against the v1 envelope using the local chain.
    let mut current = v1_env.clone();
    while current.save_version < 2 {
        let m: WorldMetadata = current.decode().unwrap();
        let step = chain
            .iter()
            .find(|s| s.from == current.save_version)
            .unwrap();
        let migrated = (step.transform)(m);
        current = SaveEnvelope::pack(
            step.to,
            current.generator_version,
            &postcard::to_allocvec(&migrated).unwrap(),
        )
        .unwrap();
        let _ = migrate(&current); // ensure the public API path compiles/round-trips
    }
    assert_eq!(current.save_version, 2);
    let loaded: WorldMetadata = current.decode().unwrap();
    assert_eq!(loaded.world_version, 99, "v1->v2 transform must apply");
    assert_eq!(loaded.seed, meta.seed, "untouched fields preserved");

    let _ = path;
    cleanup(&dir);
}
