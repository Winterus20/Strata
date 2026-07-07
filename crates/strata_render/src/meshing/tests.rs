//! M3 meshing verification (plan 06 §6): round-trip, single-block, full-sector,
//! greedy-vs-naive, AO range, and a timing probe.

use crate::meshing::packed_quad::{PackedQuad, PackedVertex4};
use crate::meshing::{GreedyMesher, MeshStorage, Mesher, NeighborView};
use strata_core::prelude::*;

fn none_neighbors(pool: &GlobalBrickPool) -> [NeighborView<'_>; 6] {
    [NeighborView {
        sector: None,
        palette: None,
        pool,
    }; 6]
}

#[test]
fn packed_quad_round_trips() {
    let q = PackedQuad::new(
        3,
        17,
        25,
        8,
        4,
        4,
        11,
        PackedQuad::pack_ao([0, 1, 2, 3]),
        0xAB,
        0x7,
    );
    assert_eq!(q.repack(), q, "pack -> unpack -> pack must be identical");
    // Field accessors must agree with what we packed.
    assert_eq!(q.x(), 3);
    assert_eq!(q.y(), 17);
    assert_eq!(q.z(), 25);
    assert_eq!(q.width(), 8);
    assert_eq!(q.height(), 4);
    assert_eq!(q.face() as u8, 4);
    assert_eq!(q.block_type(), 11);
    assert_eq!(q.ao(), [0, 1, 2, 3]);
    assert_eq!(q.light(), 0xAB);
    assert_eq!(q.flags(), 0x7);
}

#[test]
fn packed_vertex4_round_trips() {
    let v = PackedVertex4::pack([10, 20, 30], 2, [1, 0], 3, 5);
    let (pos, normal, uv, ao, color) = v.unpack();
    assert_eq!(pos, [10, 20, 30]);
    assert_eq!(normal, 2);
    assert_eq!(uv, [1, 0]);
    assert_eq!(ao, 3);
    assert_eq!(color, 5);
    // A PackedQuad's corner must expand to a round-trippable vertex.
    let q = PackedQuad::new(1, 2, 3, 4, 5, 0, 7, 0, 0, 0);
    for c in 0..4u8 {
        let vtx = q.vertex(c);
        let (p, _, _, _, _) = vtx.unpack();
        assert_eq!(p, q.corner_pos(c));
    }
}

fn stone() -> BlockId {
    let reg = load_block_registry();
    reg.id_by_name("stone").expect("stone must be registered")
}

#[test]
fn single_block_emits_six_quads() {
    let reg = load_block_registry();
    let stone = stone();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 5, 5), stone);

    let mesher = GreedyMesher::new(&reg);
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));

    let mut hist = [0u32; 6];
    for q in &mesh.opaque {
        hist[q.face() as usize] += 1;
    }
    eprintln!("single-block hist {:?} total {}", hist, mesh.total_quads());
    assert_eq!(mesh.total_quads(), 6, "a lone block has exactly 6 faces");
    assert_eq!(mesh.opaque.len(), 6);
    assert!(mesh.transparent.is_empty());

    // Fully exposed block -> all corner AO = 3 (open).
    for q in &mesh.opaque {
        assert_eq!(q.ao(), [3, 3, 3, 3]);
    }
}

#[test]
fn fully_filled_sector_emits_no_quads() {
    let reg = load_block_registry();
    let stone = stone();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, y, z), stone);
            }
        }
    }

    let mesher = GreedyMesher::new(&reg);
    // Neighbors unloaded => conservative solid => all internal faces culled.
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));
    assert_eq!(mesh.total_quads(), 0, "a full sector has no exposed faces");
}

