//! Headless ECS tests: drive break/place through injected `PlayerBreak`/`PlayerPlace`
//! messages (no window / no real key input) and assert sector edits + dirty markers.

use bevy::prelude::*;
use strata_core::component::SectorSnapshot;
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
    app.insert_resource(load_block_registry());
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
    )
    .expect("test set_block");
    let snapshot = SectorSnapshot(std::sync::Arc::new(map.pack(&pool, &palette).expect("pack")));
    let sector = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, snapshot))
        .id();

    // Player positioned just outside the +Z face (default yaw=0 looks toward -Z),
    // with the eye (translation + EYE_HEIGHT) on the block's center line (y=7.5).
    let player = app
        .world_mut()
        .spawn((
            PlayerController::default(),
            PlayerState::default(),
            PlayerLook::default(),
            Inventory::default(),
            Transform::from_translation(Vec3::new(5.5, 6.5, 6.5)),
            GlobalTransform::from_translation(Vec3::new(5.5, 6.5, 6.5)),
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

    // The durable snapshot must reflect the edit, otherwise saves persist the
    // original PCG data and the break is lost on reload.
    let snap = entity.get::<SectorSnapshot>().unwrap();
    let mut unpack_pool = GlobalBrickPool::new();
    let Ok((pool2, pal2)) = snap.0.unpack(&mut unpack_pool) else {
        panic!("snapshot unpack failed");
    };
    assert_eq!(
        pool2.get_block(&unpack_pool, &pal2, VoxelCoord::new(5, 7, 3)),
        BlockId::AIR,
        "snapshot must be updated to AIR after break"
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

#[test]
fn break_message_at_boundary_marks_neighbor_sector() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GlobalBrickPool::new());
    app.insert_resource(load_block_registry());
    app.insert_resource(PlayerInput::default());
    app.add_message::<PlayerBreak>();
    app.add_message::<PlayerPlace>();
    app.add_systems(
        Update,
        (player_break_system, player_place_system, hotbar_system),
    );

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    // Solid block at the +X boundary: local (31, 7, 3)
    map.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(31, 7, 3),
        BlockId(1),
    )
    .expect("test boundary set_block");
    let snapshot = SectorSnapshot(std::sync::Arc::new(map.pack(&pool, &palette).expect("pack")));
    let sector = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, snapshot))
        .id();

    // Spawn neighbor sector at (1, 0, 0)
    let neighbor_map = XBrickMap::new(SectorCoord(1, 0, 0));
    let neighbor_palette = SectorPalette::new();
    let neighbor_snapshot = SectorSnapshot(std::sync::Arc::new(
        neighbor_map.pack(&pool, &neighbor_palette).expect("pack"),
    ));
    let neighbor_sector = app
        .world_mut()
        .spawn((
            SectorCoord(1, 0, 0),
            neighbor_map,
            neighbor_palette,
            neighbor_snapshot,
        ))
        .id();

    // Position player looking at the block at (31, 7, 3) in sector (0,0,0)
    // block center is at x=31.5, y=7.5, z=3.5.
    // Player eye at (31.5, 7.5, 6.5), looking towards -Z.
    app.world_mut().spawn((
        PlayerController::default(),
        PlayerState::default(),
        PlayerLook::default(),
        Inventory::default(),
        Transform::from_translation(Vec3::new(31.5, 6.5, 6.5)),
        GlobalTransform::from_translation(Vec3::new(31.5, 6.5, 6.5)),
    ));

    app.world_mut().insert_resource(pool);

    app.world_mut().write_message(PlayerBreak);
    app.update();

    // The boundary block must be AIR
    let pool = app.world().resource::<GlobalBrickPool>();
    let map = app.world().entity(sector).get::<XBrickMap>().unwrap();
    let palette = app.world().entity(sector).get::<SectorPalette>().unwrap();
    assert_eq!(
        map.get_block(pool, palette, VoxelCoord::new(31, 7, 3)),
        BlockId::AIR,
        "break must set the hit voxel to AIR"
    );

    // Both sectors must be marked as NeedsRemesh
    assert!(
        app.world().entity(sector).contains::<NeedsRemesh>(),
        "edited sector must be NeedsRemesh"
    );
    assert!(
        app.world()
            .entity(neighbor_sector)
            .contains::<NeedsRemesh>(),
        "neighbor sector must be marked NeedsRemesh"
    );
}

