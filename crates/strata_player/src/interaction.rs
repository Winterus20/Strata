//! Block interaction (plan 14 §Block Interaction): branchless raycast-driven
//! break and place. Pure functions [`apply_break`]/[`apply_place`] mutate a
//! sector's [`XBrickMap`]; ECS systems [`player_break_system`]/[`player_place_system`]
//! drive them from `PlayerBreak`/`PlayerPlace` events (injectable in tests, no
//! window required) and insert `ChunkDirty` + `NeedsRemesh` for downstream meshing
//! and collider sync (Filter-First via the `With<PlayerController>` player query).
//!
//! Prototype note: client-local break/place only — full server-authoritative
//! multiplayer interaction is out of scope for this module milestone.

use bevy::prelude::*;
use smallvec::SmallVec;
use strata_core::component::SectorSnapshot;
use strata_core::prelude::*;
use strata_physics::voxel_collider::sector_world_origin;

use crate::controller::{
    EYE_HEIGHT, PLAYER_HALF_HEIGHT, PLAYER_RADIUS, PlayerController, PlayerLook,
};
use crate::inventory::Inventory;
use crate::raycast::{FaceNormal, look_direction, raycast_voxel};

/// O(1) sector entity lookup by coordinate. Populated by the streaming system
/// (and test harnesses) so interaction systems never scan all sectors.
#[derive(Resource, Default)]
pub struct ChunkMap(pub std::collections::HashMap<SectorCoord, Entity>);

/// Maximum interaction reach in world units.
pub const REACH: f32 = 8.0;

/// A raycast hit: the hit voxel (sector-local), the face normal, and the owning
/// sector. Bundled so downstream logic can compute world-space target voxels.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    pub voxel: VoxelCoord,
    pub normal: FaceNormal,
    pub sector_coord: SectorCoord,
}

/// Event: the player requested a break (driven by input mapper or injected in tests).
#[derive(Debug, Clone, Copy, Message)]
pub struct PlayerBreak;

/// Event: the player requested a place (driven by input mapper or injected in tests).
#[derive(Debug, Clone, Copy, Message)]
pub struct PlayerPlace;

/// Remove the hit voxel (set to AIR). Returns the old `BlockId` that was
/// replaced, or `None` if the voxel was empty or `set_block` fails (palette full).
pub fn apply_break(
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    hit: &RayHit,
) -> Option<BlockId> {
    let was_occupied = map.is_occupied(pool, hit.voxel);
    if !was_occupied {
        return None;
    }
    let old_id = map.get_block(pool, palette, hit.voxel);
    match map.set_block(pool, palette, hit.voxel, BlockId::AIR) {
        Ok(()) => Some(old_id),
        Err(PaletteFullError) => {
            // AIR never inserts into the palette; treat as failed break.
            warn!(
                "apply_break: set_block AIR failed at {:?} (PaletteFullError)",
                hit.voxel
            );
            None
        }
    }
}

/// Place `block` on the face of the hit voxel (same-sector only — does not
/// resolve cross-sector targets). Blocked (returns `false`) when the target is
/// outside the sector (sky), already occupied, or would overlap the player's AABB.
pub fn apply_place(
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    hit: &RayHit,
    block: BlockId,
    player_center: Vec3,
) -> bool {
    let (dx, dy, dz) = hit.normal.delta();
    let tvx = hit.voxel.x as i32 + dx;
    let tvy = hit.voxel.y as i32 + dy;
    let tvz = hit.voxel.z as i32 + dz;

    // Outside the sector -> "sky": placing is not allowed here.
    if !(0..32).contains(&tvx) || !(0..32).contains(&tvy) || !(0..32).contains(&tvz) {
        return false;
    }
    let target = VoxelCoord::new(tvx as u32, tvy as u32, tvz as u32);
    apply_place_in_sector(
        map,
        pool,
        palette,
        hit.sector_coord,
        target,
        block,
        player_center,
    )
}

