//! M6 physics verification tests (headless Rapier).

use bevy::prelude::*;
use bevy::time::TimePlugin;
use bevy_rapier3d::prelude::*;
use strata_core::prelude::*;
use strata_world::prelude::Generated;

use crate::plugin::PhysicsPlugin;
use crate::voxel_collider::{SectorCollider, VOXEL_SIZE, ground_below, set_sector_voxel};

/// Configurable downward ray used by the probe system.
#[derive(Resource)]
struct RayProbe {
    origin: Vec3,
    dir: Vec3,
    max_toi: f32,
}

impl Default for RayProbe {
    fn default() -> Self {
        Self {
            origin: Vec3::new(5.5 * VOXEL_SIZE, 10.0, 5.5 * VOXEL_SIZE),
            dir: Vec3::new(0.0, -1.0, 0.0),
            max_toi: 100.0,
        }
    }
}

/// Result of the probe raycast against the Rapier world.
#[derive(Resource, Default)]
struct RayHit(pub Option<(Entity, f32)>);

/// One-shot edit request consumed by [`apply_edit`] during a test.
#[derive(Resource)]
struct EditPlan {
    entity: Entity,
    voxel: VoxelCoord,
    block: BlockId,
}

/// One-shot request consumed by [`toggle_voxel`] to exercise the O(1) collider
/// path from inside a system (required so Bevy change detection flags the
/// `Collider` for Rapier's `apply_collider_user_changes`).
#[derive(Resource)]
struct TogglePlan {
    entity: Entity,
    voxel: VoxelCoord,
    occupied: bool,
}

fn toggle_voxel(
    mut commands: Commands,
    plan: Option<Res<TogglePlan>>,
    mut query: Query<&mut Collider>,
) {
    let Some(plan) = plan else {
        return;
    };
    if let Ok(mut collider) = query.get_mut(plan.entity) {
        set_sector_voxel(&mut collider, plan.voxel, plan.occupied);
    }
    commands.remove_resource::<TogglePlan>();
}

fn probe_down(ctx: ReadRapierContext, probe: Res<RayProbe>, mut hit: ResMut<RayHit>) {
    let Ok(ctx) = ctx.single() else {
        return;
    };
    hit.0 = ctx.cast_ray(
        probe.origin,
        probe.dir,
        probe.max_toi,
        true,
        QueryFilter::default(),
    );
}

fn apply_edit(
    mut commands: Commands,
    plan: Option<Res<EditPlan>>,
    pool: Option<ResMut<GlobalBrickPool>>,
    mut query: Query<(&mut XBrickMap, &mut SectorPalette)>,
) {
    let Some(plan) = plan else {
        return;
    };
    let Some(mut pool) = pool else {
        return;
    };
    if let Ok((mut map, mut palette)) = query.get_mut(plan.entity) {
        map.set_block(&mut pool, &mut palette, plan.voxel, plan.block);
        commands.entity(plan.entity).insert(ChunkDirty);
    }
    commands.remove_resource::<EditPlan>();
}

/// A headless app with the Strata physics plugin and a configurable probe.
fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((TransformPlugin, TimePlugin));
    app.add_strata_plugin(PhysicsPlugin);
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.insert_resource(load_block_registry());
    app.insert_resource(RayProbe::default());
    app.insert_resource(RayHit::default());
    app.add_systems(
        Update,
        (
            apply_edit.in_set(StrataSet::Input),
            toggle_voxel.in_set(StrataSet::Input),
        ),
    );
    app.add_systems(Last, probe_down);
    app
}

fn solid_id(registry: &BlockRegistry) -> BlockId {
    registry.id_by_name("stone").unwrap_or(BlockId(1))
}

fn drain_sector_colliders(app: &mut App) {
    for _ in 0..120 {
        app.update();
        let ready = {
            let mut q = app
                .world_mut()
                .query_filtered::<Entity, With<SectorCollider>>();
            q.iter(app.world()).next().is_some()
        };
        if ready {
            return;
        }
    }
    panic!("sector collider did not finish building within frame budget");
}

/// A downward Rapier raycast against a sector's `Voxels` collider hits.
#[test]
fn downward_raycast_hits_sector_voxel_collider() {
    let mut app = headless_app();
    let solid = solid_id(&load_block_registry());

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 5, 5), solid);
    app.insert_resource(pool);

    app.world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, Generated));

    drain_sector_colliders(&mut app);
    app.update();

    let hit = app.world().resource::<RayHit>();
    let (entity, toi) = hit.0.expect("downward ray must hit the voxel collider");
    assert!(toi > 0.0 && toi < 100.0, "toi out of range: {toi}");
    assert!(
        app.world().entity(entity).contains::<SectorCollider>(),
        "hit entity must be the sector collider"
    );
}

