//! Core scheduling plugin: `StrataSet` chain, no-op stubs, and the filter-first demo.

use bevy::prelude::*;

use crate::component::{
    ChunkDirty, DirtySectorCount, SectorCoord, SectorEntity, count_dirty_sectors,
};
use crate::plugin::StrataPlugin;
use crate::sets::StrataSet;

/// Bootstraps the core pipeline ordering and registers placeholder systems.
pub struct StrataSchedulingPlugin;

impl StrataPlugin for StrataSchedulingPlugin {
    fn name(&self) -> &'static str {
        "strata_scheduling"
    }

    fn build(&self, app: &mut App) {
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

        app.add_systems(
            Update,
            (
                stub_streaming.in_set(StrataSet::Streaming),
                stub_input.in_set(StrataSet::Input),
                stub_worldgen.in_set(StrataSet::WorldGen),
                stub_meshing.in_set(StrataSet::Meshing),
                stub_physics.in_set(StrataSet::Physics),
                stub_lighting.in_set(StrataSet::Lighting),
                hello_system.in_set(StrataSet::RenderUpdate),
            ),
        );
    }
}

fn stub_streaming() {}
fn stub_input() {}
fn stub_worldgen() {}
fn stub_meshing() {}
fn stub_physics() {}
fn stub_lighting() {}

fn hello_system() {
    info!("Strata: core pipeline booted (StrataSet chain active)");
}

/// Demonstrates Filter-First: spawns fake sectors, some `ChunkDirty`, and counts
/// only dirty ones via `Query<&SectorCoord, With<ChunkDirty>>`.
pub struct FilterFirstDemoPlugin;

impl StrataPlugin for FilterFirstDemoPlugin {
    fn name(&self) -> &'static str {
        "filter_first_demo"
    }

    fn build(&self, app: &mut App) {
        app.insert_resource(DirtySectorCount::default());
        app.add_systems(Startup, spawn_demo_sectors);
        app.add_systems(Update, count_dirty_sectors);
    }
}

fn spawn_demo_sectors(mut commands: Commands) {
    let dirty = [
        SectorCoord(0, 0, 0),
        SectorCoord(1, 0, 0),
        SectorCoord(0, 1, 0),
    ];
    let clean = [SectorCoord(2, 0, 0), SectorCoord(0, 0, 1)];
    for coord in dirty {
        commands.spawn((coord, ChunkDirty));
    }
    for coord in clean {
        commands.spawn((SectorEntity { coord },));
    }
}
