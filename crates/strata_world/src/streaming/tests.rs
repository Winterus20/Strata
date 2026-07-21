//! M9a streaming tests: load/unload, hysteresis, determinism, and the
//! streaming -> worldgen end-to-end chain (headless, no rendering).

use std::collections::HashSet;

use bevy::prelude::*;

use bevy_ecs::system::RunSystemOnce;
use strata_core::prelude::*;
use strata_core::registry::load_block_registry;
use strata_storage::backend::AsyncStorageBackend;

use crate::plugin::{
    Generated, Generating, LOAD_SPAWN_BUDGET, LoadFailed, Loading, PendingSectorLoad,
    PendingWorldGen, WorldGenPlugin,
};
use crate::streaming::{StreamingManager, StreamingPlugin};

/// Configure the full `StrataSet` chain so the `Streaming` set runs before
/// `WorldGen` within the same frame (deterministic ordering for tests).
fn configure_chain(app: &mut App) {
    app.configure_sets(
        Update,
        (
            StrataSet::Streaming,
            StrataSet::Input,
            StrataSet::WorldGen,
            StrataSet::Meshing,
            StrataSet::Physics,
            StrataSet::Lighting,
            StrataSet::RenderUpdate,
        )
            .chain(),
    );
}

/// Dump of every living sector coordinate, collected via a one-shot system so we
/// avoid holding conflicting borrows of `World` while counting.
#[derive(Resource, Default)]
struct SectorDump {
    coords: Vec<SectorCoord>,
    bricks: usize,
}

fn dump_system(
    sectors: Query<&SectorCoord>,
    pool: Res<GlobalBrickPool>,
    mut dump: ResMut<SectorDump>,
) {
    dump.coords = sectors.iter().copied().collect();
    dump.bricks = pool.brick_count();
}

/// Build an app with the streaming plugin (no player entity → origin).
fn streaming_app() -> App {
    let mut app = App::new();
    configure_chain(&mut app);
    app.add_strata_plugin(StreamingPlugin::default());
    app.insert_resource(SectorDump::default());
    // Run the dump AFTER streaming (last set in the chain) so it observes the
    // sectors spawned this frame.
    app.add_systems(Update, dump_system.in_set(StrataSet::RenderUpdate));
    app
}

fn coords(app: &mut App) -> Vec<SectorCoord> {
    app.world().resource::<SectorDump>().coords.to_vec()
}

fn brick_count(app: &App) -> usize {
    app.world().resource::<SectorDump>().bricks
}

#[test]
fn player_at_origin_spawns_expected_sector_count() {
    let mut app = streaming_app();
    app.update();

    // radius 3 + hysteresis 1 = effective 4 → (2*4+1)^3 = 729 sectors.
    let cs = coords(&mut app);
    assert_eq!(cs.len(), 729, "expected full (R+H) cube of sectors");

    // Every resident sector must be within radius+hysteresis of the origin.
    let mgr = app.world().resource::<StreamingManager>();
    let eff = mgr.effective_radius();
    for c in &cs {
        assert!(
            crate::streaming::chebyshev(*c, SectorCoord(0, 0, 0)) <= eff,
            "sector {c:?} exceeds effective radius {eff}"
        );
    }
}

#[test]
fn determinism_same_player_sector_same_resident_set() {
    let mut app = streaming_app();
    app.update();
    let a: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();

    let mut app2 = streaming_app();
    app2.update();
    let b: HashSet<SectorCoord> = coords(&mut app2).into_iter().collect();

    assert_eq!(a, b, "same player sector must yield identical resident set");
}

#[test]
fn move_player_unloads_distant_and_loads_near_sectors() {
    let mut app = streaming_app();
    // Spawn a player entity at the origin so the streaming system tracks it.
    let player = app
        .world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)))
        .id();
    app.update();
    let start: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();
    assert!(start.contains(&SectorCoord(0, 0, 0)));
    assert!(start.len() == 729);

    // Teleport the player ~10 sectors away along +X.
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(
            10.0 * 32.0,
            0.0,
            0.0,
        )));
    app.update();

    for _ in 0..120 {
        app.update();
        let after: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();
        // 729 = (R+H) ball; +1 when predictive prefetch sits outside the ball.
        if (after.len() == 729 || after.len() == 730) && !after.contains(&SectorCoord(0, 0, 0)) {
            assert!(
                after.contains(&SectorCoord(10, 0, 0)),
                "new player sector must be loaded"
            );
            return;
        }
    }
    panic!("origin must unload and resident set must converge after move");
}

