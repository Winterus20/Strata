//! M7 lighting unit + integration tests.
//!
//! Covers `LightData` round-trip/boundary bits, the BFS gradient from a
//! `glowstone` source, two-phase removal (recompute path + `remove_source`),
//! and sky-light boundary behaviour (dark enclosed room, open column). The
//! final test drives the ECS `LightingPlugin` end-to-end.

use super::*;

// ── test helpers ─────────────────────────────────────────────────────────────

fn registry() -> BlockRegistry {
    load_block_registry()
}

fn empty_sector() -> (XBrickMap, GlobalBrickPool, SectorPalette) {
    let pool = GlobalBrickPool::new();
    let palette = SectorPalette::new();
    let map = XBrickMap::new(SectorCoord(0, 0, 0));
    (map, pool, palette)
}

fn set(
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    pal: &mut SectorPalette,
    x: u32,
    y: u32,
    z: u32,
    id: BlockId,
) {
    map.set_block(pool, pal, VoxelCoord::new(x, y, z), id);
}

// ── LightData ────────────────────────────────────────────────────────────────

#[test]
fn lightdata_round_trip() {
    let d = LightData::pack(15, 7);
    assert_eq!(d.sky(), 15);
    assert_eq!(d.block(), 7);
    assert_eq!(d.block_r(), 7);
    assert_eq!(d.block_g(), 7);
    assert_eq!(d.block_b(), 7);

    let d2 = LightData::pack(3, 12);
    assert_eq!(d2.sky(), 3);
    assert_eq!(d2.block(), 12);
}

#[test]
fn lightdata_boundary_bits() {
    let d = LightData::pack(15, 15);
    assert_eq!(d.sky(), 15);
    assert_eq!(d.block(), 15);
    assert_eq!(d.0 & 0xFF, 0xFF);

    let zero = LightData::pack(0, 0);
    assert_eq!(zero.sky(), 0);
    assert_eq!(zero.block(), 0);

    // Overflow clamps to 15 (4-bit channel).
    let over = LightData::pack(31, 31);
    assert_eq!(over.sky(), 15);
    assert_eq!(over.block(), 15);

    let mut m = LightData::default();
    m.set_sky(9);
    m.set_block(4);
    assert_eq!(m, LightData::pack(9, 4));
}

// ── BFS gradient ───────────────────────────────────────────────────────────

#[test]
fn glowstone_gradient() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    let c = VoxelCoord::new(16, 16, 16);
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);

    let engine = LightEngine::default();
    let light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Source voxel is at full emission.
    assert_eq!(light.block(c), 15);

    // Manhattan-distance falloff along each axis (15 - d).
    assert_eq!(light.block(VoxelCoord::new(17, 16, 16)), 14);
    assert_eq!(light.block(VoxelCoord::new(21, 16, 16)), 10);
    assert_eq!(light.block(VoxelCoord::new(31, 16, 16)), 0); // dist 15 -> 0
    assert_eq!(light.block(VoxelCoord::new(16, 19, 16)), 12);
    assert_eq!(light.block(VoxelCoord::new(16, 16, 19)), 12);

    // Manhattan-distance-2 voxel (e.g. (17,17,16)) receives 13.
    assert_eq!(light.block(VoxelCoord::new(17, 17, 16)), 13);
}

// ── Two-phase removal ──────────────────────────────────────────────────────

#[test]
fn two_phase_removal_recompute() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    let c = VoxelCoord::new(16, 16, 16);
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);

    let engine = LightEngine::default();
    let mut light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    let probe = VoxelCoord::new(19, 16, 16); // dist 3 -> 12
    assert_eq!(light.block(probe), 12);

    // Break the block, recompute (apply_edit path).
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, BlockId::AIR);
    light = engine.apply_edit(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Light retracts: previously-lit voxels return to 0.
    assert_eq!(light.block(probe), 0);
    assert_eq!(light.block(c), 0);
}