/// AABB overlap test between a target voxel (unit cube at `v`) and the player box.
#[inline]
fn voxel_overlaps_player(v: Vec3, center: Vec3) -> bool {
    let vmin = v;
    let vmax = v + Vec3::ONE;
    let pmin = center - Vec3::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT, PLAYER_RADIUS);
    let pmax = center + Vec3::new(PLAYER_RADIUS, PLAYER_HALF_HEIGHT, PLAYER_RADIUS);
    vmin.x < pmax.x
        && pmin.x < vmax.x
        && vmin.y < pmax.y
        && pmin.y < vmax.y
        && vmin.z < pmax.z
        && pmin.z < vmax.z
}

/// Resolve the target sector and local voxel coordinate for a place operation,
/// handling cross-sector wrapping when the target falls outside the hit sector.
#[inline]
fn resolve_place_target(
    hit_coord: SectorCoord,
    hit_voxel: VoxelCoord,
    normal: FaceNormal,
) -> (SectorCoord, VoxelCoord) {
    let (dx, dy, dz) = normal.delta();
    let mut target_sc = hit_coord;
    let mut tx = hit_voxel.x as i32 + dx;
    let mut ty = hit_voxel.y as i32 + dy;
    let mut tz = hit_voxel.z as i32 + dz;

    if tx < 0 {
        target_sc.0 -= 1;
        tx += 32;
    } else if tx >= 32 {
        target_sc.0 += 1;
        tx -= 32;
    }
    if ty < 0 {
        target_sc.1 -= 1;
        ty += 32;
    } else if ty >= 32 {
        target_sc.1 += 1;
        ty -= 32;
    }
    if tz < 0 {
        target_sc.2 -= 1;
        tz += 32;
    } else if tz >= 32 {
        target_sc.2 += 1;
        tz -= 32;
    }

    let target_v = VoxelCoord::new(tx as u32, ty as u32, tz as u32);
    (target_sc, target_v)
}

/// Core place logic: occupancy check, player overlap, set block.
/// Does NOT handle cross-sector resolution — the caller must resolve the target
/// sector and local voxel coordinates before calling.
#[inline]
fn apply_place_in_sector(
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    sector_coord: SectorCoord,
    target: VoxelCoord,
    block: BlockId,
    player_center: Vec3,
) -> bool {
    if map.is_occupied(pool, target) {
        return false;
    }
    let origin = sector_world_origin(sector_coord);
    let tw = origin + Vec3::new(target.x as f32, target.y as f32, target.z as f32);
    if voxel_overlaps_player(tw, player_center) {
        return false;
    }
    match map.set_block(pool, palette, target, block) {
        Ok(()) => true,
        Err(PaletteFullError) => {
            warn!(
                "apply_place: palette full at sector {:?} voxel {:?}; skipping place",
                sector_coord, target
            );
            false
        }
    }
}

/// Helper to check if a sector coordinate is within the 8m interaction AABB around `eye`.
#[inline]
pub fn is_sector_in_reach(c: &SectorCoord, eye: Vec3, reach: f32) -> bool {
    let min_x = ((eye.x - reach) / 32.0).floor() as i32;
    let max_x = ((eye.x + reach) / 32.0).floor() as i32;
    let min_y = ((eye.y - reach) / 32.0).floor() as i32;
    let max_y = ((eye.y + reach) / 32.0).floor() as i32;
    let min_z = ((eye.z - reach) / 32.0).floor() as i32;
    let max_z = ((eye.z + reach) / 32.0).floor() as i32;

    c.0 >= min_x && c.0 <= max_x && c.1 >= min_y && c.1 <= max_y && c.2 >= min_z && c.2 <= max_z
}

/// World-space min corner of a unit voxel (size 1.0).
#[inline]
pub fn voxel_world_min(sector: SectorCoord, voxel: VoxelCoord) -> Vec3 {
    sector_world_origin(sector) + Vec3::new(voxel.x as f32, voxel.y as f32, voxel.z as f32)
}

/// Outward expand so the selection wireframe sits on the voxel surface.
pub const SELECTION_OUTLINE_EXPAND: f32 = 0.002;