#[test]
fn hysteresis_jitter_keeps_resident_set_stable() {
    let mut app = streaming_app();
    let player = app
        .world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)))
        .id();
    app.update();
    let base: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();
    let base_bricks = brick_count(&app);
    assert_eq!(base.len(), 729);
    let eff = app
        .world()
        .resource::<StreamingManager>()
        .effective_radius();

    // Jitter the player by exactly one sector (+X) and back.
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(32.0, 0.0, 0.0)));
    for _ in 0..120 {
        app.update();
        let n = coords(&mut app).len();
        // +1 allowed for predictive prefetch outside the Chebyshev ball.
        if n == 729 || n == 730 {
            break;
        }
    }
    let mid = coords(&mut app);
    assert!(
        mid.len() == 729 || mid.len() == 730,
        "count must stay bounded during jitter, got {}",
        mid.len()
    );

    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::ZERO));
    for _ in 0..120 {
        app.update();
        let n = coords(&mut app).len();
        if n == 729 || n == 730 {
            break;
        }
    }

    let back: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();
    // Prefetch may leave one extra sector outside the ball; the ball itself
    // must match the pre-jitter resident set.
    let back_ball: HashSet<SectorCoord> = back
        .iter()
        .copied()
        .filter(|c| crate::streaming::chebyshev(*c, SectorCoord(0, 0, 0)) <= eff)
        .collect();
    assert_eq!(
        back_ball, base,
        "resident ball must return to its pre-jitter state (no flapping)"
    );
    // Brick count may include one prefetch sector's bricks.
    assert!(
        brick_count(&app) >= base_bricks,
        "pool must not shrink below pre-jitter steady state"
    );
}

#[test]
fn integration_streaming_spawns_generated_sectors() {
    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(StreamingPlugin::default());
    app.add_strata_plugin(WorldGenPlugin);

    for _ in 0..2000 {
        let has_gen = {
            let w = app.world_mut();
            if let Some(origin_entity) = w
                .query::<(Entity, &SectorCoord)>()
                .iter(w)
                .find(|(_, c)| **c == SectorCoord(0, 0, 0))
                .map(|(e, _)| e)
            {
                w.entity(origin_entity).contains::<Generated>()
            } else {
                false
            }
        };
        if has_gen {
            break;
        }
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // The origin sector (spawned frame 1) must now be Generated + have a map.
    let origin = {
        let w = app.world_mut();
        w.query::<(Entity, &SectorCoord)>()
            .iter(w)
            .find(|(_, c)| **c == SectorCoord(0, 0, 0))
            .map(|(e, _)| e)
    };
    let e = origin.expect("origin sector must exist after streaming");
    let entity = app.world().entity(e);
    assert!(
        entity.contains::<Generated>(),
        "streamed sector must be Generated by WorldGen"
    );
    assert!(
        entity.contains::<XBrickMap>(),
        "streamed sector must have its XBrickMap"
    );

    // Pool must hold bricks (terrain was generated) and stay bounded.
    let bricks = app.world().resource::<GlobalBrickPool>().brick_count();
    assert!(bricks > 0, "generated terrain must allocate pooled bricks");
}

fn parked_brick_count(sector: SectorCoord, _n: usize) -> usize {
    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(StreamingPlugin::default());
    app.add_strata_plugin(WorldGenPlugin);
    app.world_mut().spawn((
        StreamingAnchor,
        Transform::from_translation(Vec3::new(
            sector.0 as f32 * 32.0,
            sector.1 as f32 * 32.0,
            sector.2 as f32 * 32.0,
        )),
    ));
    for _ in 0..2000 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let pending_empty = app.world().resource::<PendingWorldGen>().tasks.is_empty();
        let generating_empty = app
            .world_mut()
            .query_filtered::<Entity, With<Generating>>()
            .iter(app.world_mut())
            .next()
            .is_none();
        let any_unscaled = app
            .world_mut()
            .query_filtered::<Entity, (With<SectorCoord>, Without<Generated>, Without<Generating>)>(
            )
            .iter(app.world_mut())
            .next()
            .is_some();
        let has_bricks = app.world().resource::<GlobalBrickPool>().brick_count() > 0;
        if pending_empty && generating_empty && !any_unscaled && has_bricks {
            break;
        }
    }
    app.world().resource::<GlobalBrickPool>().brick_count()
}

