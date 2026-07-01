use rustlink::api;
use rustlink::config;
use rustlink::logging;
use rustlink::state::{AppState, SharedState};
use tracing::warn;

#[tokio::main]
async fn main() {
    const VERSION: &str = "1.2.0";

    let config = config::NodeLinkConfig::load_or_default("rustlink.toml").unwrap_or_default();

    logging::init_logging(&config.logging.level);

    for warning in config.validate() {
        warn!(target: "Config", "{}: {}", warning.path, warning.message);
    }

    logging::print_banner(VERSION);
    logging::started("Server", format!("version {}", VERSION));

    let port = config.server.port;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));

    if std::env::var("NODELINK_MEMORY_TRACE").is_ok() {
        logging::mem_trace(format!("bootstrap:starting-server rss=0.0MB"));
    }

    let state = AppState::new(config).await;
    let app = api::router_with_middleware(state.clone());

    // Spawn stats broadcaster every 30s
    let stats_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            broadcast_stats(&stats_state).await;
        }
    });

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    logging::started("Server", format!("Successfully listening on {}:{} (Rust Tokio)", addr.ip(), addr.port()));

    if std::env::var("NODELINK_MEMORY_TRACE").is_ok() {
        logging::mem_trace(format!("bootstrap:listening rss=0.0MB"));
    }

    axum::serve(listener, app).await.unwrap();
}

async fn broadcast_stats(state: &SharedState) {
    let players: usize = state.sessions.iter().map(|s| s.players.len()).sum();
    let mut playing_players = 0usize;
    let mut frames_sent: u64 = 0;
    let mut frames_nulled: u64 = 0;
    let mut frames_deficit: u64 = 0;

    for e in state.player_states.iter() {
        let ls = e.value().blocking_read();
        if !ls.paused {
            playing_players += 1;
        }
        frames_sent += ls.frames_sent;
        frames_nulled += ls.frames_nulled;
        frames_deficit += ls.frames_deficit;
    }

    let uptime = state.start_time.elapsed().as_millis() as u64;

    let stats = serde_json::json!({
        "op": "stats",
        "players": players,
        "playingPlayers": playing_players,
        "uptime": uptime,
        "memory": {
            "free": 0,
            "used": 0,
            "allocated": 0,
            "reservable": 0
        },
        "cpu": {
            "cores": std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1),
            "systemLoad": 0.0,
            "processLoad": 0.0
        },
        "frameStats": {
            "sent": frames_sent,
            "nulled": frames_nulled,
            "deficit": frames_deficit
        }
    });

    let senders = state.ws_senders.read().await;
    for (_id, sender) in senders.iter() {
        let _ = sender.try_send(stats.clone());
    }

    if let Some(ref cm) = state.cluster_manager {
        cm.update_player_count(players, playing_players).await;
    }
}
