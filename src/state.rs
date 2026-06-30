use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

use crate::cache::TrackCache;
use crate::cluster::ClusterManager;
use crate::config::NodeLinkConfig;
use crate::managers::lyrics_manager::LyricsManager;
use crate::managers::meaning_manager::MeaningManager;
use crate::managers::player_manager::{PlayerManager, PlayerManagerConfig};
use crate::managers::connection_manager::ConnectionManager;
use crate::managers::rate_limit_manager::RateLimitManager;
use crate::managers::route_planner_manager::RoutePlannerManager;
use crate::managers::session_manager::SessionManager;
use crate::managers::source_manager::SourceManager;
use crate::managers::stats_manager::StatsManager;
use crate::managers::track_cache_manager::TrackCacheManager;
use crate::player::worker::WorkerCommand;
use crate::plugins::PluginManager;
use crate::providers::{GoogleTtsProvider, HttpProvider, LocalProvider};
use crate::sources::SourceRegistry;
use crate::sources::amazonmusic::AmazonMusicSource;
use crate::sources::applemusic::AppleMusicSource;
use crate::sources::bandcamp::BandcampSource;
use crate::sources::bilibili::BilibiliSource;
use crate::sources::deezer::DeezerSource;
use crate::sources::gaana::GaanaSource;
use crate::sources::instagram::InstagramSource;
use crate::sources::jiosaavn::JioSaavnSource;
use crate::sources::pandora::PandoraSource;
use crate::sources::soundcloud::SoundCloudSource;
use crate::sources::twitch::TwitchSource;
use crate::sources::vkmusic::VKMusicSource;
use crate::sources::yandexmusic::YandexMusicSource;
use crate::sources::youtube::YouTubeSource;
use crate::sources::spotify::SpotifySource;
use crate::sources::eternalbox::EternalBoxSource;
use crate::sources::googledrive::GoogleDriveSource;
use crate::sources::reddit::RedditSource;
use crate::sources::telegram::TelegramSource;
use crate::sources::twitter::TwitterSource;
use crate::sources::stub::StubSource;
use base64::Engine;

pub type SharedState = Arc<AppState>;

#[derive(Debug, Clone, Default)]
pub struct LivePlayerState {
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
    pub voice: PlayerVoiceState,
    pub volume: u32,
    pub paused: bool,
    pub mixer: Option<serde_json::Value>,
    pub frames_sent: u64,
    pub frames_nulled: u64,
    pub frames_deficit: u64,
}

pub struct RoutePlannerState {
    pub ip_block_type: String,
    pub ip_block_size: String,
    pub rotating: bool,
    pub current_index: usize,
    pub addresses: Vec<String>,
    pub failing: Arc<Mutex<HashMap<String, String>>>,
    pub blocked: Arc<Mutex<HashSet<String>>>,
}