/// Cube edge endpoints (shared by full / front-facing outline builders).
const CUBE_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (1, 5),
    (5, 4),
    (4, 0),
    (2, 3),
    (3, 7),
    (7, 6),
    (6, 2),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
];

/// Per-face edge indices into [`CUBE_EDGES`] (+X, -X, +Y, -Y, +Z, -Z).
const FACE_EDGES: [[usize; 4]; 6] = [
    [9, 5, 11, 1],  // +X: (1,3),(3,7),(5,7),(1,5)
    [8, 7, 10, 3],  // -X: (0,2),(6,2),(4,6),(4,0)
    [4, 5, 6, 7],   // +Y: (2,3),(3,7),(7,6),(6,2)
    [0, 1, 2, 3],   // -Y: (0,1),(1,5),(5,4),(4,0)
    [2, 11, 6, 10], // +Z: (5,4),(5,7),(7,6),(4,6)
    [0, 9, 4, 8],   // -Z: (0,1),(1,3),(2,3),(0,2)
];

const FACE_NORMALS: [Vec3; 6] = [
    Vec3::X,
    Vec3::NEG_X,
    Vec3::Y,
    Vec3::NEG_Y,
    Vec3::Z,
    Vec3::NEG_Z,
];

/// 12 cube edges as a `LineList` (24 endpoints) around the unit voxel at `world_min`.
#[inline]
pub fn selection_outline_line_list(world_min: Vec3, expand: f32) -> [[f32; 3]; 24] {
    let corners = cube_corners(world_min, expand);
    let mut out = [[0.0f32; 3]; 24];
    for (i, &(a, b)) in CUBE_EDGES.iter().enumerate() {
        out[i * 2] = corners[a];
        out[i * 2 + 1] = corners[b];
    }
    out
}

/// Front-facing cube edges only (`N·V > 0` faces), as a `LineList`.
///
/// Edges shared by two front faces appear once. Typically 6–9 edges (12–18
/// verts). Alone this still X-rays: side-face edges that face the camera can
/// project through the hit face (e.g. bottom of a front side when looking at
/// +Y). Prefer [`selection_outline_hit_face`] for the on-screen outline.
#[inline]
pub fn selection_outline_front_facing(world_min: Vec3, expand: f32, eye: Vec3) -> Vec<[f32; 3]> {
    let center = world_min + Vec3::splat(0.5);
    let view = eye - center;
    let corners = cube_corners(world_min, expand);
    let mut edge_mask: u16 = 0;
    for (face, &n) in FACE_NORMALS.iter().enumerate() {
        if view.dot(n) > 0.0 {
            for &ei in &FACE_EDGES[face] {
                edge_mask |= 1u16 << ei;
            }
        }
    }
    // Degenerate (eye inside voxel): fall back to all 12.
    if edge_mask == 0 {
        edge_mask = (1u16 << 12) - 1;
    }
    let mut out = Vec::with_capacity(edge_mask.count_ones() as usize * 2);
    for (ei, &(a, b)) in CUBE_EDGES.iter().enumerate() {
        if edge_mask & (1u16 << ei) != 0 {
            out.push(corners[a]);
            out.push(corners[b]);
        }
    }
    out
}

/// Four edges of the raycast hit face only (Minecraft-like selection outline).
///
/// No back/side edges, so no depth buffer is required — preferred when visbuf
/// wireframe occlusion is unavailable or unreliable.
#[inline]
pub fn selection_outline_hit_face(world_min: Vec3, expand: f32, face: FaceNormal) -> [[f32; 3]; 8] {
    let corners = cube_corners(world_min, expand);
    let face_idx = match face {
        FaceNormal::PosX => 0,
        FaceNormal::NegX => 1,
        FaceNormal::PosY => 2,
        FaceNormal::NegY => 3,
        FaceNormal::PosZ => 4,
        FaceNormal::NegZ => 5,
    };
    let mut out = [[0.0f32; 3]; 8];
    for (i, &ei) in FACE_EDGES[face_idx].iter().enumerate() {
        let (a, b) = CUBE_EDGES[ei];
        out[i * 2] = corners[a];
        out[i * 2 + 1] = corners[b];
    }
    out
}

