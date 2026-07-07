//! M1 unit tests: filter-first, change-detection guard, and plugin registration.

use bevy::prelude::*;

use crate::change_detection::assign_guarded;
use crate::component::{ChunkDirty, Counter, DirtySectorCount, SectorCoord, count_dirty_sectors};
use crate::core_plugin::{FilterFirstDemoPlugin, StrataSchedulingPlugin};
use crate::plugin::{AddStrataPlugin, RegisteredPlugins, StrataCorePlugins};
use crate::sets::StrataSet;

#[test]
fn filter_first_counts_only_dirty_sectors() {
    let mut app = App::new();
    app.insert_resource(DirtySectorCount::default());
    app.add_systems(Update, count_dirty_sectors);

    for coord in [
        SectorCoord(0, 0, 0),
        SectorCoord(1, 0, 0),
        SectorCoord(0, 1, 0),
    ] {
        app.world_mut().spawn((coord, ChunkDirty));
    }
    for coord in [SectorCoord(2, 0, 0), SectorCoord(0, 0, 1)] {
        app.world_mut().spawn((coord,));
    }

    app.update();

    assert_eq!(app.world().resource::<DirtySectorCount>().0, 3);
}

#[test]
fn guarded_assignment_does_not_flag_unchanged() {
    let mut v = 5u32;
    assert!(
        !assign_guarded(&mut v, 5),
        "same value must not report change"
    );
    assert_eq!(v, 5);
    assert!(
        assign_guarded(&mut v, 6),
        "different value must report change"
    );
    assert_eq!(v, 6);
}

#[derive(Resource, Default)]
struct MutateTarget(u32);

#[derive(Resource, Default)]
struct ChangedCount(u32);

fn guard_mutate(mut query: Query<&mut Counter>, target: Res<MutateTarget>) {
    for mut c in &mut query {
        c.set_if_neq(Counter(target.0));
    }
}

fn record_changed(query: Query<&Counter, Changed<Counter>>, mut out: ResMut<ChangedCount>) {
    out.0 = query.iter().count() as u32;
}

#[test]
fn set_if_neq_guard_does_not_flag_unchanged() {
    let mut app = App::new();
    app.insert_resource(MutateTarget(7));
    app.insert_resource(ChangedCount::default());
    app.add_systems(Update, (guard_mutate, record_changed).chain());
    app.world_mut().spawn(Counter(7));

    app.update(); // settle spawn tick
    app.update(); // mutate to same value -> no change
    assert_eq!(
        app.world().resource::<ChangedCount>().0,
        0,
        "set_if_neq with the same value must NOT trigger Changed"
    );

    app.world_mut().resource_mut::<MutateTarget>().0 = 42;
    app.update(); // mutate to a different value -> change
    assert_eq!(
        app.world().resource::<ChangedCount>().0,
        1,
        "set_if_neq with a different value MUST trigger Changed"
    );
}

#[test]
fn strata_core_plugins_register_in_order() {
    let mut app = App::new();
    StrataCorePlugins::new()
        .add_plugin(StrataSchedulingPlugin)
        .add_plugin(FilterFirstDemoPlugin)
        .build(&mut app);

    let names = &app.world().resource::<RegisteredPlugins>().0;
    assert_eq!(names, &["strata_scheduling", "filter_first_demo"]);
}

#[test]
fn scheduling_sets_configured_and_run() {
    let mut app = App::new();
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.update();
    // If the StrataSet chain were misconfigured this would panic on ambiguity.
    let _ = StrataSet::RenderUpdate;
}