#[test]
fn integration_unload_frees_pool_bricks() {
    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(StreamingPlugin::default());
    app.add_strata_plugin(WorldGenPlugin);

    let player = app
        .world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)))
        .id();
    for _ in 0..2000 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let pending_empty = app.world().resource::<PendingWorldGen>().tasks.is_empty();
        let generating_empty = app
            .world_mut()
            .query_filtered::<Entity, With<Generating>>()
            .iter(app.world_mut())
            .next()
            .is_none();
        let any_unscaled = app
            .world_mut()
            .query_filtered::<Entity, (With<SectorCoord>, Without<Generated>, Without<Generating>)>(
            )
            .iter(app.world_mut())
            .next()
            .is_some();
        let has_bricks = app.world().resource::<GlobalBrickPool>().brick_count() > 0;
        if pending_empty && generating_empty && !any_unscaled && has_bricks {
            break;
        }
    }
    let before_bricks = app.world().resource::<GlobalBrickPool>().brick_count();
    assert!(
        before_bricks > 0,
        "sectors must own pooled bricks before move"
    );

    // Move far away: origin sectors unload and free their bricks.
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(
            20.0 * 32.0,
            0.0,
            0.0,
        )));
    // Enough frames for the budget to unload the old shell and drain the new
    // one, so the pool converges to the same steady state as a fresh app.
    for _ in 0..2000 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let pending_empty = app.world().resource::<PendingWorldGen>().tasks.is_empty();
        let generating_empty = app
            .world_mut()
            .query_filtered::<Entity, With<Generating>>()
            .iter(app.world_mut())
            .next()
            .is_none();
        let any_unscaled = app
            .world_mut()
            .query_filtered::<Entity, (With<SectorCoord>, Without<Generated>, Without<Generating>)>(
            )
            .iter(app.world_mut())
            .next()
            .is_some();
        if pending_empty && generating_empty && !any_unscaled {
            break;
        }
    }
    let after_bricks = app.world().resource::<GlobalBrickPool>().brick_count();

    // The unloaded origin bricks must be fully reclaimed: the moved app's pool
    // must equal a fresh app parked at the same final position (no leak).
    // Predictive prefetch may add one extra resident sector vs a stationary park.
    let expected = parked_brick_count(SectorCoord(20, 0, 0), 250);
    let delta = after_bricks.abs_diff(expected);
    assert!(
        delta <= 2048,
        "pool bricks after move must ≈ fresh park (prefetch ±1 sector); after={after_bricks} expected={expected}"
    );
    assert!(
        after_bricks < before_bricks + expected,
        "pool must not accumulate both old and new bricks (before={before_bricks}, after={after_bricks})"
    );
    let origin_present = {
        let w = app.world_mut();
        w.query::<&SectorCoord>()
            .iter(w)
            .any(|c| *c == SectorCoord(0, 0, 0))
    };
    assert!(!origin_present, "origin must be unloaded");
}

