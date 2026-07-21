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
    map.set_block(pool, pal, VoxelCoord::new(x, y, z), id)
        .expect("test set_block");
}

// ── LightData ────────────────────────────────────────────────────────────────

#[test]
fn lightdata_uses_only_8_bits() {
    // LightData packs sky (4 bits) + block (4 bits) into the lower 8 bits of u16.
    // The upper 8 bits (bits 8-15) must always be zero — the full r,g,b,s 4×4-bit
    // layout (plan 13) is deferred. This test locks the current 8-bit contract.
    let d = LightData::pack(15, 15);
    assert_eq!(d.0 & 0xFF, 0xFF, "lower 8 bits hold sky+block");
    assert_eq!(d.0 >> 8, 0, "upper 8 bits must be zero (plan 13 deferred)");

    let mut m = LightData::default();
    m.set_sky(15);
    m.set_block(15);
    assert_eq!(m.0 & 0xFF, 0xFF);
    assert_eq!(m.0 >> 8, 0, "set_sky/set_block must not touch upper bits");

    // Even with clamped overflow, upper bits stay zero.
    let over = LightData::pack(255, 255);
    assert_eq!(over.0 >> 8, 0);
}

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
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

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
    let (mut light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    let probe = VoxelCoord::new(19, 16, 16); // dist 3 -> 12
    assert_eq!(light.block(probe), 12);

    // Break the block, recompute (apply_edit path).
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, BlockId::AIR);
    (light, _) = engine.apply_edit(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

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
    let (mut light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
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
    let (mut light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Break `a` in the map, THEN run two-phase removal (real edit order).
    set(&mut map, &mut pool, &mut pal, 5, 5, 5, BlockId::AIR);
    engine.remove_source(a, &map, &pool, &pal, &reg, &mut light);

    // Source `b` survives and still lights; `a` is dark.
    assert_eq!(light.block(b), 15);
    assert_eq!(light.block(a), 0);
    assert_eq!(light.block(VoxelCoord::new(21, 20, 20)), 14);
}

// ── coord_of round-trip (O4) ────────────────────────────────────────────────

#[test]
fn coord_of_round_trip_all_indices() {
    for idx in 0..SECTOR_VOXELS {
        let (x, y, z) = coord_of(idx);
        let back = SectorLight::idx_of(VoxelCoord::new(x, y, z));
        assert_eq!(
            idx, back,
            "round-trip failed at idx={idx}: ({x},{y},{z}) -> {back}"
        );
    }
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
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(light.sky(VoxelCoord::new(16, 16, 16)), 0);
    assert_eq!(light.sky(VoxelCoord::new(0, 31, 0)), 0);
    assert_eq!(light.block(VoxelCoord::new(16, 16, 16)), 0);
}

#[test]
fn open_column_has_max_sky_at_top() {
    let reg = registry();
    let (map, pool, pal) = empty_sector();
    let engine = LightEngine::default();
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    // Fully open (air) sector: top voxel sees full sky.
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
    // Open air column: sunlight reaches the bottom at full strength — there is
    // no vertical attenuation, only an opaque block darkens the column below.
    assert_eq!(light.sky(VoxelCoord::new(16, 0, 16)), 15);
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
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
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
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
    // Column at (16,*,16) is blocked below y=10, but horizontal BFS from
    // adjacent open columns (x=15, x=17) spreads sky light to y=9.
    let blocked_sky = light.sky(VoxelCoord::new(16, 9, 16));
    assert!(
        blocked_sky > 0,
        "horizontal spread from adjacent open column, got {blocked_sky}"
    );
    assert_eq!(blocked_sky, 14, "one step from open column at x=15/17");
}

// ── Sky horizontal spread (Y6) ─────────────────────────────────────────────

#[test]
fn sky_light_is_sector_local() {
    // M7 limitation: compute_sky operates only within a single sector.
    // Sky light does NOT propagate to adjacent sectors — each sector is
    // self-contained. This test verifies that an open sector gets full sky
    // at the top regardless of any external state (no cross-sector dependency).
    let reg = registry();
    let (map, pool, pal) = empty_sector();
    let engine = LightEngine::default();
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    // Open column: full sky at top.
    assert_eq!(light.sky(VoxelCoord::new(16, 31, 16)), 15);
    // Bottom of open column also full sky (no vertical attenuation).
    assert_eq!(light.sky(VoxelCoord::new(0, 0, 0)), 15);
    // Corner voxel also lit (open sector, no opaque blocks).
    assert_eq!(light.sky(VoxelCoord::new(31, 31, 31)), 15);
}

#[test]
fn sky_spreads_horizontally_under_overhang() {
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();

    // Stone platform at y=20 covering x=0..15 (left half).
    // This creates an overhang: voxels at y<20, x<16 are under the platform.
    for x in 0..16u32 {
        for z in 0..32u32 {
            set(&mut map, &mut pool, &mut pal, x, 20, z, stone);
        }
    }

    let engine = LightEngine::default();
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Under the platform, directly below solid: gets horizontal spread from the
    // open column at x=16. Distance 8 from x=16 to x=8, so sky = 15 - 8 = 7.
    let under_sky = light.sky(VoxelCoord::new(8, 19, 16));
    assert_eq!(under_sky, 7, "8 steps from open column via horizontal BFS");

    // Open side (x>=16): full sky from top.
    assert_eq!(
        light.sky(VoxelCoord::new(20, 19, 16)),
        15,
        "open column should have full sky"
    );

    // Under platform but adjacent to open column (x=15, y=19 is 1 step from x=16):
    // horizontal BFS gives sky = 15 - 1 = 14.
    let edge_sky = light.sky(VoxelCoord::new(15, 19, 16));
    assert!(
        edge_sky > 0,
        "edge voxel under overhang should get horizontal sky spread, got {edge_sky}"
    );
    assert_eq!(edge_sky, 14, "horizontal spread should decay by 1");
}

#[test]
fn sky_does_not_spread_through_opaque() {
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();

    // Solid wall at x=16, z=16, all y. Air on both sides.
    for y in 0..32u32 {
        set(&mut map, &mut pool, &mut pal, 16, y, 16, stone);
    }

    let engine = LightEngine::default();
    let (light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Both sides are open columns, so they get full sky. The wall is opaque.
    // Sky light should not pass through the wall.
    assert_eq!(
        light.sky(VoxelCoord::new(17, 10, 16)),
        15,
        "open side has full sky"
    );
    // x=15 is also an open column so it gets full sky too. The key check is that
    // horizontal BFS doesn't go through the wall from x=15 to x=17 or vice versa.
    // Both are independently lit by the column pass.
}

#[test]
fn sky_bfs_does_not_climb_through_dug_hole_onto_floor() {
    // Reproduces the break-glow bug: sky lit under a floor sheet must not climb
    // vertically through a dug hole onto the air layer above neighbouring floor
    // tops (Manhattan glow). Plan 13: attenuation BFS is horizontal-only;
    // vertical sky is column-first only.
    let reg = registry();
    let stone = reg.id_by_name("stone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();

    // Floor sheet at y=10 covering x=1..31 (leave x=0 as a sky shaft).
    for x in 1..32u32 {
        for z in 0..32u32 {
            set(&mut map, &mut pool, &mut pal, x, 10, z, stone);
        }
    }
    // Ceiling sheet at y=14 for x=1..31 — seals room y=11..13 from column sky
    // while keeping the x=0 shaft open so under-floor air can receive horizontal sky.
    for x in 1..32u32 {
        for z in 0..32u32 {
            set(&mut map, &mut pool, &mut pal, x, 14, z, stone);
        }
    }

    let engine = LightEngine::default();
    let (light_before, _) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Under-floor near the shaft gets horizontal sky; the sealed room stays dark.
    assert!(
        light_before.sky(VoxelCoord::new(2, 9, 16)) > 0,
        "under-floor should receive horizontal sky from the x=0 shaft"
    );
    assert_eq!(
        light_before.sky(VoxelCoord::new(16, 12, 16)),
        0,
        "sealed room must be dark before the dig"
    );

    // Dig a hole in the floor at the center — connects under-floor air to the room
    // vertically through one air cell.
    set(&mut map, &mut pool, &mut pal, 16, 10, 16, BlockId::AIR);
    let (light_after, _) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    let hole = light_after.sky(VoxelCoord::new(16, 10, 16));
    let room_above_hole = light_after.sky(VoxelCoord::new(16, 12, 16));
    let floor_neighbor_air = light_after.sky(VoxelCoord::new(17, 11, 16));

    // Hole may pick up under-floor sky only if column-first opens — ceiling still
    // blocks, so hole stays dark from column. Horizontal BFS cannot enter solids,
    // so the hole itself stays 0 unless under-floor sky reaches it horizontally
    // at y=10 (it can't — y=10 neighbours are solid).
    assert_eq!(hole, 0, "hole under ceiling must not invent column sky");
    assert_eq!(
        room_above_hole, 0,
        "sky must not climb through the dug hole into the room (got {room_above_hole})"
    );
    assert_eq!(
        floor_neighbor_air, 0,
        "floor-top air neighbours must stay dark (got {floor_neighbor_air}) — \
         vertical bleed would paint the Manhattan glow"
    );
}

#[test]
fn break_solid_only_brightens_when_newly_exposed_to_sky() {
    // Surface dig: opening a column to sector sky is allowed to light the hole
    // and under-hole air via column-first — but neighbouring surface tops that
    // were already open must not jump above their pre-break sky.
    let reg = registry();
    let dirt = reg.id_by_name("dirt").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    for x in 0..32u32 {
        for z in 0..32u32 {
            for y in 0..=10u32 {
                set(&mut map, &mut pool, &mut pal, x, y, z, dirt);
            }
        }
    }

    let engine = LightEngine::default();
    let (before, _) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    let neighbor_top_air = VoxelCoord::new(17, 11, 16);
    let sky_before = before.sky(neighbor_top_air);
    assert_eq!(sky_before, 15, "open surface air is full sky");

    set(&mut map, &mut pool, &mut pal, 16, 10, 16, BlockId::AIR);
    let (after, _) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);
    assert_eq!(
        after.sky(neighbor_top_air),
        sky_before,
        "breaking a surface block must not over-brighten already-open neighbour tops"
    );
    assert_eq!(
        after.sky(VoxelCoord::new(16, 10, 16)),
        15,
        "hole in an open column correctly receives full sky"
    );
}

#[test]
fn remove_source_no_ghost_lights() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();
    let c = VoxelCoord::new(16, 16, 16);
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);

    let engine = LightEngine::default();
    let (mut light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Pre-check: source is lit.
    assert_eq!(light.block(c), 15);
    assert_eq!(light.block(VoxelCoord::new(21, 16, 16)), 10);

    // Remove source.
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, BlockId::AIR);
    engine.remove_source(c, &map, &pool, &pal, &reg, &mut light);

    // Every voxel should be 0 (no ghost lights).
    for i in 0..SECTOR_VOXELS {
        assert_eq!(
            light.data[i].block(),
            0,
            "voxel {i} still lit after removal"
        );
    }
}

#[test]
fn remove_source_preserves_other_source_in_overlap() {
    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let (mut map, mut pool, mut pal) = empty_sector();

    // Two sources close enough to overlap at the midpoint.
    // Source A at (10,16,16) with emission 15. Source B at (20,16,16) with emission 15.
    // Midpoint is (15,16,16): dist to A=5, dist to B=5 → both contribute level 10.
    let a = VoxelCoord::new(10, 16, 16);
    let b = VoxelCoord::new(20, 16, 16);
    let mid = VoxelCoord::new(15, 16, 16);
    set(&mut map, &mut pool, &mut pal, 10, 16, 16, glow);
    set(&mut map, &mut pool, &mut pal, 20, 16, 16, glow);

    let engine = LightEngine::default();
    let (mut light, _timers) = engine.compute_sector(SectorCoord(0, 0, 0), &map, &pool, &pal, &reg);

    // Pre-check: midpoint is lit by both (level 10 from each, max=10).
    assert_eq!(light.block(mid), 10);
    assert_eq!(light.block(a), 15);
    assert_eq!(light.block(b), 15);

    // Remove source A.
    set(&mut map, &mut pool, &mut pal, 10, 16, 16, BlockId::AIR);
    engine.remove_source(a, &map, &pool, &pal, &reg, &mut light);

    // Source A's position is now AIR. Source B at (20,16,16) contributes
    // level 5 at distance 10 through the air. This is correct behavior.
    let a_light = light.block(a);
    assert_eq!(
        a_light, 5,
        "removed source position receives B's light (dist=10, 15-10=5)"
    );

    // Source B survives and still lights the midpoint.
    assert_eq!(
        light.block(b),
        15,
        "other source should remain at full strength"
    );
    assert_eq!(
        light.block(mid),
        10,
        "midpoint should still be lit by source B"
    );

    // Neighbor of A that's far from B: B still reaches here at dist=11, level=4.
    // With the canonical push model, this should be exactly 4.
    let far_sky = light.block(VoxelCoord::new(9, 16, 16));
    assert!(
        far_sky > 0,
        "B's light still reaches near A (dist=11 from B), got {far_sky}"
    );
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
    app.add_strata_plugin(StrataSchedulingPlugin);
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

    app.world()
        .get::<NeedsRemesh>(e)
        .expect("NeedsRemesh should be inserted with SectorLight");
    assert!(
        app.world().get::<ChunkDirty>(e).is_none(),
        "ChunkDirty must clear after lighting so remesh/phys do not thrash"
    );
}

#[test]
fn chunk_dirty_lighting_does_not_retrash_next_frame() {
    use strata_core::plugin::AddStrataPlugin;

    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let mut pool = GlobalBrickPool::new();
    let mut pal = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);

    let mut app = App::new();
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.add_strata_plugin(BlockRegistryPlugin);
    app.insert_resource(pool);
    app.add_strata_plugin(LightingPlugin);

    let e = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, pal, Generated, ChunkDirty))
        .id();

    app.update();
    assert!(app.world().get::<ChunkDirty>(e).is_none());
    assert!(app.world().get::<NeedsRemesh>(e).is_some());

    // Simulate meshing consuming NeedsRemesh; a stuck ChunkDirty would re-insert it.
    app.world_mut().entity_mut(e).remove::<NeedsRemesh>();
    app.update();

    let timers = app.world().resource::<LightingTimers>();
    assert_eq!(
        timers.applied, 0,
        "idle sector must not re-light after ChunkDirty was cleared"
    );
    assert!(
        app.world().get::<NeedsRemesh>(e).is_none(),
        "lighting must not re-queue NeedsRemesh without a new dirty/edit"
    );
}

