use bevy_ecs::prelude::*;
use strata_core::BlockPos;

#[derive(Event, Debug)]
pub struct BlockBreakEvent(pub BlockPos);

#[derive(Event, Debug)]
pub struct BlockPlaceEvent {
    pub position: BlockPos,
    pub block_id: u16,
}
