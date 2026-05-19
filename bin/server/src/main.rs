#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use strata_network::{NetworkPlugin, NetworkMode, NetworkConfig};
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "strata-server", version, about = "Strata Headless Server")]
struct Args {
    #[arg(short, long, default_value_t = 27015)]
    port: u16,

    #[arg(short, long, default_value_t = 20)]
    tick_rate: u8,

    #[arg(short, long, default_value_t = 1024)]
    max_players: u16,

    #[arg(short, long, default_value_t = 10)]
    view_distance: u8,

    #[arg(short, long, default_value = "world")]
    world_name: String,

    #[arg(long, default_value_t = false)]
    creative_mode: bool,
}

#[derive(Resource, Default)]
struct TickCounter(pub u64);

#[derive(Resource)]
struct ServerStats {
    pub tick_rate_current: f32,
    pub _connected_players: u16,
    pub _loaded_chunks: u32,
    pub _memory_usage_mb: f32,
}

impl Default for ServerStats {
    fn default() -> Self {
        Self {
            tick_rate_current: 20.0,
            _connected_players: 0,
            _loaded_chunks: 0,
            _memory_usage_mb: 0.0,
        }
    }
}

fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("strata_server=info".parse().unwrap())
                .add_directive("strata_network=info".parse().unwrap()),
        )
        .init();

    let mut app = App::new();

    app.init_resource::<TickCounter>()
       .init_resource::<ServerStats>()
       .add_plugins(NetworkPlugin {
           config: NetworkConfig {
               server_port: args.port,
               tick_rate: args.tick_rate,
               max_clients: args.max_players,
               chunk_view_distance: args.view_distance,
               ..Default::default()
           },
           mode: NetworkMode::Server,
       });

    app.add_systems(Update, (server_tick, world_save_system, update_server_stats));

    tracing::info!("Strata Server starting on port {}", args.port);
    tracing::info!(
        "Tick rate: {} TPS, Max players: {}",
        args.tick_rate,
        args.max_players
    );

    loop {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(
            1000 / args.tick_rate as u64,
        ));
    }
}

fn server_tick(mut tick_counter: ResMut<TickCounter>) {
    tick_counter.0 += 1;
}

fn world_save_system(tick: Res<TickCounter>) {
    if tick.0 > 0 && tick.0.is_multiple_of(600) {
        tracing::debug!("World save tick");
    }
}

fn update_server_stats(mut stats: ResMut<ServerStats>) {
    stats.tick_rate_current = 20.0;
}
