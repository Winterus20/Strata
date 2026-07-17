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

/// Build neighbor views with `idx` (0=+X..5=-Z) pointing at `nmap`, the rest
/// unloaded. Used to exercise the cross-sector sampling path (AO at edges can
/// push a *second* axis out of range, which previously overflowed).
fn one_neighbor<'a>(
    pool: &'a GlobalBrickPool,
    nmap: &'a XBrickMap,
    npal: &'a SectorPalette,
    idx: usize,
) -> [NeighborView<'a>; 6] {
    let mut v = none_neighbors(pool);
    v[idx] = NeighborView {
        sector: Some(nmap),
        palette: Some(npal),
        pool,
    };
    v
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
fn mesh_with_loaded_neighbor_does_not_panic() {
    // Regression: AO sampling at a sector edge pushes a second axis out of range
    // (depth sd==32 plus a tangent offset of -1/32). With a *loaded* neighbor the
    // greedy mesher samples into the neighbour XBrickMap; the coord must be
    // wrapped into 0..31 or `get_block` overflows (`brick_index` shift panic).
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

    // +X neighbour fully solid too, so the shared boundary faces cull.
    let mut npool = GlobalBrickPool::new();
    let mut npal = SectorPalette::new();
    let mut nmap = XBrickMap::new(SectorCoord(1, 0, 0));
    for x in 0..32u32 {
        for y in 0..32u32 {
            for z in 0..32u32 {
                nmap.set_block(&mut npool, &mut npal, VoxelCoord::new(x, y, z), stone);
            }
        }
    }

    let mesher = GreedyMesher::new(&reg);
    // Exercise every neighbour direction (each can be the "second oob axis" case).
    for idx in 0..6 {
        let views = one_neighbor(&pool, &nmap, &npal, idx);
        let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &views);
        // Boundary faces between two solid sectors are culled; the 6 outer
        // faces facing the other (unloaded) neighbours are still emitted.
        assert!(
            mesh.total_quads() >= 5 && mesh.total_quads() <= 6,
            "loaded +X neighbour: expected 5-6 boundary faces, got {}",
            mesh.total_quads()
        );
    }
}

#[test]
fn face_quad_sits_on_correct_boundary() {
    // Regression: a +Y (top) face must sit on the +Y boundary of its voxel
    // (y = bottom + 1), not at the voxel's bottom. A bug placed +d faces one
    // voxel too low, hiding top faces behind the block body.
    let reg = load_block_registry();
    let stone = stone();
    let mut pool = GlobalBrickPool::new();
    let mut palette = SectorPalette::new();
    let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
    map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 5, 5), stone);

    let mesher = GreedyMesher::new(&reg);
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));

    let get = |face: u8| -> &PackedQuad {
        mesh.opaque
            .iter()
            .find(|q| q.face() as u8 == face)
            .unwrap_or_else(|| panic!("missing face {face}"))
    };

    // The CPU packs the *owning voxel* (local 5), keeping the 5-bit position
    // field from overflowing at the sector boundary (a +d plane would be 32).
    // +d faces (even index) are advanced by one voxel in the vertex shader, so
    // their true plane is `packed + 1`; -d faces (odd index) sit at the packed
    // voxel. A block at local (5,5,5) therefore packs all faces at coordinate 5.
    assert_eq!(
        get(2).y(),
        5,
        "+Y owns voxel y=5 (shader places plane at 6)"
    );
    assert_eq!(get(2).x(), 5);
    assert_eq!(get(2).z(), 5);

    assert_eq!(get(3).y(), 5, "-Y owns voxel y=5 (plane at 5)");
    assert_eq!(
        get(0).x(),
        5,
        "+X owns voxel x=5 (shader places plane at 6)"
    );
    assert_eq!(get(1).x(), 5, "-X owns voxel x=5");
    assert_eq!(
        get(4).z(),
        5,
        "+Z owns voxel z=5 (shader places plane at 6)"
    );
    assert_eq!(get(5).z(), 5, "-Z owns voxel z=5");
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
    // Unloaded neighbors are treated as AIR (not conservative-solid), so the 6
    // outer boundary faces are emitted; all internal faces are still culled.
    let mesh = mesher.mesh_sector(&map, &palette, &pool, &reg, &none_neighbors(&pool));
    assert_eq!(
        mesh.total_quads(),
        6,
        "a full sector exposes only its 6 boundary faces"
    );
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

    // With unloaded neighbors treated as AIR, the layer exposes: its 32x32 top
    // surface (1 merged quad, +Y), its bottom (1), and its 4 side walls (1 each)
    // = 6. Greedy must still collapse the top into a single quad.
    let top = mesh.opaque.iter().filter(|q| q.face() as u8 == 2).count();
    assert_eq!(top, 1, "flat top surface must merge to one quad");
    assert_eq!(mesh.total_quads(), 6, "layer exposes top+bottom+4 sides");
    let naive = 6 * 32 * 32; // worst-case per-voxel face count
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

    // Meshing is async (background thread); drive the app until the spawned
    // task finishes and `apply_mesh_tasks` stores the result.
    for _ in 0..100 {
        if app
            .world()
            .resource::<MeshStorage>()
            .meshes
            .contains_key(&SectorCoord(0, 0, 0))
        {
            break;
        }
        app.update();
    }

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
