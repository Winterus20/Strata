//! Block interaction (plan 14 §Block Interaction): branchless raycast-driven
//! break and place. Pure functions [`apply_break`]/[`apply_place`] mutate a
//! sector's [`XBrickMap`]; ECS systems [`player_break_system`]/[`player_place_system`]
//! drive them from `PlayerBreak`/`PlayerPlace` events (injectable in tests, no
//! window required) and insert `ChunkDirty` + `NeedsRemesh` for downstream meshing
//! and collider sync (Filter-First via the `With<PlayerController>` player query).

use bevy::prelude::*;
use strata_core::component::SectorSnapshot;
use strata_core::prelude::*;
use strata_physics::voxel_collider::sector_world_origin;

use crate::controller::{
    EYE_HEIGHT, PLAYER_HALF_HEIGHT, PLAYER_RADIUS, PlayerController, PlayerLook,
};
use crate::inventory::Inventory;
use crate::raycast::{FaceNormal, look_direction, raycast_voxel};

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

/// Remove the hit voxel (set to AIR). Returns `true` if a solid voxel was removed.
pub fn apply_break(
    map: &mut XBrickMap,
    pool: &mut GlobalBrickPool,
    palette: &mut SectorPalette,
    hit: &RayHit,
) -> bool {
    let was_occupied = map.is_occupied(pool, hit.voxel);
    map.set_block(pool, palette, hit.voxel, BlockId::AIR);
    let now_occupied = map.is_occupied(pool, hit.voxel);
    was_occupied && !now_occupied
}

/// Place `block` on the face of the hit voxel. Blocked (returns `false`) when the
/// target is outside the sector (sky), already occupied, or would overlap the
/// player's AABB.
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
    if map.is_occupied(pool, target) {
        return false; // target already solid
    }

    let origin = sector_world_origin(hit.sector_coord);
    let tw = origin + Vec3::new(tvx as f32, tvy as f32, tvz as f32);
    if voxel_overlaps_player(tw, player_center) {
        return false; // would intersect the player
    }

    map.set_block(pool, palette, target, block);
    true
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

/// ECS system: on `PlayerBreak`, raycast from the player and remove the hit voxel,
/// then mark the sector `ChunkDirty` + `NeedsRemesh`.
#[allow(clippy::type_complexity)]
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