#[test]
fn streaming_unload_dirty_sector_triggers_flush() {
    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(StreamingPlugin::default());
    app.add_strata_plugin(WorldGenPlugin);

    // Setup SavePlugin requirements
    app.insert_resource(strata_save::plugin::DirtyQueue::default());
    app.init_resource::<bevy_ecs::message::Messages<strata_save::plugin::SectorSave>>();

    let player = app
        .world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)))
        .id();

    // Wait for terrain generator to finish generating origin sectors
    for _ in 0..2000 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let pending_empty = app.world().resource::<PendingWorldGen>().tasks.is_empty();
        let generating_empty = app
            .world_mut()
            .query_filtered::<Entity, With<Generating>>()
            .iter(app.world_mut())
            .next()
            .is_none();
        let any_unscaled = app
            .world_mut()
            .query_filtered::<Entity, (With<SectorCoord>, Without<Generated>, Without<Generating>)>(
            )
            .iter(app.world_mut())
            .next()
            .is_some();
        let has_bricks = app.world().resource::<GlobalBrickPool>().brick_count() > 0;
        if pending_empty && generating_empty && !any_unscaled && has_bricks {
            break;
        }
    }

    // Mark origin as dirty
    let coord = SectorCoord(0, 0, 0);
    {
        let dq = app.world().resource::<strata_save::plugin::DirtyQueue>();
        dq.tracker.mark_dirty(coord);
    }

    // Move player far away so origin unloads
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(
            20.0 * 32.0,
            0.0,
            0.0,
        )));

    for _ in 0..200 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    // Verify SectorSave event was triggered
    let emitted = app.world_mut().run_system_once(
        |mut reader: MessageReader<strata_save::plugin::SectorSave>| {
            reader.read().map(|e| e.0).collect::<Vec<SectorCoord>>()
        },
    );
    assert!(
        emitted.unwrap().contains(&coord),
        "dirty sector must trigger SectorSave on unload"
    );
}

#[tokio::test]
async fn streaming_load_existing_sector_skips_regen() {
    let dir = std::env::temp_dir().join(format!(
        "strata_load_test_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let backend = strata_storage::backend::TokioBackend::new(dir.clone()).unwrap();
    let meta_store = strata_storage::metadata::FjallMetadata::open(&dir.join("metadata")).unwrap();
    let save_manager = strata_save::save_manager::SaveManager::new(
        std::sync::Arc::new(meta_store),
        std::time::Duration::from_secs(300),
    );

    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());

    // Radius 0: only the origin sector — avoids a 200+ load-task backlog that
    // starved the target sector under parallel `cargo test` load.
    app.add_strata_plugin(StreamingPlugin::new(0, 0));
    app.add_strata_plugin(WorldGenPlugin);

    app.insert_resource(strata_save::plugin::SaveBackend(backend.clone()));
    app.insert_resource(save_manager.clone());

    let coord = SectorCoord(0, 0, 0);
    let mock_data = std::sync::Arc::new(CompressedChunkData {
        coord: [0, 0, 0],
        sector_mask: 0,
        palette: vec![strata_core::registry::BlockId::AIR],
        bricks: Vec::new(),
    });
    let payload = postcard::to_allocvec(&*mock_data).unwrap();
    let payload_hash = blake3::hash(&payload).into();

    backend
        .write_sector_with_priority(
            coord,
            payload.clone(),
            strata_storage::backend::priority::ACTIVE,
        )
        .await
        .unwrap();

    let meta = strata_storage::metadata::SectorMetadata {
        coord,
        hash: payload_hash,
        size: payload.len() as u64,
        mtime: 12345,
        tier: 0,
        version: 1,
        dirty: false,
    };
    save_manager.metadata.put(meta).await.unwrap();

    app.world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)));

    for _ in 0..200 {
        app.update();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let w = app.world_mut();
        let entity = w
            .query::<(Entity, &SectorCoord)>()
            .iter(w)
            .find(|(_, c)| **c == coord)
            .map(|(e, _)| w.entity(e));

        if let Some(entity) = entity {
            let is_ready = entity.contains::<Generated>() && entity.contains::<SectorSnapshot>();
            if is_ready {
                assert!(
                    !entity.contains::<Generating>(),
                    "loaded sector must not trigger PCG"
                );
                std::fs::remove_dir_all(&dir).ok();
                return;
            }
            if entity.contains::<LoadFailed>() {
                std::fs::remove_dir_all(&dir).ok();
                panic!("sector LoadFailed (disk/hash/deserialize/unpack)");
            }
        }
    }

    let w = app.world_mut();
    let diag = w
        .query::<(Entity, &SectorCoord)>()
        .iter(w)
        .find(|(_, c)| **c == coord)
        .map(|(e, _)| {
            let ent = w.entity(e);
            format!(
                "generated={} snapshot={} loading={} generating={} load_failed={} pending_load={}",
                ent.contains::<Generated>(),
                ent.contains::<SectorSnapshot>(),
                ent.contains::<Loading>(),
                ent.contains::<Generating>(),
                ent.contains::<LoadFailed>(),
                w.resource::<PendingSectorLoad>().tasks.len(),
            )
        });
    std::fs::remove_dir_all(&dir).ok();
    panic!("sector failed to load from disk and apply; diag={diag:?}");
}