impl RoutePlannerState {
    pub fn new() -> Self {
        Self {
            ip_block_type: "Inet4Address".to_string(),
            ip_block_size: "0".to_string(),
            rotating: false,
            current_index: 0,
            addresses: Vec::new(),
            failing: Arc::new(Mutex::new(HashMap::new())),
            blocked: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

pub struct AppState {
    pub config: NodeLinkConfig,
    pub sessions: DashMap<String, Session>,
    pub api_requests: DashMap<String, u64>,
    pub sources: SourceRegistry,
    pub workers: RwLock<HashMap<String, mpsc::Sender<WorkerCommand>>>,
    pub ws_senders: RwLock<HashMap<String, mpsc::Sender<serde_json::Value>>>,
    pub player_states: DashMap<String, Arc<RwLock<LivePlayerState>>>,
    pub sponsorblock: DashMap<String, Vec<String>>,
    pub route_planner: RoutePlannerState,
    pub start_time: Instant,
    pub plugin_manager: Arc<PluginManager>,
    pub cluster_manager: Option<Arc<ClusterManager>>,
    pub lyrics_subscribers: Arc<tokio::sync::Mutex<HashSet<String>>>,
    pub ws_connections: Arc<std::sync::atomic::AtomicU32>,

    // New manager fields
    pub player_manager: Arc<PlayerManager>,
    pub session_manager: Arc<SessionManager>,
    pub stats_manager: Arc<StatsManager>,
    pub rate_limit_manager: Arc<RateLimitManager>,
    pub lyrics_manager: Arc<LyricsManager>,
    pub meaning_manager: Arc<MeaningManager>,
    pub source_manager: Arc<SourceManager>,
    pub track_cache_manager: Arc<TrackCacheManager>,
    pub route_planner_manager: Arc<RoutePlannerManager>,
    pub connection_manager: Arc<ConnectionManager>,
    pub worker_manager: Arc<crate::managers::worker_manager::WorkerPool>,
    pub source_worker_manager: Arc<crate::managers::source_worker_manager::SourceWorkerPool>,
}

impl AppState {
    pub async fn new(config: NodeLinkConfig) -> SharedState {
        let sources = SourceRegistry::default();

        if config.sources.http.enabled {
            sources.register(HttpProvider::default()).await;
        }
        if config.sources.local.enabled {
            sources
                .register(LocalProvider::new(config.sources.local.base_path.clone()))
                .await;
        }
        if config.sources.google_tts.enabled {
            sources
                .register(GoogleTtsProvider::new(
                    config.sources.google_tts.language.clone(),
                ))
                .await;
        }
        if config.sources.youtube.enabled {
            let youtube = YouTubeSource::new(
                config.sources.youtube.hl.clone(),
                config.sources.youtube.gl.clone(),
                config.sources.youtube.allow_itag.clone(),
                config.sources.youtube.refresh_tokens.clone(),
                config.sources.youtube.potoken.clone(),
                config.sources.youtube.po_token_endpoint.clone(),
            );
            youtube.start_background_tasks();
            sources.register(youtube).await;
        }
        if config.sources.spotify.enabled {
            sources
                .register(SpotifySource::new(
                    config.sources.spotify.client_id.clone(),
                    config.sources.spotify.client_secret.clone(),
                    config.sources.spotify.market.clone(),
                ))
                .await;
        }
        if config.sources.soundcloud.enabled {
            sources.register(SoundCloudSource::new()).await;
        }
        if config.sources.applemusic.enabled {
            sources.register(AppleMusicSource::new()).await;
        }
        if config.sources.deezer.enabled {
            sources.register(DeezerSource::new(config.sources.deezer.arl.clone())).await;
        }
        if config.sources.jiosaavn.enabled {
            sources.register(JioSaavnSource::new()).await;
        }
        if config.sources.pandora.enabled {
            sources.register(PandoraSource::new(
                config.sources.pandora.csrf_token.clone(),
                config.sources.pandora.remote_token_url.clone(),
            )).await;
        }
        if config.sources.vkmusic.enabled {
            sources.register(VKMusicSource::new(
                config.sources.vkmusic.user_token.clone(),
                config.sources.vkmusic.user_cookie.clone(),
            )).await;
        }
        if config.sources.yandexmusic.enabled {
            sources.register(YandexMusicSource::new(
                config.sources.yandexmusic.access_token.clone(),
                config.sources.yandexmusic.artist_load_limit,
                config.sources.yandexmusic.album_load_limit,
                config.sources.yandexmusic.playlist_load_limit,
                config.sources.yandexmusic.allow_unavailable,
            )).await;
        }
        if config.sources.qobuz.enabled {
            sources.register(StubSource::new("qobuz")).await;
        }
        if config.sources.bandcamp.enabled {
            sources.register(BandcampSource::new()).await;
        }
        if config.sources.audius.enabled {
            sources.register(StubSource::new("audius")).await;
        }
        if config.sources.audiomack.enabled {
            sources.register(StubSource::new("audiomack")).await;
        }
        if config.sources.mixcloud.enabled {
            sources.register(StubSource::new("mixcloud")).await;
        }
        if config.sources.vimeo.enabled {
            sources.register(StubSource::new("vimeo")).await;
        }
        if config.sources.twitch.enabled {
            sources.register(TwitchSource::new(None)).await;
        }
        if config.sources.reddit.enabled {
            sources.register(RedditSource::new()).await;
        }
        if config.sources.tumblr.enabled {
            sources.register(StubSource::new("tumblr")).await;
        }
        if config.sources.twitter.enabled {
            sources.register(TwitterSource::new()).await;
        }
        if config.sources.bilibili.enabled {
            sources.register(BilibiliSource::new()).await;
        }
        if config.sources.nicovideo.enabled {
            sources.register(StubSource::new("nicovideo")).await;
        }
        if config.sources.telegram.enabled {
            sources.register(TelegramSource::new()).await;
        }
        if config.sources.googledrive.enabled {
            sources.register(GoogleDriveSource::new()).await;
        }
        if config.sources.instagram.enabled {
            sources.register(InstagramSource::new()).await;
        }
        if config.sources.amazonmusic.enabled {
            sources.register(AmazonMusicSource::new()).await;
        }
        if config.sources.gaana.enabled {
            sources.register(GaanaSource::new()).await;
        }
        if config.enable_holo_tracks {
            sources.register(EternalBoxSource::new(&config.sources.eternalbox)).await;
        }

        let mut plugin_manager = PluginManager::new();
        // Convert config definitions to plugin definitions
        let defs: Vec<_> = config.plugins.definitions.iter().map(|d| {
            crate::plugins::PluginDefinition {
                name: d.name.clone(),
                source: d.source.clone(),
                path: d.path.clone(),
                package: d.package.clone(),
                config: d.config.clone(),
            }
        }).collect();
        plugin_manager.set_definitions(defs);
        plugin_manager.load_from_config(&config.plugins);
        // Load all defined plugins in master context
        plugin_manager.load(crate::plugins::PluginContextType::Master, "main").await;
        let plugin_manager = Arc::new(plugin_manager);

        let cluster_manager = if config.cluster.enabled {
            let cm = ClusterManager::new(&config.cluster);
            cm.start().await;
            Some(Arc::new(cm))
        } else {
            None
        };

        let cache_encryption_key = config.cache_encryption_key.clone();
        let cache = {
            let cache = TrackCache::new(21600, 5000);
            if let Some(ref key_b64) = cache_encryption_key {
                let mut key = [0u8; 32];
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(key_b64)
                    .unwrap_or_default();
                if decoded.len() == 32 {
                    key.copy_from_slice(&decoded);
                    cache.with_encryption(key)
                } else {
                    cache
                }
            } else {
                cache
            }
        };

        // Initialize new managers
        let source_manager = Arc::new(SourceManager::new(sources.clone()));
        let session_manager = Arc::new(SessionManager::new());
        let stats_manager = Arc::new(StatsManager::new());
        let route_planner_manager = Arc::new(RoutePlannerManager::new());
        let rate_limit_manager = Arc::new(RateLimitManager::new(
            crate::managers::rate_limit_manager::RateLimitConfig {
                enabled: config.rate_limit.enabled,
                global: crate::managers::rate_limit_manager::RateLimitRule {
                    max_requests: config.rate_limit.max_requests.max(100),
                    time_window_ms: config.rate_limit.window_ms,
                },
                per_ip: crate::managers::rate_limit_manager::RateLimitRule {
                    max_requests: 100,
                    time_window_ms: 10_000,
                },
                per_user_id: Some(crate::managers::rate_limit_manager::RateLimitRule {
                    max_requests: 50,
                    time_window_ms: 5_000,
                }),
                per_guild_id: Some(crate::managers::rate_limit_manager::RateLimitRule {
                    max_requests: 20,
                    time_window_ms: 5_000,
                }),
                ignore_paths: Vec::new(),
                ignore: crate::managers::rate_limit_manager::IgnoreConfig::default(),
                trust_proxy: false,
                max_entries: 10_000,
            },
        ));
        let lyrics_manager = Arc::new(LyricsManager::new(config.lyrics.clone()));
        let mut meaning_manager = MeaningManager::new();
        meaning_manager.load_from_config(&config.meanings);
        let meaning_manager = Arc::new(meaning_manager);
        let track_cache_manager = Arc::new(TrackCacheManager::new(cache));
        let player_manager = Arc::new(PlayerManager::new(
            sources.clone(),
            plugin_manager.clone(),
            PlayerManagerConfig {
                fade_config: config.audio.fading.clone(),
                crossfade_config: config.audio.crossfade.clone(),
                track_stuck_threshold_ms: config.track_stuck_threshold_ms,
                player_update_interval: config.player_update_interval,
                resample_quality: config.audio.resampling_quality.clone(),
                loudness_normalizer: config.audio.loudness_normalizer,
                lookahead_ms: config.audio.lookahead_ms,
                gate_threshold_lufs: config.audio.gate_threshold_lufs,
                sponsorblock_config: config.sponsorblock.clone(),
                event_timeout_ms: config.event_timeout_ms,
                max_stuck_recovery_attempts: 3,
            },
        ));

        let connection_manager = Arc::new(
            ConnectionManager::new()
                .with_config(&config.connection)
        );
        connection_manager.clone().start();

        let cluster_config = config.cluster.clone();
        let specialized_source_config = cluster_config.specialized_source_worker.clone();

        let source_worker_manager = {
            let sm = source_manager.clone();
            let lm = lyrics_manager.clone();
            let mm = meaning_manager.clone();
            let process_mode = cluster_config.process_mode.clone();
            Arc::new(crate::managers::source_worker_manager::SourceWorkerPool::new(
                specialized_source_config,
                process_mode,
                sm, lm, mm,
            ))
        };

        Arc::new(Self {
            config,
            sessions: DashMap::new(),
            api_requests: DashMap::new(),
            sources,
            workers: RwLock::new(HashMap::new()),
            ws_senders: RwLock::new(HashMap::new()),
            player_states: DashMap::new(),
            sponsorblock: DashMap::new(),
            route_planner: RoutePlannerState::new(),
            start_time: Instant::now(),
            plugin_manager,
            cluster_manager,
            lyrics_subscribers: Arc::new(Mutex::new(HashSet::new())),
            ws_connections: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            player_manager,
            session_manager,
            stats_manager,
            rate_limit_manager,
            lyrics_manager,
            meaning_manager,
            source_manager,
            track_cache_manager,
            route_planner_manager,
            connection_manager,
            worker_manager: Arc::new(crate::managers::worker_manager::WorkerPool::new(cluster_config)),
            source_worker_manager,
        })
    }

    pub fn increment_api_request(&self, path: &str) {
        self.api_requests
            .entry(path.to_owned())
            .and_modify(|count| *count += 1)
            .or_insert(1);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub resuming: bool,
    pub timeout: u64,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub guild_id: String,
    pub track: Option<serde_json::Value>,
    pub volume: u32,
    pub paused: bool,
    pub state: PlayerEventState,
    pub voice: PlayerVoiceState,
    pub filters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerEventState {
    pub time: u64,
    pub position: u64,
    pub connected: bool,
    pub ping: i64,
}

impl Default for PlayerEventState {
    fn default() -> Self {
        Self {
            time: 0,
            position: 0,
            connected: false,
            ping: -1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerVoiceState {
    pub token: Option<String>,
    pub endpoint: Option<String>,
    pub session_id: Option<String>,
    pub channel_id: Option<String>,
}