/// `ground_below` is true only when a solid voxel sits directly beneath the point.
#[test]
fn ground_below_detects_solid_voxel() {
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(5, 5, 5),
        BlockId(1),
    );

    // Standing just above voxel (5,5,5): floor(pos.y)=6 -> voxel below is (5,5,5).
    assert!(
        ground_below(&map, &pool, Vec3::new(5.5, 6.0, 5.5)),
        "voxel directly below must register as solid"
    );
    // One voxel higher: the voxel below is (5,6,5) which is empty.
    assert!(
        !ground_below(&map, &pool, Vec3::new(5.5, 7.0, 5.5)),
        "no solid voxel directly below at height 7"
    );
    // An empty map is never grounded.
    let empty = XBrickMap::new(SectorCoord(0, 0, 0));
    assert!(
        !ground_below(&empty, &pool, Vec3::new(5.5, 6.0, 5.5)),
        "empty sector is never grounded"
    );
}

/// Placing a block (set_block + ChunkDirty) updates the sector collider so a
/// downward raycast now hits the newly placed voxel.
#[test]
fn edit_updates_collider_solid() {
    let mut app = headless_app();
    let solid = solid_id(&load_block_registry());

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 5, 5), solid);
    app.insert_resource(pool);

    let entity = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, Generated))
        .id();

    drain_sector_colliders(&mut app);
    app.update();

    // Before edit: a ray at the future voxel (7,7,7) must miss.
    app.world_mut().resource_mut::<RayProbe>().origin =
        Vec3::new(7.5 * VOXEL_SIZE, 10.0, 7.5 * VOXEL_SIZE);
    app.update();
    assert!(
        app.world().resource::<RayHit>().0.is_none(),
        "voxel (7,7,7) should not exist yet"
    );

    // Place the new block and mark the sector dirty via the one-shot edit system.
    app.world_mut().insert_resource(EditPlan {
        entity,
        voxel: VoxelCoord::new(7, 7, 7),
        block: solid,
    });
    app.update();
    app.update();

    app.world_mut().resource_mut::<RayProbe>().origin =
        Vec3::new(7.5 * VOXEL_SIZE, 10.0, 7.5 * VOXEL_SIZE);
    app.update();

    let hit = app.world().resource::<RayHit>().0;
    assert!(
        hit.is_some(),
        "newly placed voxel (7,7,7) must be solid to a raycast"
    );
    let toi = hit.unwrap().1;
    assert!(toi > 0.0 && toi < 100.0, "edit ray toi out of range: {toi}");
}

/// `set_sector_voxel` exercises the O(1) partial-rebuild path and the change is
/// reflected by a Rapier raycast (no full rebuild needed).
#[test]
fn set_voxel_partial_path_is_safe() {
    let mut app = headless_app();
    let solid = solid_id(&load_block_registry());

    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(0, 0, 0), solid);
    app.insert_resource(pool);

    let entity = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, Generated))
        .id();
    drain_sector_colliders(&mut app);
    app.update();

    // Toggle voxel (3,3,3) on through the O(1) path from inside a system (so the
    // `Collider` change is flagged for Rapier's `apply_collider_user_changes`),
    // then raycast it.
    app.world_mut().insert_resource(TogglePlan {
        entity,
        voxel: VoxelCoord::new(3, 3, 3),
        occupied: true,
    });
    app.world_mut().resource_mut::<RayProbe>().origin =
        Vec3::new(3.5 * VOXEL_SIZE, 10.0, 3.5 * VOXEL_SIZE);
    app.update();
    assert!(
        app.world().resource::<RayHit>().0.is_some(),
        "voxel toggled via set_voxel must be solid to a raycast"
    );
}

/// A voxel at a sector edge and its neighbour in the adjacent sector both register
/// as solid (collider continuity premise) and are world-contiguous.
#[test]
fn sector_edge_and_neighbor_both_solid() {
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut a = XBrickMap::new(SectorCoord(0, 0, 0));
    a.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(31, 10, 10),
        BlockId(1),
    );
    let mut b = XBrickMap::new(SectorCoord(1, 0, 0));
    b.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(0, 10, 10),
        BlockId(1),
    );

    assert!(
        a.is_occupied(&pool, VoxelCoord::new(31, 10, 10)),
        "sector A edge voxel must be solid"
    );
    assert!(
        b.is_occupied(&pool, VoxelCoord::new(0, 10, 10)),
        "sector B neighbour edge voxel must be solid"
    );

    let wa = world_voxel(a.coord, VoxelCoord::new(31, 10, 10));
    let wb = world_voxel(b.coord, VoxelCoord::new(0, 10, 10));
    assert_eq!(wa.x + 1, wb.x, "sector edge voxels must be world-adjacent");
}

#[inline]
fn world_voxel(coord: SectorCoord, local: VoxelCoord) -> IVec3 {
    IVec3::new(
        coord.0 * 32 + local.x as i32,
        coord.1 * 32 + local.y as i32,
        coord.2 * 32 + local.z as i32,
    )
}

#[test]
fn test_ground_below_negative_y() {
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, -1, 0));
    map.set_block(
        &mut pool,
        &mut palette,
        VoxelCoord::new(5, 20, 5),
        BlockId(1),
    );

    assert!(
        ground_below(&map, &pool, Vec3::new(5.5, -11.0, 5.5)),
        "ground_below must return true for solid voxel at negative Y (-12)"
    );
}
