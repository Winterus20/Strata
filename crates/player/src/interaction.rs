use bevy_ecs::prelude::*;
use bevy_math::{Quat, Vec3};
use bevy_rapier3d::prelude::*;
use bevy_transform::components::Transform;
use strata_core::BlockPos;
use strata_ecs::components::interaction::{BlockBreakEvent, BlockPlaceEvent};
use strata_inventory::components::Inventory;
use strata_physics::{ENTITY, PLAYER, RAYCAST, TERRAIN};

use crate::components::{Player, PlayerInput};

pub fn block_interaction_system(
    rapier_context: ReadRapierContext,
    mut commands: Commands,
    mut query: Query<(&Transform, &mut PlayerInput, &Player)>,
    mut inventory_query: Query<&mut Inventory>,
) {
    let Ok(rapier_context) = rapier_context.single() else {
        return;
    };

    for (transform, mut input, _player) in query.iter_mut() {
        if input.left_click {
            let ray_pos = transform.translation;
            let ray_dir = input.look_direction;
            let max_toi = 5.0;
            let solid = true;
            let filter = QueryFilter::new().groups(CollisionGroups::new(RAYCAST, TERRAIN));

            if let Some((_entity, hit)) =
                rapier_context.cast_ray_and_get_normal(ray_pos, ray_dir, max_toi, solid, filter)
            {
                let hit_point = hit.point + hit.normal * -0.01;
                let bp = hit_point.floor();
                let block_pos = BlockPos(glam::IVec3::new(bp.x as i32, bp.y as i32, bp.z as i32));
                commands.trigger(BlockBreakEvent(block_pos));
            }

            input.left_click = false;
        }

        if input.right_click {
            let ray_pos = transform.translation;
            let ray_dir = input.look_direction;
            let max_toi = 5.0;
            let solid = true;
            let filter = QueryFilter::new().groups(CollisionGroups::new(RAYCAST, TERRAIN));

            if let Some((_entity, hit)) =
                rapier_context.cast_ray_and_get_normal(ray_pos, ray_dir, max_toi, solid, filter)
            {
                let place_pos = hit.point + hit.normal * 0.01;
                let bp = place_pos.floor();
                let shape = Collider::cuboid(0.5, 0.5, 0.5);
                let shape_pos = bp + Vec3::new(0.5, 0.5, 0.5);
                let shape_filter =
                    QueryFilter::new().groups(CollisionGroups::new(RAYCAST, PLAYER | ENTITY));

                let mut intersects = false;
                rapier_context.intersect_shape(
                    shape_pos,
                    Quat::IDENTITY,
                    &*shape.raw,
                    shape_filter,
                    |_| {
                        intersects = true;
                        false
                    },
                );

                if !intersects {
                    let block_pos =
                        BlockPos(glam::IVec3::new(bp.x as i32, bp.y as i32, bp.z as i32));

                    let block_id = inventory_query
                        .single_mut()
                        .ok()
                        .and_then(|inv| inv.get_selected().map(|stack| stack.id))
                        .unwrap_or(1);

                    commands.trigger(BlockPlaceEvent {
                        position: block_pos,
                        block_id,
                    });
                }
            }

            input.right_click = false;
        }
    }
}