#[inline]
fn cube_corners(world_min: Vec3, expand: f32) -> [[f32; 3]; 8] {
    let min = world_min - Vec3::splat(expand);
    let max = world_min + Vec3::ONE + Vec3::splat(expand);
    [
        [min.x, min.y, min.z],
        [max.x, min.y, min.z],
        [min.x, max.y, min.z],
        [max.x, max.y, min.z],
        [min.x, min.y, max.z],
        [max.x, min.y, max.z],
        [min.x, max.y, max.z],
        [max.x, max.y, max.z],
    ]
}

/// Raycast across in-reach sectors and return the nearest solid hit (and distance).
///
/// Shared by break/place and the client selection outline so all use the same
/// solidity rule (`AIR` skip + [`BlockRegistry::is_solid`]).
pub fn pick_solid_voxel<'a>(
    eye: Vec3,
    dir: Vec3,
    pool: &GlobalBrickPool,
    registry: &BlockRegistry,
    sectors: impl IntoIterator<Item = (&'a SectorCoord, &'a XBrickMap, &'a SectorPalette)>,
) -> Option<(RayHit, f32)> {
    let mut best: Option<(RayHit, f32)> = None;
    for (c, map, palette) in sectors {
        if !is_sector_in_reach(c, eye, REACH) {
            continue;
        }
        let is_solid = |v: VoxelCoord| {
            let id = map.get_block(pool, palette, v);
            id != BlockId::AIR && registry.is_solid(id)
        };
        if let Some((v, n, t)) = raycast_voxel(map, pool, eye, dir, REACH, is_solid)
            && best.as_ref().is_none_or(|(_, bt)| t < *bt)
        {
            best = Some((
                RayHit {
                    voxel: v,
                    normal: n,
                    sector_coord: *c,
                },
                t,
            ));
        }
    }
    best
}

/// ECS system: on `PlayerBreak`, raycast from the player and remove the hit voxel,
/// then mark the sector `ChunkDirty` + `NeedsRemesh`.
#[allow(clippy::type_complexity)]
pub fn player_break_system(
    mut commands: Commands,
    mut break_ev: MessageReader<PlayerBreak>,
    mut pool: ResMut<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    chunk_map: Res<ChunkMap>,
    mut sectors: Query<(
        Entity,
        &SectorCoord,
        &mut XBrickMap,
        &mut SectorPalette,
        Option<&mut SectorSnapshot>,
    )>,
    player: Query<(&Transform, &PlayerLook), With<PlayerController>>,
) {
    let mut triggered = false;
    for _ in break_ev.read() {
        triggered = true;
    }
    if !triggered {
        return;
    }
    let Ok((tf, look)) = player.single() else {
        return;
    };
    let eye = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let dir = look_direction(look);

    let Some((hit, _)) = pick_solid_voxel(
        eye,
        dir,
        &pool,
        &registry,
        sectors
            .iter()
            .map(|(_, c, map, palette, _)| (c, map, palette)),
    ) else {
        return;
    };

    let hit_entity = chunk_map.0.get(&hit.sector_coord).copied();
    let Some(hit_entity) = hit_entity else {
        return;
    };

    if let Ok((e, c, mut map, mut palette, snap)) = sectors.get_mut(hit_entity) {
        let v = hit.voxel;
        let sc = *c; // Capture sector coord before mutable borrow is used below.
        if let Some(old_id) = apply_break(&mut map, &mut pool, &mut palette, &hit) {
            if let Some(mut snap) = snap
                && let Ok(packed) = map.pack(&pool, &palette)
            {
                *snap = SectorSnapshot(std::sync::Arc::new(packed));
            }
            commands
                .entity(e)
                .insert(ChunkDirty)
                .insert(NeedsRemesh)
                .insert(DirtyVoxel {
                    voxel: v,
                    old_block: old_id,
                });

            let mut modified_neighbors: SmallVec<[SectorCoord; 6]> = SmallVec::new();
            if v.x == 0 {
                modified_neighbors.push(SectorCoord(sc.0 - 1, sc.1, sc.2));
            } else if v.x == 31 {
                modified_neighbors.push(SectorCoord(sc.0 + 1, sc.1, sc.2));
            }
            if v.y == 0 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1 - 1, sc.2));
            } else if v.y == 31 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1 + 1, sc.2));
            }
            if v.z == 0 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1, sc.2 - 1));
            } else if v.z == 31 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1, sc.2 + 1));
            }

            if !modified_neighbors.is_empty() {
                for neigh_sc in &modified_neighbors {
                    if let Some(e_neigh) = chunk_map.0.get(neigh_sc)
                        && let Ok((_, c_neigh, neigh_map, neigh_palette, _)) = sectors.get(*e_neigh)
                    {
                        // Compute the border voxel coordinate in the neighbor's
                        // local frame so the lighting system can do column-only
                        // sky recomputation.
                        let neigh_voxel = VoxelCoord::new(
                            if c_neigh.0 < sc.0 {
                                31
                            } else if c_neigh.0 > sc.0 {
                                0
                            } else {
                                v.x
                            },
                            if c_neigh.1 < sc.1 {
                                31
                            } else if c_neigh.1 > sc.1 {
                                0
                            } else {
                                v.y
                            },
                            if c_neigh.2 < sc.2 {
                                31
                            } else if c_neigh.2 > sc.2 {
                                0
                            } else {
                                v.z
                            },
                        );
                        // Look up the neighbor's border voxel's actual block so
                        // the lighting system's `remove_source` decision is based
                        // on the neighbor's voxel, not the edited voxel's old block.
                        let neigh_old_block =
                            neigh_map.get_block(&pool, neigh_palette, neigh_voxel);
                        commands
                            .entity(*e_neigh)
                            .insert(ChunkDirty)
                            .insert(NeedsRemesh)
                            .insert(DirtyVoxel {
                                voxel: neigh_voxel,
                                old_block: neigh_old_block,
                            });
                    }
                }
            }
        }
    }
}