#[test]
fn greedy_merges_far_below_naive() {
    let reg = load_block_registry();
    let stone = stone();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    // One full y-layer of stone at y=0. Neighbors unloaded => side/bottom culled.
    for x in 0..32u32 {
        for z in 0..32u32 {
            map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, 0, z), stone);
        }
    }

    let mesher = GreedyMesher::new(&reg);
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));

    // Greedy collapses the 32x32 top surface into a single quad.
    let naive = 6 * 32 * 32; // worst-case per-voxel face count
    assert_eq!(mesh.total_quads(), 1, "flat layer merges to one quad");
    assert!(
        mesh.total_quads() * 1000 < naive,
        "greedy must be far below naive"
    );
}

#[test]
fn transparent_block_goes_to_transparent_batch() {
    let reg = load_block_registry();
    let glass = reg.id_by_name("glass").expect("glass registered");
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 5, 5), glass);

    let mesher = GreedyMesher::new(&reg);
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));
    assert_eq!(mesh.transparent.len(), 6);
    assert!(mesh.opaque.is_empty());
}

/// Stress: build a wide variety of blocks and ensure meshing never panics and
/// always stays well below the naive ceiling.
#[test]
fn random_fill_stays_below_naive() {
    let reg = load_block_registry();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                if (x ^ y ^ z) % 3 == 0 {
                    let id = BlockId((((x + y + z) % (reg.count() as u32)) as u16).max(1));
                    map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, y, z), id);
                }
            }
        }
    }
    let mesher = GreedyMesher::new(&reg);
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));
    let naive = 6 * 32 * 32 * 32;
    assert!(
        mesh.total_quads() < naive / 2,
        "greedy must beat naive substantially"
    );
}

/// Timing probe for the full 32³ sector. Run with `cargo test -- --ignored`
/// (release build recommended; dev profile may exceed the <0.5ms target).
#[test]
#[ignore]
fn bench_full_sector_mesh_time() {
    use std::time::Instant;
    let reg = load_block_registry();
    let stone = stone();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                map.set_block(&mut pool, &mut palette, VoxelCoord::new(x, y, z), stone);
            }
        }
    }
    let mesher = GreedyMesher::new(&reg);
    let neighbors = none_neighbors(&pool);

    // Warm-up.
    let _ = mesher.mesh_sector(&map, &palette, &pool, &reg, &neighbors);
    let start = Instant::now();
    let iters = 200;
    for _ in 0..iters {
        let _m = mesher.mesh_sector(&map, &palette, &pool, &reg, &neighbors);
        std::hint::black_box(&_m);
    }
    let elapsed = start.elapsed() / iters;
    println!("full 32³ sector mesh: {:.3?} (target < 0.5ms)", elapsed);
}

/// The `NeedsRemesh` ECS system stores a result and clears the marker.
#[test]
fn needs_remesh_system_meshes_and_clears() {
    use crate::meshing::MeshingPlugin;
    use strata_core::component::NeedsRemesh;

    let reg = load_block_registry();
    let stone = stone();

    let mut app = App::new();
    app.add_strata_plugin(MeshingPlugin);
    app.insert_resource(reg);
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(2, 2, 2), stone);
    app.insert_resource(pool);

    let entity = app
        .world_mut()
        .spawn((SectorCoord(0, 0, 0), map, palette, NeedsRemesh))
        .id();

    // Neighbor lookup comes from the same world; spawn a neighbor too.
    let mut nmap = XBrickMap::new(SectorCoord(1, 0, 0));
    let mut npal = SectorPalette::new();
    let mut npool = app.world_mut().resource_mut::<GlobalBrickPool>();
    nmap.set_block(&mut npool, &mut npal, VoxelCoord::new(0, 0, 0), stone);
    app.world_mut().spawn((SectorCoord(1, 0, 0), nmap, npal));

    app.update();

    let storage = app.world().resource::<MeshStorage>();
    assert!(
        storage.meshes.contains_key(&SectorCoord(0, 0, 0)),
        "mesh must be stored"
    );
    assert_eq!(storage.meshes[&SectorCoord(0, 0, 0)].total_quads(), 6);
    assert!(
        app.world_mut()
            .entity(entity)
            .get::<NeedsRemesh>()
            .is_none(),
        "NeedsRemesh must be removed after meshing"
    );
}
