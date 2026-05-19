use bevy_app::prelude::*;
use bevy_replicon::prelude::*;
use crate::events::*;

pub struct NetworkProtocolPlugin;

impl Plugin for NetworkProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.replicate::<strata_ecs::components::Position>()
           .replicate::<strata_ecs::components::Velocity>();

        app.add_client_event::<PlayerInputEvent>(Channel::Unordered)
           .add_client_event::<BlockInteractEvent>(Channel::Ordered)
           .add_client_event::<ChatMessageEvent>(Channel::Ordered)
           .add_server_event::<EntitySpawnEvent>(Channel::Unordered);
    }
}
