//! Headless ECS tests: drive break/place through injected `PlayerBreak`/`PlayerPlace`
//! messages (no window / no real key input) and assert sector edits + dirty markers.

use bevy::prelude::*;
use strata_core::prelude::*;

use strata_player::controller::{PlayerController, PlayerLook, PlayerState};
use strata_player::input::PlayerInput;
use strata_player::interaction::{
    PlayerBreak, PlayerPlace, player_break_system, player_place_system,
};
use strata_player::inventory::{Inventory, hotbar_system};

fn build_app() -> (App, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GlobalBrickPool::new());
    app.insert_resource(PlayerInput::default());
    app.add_message::<PlayerBreak>();
    app.add_message::<PlayerPlace>();
    app.add_systems(
        Update,
        (player_break_system, player_place_system, hotbar_system),
    );

    // Sector (0,0,0) with one solid block at local (5,7,3).
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(5, 7, 3),
        BlockId(1),
    );
    let sector = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette))
        .id();

    // Player positioned just outside the +Z face (default yaw=0 looks toward -Z),
    // with the eye (translation + EYE_HEIGHT) on the block's center line.
    let player = app
        .world_mut()
        .spawn((
            PlayerController::default(),
            PlayerState::default(),
            PlayerLook::default(),
            Inventory::default(),
            Transform::from_translation(Vec3::new(5.5, 5.9, 6.5)),
            GlobalTransform::from_translation(Vec3::new(5.5, 5.9, 6.5)),
        ))
        .id();

    // Stash the pool resource that the sector's previously-built map referenced.
    app.world_mut().insert_resource(pool);

    (app, sector, player)
}

#[test]
fn break_message_clears_voxel_and_marks_sector() {
    let (mut app, sector, _) = build_app();
    app.world_mut().write_message(PlayerBreak);
    app.update();

    let pool = app.world().resource::<GlobalBrickPool>();
    let palette = app.world().entity(sector).get::<SectorPalette>().unwrap();
    let map = app.world().entity(sector).get::<XBrickMap>().unwrap();
    assert_eq!(
        map.get_block(pool, palette, VoxelCoord::new(5, 7, 3)),
        BlockId::AIR,
        "break must set the hit voxel to AIR"
    );
    let entity = app.world().entity(sector);
    assert!(entity.contains::<ChunkDirty>(), "sector must be ChunkDirty");
    assert!(
        entity.contains::<NeedsRemesh>(),
        "sector must be NeedsRemesh"
    );
}

#[test]
fn place_message_puts_block_at_face_neighbour() {
    let (mut app, sector, _) = build_app();
    app.world_mut().write_message(PlayerPlace);
    app.update();

    let pool = app.world().resource::<GlobalBrickPool>();
    let palette = app.world().entity(sector).get::<SectorPalette>().unwrap();
    let map = app.world().entity(sector).get::<XBrickMap>().unwrap();
    // Target neighbour is (5,7,4) (hit +Z normal). Inventory default block = BlockId(1).
    assert_eq!(
        map.get_block(pool, palette, VoxelCoord::new(5, 7, 4)),
        BlockId(1),
        "place must put the selected block at hit + normal"
    );
}

#[test]
fn hotbar_next_cycles_active_slot() {
    let (mut app, _, player) = build_app();
    app.world_mut().resource_mut::<PlayerInput>().hotbar_next = true;
    app.update();
    let inv = app.world().entity(player).get::<Inventory>().unwrap();
    assert_eq!(inv.active, 1, "hotbar_next should advance to slot 1");
    // flag consumed
    assert!(!app.world().resource::<PlayerInput>().hotbar_next);
}
