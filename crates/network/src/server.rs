use std::net::{SocketAddr, UdpSocket};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_renet2::renet2::{ConnectionConfig, RenetServer};
use bevy_replicon_renet2::netcode::{
    NetcodeServerTransport, NativeSocket, ServerAuthentication, ServerSetupConfig,
};
use bevy_replicon_renet2::RenetChannelsExt;
use crate::config::NetworkConfig;

pub struct ServerPlugin {
    pub _config: NetworkConfig,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_server);
    }
}

fn setup_server(
    channels: Res<RepliconChannels>,
    config: Res<NetworkConfig>,
    mut commands: Commands,
) {
    let connection_config = ConnectionConfig::from_channels(
        channels.server_configs(),
        channels.client_configs(),
    );

    let server = RenetServer::new(connection_config);
    commands.insert_resource(server);

    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap();

    let server_addr: SocketAddr = ([0, 0, 0, 0], config.server_port).into();
    let socket = UdpSocket::bind(server_addr).expect("Failed to bind UDP socket");
    let native_socket = NativeSocket::new(socket).expect("Failed to create native socket");

    let transport_config = ServerSetupConfig {
        current_time,
        max_clients: config.max_clients as usize,
        protocol_id: 0,
        socket_addresses: vec![vec![server_addr]],
        authentication: ServerAuthentication::Unsecure,
    };

    let transport =
        NetcodeServerTransport::new(transport_config, native_socket)
            .expect("Failed to create server transport");

    commands.insert_resource(transport);

    tracing::info!(
        "Server started on port {} (max {} clients)",
        config.server_port,
        config.max_clients
    );
}