#[test]
fn break_message_crosses_sector_boundary() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GlobalBrickPool::new());
    app.insert_resource(load_block_registry());
    app.insert_resource(PlayerInput::default());
    app.add_message::<PlayerBreak>();
    app.add_message::<PlayerPlace>();
    app.add_systems(
        Update,
        (player_break_system, player_place_system, hotbar_system),
    );

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let map_a = XBrickMap::new(SectorCoord(0, 0, 0));
    let snapshot_a = SectorSnapshot(std::sync::Arc::new(map_a.pack(&pool, &palette).expect("pack")));
    let sector_a = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map_a, palette.clone(), snapshot_a))
        .id();

    // Solid block in sector B at (0, 7, 3)
    let mut map_b = XBrickMap::new(SectorCoord(1, 0, 0));
    map_b.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(0, 7, 3),
        BlockId(1),
    )
    .expect("test sector_b set_block");
    let snapshot_b = SectorSnapshot(std::sync::Arc::new(map_b.pack(&pool, &palette).expect("pack")));
    let sector_b = app
        .world_mut()
        .spawn((SectorCoord(1, 0, 0), map_b, palette, snapshot_b))
        .id();

    // Player stands in sector A at (31.5, 6.5, 3.5), looking towards +X (yaw = -90.0)
    app.world_mut().spawn((
        PlayerController::default(),
        PlayerState::default(),
        PlayerLook {
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        },
        Inventory::default(),
        Transform::from_translation(Vec3::new(31.5, 6.5, 3.5)),
        GlobalTransform::from_translation(Vec3::new(31.5, 6.5, 3.5)),
    ));

    app.world_mut().insert_resource(pool);

    app.world_mut().write_message(PlayerBreak);
    app.update();

    // The boundary block in sector B must be AIR
    let pool = app.world().resource::<GlobalBrickPool>();
    let map = app.world().entity(sector_b).get::<XBrickMap>().unwrap();
    let palette = app.world().entity(sector_b).get::<SectorPalette>().unwrap();
    assert_eq!(
        map.get_block(pool, palette, VoxelCoord::new(0, 7, 3)),
        BlockId::AIR,
        "cross-boundary break must set the hit voxel in neighbor sector to AIR"
    );

    // Both sectors must be marked as NeedsRemesh/ChunkDirty accordingly
    assert!(
        app.world().entity(sector_b).contains::<NeedsRemesh>(),
        "hit sector must be NeedsRemesh"
    );
    assert!(
        app.world().entity(sector_a).contains::<NeedsRemesh>(),
        "neighbor sector must be marked NeedsRemesh due to boundary change"
    );
}

#[test]
fn place_message_crosses_sector_boundary() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GlobalBrickPool::new());
    app.insert_resource(load_block_registry());
    app.insert_resource(PlayerInput::default());
    app.add_message::<PlayerBreak>();
    app.add_message::<PlayerPlace>();
    app.add_systems(
        Update,
        (player_break_system, player_place_system, hotbar_system),
    );

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();

    // Empty sector A at (0, 0, 0)
    let map_a = XBrickMap::new(SectorCoord(0, 0, 0));
    let snapshot_a = SectorSnapshot(std::sync::Arc::new(map_a.pack(&pool, &palette).expect("pack")));
    let sector_a = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map_a, palette.clone(), snapshot_a))
        .id();

    // Solid block in sector B at (0, 7, 3)
    let mut map_b = XBrickMap::new(SectorCoord(1, 0, 0));
    map_b.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(0, 7, 3),
        BlockId(1),
    )
    .expect("test sector_b set_block");
    let snapshot_b = SectorSnapshot(std::sync::Arc::new(map_b.pack(&pool, &palette).expect("pack")));
    let sector_b = app
        .world_mut()
        .spawn((SectorCoord(1, 0, 0), map_b, palette, snapshot_b))
        .id();

    // Player stands in sector A at (29.5, 6.5, 3.5), looking towards +X (yaw = -90.0)
    // Hotbar has Stone (BlockId(1))
    let inventory = Inventory::default();
    app.world_mut().spawn((
        PlayerController::default(),
        PlayerState::default(),
        PlayerLook {
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
        },
        inventory,
        Transform::from_translation(Vec3::new(29.5, 6.5, 3.5)),
        GlobalTransform::from_translation(Vec3::new(29.5, 6.5, 3.5)),
    ));

    app.world_mut().insert_resource(pool);

    app.world_mut().write_message(PlayerPlace);
    app.update();

    // The block must be placed at (31, 7, 3) in sector A (cross boundary from B)
    let pool = app.world().resource::<GlobalBrickPool>();
    let map = app.world().entity(sector_a).get::<XBrickMap>().unwrap();
    let palette = app.world().entity(sector_a).get::<SectorPalette>().unwrap();
    assert_eq!(
        map.get_block(pool, palette, VoxelCoord::new(31, 7, 3)),
        BlockId(1),
        "cross-boundary place must place the voxel in neighbor sector"
    );

    // Both sectors must be marked as NeedsRemesh/ChunkDirty accordingly
    assert!(
        app.world().entity(sector_b).contains::<NeedsRemesh>(),
        "hit sector must be NeedsRemesh"
    );
    assert!(
        app.world().entity(sector_a).contains::<NeedsRemesh>(),
        "placed sector must be marked NeedsRemesh"
    );
}