#[tokio::test]
async fn corrupt_hash_must_not_fall_through_to_pcg() {
    let dir = std::env::temp_dir().join(format!(
        "strata_corrupt_hash_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let backend = strata_storage::backend::TokioBackend::new(dir.clone()).unwrap();
    let meta_store = strata_storage::metadata::FjallMetadata::open(&dir.join("metadata")).unwrap();
    let save_manager = strata_save::save_manager::SaveManager::new(
        std::sync::Arc::new(meta_store),
        std::time::Duration::from_secs(300),
    );

    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(WorldGenPlugin);
    app.insert_resource(strata_save::plugin::SaveBackend(backend.clone()));
    app.insert_resource(save_manager.clone());

    let coord = SectorCoord(0, 0, 0);
    let mock_data = CompressedChunkData {
        coord: [0, 0, 0],
        sector_mask: 0,
        palette: vec![strata_core::registry::BlockId::AIR],
        bricks: Vec::new(),
    };
    let payload = postcard::to_allocvec(&mock_data).unwrap();

    backend
        .write_sector_with_priority(
            coord,
            payload.clone(),
            strata_storage::backend::priority::ACTIVE,
        )
        .await
        .unwrap();

    // Metadata claims a different hash → integrity failure must fail closed.
    let meta = strata_storage::metadata::SectorMetadata {
        coord,
        hash: [0xAB; 32],
        size: payload.len() as u64,
        mtime: 12345,
        tier: 0,
        version: 1,
        dirty: false,
    };
    save_manager.metadata.put(meta).await.unwrap();

    let e = app.world_mut().spawn(SectorCoord(0, 0, 0)).id();

    for _ in 0..200 {
        app.update();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let entity = app.world().entity(e);
        if entity.contains::<LoadFailed>() {
            assert!(
                !entity.contains::<Generated>(),
                "corrupt load must not mark Generated / fall through to PCG"
            );
            assert!(
                !entity.contains::<Generating>(),
                "corrupt load must not enter PCG Generating"
            );
            assert!(
                !entity.contains::<SectorSnapshot>(),
                "corrupt load must not apply a snapshot"
            );
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        if entity.contains::<Generated>() {
            std::fs::remove_dir_all(&dir).ok();
            panic!("corrupt hash fell through to Generated (PCG or bad load)");
        }
    }

    let entity = app.world().entity(e);
    let loading = entity.contains::<Loading>();
    let pending = app.world().resource::<PendingSectorLoad>().tasks.len();
    std::fs::remove_dir_all(&dir).ok();
    panic!("expected LoadFailed after hash mismatch (loading={loading}, pending={pending})");
}

#[tokio::test]
async fn disk_load_spawn_respects_budget() {
    let dir = std::env::temp_dir().join(format!(
        "strata_load_budget_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    let backend = strata_storage::backend::TokioBackend::new(dir.clone()).unwrap();
    let meta_store = strata_storage::metadata::FjallMetadata::open(&dir.join("metadata")).unwrap();
    let save_manager = strata_save::save_manager::SaveManager::new(
        std::sync::Arc::new(meta_store),
        std::time::Duration::from_secs(300),
    );

    let mock_data = CompressedChunkData {
        coord: [0, 0, 0],
        sector_mask: 0,
        palette: vec![strata_core::registry::BlockId::AIR],
        bricks: Vec::new(),
    };
    let payload = postcard::to_allocvec(&mock_data).unwrap();
    let payload_hash: [u8; 32] = blake3::hash(&payload).into();

    // More sectors than the spawn budget so an uncapped path would queue them all.
    let coords_list: Vec<SectorCoord> = (0..LOAD_SPAWN_BUDGET + 4)
        .map(|i| SectorCoord(i as i32, 0, 0))
        .collect();

    for &coord in &coords_list {
        backend
            .write_sector_with_priority(
                coord,
                payload.clone(),
                strata_storage::backend::priority::ACTIVE,
            )
            .await
            .unwrap();
        save_manager
            .metadata
            .put(strata_storage::metadata::SectorMetadata {
                coord,
                hash: payload_hash,
                size: payload.len() as u64,
                mtime: 1,
                tier: 0,
                version: 1,
                dirty: false,
            })
            .await
            .unwrap();
    }

    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    app.add_strata_plugin(WorldGenPlugin);
    app.insert_resource(strata_save::plugin::SaveBackend(backend));
    app.insert_resource(save_manager);

    for &c in &coords_list {
        app.world_mut().spawn(SectorCoord(c.0, c.1, c.2));
    }

    app.update();

    let pending = app.world().resource::<PendingSectorLoad>().tasks.len();
    let loading: usize = app
        .world_mut()
        .query_filtered::<Entity, With<Loading>>()
        .iter(app.world())
        .count();

    std::fs::remove_dir_all(&dir).ok();

    assert!(
        pending <= LOAD_SPAWN_BUDGET,
        "load spawn must be budgeted: pending={pending} budget={LOAD_SPAWN_BUDGET}"
    );
    assert!(
        loading <= LOAD_SPAWN_BUDGET,
        "Loading markers must match spawn budget: loading={loading}"
    );
}

#[test]
fn unload_cancels_inflight_gen_and_load_tasks() {
    let mut app = App::new();
    configure_chain(&mut app);
    app.insert_resource(load_block_registry());
    // Large enough ball that gen tasks are still in-flight when we teleport.
    app.add_strata_plugin(StreamingPlugin::new(2, 0));
    app.add_strata_plugin(WorldGenPlugin);

    let player = app
        .world_mut()
        .spawn((StreamingAnchor, Transform::from_translation(Vec3::ZERO)))
        .id();
    app.update();

    let pending_before = app.world().resource::<PendingWorldGen>().tasks.len();
    assert!(
        pending_before > 0,
        "expected in-flight worldgen tasks after first frame"
    );

    // Teleport far enough that the origin shell fully leaves the desired set.
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(
            20.0 * 32.0,
            0.0,
            0.0,
        )));

    // Drain unload budget across frames until origin is gone.
    for _ in 0..80 {
        app.update();
        let still_has_origin = app
            .world()
            .resource::<StreamingManager>()
            .is_resident(&SectorCoord(0, 0, 0));
        if !still_has_origin {
            break;
        }
    }

    assert!(
        !app.world()
            .resource::<StreamingManager>()
            .is_resident(&SectorCoord(0, 0, 0)),
        "origin must unload after teleport"
    );

    let pending_gen = &app.world().resource::<PendingWorldGen>().tasks;
    let pending_load = &app.world().resource::<PendingSectorLoad>().tasks;
    assert!(
        !pending_gen.contains_key(&SectorCoord(0, 0, 0)),
        "unload must drop PendingWorldGen task for despawned sector"
    );
    assert!(
        !pending_load.contains_key(&SectorCoord(0, 0, 0)),
        "unload must drop PendingSectorLoad task for despawned sector"
    );
}

#[test]
fn predictive_prefetch_expands_beyond_chebyshev_ball() {
    let mut mgr = StreamingManager::new(2, 0);
    mgr.move_dir = SectorCoord(1, 0, 0);
    let player = SectorCoord(0, 0, 0);
    let set = mgr.desired_resident_set(player);
    let r = mgr.effective_radius();
    let prefetch = SectorCoord(player.0 + (r + 1), 0, 0);
    assert!(
        set.contains(&prefetch),
        "prefetch must include sector at radius+1 ahead, got r={r} missing {prefetch:?}"
    );
    assert!(
        !set.contains(&SectorCoord(player.0 + (r + 2), 0, 0)),
        "prefetch must not expand further than radius+1"
    );
}