/// ECS system: on `PlayerBreak`, raycast from the player and remove the hit voxel,
/// then mark the sector `ChunkDirty` + `NeedsRemesh`.
#[allow(clippy::type_complexity)]
pub fn player_break_system(
    mut commands: Commands,
    mut break_ev: MessageReader<PlayerBreak>,
    mut pool: ResMut<GlobalBrickPool>,
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

    let mut best_hit: Option<(Entity, SectorCoord, VoxelCoord, FaceNormal, f32)> = None;
    for (e, c, map, _, _) in sectors.iter() {
        if !is_sector_in_reach(c, eye, REACH) {
            continue;
        }
        if let Some((v, n, t)) = raycast_voxel(map, &pool, eye, dir, REACH) {
            if best_hit.is_none() || t < best_hit.unwrap().4 {
                best_hit = Some((e, *c, v, n, t));
            }
        }
    }

    if let Some((hit_entity, _hit_coord, v, n, _)) = best_hit {
        if let Ok((e, c, mut map, mut palette, snap)) = sectors.get_mut(hit_entity) {
            let hit = RayHit {
                voxel: v,
                normal: n,
                sector_coord: *c,
            };
            if apply_break(&mut map, &mut pool, &mut palette, &hit) {
                if let Some(mut snap) = snap {
                    *snap = SectorSnapshot(std::sync::Arc::new(map.pack(&pool, &palette)));
                }
                commands.entity(e).insert(ChunkDirty).insert(NeedsRemesh);

                let mut modified_neighbors = Vec::new();
                if v.x == 0 {
                    modified_neighbors.push(SectorCoord(c.0 - 1, c.1, c.2));
                } else if v.x == 31 {
                    modified_neighbors.push(SectorCoord(c.0 + 1, c.1, c.2));
                }
                if v.y == 0 {
                    modified_neighbors.push(SectorCoord(c.0, c.1 - 1, c.2));
                } else if v.y == 31 {
                    modified_neighbors.push(SectorCoord(c.0, c.1 + 1, c.2));
                }
                if v.z == 0 {
                    modified_neighbors.push(SectorCoord(c.0, c.1, c.2 - 1));
                } else if v.z == 31 {
                    modified_neighbors.push(SectorCoord(c.0, c.1, c.2 + 1));
                }

                if !modified_neighbors.is_empty() {
                    for (e_neigh, c_neigh, _, _, _) in sectors.iter() {
                        if modified_neighbors.contains(c_neigh) {
                            commands
                                .entity(e_neigh)
                                .insert(ChunkDirty)
                                .insert(NeedsRemesh);
                        }
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

    let mut best_hit: Option<(Entity, SectorCoord, VoxelCoord, FaceNormal, f32)> = None;
    for (e, c, map, _, _) in sectors.iter() {
        if !is_sector_in_reach(c, eye, REACH) {
            continue;
        }
        if let Some((v, n, t)) = raycast_voxel(map, &pool, eye, dir, REACH) {
            if best_hit.is_none() || t < best_hit.unwrap().4 {
                best_hit = Some((e, *c, v, n, t));
            }
        }
    }

    if let Some((_, hit_coord, v, n, _)) = best_hit {
        let (dx, dy, dz) = n.delta();
        let mut target_sc = hit_coord;
        let mut tx = v.x as i32 + dx;
        let mut ty = v.y as i32 + dy;
        let mut tz = v.z as i32 + dz;

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

        let mut target_entity = None;
        for (e, c, _, _, _) in sectors.iter() {
            if *c == target_sc {
                target_entity = Some(e);
                break;
            }
        }

        if let Some(target_ent) = target_entity {
            if let Ok((e, c, mut map, mut palette, snap)) = sectors.get_mut(target_ent) {
                let target_v = VoxelCoord::new(tx as u32, ty as u32, tz as u32);
                if !map.is_occupied(&pool, target_v) {
                    let origin = sector_world_origin(*c);
                    let tw = origin + Vec3::new(tx as f32, ty as f32, tz as f32);
                    if !voxel_overlaps_player(tw, tf.translation) {
                        map.set_block(&mut pool, &mut palette, target_v, block);
                        inv.consume_active();
                        if let Some(mut snap) = snap {
                            *snap = SectorSnapshot(std::sync::Arc::new(map.pack(&pool, &palette)));
                        }
                        commands.entity(e).insert(ChunkDirty).insert(NeedsRemesh);

                        let mut modified_neighbors = Vec::new();
                        if tx == 0 {
                            modified_neighbors.push(SectorCoord(c.0 - 1, c.1, c.2));
                        } else if tx == 31 {
                            modified_neighbors.push(SectorCoord(c.0 + 1, c.1, c.2));
                        }
                        if ty == 0 {
                            modified_neighbors.push(SectorCoord(c.0, c.1 - 1, c.2));
                        } else if ty == 31 {
                            modified_neighbors.push(SectorCoord(c.0, c.1 + 1, c.2));
                        }
                        if tz == 0 {
                            modified_neighbors.push(SectorCoord(c.0, c.1, c.2 - 1));
                        } else if tz == 31 {
                            modified_neighbors.push(SectorCoord(c.0, c.1, c.2 + 1));
                        }

                        if !modified_neighbors.is_empty() {
                            for (e_neigh, c_neigh, _, _, _) in sectors.iter() {
                                if modified_neighbors.contains(c_neigh) {
                                    commands
                                        .entity(e_neigh)
                                        .insert(ChunkDirty)
                                        .insert(NeedsRemesh);
                                }
                            }
                        }
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
        );
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
        assert!(apply_break(&mut map, &mut pool, &mut palette, &hit));
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
        );
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
        );
        map.set_block(
            &mut pool,
            &mut palette,
            VoxelCoord::new(6, 7, 3),
            BlockId(1),
        );
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
    fn test_place_block_reduces_inventory() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GlobalBrickPool::new());
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
        );
        let snapshot = SectorSnapshot(std::sync::Arc::new(map.pack(&pool, &palette)));
        let sector = app
            .world_mut()
            .spawn((SectorCoord(0, 0, 0), map, palette, snapshot))
            .id();

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
