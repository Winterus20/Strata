//! M11d migration tests (plan 15 §38 §2).

use strata_save::envelope::SaveEnvelope;
use strata_save::migration::{CURRENT_SAVE_VERSION, migrate};
use strata_save::world_metadata::WorldMetadata;

#[test]
fn migration_chain_no_op_at_current() {
    let meta = WorldMetadata {
        seed: 12345,
        spawn_point: [0.0, 64.0, 0.0],
        time_played: 1,
        world_version: 1,
        generator_version: 1,
        last_modified: 0,
    };
    let env = SaveEnvelope::pack(
        CURRENT_SAVE_VERSION,
        1,
        &postcard::to_allocvec(&meta).unwrap(),
    )
    .unwrap();

    let migrated = migrate(&env).unwrap();
    assert_eq!(migrated.save_version, CURRENT_SAVE_VERSION);
    // Already at current → identity: payload is byte-identical.
    assert_eq!(migrated.payload, env.payload);
    let loaded: WorldMetadata = migrated.decode().unwrap();
    assert_eq!(loaded, meta);
}