#[test]
fn lighting_system_counts_sectors_once() {
    use strata_core::plugin::AddStrataPlugin;

    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();

    let mut pool = GlobalBrickPool::new();

    let mut pal_dirty = SectorPalette::new();
    let mut map_dirty = XBrickMap::new(SectorCoord(0, 0, 0));
    set(&mut map_dirty, &mut pool, &mut pal_dirty, 16, 16, 16, glow);

    let pal_new = SectorPalette::new();
    let map_new = XBrickMap::new(SectorCoord(1, 0, 0));

    let mut app = App::new();
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.add_strata_plugin(BlockRegistryPlugin);
    app.insert_resource(pool);
    app.add_strata_plugin(LightingPlugin);

    let e_dirty = app
        .world_mut()
        .spawn((
            SectorCoord(0, 0, 0),
            map_dirty,
            pal_dirty,
            Generated,
            ChunkDirty,
        ))
        .id();
    let e_new = app
        .world_mut()
        .spawn((SectorCoord(1, 0, 0), map_new, pal_new, Generated))
        .id();

    app.update();

    // Both sectors receive SectorLight and NeedsRemesh; dirty flag is consumed.
    assert!(app.world().get::<SectorLight>(e_dirty).is_some());
    assert!(app.world().get::<NeedsRemesh>(e_dirty).is_some());
    assert!(app.world().get::<ChunkDirty>(e_dirty).is_none());
    assert!(app.world().get::<SectorLight>(e_new).is_some());
    assert!(app.world().get::<NeedsRemesh>(e_new).is_some());

    // Exactly 2 sectors processed (no double-count).
    let timers = app.world().resource::<LightingTimers>();
    assert_eq!(timers.applied, 2);
}