/// ECS system: on `PlayerPlace`, raycast and place the active hotbar block at the
/// face neighbour, unless blocked (sky / occupied / player overlap).
#[allow(clippy::type_complexity)]
pub fn player_place_system(
    mut commands: Commands,
    mut place_ev: MessageReader<PlayerPlace>,
    mut pool: ResMut<GlobalBrickPool>,
    registry: Res<BlockRegistry>,
    chunk_map: Res<ChunkMap>,
    mut sectors: Query<(
        Entity,
        &SectorCoord,
        &mut XBrickMap,
        &mut SectorPalette,
        Option<&mut SectorSnapshot>,
    )>,
    mut player: Query<(&Transform, &PlayerLook, &mut Inventory), With<PlayerController>>,
) {
    let mut triggered = false;
    for _ in place_ev.read() {
        triggered = true;
    }
    if !triggered {
        return;
    }
    let Ok((tf, look, mut inv)) = player.single_mut() else {
        return;
    };
    let active_stack = inv.active_slot();
    if active_stack.count == 0 || active_stack.block == BlockId::AIR {
        return;
    }
    let block = active_stack.block;

    let eye = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let dir = look_direction(look);

    let Some((hit, _)) = pick_solid_voxel(
        eye,
        dir,
        &pool,
        &registry,
        sectors
            .iter()
            .map(|(_, c, map, palette, _)| (c, map, palette)),
    ) else {
        return;
    };

    let (target_sc, target_v) = resolve_place_target(hit.sector_coord, hit.voxel, hit.normal);

    let Some(target_ent) = chunk_map.0.get(&target_sc).copied() else {
        return;
    };
    if let Ok((e, c, mut map, mut palette, snap)) = sectors.get_mut(target_ent) {
        let sc = *c; // Capture before the mutable borrow is held across the iter() below.
        if apply_place_in_sector(
            &mut map,
            &mut pool,
            &mut palette,
            sc,
            target_v,
            block,
            tf.translation,
        ) {
            inv.consume_active();
            if let Some(mut snap) = snap
                && let Ok(packed) = map.pack(&pool, &palette)
            {
                *snap = SectorSnapshot(std::sync::Arc::new(packed));
            }
            commands
                .entity(e)
                .insert(ChunkDirty)
                .insert(NeedsRemesh)
                .insert(DirtyVoxel {
                    voxel: target_v,
                    old_block: BlockId::AIR,
                });

            let mut modified_neighbors: SmallVec<[SectorCoord; 6]> = SmallVec::new();
            let tx = target_v.x as i32;
            let ty = target_v.y as i32;
            let tz = target_v.z as i32;
            if tx == 0 {
                modified_neighbors.push(SectorCoord(sc.0 - 1, sc.1, sc.2));
            } else if tx == 31 {
                modified_neighbors.push(SectorCoord(sc.0 + 1, sc.1, sc.2));
            }
            if ty == 0 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1 - 1, sc.2));
            } else if ty == 31 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1 + 1, sc.2));
            }
            if tz == 0 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1, sc.2 - 1));
            } else if tz == 31 {
                modified_neighbors.push(SectorCoord(sc.0, sc.1, sc.2 + 1));
            }

            if !modified_neighbors.is_empty() {
                for neigh_sc in &modified_neighbors {
                    if let Some(e_neigh) = chunk_map.0.get(neigh_sc)
                        && let Ok((_, c_neigh, neigh_map, neigh_palette, _)) = sectors.get(*e_neigh)
                    {
                        let neigh_voxel = VoxelCoord::new(
                            if c_neigh.0 < sc.0 {
                                31
                            } else if c_neigh.0 > sc.0 {
                                0
                            } else {
                                target_v.x
                            },
                            if c_neigh.1 < sc.1 {
                                31
                            } else if c_neigh.1 > sc.1 {
                                0
                            } else {
                                target_v.y
                            },
                            if c_neigh.2 < sc.2 {
                                31
                            } else if c_neigh.2 > sc.2 {
                                0
                            } else {
                                target_v.z
                            },
                        );
                        let neigh_old_block =
                            neigh_map.get_block(&pool, neigh_palette, neigh_voxel);
                        commands
                            .entity(*e_neigh)
                            .insert(ChunkDirty)
                            .insert(NeedsRemesh)
                            .insert(DirtyVoxel {
                                voxel: neigh_voxel,
                                old_block: neigh_old_block,
                            });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::PlayerState;

    fn harness() -> (XBrickMap, GlobalBrickPool, SectorPalette, RayHit) {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 3),
            BlockId(1),
        )
        .expect("test harness set_block");
        let hit = RayHit {
            voxel: VoxelCoord::new(5, 7, 3),
            normal: FaceNormal::PosX,
            sector_coord: SectorCoord(0, 0, 0),
        };
        (map, pool, palette, hit)
    }

    #[test]
    fn break_removes_solid_voxel() {
        let (mut map, mut pool, mut palette, hit) = harness();
        assert_eq!(
            apply_break(&mut map, &mut pool, &mut palette, &hit),
            Some(BlockId(1)),
            "must return old BlockId on success"
        );
        assert_eq!(
            map.get_block(&pool, &palette, hit.voxel),
            BlockId::AIR,
            "hit voxel must be AIR after break"
        );
    }

    #[test]
    fn place_puts_block_at_face_neighbour() {
        let (mut map, mut pool, mut palette, hit) = harness();
        // Normal +X -> place at (6,7,3). Player far away so no overlap.
        let ok = apply_place(
            &mut map,
            &mut pool,
            &mut palette,
            &hit,
            BlockId(2),
            Vec3::new(100.0, 100.0, 100.0),
        );
        assert!(ok, "placement should succeed in free space");
        assert_eq!(
            map.get_block(&pool, &palette, VoxelCoord::new(6, 7, 3)),
            BlockId(2),
            "block placed at neighbour"
        );
    }

    #[test]
    fn place_blocked_by_player_overlap() {
        let (mut map, mut pool, mut palette, hit) = harness();
        // Player standing exactly where the placed block would go.
        let player_center = sector_world_origin(SectorCoord(0, 0, 0)) + Vec3::new(6.5, 7.5, 3.5);
        let ok = apply_place(
            &mut map,
            &mut pool,
            &mut palette,
            &hit,
            BlockId(2),
            player_center,
        );
        assert!(!ok, "placement must be blocked when overlapping the player");
        assert_eq!(
            map.get_block(&pool, &palette, VoxelCoord::new(6, 7, 3)),
            BlockId::AIR
        );
    }

    #[test]
    fn place_blocked_when_target_outside_sector() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(0, 7, 3),
            BlockId(1),
        )
        .expect("test edge set_block");
        // Hit the -X face of the edge block -> neighbour is at x=-1 (sky).
        let hit = RayHit {
            voxel: VoxelCoord::new(0, 7, 3),
            normal: FaceNormal::NegX,
            sector_coord: SectorCoord(0, 0, 0),
        };
        let ok = apply_place(
            &mut map,
            &mut pool,
            &mut palette,
            &hit,
            BlockId(2),
            Vec3::new(100.0, 100.0, 100.0),
        );
        assert!(!ok, "placement into the sky must be blocked");
    }

    #[test]
    fn place_blocked_when_target_occupied() {
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 3),
            BlockId(1),
        )
        .expect("test occupied set_block");
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(6, 7, 3),
            BlockId(1),
        )
        .expect("test occupied neighbour set_block");
        let hit = RayHit {
            voxel: VoxelCoord::new(5, 7, 3),
            normal: FaceNormal::PosX,
            sector_coord: SectorCoord(0, 0, 0),
        };
        let ok = apply_place(
            &mut map,
            &mut pool,
            &mut palette,
            &hit,
            BlockId(2),
            Vec3::new(100.0, 100.0, 100.0),
        );
        assert!(!ok, "placement into an occupied voxel must be blocked");
    }

    #[test]
    fn spatial_filtering_bounds_check() {
        let eye = Vec3::new(10.0, 10.0, 10.0);
        assert!(is_sector_in_reach(&SectorCoord(0, 0, 0), eye, REACH));
        assert!(!is_sector_in_reach(&SectorCoord(5, 0, 0), eye, REACH));
        assert!(!is_sector_in_reach(&SectorCoord(0, 5, 0), eye, REACH));
        assert!(!is_sector_in_reach(&SectorCoord(-5, 0, 0), eye, REACH));
    }

    #[test]
    fn selection_outline_has_twelve_edges_and_unit_span() {
        let min = Vec3::new(5.0, 7.0, 3.0);
        let lines = selection_outline_line_list(min, 0.0);
        assert_eq!(lines.len(), 24);
        // Every endpoint must lie on the unit cube AABB.
        for p in &lines {
            assert!((0.0..=1.0).contains(&(p[0] - min.x)));
            assert!((0.0..=1.0).contains(&(p[1] - min.y)));
            assert!((0.0..=1.0).contains(&(p[2] - min.z)));
        }
        // Each edge is axis-aligned and length 1.
        for edge in lines.chunks_exact(2) {
            let a = edge[0];
            let b = edge[1];
            let dx = (a[0] - b[0]).abs();
            let dy = (a[1] - b[1]).abs();
            let dz = (a[2] - b[2]).abs();
            let nonzero = [dx > 1e-6, dy > 1e-6, dz > 1e-6]
                .into_iter()
                .filter(|&x| x)
                .count();
            assert_eq!(nonzero, 1, "edge must be axis-aligned");
            assert!((dx + dy + dz - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn selection_outline_front_facing_culls_back_edges() {
        let min = Vec3::new(0.0, 0.0, 0.0);
        // Look from +X / +Y / +Z octant → three faces front → 9 unique edges.
        let eye = Vec3::new(3.0, 3.0, 3.0);
        let lines = selection_outline_front_facing(min, 0.0, eye);
        assert_eq!(lines.len(), 18, "3 front faces share 9 unique edges");
        for edge in lines.chunks_exact(2) {
            let a = edge[0];
            let b = edge[1];
            let dx = (a[0] - b[0]).abs();
            let dy = (a[1] - b[1]).abs();
            let dz = (a[2] - b[2]).abs();
            assert_eq!(
                [dx > 1e-6, dy > 1e-6, dz > 1e-6]
                    .into_iter()
                    .filter(|&x| x)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn selection_outline_hit_face_has_four_edges() {
        let min = Vec3::new(1.0, 2.0, 3.0);
        let lines = selection_outline_hit_face(min, 0.0, FaceNormal::PosY);
        assert_eq!(lines.len(), 8);
        // All endpoints on the +Y face (y = min.y + 1).
        for p in &lines {
            assert!((p[1] - (min.y + 1.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn selection_outline_expand_grows_aabb() {
        let min = Vec3::new(0.0, 0.0, 0.0);
        let lines = selection_outline_line_list(min, SELECTION_OUTLINE_EXPAND);
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &lines {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        assert!((lo[0] + SELECTION_OUTLINE_EXPAND).abs() < 1e-6);
        assert!((hi[0] - (1.0 + SELECTION_OUTLINE_EXPAND)).abs() < 1e-6);
    }

    #[test]
    fn pick_solid_voxel_hits_nearest() {
        let registry = load_block_registry();
        let stone = registry.id_by_name("stone").unwrap_or(BlockId(1));
        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(&mut pool, &mut palette, VoxelCoord::new(5, 7, 3), stone)
            .unwrap();
        map.set_block(&mut pool, &mut palette, VoxelCoord::new(8, 7, 3), stone)
            .unwrap();
        let eye = Vec3::new(10.5, 7.5, 3.5);
        let dir = Vec3::new(-1.0, 0.0, 0.0);
        let (hit, t) = pick_solid_voxel(
            eye,
            dir,
            &pool,
            &registry,
            [(&SectorCoord(0, 0, 0), &map, &palette)],
        )
        .expect("must hit");
        assert_eq!(hit.voxel, VoxelCoord::new(8, 7, 3));
        assert!(t < 3.0);
    }

    #[test]
    fn test_place_block_reduces_inventory() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GlobalBrickPool::new());
        app.insert_resource(load_block_registry());
        app.insert_resource(ChunkMap::default());
        app.add_message::<PlayerPlace>();
        app.add_systems(Update, player_place_system);

        let mut pool = GlobalBrickPool::new();
        let mut palette = SectorPalette::new();
        let mut map = XBrickMap::new(SectorCoord(0, 0, 0));
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(5, 7, 3),
            BlockId(1),
        )
        .expect("test inventory set_block");
        let snapshot = SectorSnapshot(std::sync::Arc::new(
            map.pack(&pool, &palette).expect("pack test sector"),
        ));
        let sector = app
            .world_mut()
            .spawn((SectorCoord(0, 0, 0), map, palette, snapshot))
            .id();

        app.world_mut()
            .resource_mut::<ChunkMap>()
            .0
            .insert(SectorCoord(0, 0, 0), sector);

        let player = app
            .world_mut()
            .spawn((
                PlayerController::default(),
                PlayerState::default(),
                PlayerLook::default(),
                Inventory::default(),
                Transform::from_translation(Vec3::new(5.5, 6.5, 6.5)),
            ))
            .id();

        app.world_mut().insert_resource(pool);

        // Initially 64 items
        let inv = app.world().entity(player).get::<Inventory>().unwrap();
        assert_eq!(inv.active_slot().count, 64);

        app.world_mut().write_message(PlayerPlace);
        app.update();

        // Check block was placed
        let pool = app.world().resource::<GlobalBrickPool>();
        let map = app.world().entity(sector).get::<XBrickMap>().unwrap();
        let palette = app.world().entity(sector).get::<SectorPalette>().unwrap();
        assert_eq!(
            map.get_block(pool, palette, VoxelCoord::new(5, 7, 4)),
            BlockId(1)
        );

        // Check inventory count reduced to 63
        let inv = app.world().entity(player).get::<Inventory>().unwrap();
        assert_eq!(inv.active_slot().count, 63);
    }
}
