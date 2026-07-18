//! M9a streaming tests: load/unload, hysteresis, determinism, and the
//! streaming -> worldgen end-to-end chain (headless, no rendering).

use std::collections::HashSet;

use bevy::prelude::*;

use strata_core::prelude::*;
use strata_core::registry::load_block_registry;

use crate::plugin::{Generated, Generating, PendingWorldGen, WorldGenPlugin};
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
        if after.len() == 729 && !after.contains(&SectorCoord(0, 0, 0)) {
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

    // Jitter the player by exactly one sector (+X) and back.
    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::new(32.0, 0.0, 0.0)));
    for _ in 0..120 {
        app.update();
        if coords(&mut app).len() == 729 {
            break;
        }
    }
    let mid = coords(&mut app);
    assert_eq!(mid.len(), 729, "count must stay bounded during jitter");

    app.world_mut()
        .entity_mut(player)
        .insert(Transform::from_translation(Vec3::ZERO));
    for _ in 0..120 {
        app.update();
        if coords(&mut app).len() == 729 {
            break;
        }
    }

    let back: HashSet<SectorCoord> = coords(&mut app).into_iter().collect();
    assert_eq!(
        back, base,
        "resident set must return to its pre-jitter state (no flapping)"
    );
    assert_eq!(
        brick_count(&app),
        base_bricks,
        "pool brick count must return to steady state after jitter"
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
    let expected = parked_brick_count(SectorCoord(20, 0, 0), 250);
    assert_eq!(
        after_bricks, expected,
        "pool bricks after move must equal a fresh app at the same position (no leak)"
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