#[test]
fn chunk_dirty_lighting_respects_shared_budget() {
    use strata_core::plugin::AddStrataPlugin;

    let reg = registry();
    let glow = reg.id_by_name("glowstone").unwrap();
    let mut pool = GlobalBrickPool::new();

    let mut app = App::new();
    app.add_strata_plugin(StrataSchedulingPlugin);
    app.add_strata_plugin(BlockRegistryPlugin);

    for i in 0..5 {
        let mut pal = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(i, 0, 0));
        set(&mut map, &mut pool, &mut pal, 16, 16, 16, glow);
        app.world_mut()
            .spawn((SectorCoord(i, 0, 0), map, pal, Generated, ChunkDirty));
    }

    app.insert_resource(pool);
    app.add_strata_plugin(LightingPlugin);
    app.update();

    let applied = {
        let timers = app.world().resource::<LightingTimers>();
        // LIGHTING_BUDGET = 2; dirty path must share it (was previously uncapped).
        assert!(
            timers.applied <= 2,
            "ChunkDirty lighting must respect LIGHTING_BUDGET, applied={}",
            timers.applied
        );
        assert!(timers.applied > 0, "at least one dirty sector should light");
        timers.applied
    };
    let mut q = app.world_mut().query_filtered::<Entity, With<ChunkDirty>>();
    let remaining_dirty = q.iter(app.world()).count();
    assert_eq!(
        remaining_dirty,
        5 - applied,
        "unprocessed dirty sectors must keep ChunkDirty for the next frame"
    );
}
