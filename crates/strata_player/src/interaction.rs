//! Block interaction (plan 14 §Block Interaction): branchless raycast-driven
//! break and place. Pure functions [`apply_break`]/[`apply_place`] mutate a
//! sector's [`XBrickMap`]; ECS systems [`player_break_system`]/[`player_place_system`]
//! drive them from `PlayerBreak`/`PlayerPlace` events (injectable in tests, no
//! window required) and insert `ChunkDirty` + `NeedsRemesh` for downstream meshing
//! and collider sync (Filter-First via the `With<PlayerController>` player query).

use bevy::prelude::*;
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
pub fn player_break_system(
    mut commands: Commands,
    mut break_ev: MessageReader<PlayerBreak>,
    mut pool: ResMut<GlobalBrickPool>,
    mut sectors: Query<(Entity, &SectorCoord, &mut XBrickMap, &mut SectorPalette)>,
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
    let sc = SectorCoord(
        (eye.x / 32.0).floor() as i32,
        (eye.y / 32.0).floor() as i32,
        (eye.z / 32.0).floor() as i32,
    );
    for (e, c, mut map, mut palette) in &mut sectors {
        if *c != sc {
            continue;
        }
        if let Some((v, n)) = raycast_voxel(&map, &pool, eye, dir, REACH) {
            let hit = RayHit {
                voxel: v,
                normal: n,
                sector_coord: sc,
            };
            if apply_break(&mut map, &mut pool, &mut palette, &hit) {
                commands.entity(e).insert(ChunkDirty).insert(NeedsRemesh);
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
    mut sectors: Query<(Entity, &SectorCoord, &mut XBrickMap, &mut SectorPalette)>,
    player: Query<(&Transform, &PlayerLook, &Inventory), With<PlayerController>>,
) {
    let mut triggered = false;
    for _ in place_ev.read() {
        triggered = true;
    }
    if !triggered {
        return;
    }
    let Ok((tf, look, inv)) = player.single() else {
        return;
    };
    let eye = tf.translation + Vec3::new(0.0, EYE_HEIGHT, 0.0);
    let dir = look_direction(look);
    let sc = SectorCoord(
        (eye.x / 32.0).floor() as i32,
        (eye.y / 32.0).floor() as i32,
        (eye.z / 32.0).floor() as i32,
    );
    let block = inv.active_block();
    for (e, c, mut map, mut palette) in &mut sectors {
        if *c != sc {
            continue;
        }
        if let Some((v, n)) = raycast_voxel(&map, &pool, eye, dir, REACH) {
            let hit = RayHit {
                voxel: v,
                normal: n,
                sector_coord: sc,
            };
            if apply_place(
                &mut map,
                &mut pool,
                &mut palette,
                &hit,
                block,
                tf.translation,
            ) {
                commands.entity(e).insert(ChunkDirty).insert(NeedsRemesh);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