#[test]
fn two_phase_removal_remove_source() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    let c = VoxelCoord::new(10, 10, 10);
    set(&mut map, &mut pool, &mut pal, 10, 10, 10, glow);

    let engine = LightEngine::default();
    let mut light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(light.block(c), 15);

    // Break the block in the map, THEN run two-phase removal (real edit order).
    set(&mut map, &mut pool, &mut pal, 10, 10, 10, BlockId::AIR);
    engine.remove_source(c, &map, &pool, &pal, &reg, &mut light);

    for i in 0..SECTOR_VOXELS {
        assert_eq!(
            light.data[i].block(),
            0,
            "voxel {i} still lit after removal"
        );
    }
}

#[test]
fn two_phase_removal_keeps_remaining_source() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    let a = VoxelCoord::new(5, 5, 5);
    let b = VoxelCoord::new(20, 20, 20);
    set(&mut map, &mut pool, &mut pal, 5, 5, 5, glow);
    set(&mut map, &mut pool, &mut pal, 20, 20, 20, glow);

    let engine = LightEngine::default();
    let mut light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Break `a` in the map, THEN run two-phase removal (real edit order).
    set(&mut map, &mut pool, &mut pal, 5, 5, 5, BlockId::AIR);
    engine.remove_source(a, &map, &pool, &pal, &reg, &mut light);

    // Source `b` survives and still lights; `a` is dark.
    assert_eq!(light.block(b), 15);
    assert_eq!(light.block(a), 0);
    assert_eq!(light.block(VoxelCoord::new(21, 20, 20)), 14);
}

// ── Sky-light boundaries ───────────────────────────────────────────────────

#[test]
fn fully_solid_sector_is_dark() {
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                set(&mut map, &mut pool, &mut pal, x, y, z, stone);
            }
        }
    }
    let engine = LightEngine::default();
    let light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(light.sky(VoxelCoord::new(16, 16, 16)), 0);
    assert_eq!(light.sky(VoxelCoord::new(0, 31, 0)), 0);
    assert_eq!(light.block(VoxelCoord::new(16, 16, 16)), 0);
}

#[test]
fn open_column_has_max_sky_at_top() {
    let reg = registry();
    let (map, pool, pal) = empty_sector();
    let engine = LightEngine::default();
    let light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    // Fully open (air) sector: top voxel sees full sky.
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
    // Bottom of the column is fully attenuated.
    assert_eq!(light.sky(VoxelCoord::new(16, 0, 16)), 0);
}

#[test]
fn hollow_room_interior_is_dark() {
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    // Stone shell on the outer two voxels of each axis; air cavity inside.
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                let inner = (2..=29).contains(&x) && (2..=29).contains(&y) && (2..=29).contains(&z);
                if !inner {
                    set(&mut map, &mut pool, &mut pal, x, y, z, stone);
                }
            }
        }
    }
    let engine = LightEngine::default();
    let light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    // Interior cavity is enclosed -> no sky reaches it.
    assert_eq!(light.sky(VoxelCoord::new(16, 16, 16)), 0);
    // Top shell voxel is solid -> blocks sky for everything below.
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 0);
}

#[test]
fn terrain_blocks_sky_below() {
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    set(&mut map, &mut pool, &mut pal, 16, 10, 16, stone);
    let engine = LightEngine::default();
    let light = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
    assert_eq!(light.sky(VoxelCoord::new(16, 9, 16)), 0);
}

// ── ECS integration ─────────────────────────────────────────────────────────

#[test]
fn lighting_plugin_computes_sector_component() {
    use strata_core::plugin::AddStrataPlugin;

    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    // The pool that owns the bricks must be the one the system reads as a
    // resource, so build the sector against that same pool.
    let mut pool = GlobalBrickPool::new();
    let mut pal = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);

    let mut app = App::new();
    app.add_strata_plugin(BlockRegistryPlugin);
    app.insert_resource(pool);
    app.add_strata_plugin(LightingPlugin);

    let e = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, pal, Generated, ChunkDirty))
        .id();

    app.update();

    let light = app
        .world()
        .get::<SectorLight>(e)
        .expect("SectorLight computed");
    assert_eq!(light.block(VoxelCoord::new(16, 16, 16)), 15);
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
}
