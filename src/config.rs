use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeLinkConfig {
    pub server: ServerConfig,
    pub cluster: ClusterConfig,
    pub logging: LoggingConfig,
    pub connection: ConnectionConfig,
    pub metrics: MetricsConfig,
    pub rate_limit: RateLimitConfig,
    pub dos_protection: DosProtectionConfig,
    pub sources: SourcesConfig,
    pub lyrics: LyricsConfig,
    pub meanings: MeaningsConfig,
    pub audio: AudioConfig,
    pub filters: FiltersConfig,
    pub sponsorblock: SponsorBlockConfig,
    pub plugins: PluginsConfig,
    #[serde(rename = "voiceReceive")]
    pub voice_receive: VoiceReceiveConfig,
    #[serde(rename = "routePlanner")]
    pub route_planner: RoutePlannerConfig,
    pub mix: MixConfig,
    #[serde(rename = "pluginConfig")]
    pub plugin_config: Option<serde_json::Value>,
    pub default_search_source: Vec<String>,
    pub unified_search_sources: Vec<String>,
    pub max_search_results: usize,
    pub max_album_playlist_length: usize,
    pub player_update_interval: u64,
    pub stats_update_interval_ms: u64,
    pub track_stuck_threshold_ms: u64,
    pub event_timeout_ms: u64,
    pub zombie_threshold_ms: u64,
    pub enable_holo_tracks: bool,
    pub enable_track_stream_endpoint: bool,
    pub enable_load_stream_endpoint: bool,
    pub resolve_external_links: bool,
    pub fetch_channel_info: bool,
    pub cache_encryption_key: Option<String>,
}

impl Default for NodeLinkConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            cluster: ClusterConfig::default(),
            logging: LoggingConfig::default(),
            connection: ConnectionConfig::default(),
            metrics: MetricsConfig::default(),
            rate_limit: RateLimitConfig::default(),
            dos_protection: DosProtectionConfig::default(),
            sources: SourcesConfig::default(),
            lyrics: LyricsConfig::default(),
            meanings: MeaningsConfig::default(),
            audio: AudioConfig::default(),
            filters: FiltersConfig::default(),
            sponsorblock: SponsorBlockConfig::default(),
            plugins: PluginsConfig::default(),
            voice_receive: VoiceReceiveConfig::default(),
            route_planner: RoutePlannerConfig::default(),
            mix: MixConfig::default(),
            plugin_config: None,
            default_search_source: vec!["youtube".into(), "soundcloud".into()],
            unified_search_sources: vec!["youtube".into(), "soundcloud".into()],
            max_search_results: 50,
            max_album_playlist_length: 500,
            player_update_interval: 1_000,
            stats_update_interval_ms: 15_000,
            track_stuck_threshold_ms: 10_000,
            event_timeout_ms: 15_000,
            zombie_threshold_ms: 60_000,
            enable_holo_tracks: true,
            enable_track_stream_endpoint: true,
            enable_load_stream_endpoint: true,
            resolve_external_links: true,
            fetch_channel_info: true,
            cache_encryption_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub max_body_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".into(),
            port: 2333,
            password: "youshallnotpass".into(),
            max_body_size: 10 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceReceiveConfig {
    pub enabled: bool,
    pub format: String,
}

impl Default for VoiceReceiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            format: "opus".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RoutePlannerConfig {
    pub strategy: String,
    #[serde(rename = "bannedIpCooldown")]
    pub banned_ip_cooldown: u64,
    #[serde(rename = "ipBlocks")]
    pub ip_blocks: Vec<String>,
}

impl Default for RoutePlannerConfig {
    fn default() -> Self {
        Self {
            strategy: "RotateOnBan".into(),
            banned_ip_cooldown: 600_000,
            ip_blocks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MixConfig {
    pub enabled: bool,
    #[serde(rename = "defaultVolume")]
    pub default_volume: f64,
    #[serde(rename = "maxLayersMix")]
    pub max_layers_mix: usize,
    #[serde(rename = "autoCleanup")]
    pub auto_cleanup: bool,
}

impl Default for MixConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_volume: 0.8,
            max_layers_mix: 5,
            auto_cleanup: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterConfig {
    pub enabled: bool,
    pub workers: usize,
    pub min_workers: usize,
    pub command_timeout: u64,
    pub fast_command_timeout: u64,
    pub max_retries: usize,
    pub process_mode: WorkerProcessMode,
    pub specialized_source_worker: SpecializedSourceWorkerConfig,
    pub hibernation: HibernationConfig,
    pub scaling: ScalingConfig,
    pub redis_url: Option<String>,
    pub node_id: Option<String>,
    pub heartbeat_interval_secs: u64,
    pub node_timeout_secs: u64,
    pub endpoint: ClusterEndpointConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            workers: 0,
            min_workers: 1,
            command_timeout: 15_000,
            fast_command_timeout: 4_000,
            max_retries: 3,
            process_mode: WorkerProcessMode::InProcess,
            specialized_source_worker: SpecializedSourceWorkerConfig::default(),
            hibernation: HibernationConfig::default(),
            scaling: ScalingConfig::default(),
            redis_url: None,
            node_id: None,
            heartbeat_interval_secs: 10,
            node_timeout_secs: 30,
            endpoint: ClusterEndpointConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClusterEndpointConfig {
    pub code: String,
    pub patch_enabled: bool,
    pub allow_external_patch: bool,
}

impl Default for ClusterEndpointConfig {
    fn default() -> Self {
        Self {
            code: String::new(),
            patch_enabled: false,
            allow_external_patch: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HibernationConfig {
    pub enabled: bool,
    #[serde(rename = "timeoutMs")]
    pub timeout_ms: u64,
}

impl Default for HibernationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 1_200_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingConfig {
    #[serde(rename = "maxPlayersPerWorker")]
    pub max_players_per_worker: usize,
    #[serde(rename = "targetUtilization")]
    pub target_utilization: f64,
    #[serde(rename = "scaleUpThreshold")]
    pub scale_up_threshold: f64,
    #[serde(rename = "scaleDownThreshold")]
    pub scale_down_threshold: f64,
    #[serde(rename = "checkIntervalMs")]
    pub check_interval_ms: u64,
    #[serde(rename = "idleWorkerTimeoutMs")]
    pub idle_worker_timeout_ms: u64,
    #[serde(rename = "queueLengthScaleUpFactor")]
    pub queue_length_scale_up_factor: usize,
    #[serde(rename = "lagPenaltyLimit")]
    pub lag_penalty_limit: u64,
    #[serde(rename = "cpuPenaltyLimit")]
    pub cpu_penalty_limit: f64,
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self {
            max_players_per_worker: 20,
            target_utilization: 0.7,
            scale_up_threshold: 0.75,
            scale_down_threshold: 0.3,
            check_interval_ms: 5_000,
            idle_worker_timeout_ms: 60_000,
            queue_length_scale_up_factor: 5,
            lag_penalty_limit: 60,
            cpu_penalty_limit: 0.85,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpecializedSourceWorkerConfig {
    pub enabled: bool,
    pub count: usize,
    pub micro_workers: usize,
    pub tasks_per_worker: usize,
    pub silent_logs: bool,
}

impl Default for SpecializedSourceWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            count: 1,
            micro_workers: 2,
            tasks_per_worker: 100,
            silent_logs: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file: FileLoggingConfig,
    pub debug: DebugLoggingConfig,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            file: FileLoggingConfig::default(),
            debug: DebugLoggingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileLoggingConfig {
    pub enabled: bool,
    pub path: String,
    pub rotation: String,
    pub ttl_days: u32,
}

impl Default for FileLoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: "logs".into(),
            rotation: "daily".into(),
            ttl_days: 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DebugLoggingConfig {
    pub all: bool,
    pub request: bool,
    pub session: bool,
    pub player: bool,
    pub filters: bool,
    pub sources: bool,
    pub lyrics: bool,
    pub youtube: bool,
    #[serde(rename = "youtube-cipher")]
    pub youtube_cipher: bool,
    pub sabr: bool,
    pub potoken: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    pub log_all_checks: bool,
    pub interval: u64,
    pub timeout: u64,
    pub thresholds: ConnectionThresholds,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            log_all_checks: false,
            interval: 2_147_483_647,
            timeout: 2_000,
            thresholds: ConnectionThresholds::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionThresholds {
    pub bad: u64,
    pub average: u64,
}

impl Default for ConnectionThresholds {
    fn default() -> Self {
        Self { bad: 1, average: 5 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub username: String,
    pub password: Option<String>,
    pub interval: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            username: "admin".into(),
            password: None,
            interval: 5_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub max_requests: u32,
    pub window_ms: u64,
    pub global: RateLimitTierConfig,
    #[serde(rename = "perIp")]
    pub per_ip: RateLimitTierConfig,
    #[serde(rename = "perUserId")]
    pub per_user_id: RateLimitTierConfig,
    #[serde(rename = "perGuildId")]
    pub per_guild_id: RateLimitTierConfig,
    #[serde(rename = "ignorePaths")]
    pub ignore_paths: Vec<String>,
    pub ignore: RateLimitIgnoreConfig,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_requests: 300,
            window_ms: 60_000,
            global: RateLimitTierConfig {
                max_requests: 1000,
                time_window_ms: 60_000,
            },
            per_ip: RateLimitTierConfig {
                max_requests: 100,
                time_window_ms: 10_000,
            },
            per_user_id: RateLimitTierConfig {
                max_requests: 50,
                time_window_ms: 5_000,
            },
            per_guild_id: RateLimitTierConfig {
                max_requests: 20,
                time_window_ms: 5_000,
            },
            ignore_paths: Vec::new(),
            ignore: RateLimitIgnoreConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitTierConfig {
    #[serde(rename = "maxRequests")]
    pub max_requests: u32,
    #[serde(rename = "timeWindowMs")]
    pub time_window_ms: u64,
}

impl Default for RateLimitTierConfig {
    fn default() -> Self {
        Self {
            max_requests: 100,
            time_window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitIgnoreConfig {
    #[serde(rename = "userIds")]
    pub user_ids: Vec<String>,
    #[serde(rename = "guildIds")]
    pub guild_ids: Vec<String>,
    pub ips: Vec<String>,
}

impl Default for RateLimitIgnoreConfig {
    fn default() -> Self {
        Self {
            user_ids: Vec::new(),
            guild_ids: Vec::new(),
            ips: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DosProtectionConfig {
    pub enabled: bool,
    pub max_body_size: usize,
    pub max_requests_per_second: u32,
    pub delay_ms: u64,
    pub thresholds: DosThresholds,
    pub mitigation: DosMitigation,
    pub ignore: RateLimitIgnoreConfig,
}

impl Default for DosProtectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_body_size: 10 * 1024 * 1024,
            max_requests_per_second: 50,
            delay_ms: 0,
            thresholds: DosThresholds::default(),
            mitigation: DosMitigation::default(),
            ignore: RateLimitIgnoreConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DosThresholds {
    #[serde(rename = "burstRequests")]
    pub burst_requests: u32,
    #[serde(rename = "timeWindowMs")]
    pub time_window_ms: u64,
}

impl Default for DosThresholds {
    fn default() -> Self {
        Self {
            burst_requests: 50,
            time_window_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DosMitigation {
    #[serde(rename = "delayMs")]
    pub delay_ms: u64,
    #[serde(rename = "blockDurationMs")]
    pub block_duration_ms: u64,
}

impl Default for DosMitigation {
    fn default() -> Self {
        Self {
            delay_ms: 500,
            block_duration_ms: 300_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub quality: String,
    pub encryption: String,
    pub resampling_quality: String,
    pub loudness_normalizer: bool,
    pub lookahead_ms: u64,
    pub gate_threshold_lufs: f64,
    pub fading: FadingConfig,
    pub crossfade: CrossfadeConfig,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            quality: "high".into(),
            encryption: "aead_aes256_gcm_rtpsize".into(),
            resampling_quality: "best".into(),
            loudness_normalizer: true,
            lookahead_ms: 200,
            gate_threshold_lufs: -60.0,
            fading: FadingConfig::default(),
            crossfade: CrossfadeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FadingConfig {
    pub enabled: bool,
    pub track_start: FadingSection,
    pub track_end: FadingSection,
    pub track_stop: FadingSection,
    pub seek: FadingSection,
    pub pause: FadingSection,
    pub resume: FadingSection,
    pub ducking: FadingDuckingConfig,
}

impl Default for FadingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            track_start: FadingSection::default(),
            track_end: FadingSection::default(),
            track_stop: FadingSection::default(),
            seek: FadingSection::default(),
            pause: FadingSection {
                curve: "sinusoidal".into(),
                kind: "tape".into(),
                ..FadingSection::default()
            },
            resume: FadingSection {
                curve: "sinusoidal".into(),
                kind: "tape".into(),
                ..FadingSection::default()
            },
            ducking: FadingDuckingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FadingSection {
    pub duration: u64,
    pub curve: String,
    #[serde(rename = "type")]
    pub kind: String,
}

impl Default for FadingSection {
    fn default() -> Self {
        Self {
            duration: 0,
            curve: "linear".into(),
            kind: "volume".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FadingDuckingConfig {
    pub enabled: bool,
    pub duration: u64,
    #[serde(rename = "targetVolume")]
    pub target_volume: f64,
    pub curve: String,
}

impl Default for FadingDuckingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            duration: 0,
            target_volume: 0.3,
            curve: "linear".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CrossfadeConfig {
    pub enabled: bool,
    pub duration: u64,
    pub curve: String,
    pub mode: String,
    pub min_buffer_ms: u64,
    pub buffer_ms: u64,
}

impl Default for CrossfadeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration: 2_000,
            curve: "sinusoidal".into(),
            mode: "preload".into(),
            min_buffer_ms: 300,
            buffer_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FiltersConfig {
    pub enabled: EnabledFiltersConfig,
}

impl Default for FiltersConfig {
    fn default() -> Self {
        Self {
            enabled: EnabledFiltersConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EnabledFiltersConfig {
    pub tremolo: bool,
    pub vibrato: bool,
    pub lowpass: bool,
    pub highpass: bool,
    pub rotation: bool,
    pub karaoke: bool,
    pub distortion: bool,
    pub channel_mix: bool,
    pub equalizer: bool,
    pub chorus: bool,
    pub compressor: bool,
    pub echo: bool,
    pub phaser: bool,
    pub timescale: bool,
}

impl Default for EnabledFiltersConfig {
    fn default() -> Self {
        Self {
            tremolo: true,
            vibrato: true,
            lowpass: true,
            highpass: true,
            rotation: true,
            karaoke: true,
            distortion: true,
            channel_mix: true,
            equalizer: true,
            chorus: true,
            compressor: true,
            echo: true,
            phaser: true,
            timescale: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SponsorBlockConfig {
    pub enabled: bool,
    pub api: String,
    pub categories: Vec<String>,
    pub action_types: Vec<String>,
    pub skip_margin_ms: u64,
}

impl Default for SponsorBlockConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api: "https://sponsor.ajay.app".into(),
            categories: vec![
                "sponsor".into(),
                "selfpromo".into(),
                "interaction".into(),
                "intro".into(),
                "outro".into(),
                "preview".into(),
                "music_offtopic".into(),
                "filler".into(),
            ],
            action_types: vec!["skip".into()],
            skip_margin_ms: 150,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginsConfig {
    pub enabled: bool,
    pub paths: Vec<String>,
    pub definitions: Vec<PluginDefinitionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginDefinitionConfig {
    pub name: String,
    pub source: Option<String>,
    pub path: Option<String>,
    pub package: Option<String>,
    pub config: Option<serde_json::Value>,
}

impl Default for PluginDefinitionConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            source: None,
            path: None,
            package: None,
            config: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsConfig {
    pub fallback_source: String,
    pub youtube: SourceToggleConfig,
    pub genius: SourceToggleConfig,
    pub musixmatch: SourceToggleConfig,
    pub deezer: SourceToggleConfig,
    pub lrclib: SourceToggleConfig,
    pub letrasmus: SourceToggleConfig,
    pub bilibili: SourceToggleConfig,
    pub yandexmusic: SourceToggleConfig,
    pub monochrome: SourceToggleConfig,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            fallback_source: "genius".into(),
            youtube: SourceToggleConfig { enabled: true },
            genius: SourceToggleConfig { enabled: true },
            musixmatch: SourceToggleConfig { enabled: true },
            deezer: SourceToggleConfig { enabled: true },
            lrclib: SourceToggleConfig { enabled: true },
            letrasmus: SourceToggleConfig { enabled: true },
            bilibili: SourceToggleConfig { enabled: true },
            yandexmusic: SourceToggleConfig { enabled: true },
            monochrome: SourceToggleConfig { enabled: true },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MeaningsConfig {
    pub letrasmus: SourceToggleConfig,
    pub wikipedia: SourceToggleConfig,
}

impl Default for MeaningsConfig {
    fn default() -> Self {
        Self {
            letrasmus: SourceToggleConfig { enabled: true },
            wikipedia: SourceToggleConfig { enabled: true },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesConfig {
    pub http: SourceToggleConfig,
    pub local: LocalSourceConfig,
    #[serde(rename = "google-tts")]
    pub google_tts: GoogleTtsSourceConfig,
    pub youtube: YouTubeSourceConfig,
    pub spotify: SpotifySourceConfig,
    pub soundcloud: SourceToggleConfig,
    pub applemusic: AppleMusicSourceConfig,
    pub deezer: DeezerSourceConfig,
    pub jiosaavn: JiosaavnSourceConfig,
    pub yandexmusic: YandexMusicSourceConfig,
    pub qobuz: QobuzSourceConfig,
    pub bandcamp: SourceToggleConfig,
    pub audius: AudiusSourceConfig,
    pub audiomack: SourceToggleConfig,
    pub mixcloud: SourceToggleConfig,
    pub vimeo: SourceToggleConfig,
    pub twitch: SourceToggleConfig,
    pub reddit: SourceToggleConfig,
    pub tumblr: SourceToggleConfig,
    pub twitter: SourceToggleConfig,
    pub bilibili: BilibiliSourceConfig,
    pub nicovideo: SourceToggleConfig,
    pub telegram: SourceToggleConfig,
    pub googledrive: SourceToggleConfig,
    pub instagram: SourceToggleConfig,
    pub amazonmusic: SourceToggleConfig,
    pub gaana: GaanaSourceConfig,
    pub pandora: PandoraSourceConfig,
    pub vkmusic: VKMusicSourceConfig,
    pub eternalbox: EternalBoxConfig,
    pub bluesky: SourceToggleConfig,
    pub anghami: AnghamiSourceConfig,
    pub rss: SourceToggleConfig,
    pub songlink: SonglinkSourceConfig,
    pub iheartradio: SourceToggleConfig,
    pub shazam: ShazamSourceConfig,
    pub genius: SourceToggleConfig,
    pub pinterest: SourceToggleConfig,
    pub kwai: SourceToggleConfig,
    pub flowery: FlowerySourceConfig,
    pub lazypytts: LazyPyTTSConfig,
    pub pipertts: PiperTTSConfig,
    pub tidal: TidalSourceConfig,
    pub lastfm: LastFmSourceConfig,
    pub netease: NeteaseSourceConfig,
    pub letrasmus: SourceToggleConfig,
    pub monochrome: MonochromeSourceConfig,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            http: SourceToggleConfig { enabled: true },
            local: LocalSourceConfig::default(),
            google_tts: GoogleTtsSourceConfig::default(),
            youtube: YouTubeSourceConfig::default(),
            spotify: SpotifySourceConfig::default(),
            soundcloud: SourceToggleConfig { enabled: true },
            applemusic: AppleMusicSourceConfig::default(),
            deezer: DeezerSourceConfig::default(),
            jiosaavn: JiosaavnSourceConfig::default(),
            yandexmusic: YandexMusicSourceConfig::default(),
            qobuz: QobuzSourceConfig::default(),
            bandcamp: SourceToggleConfig { enabled: true },
            audius: AudiusSourceConfig::default(),
            audiomack: SourceToggleConfig { enabled: true },
            mixcloud: SourceToggleConfig { enabled: true },
            vimeo: SourceToggleConfig { enabled: true },
            twitch: SourceToggleConfig { enabled: true },
            reddit: SourceToggleConfig { enabled: true },
            tumblr: SourceToggleConfig { enabled: true },
            twitter: SourceToggleConfig { enabled: true },
            bilibili: BilibiliSourceConfig::default(),
            nicovideo: SourceToggleConfig { enabled: true },
            telegram: SourceToggleConfig { enabled: true },
            googledrive: SourceToggleConfig { enabled: true },
            instagram: SourceToggleConfig { enabled: true },
            amazonmusic: SourceToggleConfig { enabled: true },
            gaana: GaanaSourceConfig::default(),
            pandora: PandoraSourceConfig::default(),
            vkmusic: VKMusicSourceConfig::default(),
            eternalbox: EternalBoxConfig::default(),
            bluesky: SourceToggleConfig { enabled: true },
            anghami: AnghamiSourceConfig::default(),
            rss: SourceToggleConfig { enabled: true },
            songlink: SonglinkSourceConfig::default(),
            iheartradio: SourceToggleConfig { enabled: true },
            shazam: ShazamSourceConfig::default(),
            genius: SourceToggleConfig { enabled: true },
            pinterest: SourceToggleConfig { enabled: true },
            kwai: SourceToggleConfig { enabled: true },
            flowery: FlowerySourceConfig::default(),
            lazypytts: LazyPyTTSConfig::default(),
            pipertts: PiperTTSConfig::default(),
            tidal: TidalSourceConfig::default(),
            lastfm: LastFmSourceConfig::default(),
            netease: NeteaseSourceConfig::default(),
            letrasmus: SourceToggleConfig { enabled: true },
            monochrome: MonochromeSourceConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProxyConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            username: None,
            password: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceToggleConfig {
    pub enabled: bool,
}

impl Default for SourceToggleConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSourceConfig {
    pub enabled: bool,
    pub base_path: String,
}

impl Default for LocalSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_path: "./local-music".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GoogleTtsSourceConfig {
    pub enabled: bool,
    pub language: String,
}

impl Default for GoogleTtsSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            language: "en-US".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeezerSourceConfig {
    pub enabled: bool,
    pub arl: Option<String>,
    #[serde(rename = "decryptionKey")]
    pub decryption_key: Option<String>,
}

impl Default for DeezerSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            arl: None,
            decryption_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YouTubeSourceConfig {
    pub enabled: bool,
    pub allow_itag: Vec<u32>,
    pub target_itag: Option<u32>,
    pub get_oauth_token: bool,
    pub refresh_tokens: Vec<String>,
    pub potoken: Option<String>,
    pub po_token_endpoint: Option<String>,
    pub hl: String,
    pub gl: String,
    pub fallback_sources: Vec<String>,
    pub proxies: Vec<ProxyConfig>,
    pub clients: YouTubeClientsConfig,
    pub cipher: YouTubeCipherConfig,
}

impl Default for YouTubeSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_itag: vec![251, 140],
            target_itag: None,
            get_oauth_token: false,
            refresh_tokens: Vec::new(),
            potoken: None,
            po_token_endpoint: None,
            hl: "en".into(),
            gl: "US".into(),
            fallback_sources: vec![
                "soundcloud".into(),
                "deezer".into(),
                "jiosaavn".into(),
                "qobuz".into(),
                "bandcamp".into(),
                "audius".into(),
                "mixcloud".into(),
            ],
            proxies: Vec::new(),
            clients: YouTubeClientsConfig::default(),
            cipher: YouTubeCipherConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YouTubeClientsConfig {
    pub search: Vec<String>,
    pub playback: Vec<String>,
    pub resolve: Vec<String>,
    pub settings: Option<HashMap<String, serde_json::Value>>,
}

impl Default for YouTubeClientsConfig {
    fn default() -> Self {
        Self {
            search: vec!["Android".into()],
            playback: vec![
                "AndroidVR".into(),
                "TV".into(),
                "TVCast".into(),
                "WebEmbedded".into(),
                "WebParentTools".into(),
                "Web".into(),
                "IOS".into(),
            ],
            resolve: vec![
                "AndroidVR".into(),
                "TV".into(),
                "TVCast".into(),
                "WebEmbedded".into(),
                "WebParentTools".into(),
                "IOS".into(),
                "Web".into(),
            ],
            settings: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YouTubeCipherConfig {
    pub url: String,
    pub token: Option<String>,
}

impl Default for YouTubeCipherConfig {
    fn default() -> Self {
        Self {
            url: "https://cipher.kikkia.dev/api".into(),
            token: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpotifySourceConfig {
    pub enabled: bool,
    pub client_id: String,
    pub client_secret: String,
    pub external_auth_url: String,
    pub market: String,
    pub playlist_load_limit: usize,
    pub album_load_limit: usize,
    #[serde(rename = "allowExplicit")]
    pub allow_explicit: bool,
    pub allow_local_files: bool,
    pub sp_dc: String,
    #[serde(rename = "playlistPageLoadConcurrency")]
    pub playlist_page_load_concurrency: usize,
    #[serde(rename = "albumPageLoadConcurrency")]
    pub album_page_load_concurrency: usize,
}

impl Default for SpotifySourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            client_id: String::new(),
            client_secret: String::new(),
            external_auth_url: String::new(),
            market: "US".into(),
            playlist_load_limit: 0,
            album_load_limit: 0,
            allow_explicit: true,
            allow_local_files: true,
            sp_dc: String::new(),
            playlist_page_load_concurrency: 10,
            album_page_load_concurrency: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppleMusicSourceConfig {
    pub enabled: bool,
    #[serde(rename = "mediaApiToken")]
    pub media_api_token: String,
    pub market: String,
    #[serde(rename = "playlistLoadLimit")]
    pub playlist_load_limit: usize,
    #[serde(rename = "albumLoadLimit")]
    pub album_load_limit: usize,
    #[serde(rename = "playlistPageLoadConcurrency")]
    pub playlist_page_load_concurrency: usize,
    #[serde(rename = "albumPageLoadConcurrency")]
    pub album_page_load_concurrency: usize,
    #[serde(rename = "allowExplicit")]
    pub allow_explicit: bool,
}

impl Default for AppleMusicSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            media_api_token: "token_here".into(),
            market: "US".into(),
            playlist_load_limit: 0,
            album_load_limit: 0,
            playlist_page_load_concurrency: 5,
            album_page_load_concurrency: 5,
            allow_explicit: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PandoraSourceConfig {
    pub enabled: bool,
    pub csrf_token: Option<String>,
    pub remote_token_url: Option<String>,
}

impl Default for PandoraSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            csrf_token: None,
            remote_token_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VKMusicSourceConfig {
    pub enabled: bool,
    pub user_token: Option<String>,
    pub user_cookie: Option<String>,
    pub proxy: ProxyConfig,
}

impl Default for VKMusicSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user_token: None,
            user_cookie: None,
            proxy: ProxyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct YandexMusicSourceConfig {
    pub enabled: bool,
    pub access_token: Option<String>,
    pub artist_load_limit: usize,
    pub album_load_limit: usize,
    pub playlist_load_limit: usize,
    pub allow_unavailable: bool,
    pub allow_explicit: bool,
    pub proxy: ProxyConfig,
}

impl Default for YandexMusicSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            access_token: None,
            artist_load_limit: 1,
            album_load_limit: 1,
            playlist_load_limit: 1,
            allow_unavailable: false,
            allow_explicit: true,
            proxy: ProxyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EternalBoxConfig {
    pub enabled: bool,
    pub base_url: String,
    pub search_results: usize,
    #[serde(rename = "enrichSpotify")]
    pub enrich_spotify: bool,
    #[serde(rename = "includeAnalysis")]
    pub include_analysis: bool,
    #[serde(rename = "includeAnalysisSummary")]
    pub include_analysis_summary: bool,
    #[serde(rename = "eternalStream")]
    pub eternal_stream: bool,
    #[serde(rename = "cacheMaxBytes")]
    pub cache_max_bytes: usize,
    #[serde(rename = "maxBranches")]
    pub max_branches: usize,
    #[serde(rename = "maxBranchThreshold")]
    pub max_branch_threshold: usize,
    #[serde(rename = "branchThresholdStart")]
    pub branch_threshold_start: usize,
    #[serde(rename = "branchThresholdStep")]
    pub branch_threshold_step: usize,
    #[serde(rename = "branchTargetDivisor")]
    pub branch_target_divisor: usize,
    #[serde(rename = "addLastEdge")]
    pub add_last_edge: bool,
    #[serde(rename = "justBackwards")]
    pub just_backwards: bool,
    #[serde(rename = "justLongBranches")]
    pub just_long_branches: bool,
    #[serde(rename = "removeSequentialBranches")]
    pub remove_sequential_branches: bool,
    #[serde(rename = "useFilteredSegments")]
    pub use_filtered_segments: bool,
    #[serde(rename = "minRandomBranchChance")]
    pub min_random_branch_chance: f64,
    #[serde(rename = "maxRandomBranchChance")]
    pub max_random_branch_chance: f64,
    #[serde(rename = "randomBranchChanceDelta")]
    pub random_branch_chance_delta: f64,
    #[serde(rename = "timbreWeight")]
    pub timbre_weight: usize,
    #[serde(rename = "pitchWeight")]
    pub pitch_weight: usize,
    #[serde(rename = "loudStartWeight")]
    pub loud_start_weight: usize,
    #[serde(rename = "loudMaxWeight")]
    pub loud_max_weight: usize,
    #[serde(rename = "durationWeight")]
    pub duration_weight: usize,
    #[serde(rename = "confidenceWeight")]
    pub confidence_weight: usize,
    #[serde(rename = "infiniteStream")]
    pub infinite_stream: bool,
    pub max_reconnects: usize,
    #[serde(rename = "reconnectDelayMs")]
    pub reconnect_delay_ms: u64,
}

impl Default for EternalBoxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://eternalboxmirror.xyz".into(),
            search_results: 10,
            enrich_spotify: true,
            include_analysis: true,
            include_analysis_summary: true,
            eternal_stream: true,
            cache_max_bytes: 20 * 1024 * 1024,
            max_branches: 4,
            max_branch_threshold: 75,
            branch_threshold_start: 10,
            branch_threshold_step: 5,
            branch_target_divisor: 6,
            add_last_edge: true,
            just_backwards: false,
            just_long_branches: false,
            remove_sequential_branches: true,
            use_filtered_segments: true,
            min_random_branch_chance: 0.18,
            max_random_branch_chance: 0.5,
            random_branch_chance_delta: 0.09,
            timbre_weight: 1,
            pitch_weight: 10,
            loud_start_weight: 1,
            loud_max_weight: 1,
            duration_weight: 100,
            confidence_weight: 1,
            infinite_stream: true,
            max_reconnects: 0,
            reconnect_delay_ms: 1000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudiusSourceConfig {
    pub enabled: bool,
    #[serde(rename = "appName")]
    pub app_name: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(rename = "apiSecret")]
    pub api_secret: String,
    #[serde(rename = "playlistLoadLimit")]
    pub playlist_load_limit: usize,
    #[serde(rename = "albumLoadLimit")]
    pub album_load_limit: usize,
}

impl Default for AudiusSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            app_name: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            playlist_load_limit: 100,
            album_load_limit: 100,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GaanaSourceConfig {
    pub enabled: bool,
    #[serde(rename = "streamQuality")]
    pub stream_quality: String,
    #[serde(rename = "playlistLoadLimit")]
    pub playlist_load_limit: usize,
    #[serde(rename = "albumLoadLimit")]
    pub album_load_limit: usize,
    #[serde(rename = "artistLoadLimit")]
    pub artist_load_limit: usize,
    pub proxy: ProxyConfig,
}

impl Default for GaanaSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            stream_quality: "high".into(),
            playlist_load_limit: 100,
            album_load_limit: 100,
            artist_load_limit: 100,
            proxy: ProxyConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JiosaavnSourceConfig {
    pub enabled: bool,
    #[serde(rename = "playlistLoadLimit")]
    pub playlist_load_limit: usize,
    #[serde(rename = "artistLoadLimit")]
    pub artist_load_limit: usize,
    pub proxy: ProxyConfig,
    #[serde(rename = "secretKey")]
    pub secret_key: Option<String>,
}

impl Default for JiosaavnSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            playlist_load_limit: 50,
            artist_load_limit: 20,
            proxy: ProxyConfig::default(),
            secret_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BilibiliSourceConfig {
    pub enabled: bool,
    pub sessdata: Option<String>,
}

impl Default for BilibiliSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sessdata: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AnghamiSourceConfig {
    pub enabled: bool,
    pub cookies: Option<String>,
}

impl Default for AnghamiSourceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cookies: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShazamSourceConfig {
    pub enabled: bool,
    #[serde(rename = "allowExplicit")]
    pub allow_explicit: bool,
}

impl Default for ShazamSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_explicit: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FlowerySourceConfig {
    pub enabled: bool,
    pub voice: String,
    pub translate: bool,
    pub silence: u32,
    pub speed: f64,
    #[serde(rename = "enforceConfig")]
    pub enforce_config: bool,
}

impl Default for FlowerySourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            voice: "Salli".into(),
            translate: false,
            silence: 0,
            speed: 1.0,
            enforce_config: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LazyPyTTSConfig {
    pub enabled: bool,
    pub service: String,
    pub voice: String,
    #[serde(rename = "maxTextLength")]
    pub max_text_length: usize,
    #[serde(rename = "enforceConfig")]
    pub enforce_config: bool,
}

impl Default for LazyPyTTSConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            service: "Cerence".into(),
            voice: "Luciana".into(),
            max_text_length: 3000,
            enforce_config: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PiperTTSConfig {
    pub enabled: bool,
    pub url: String,
    pub voice: Option<String>,
    pub speaker: Option<u32>,
    #[serde(rename = "lengthScale")]
    pub length_scale: Option<f64>,
    #[serde(rename = "noiseScale")]
    pub noise_scale: Option<f64>,
    #[serde(rename = "noiseWScale")]
    pub noise_w_scale: Option<f64>,
}

impl Default for PiperTTSConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: "http://localhost:5000".into(),
            voice: None,
            speaker: None,
            length_scale: None,
            noise_scale: None,
            noise_w_scale: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QobuzSourceConfig {
    pub enabled: bool,
    pub user_token: Option<String>,
    #[serde(rename = "formatId")]
    pub format_id: String,
    #[serde(rename = "allowExplicit")]
    pub allow_explicit: bool,
}

impl Default for QobuzSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            user_token: None,
            format_id: "5".into(),
            allow_explicit: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LastFmSourceConfig {
    pub enabled: bool,
    #[serde(rename = "apiKey")]
    pub api_key: String,
}

impl Default for LastFmSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TidalSourceConfig {
    pub enabled: bool,
    pub token: String,
    #[serde(rename = "countryCode")]
    pub country_code: String,
    #[serde(rename = "playlistLoadLimit")]
    pub playlist_load_limit: usize,
    #[serde(rename = "playlistPageLoadConcurrency")]
    pub playlist_page_load_concurrency: usize,
    #[serde(rename = "hifiApis")]
    pub hifi_apis: Vec<String>,
    #[serde(rename = "hifiQualities")]
    pub hifi_qualities: Vec<String>,
}

impl Default for TidalSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            token: "token_here".into(),
            country_code: "US".into(),
            playlist_load_limit: 2,
            playlist_page_load_concurrency: 5,
            hifi_apis: vec![String::new()],
            hifi_qualities: vec![
                "HI_RES_LOSSLESS".into(),
                "LOSSLESS".into(),
                "HIGH".into(),
                "LOW".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MonochromeSourceConfig {
    pub enabled: bool,
    pub instances: Vec<String>,
    #[serde(rename = "streamingInstances")]
    pub streaming_instances: Vec<String>,
    pub quality: String,
}

impl Default for MonochromeSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            instances: Vec::new(),
            streaming_instances: Vec::new(),
            quality: "HI_RES_LOSSLESS".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NeteaseSourceConfig {
    pub enabled: bool,
}

impl Default for NeteaseSourceConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SonglinkSourceConfig {
    pub enabled: bool,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(rename = "userCountry")]
    pub user_country: String,
    #[serde(rename = "songIfSingle")]
    pub song_if_single: bool,
    #[serde(rename = "useApi")]
    pub use_api: bool,
    #[serde(rename = "useScrapeFallback")]
    pub use_scrape_fallback: bool,
    #[serde(rename = "preferredPlatforms")]
    pub preferred_platforms: Vec<String>,
    #[serde(rename = "fallbackToAny")]
    pub fallback_to_any: bool,
}

impl Default for SonglinkSourceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key: String::new(),
            user_country: "US".into(),
            song_if_single: true,
            use_api: true,
            use_scrape_fallback: true,
            preferred_platforms: vec![
                "spotify".into(),
                "appleMusic".into(),
                "youtubeMusic".into(),
                "youtube".into(),
                "deezer".into(),
                "tidal".into(),
                "amazonMusic".into(),
                "soundcloud".into(),
                "bandcamp".into(),
                "audius".into(),
                "audiomack".into(),
                "pandora".into(),
                "itunes".into(),
                "amazonStore".into(),
            ],
            fallback_to_any: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkerProcessMode {
    #[serde(rename = "in-process")]
    InProcess,
    #[serde(rename = "multi-process")]
    MultiProcess,
}

impl Default for WorkerProcessMode {
    fn default() -> Self {
        Self::InProcess
    }
}

impl NodeLinkConfig {
    pub fn load_or_default(path: &str) -> Result<Self, config::ConfigError> {
        let builder = config::Config::builder();
        let builder = if std::path::Path::new(path).exists() {
            builder.add_source(config::File::with_name(path).required(false))
        } else {
            builder
        };

        builder
            .add_source(config::Environment::with_prefix("NODELINK").separator("__"))
            .add_source(config::Environment::with_prefix("RUSTLINK").separator("__"))
            .build()?
            .try_deserialize()
    }

    pub fn validate(&self) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();

        if self.server.password.trim().is_empty() {
            warnings.push(ConfigWarning::new(
                "server.password",
                "server password is empty; all authenticated endpoints will reject clients",
            ));
        }
        if self.server.port == 0 {
            warnings.push(ConfigWarning::new(
                "server.port",
                "port 0 lets the OS choose a random port and is not suitable for Lavalink clients",
            ));
        }
        if self.default_search_source.is_empty() {
            warnings.push(ConfigWarning::new(
                "default_search_source",
                "no default search source is configured",
            ));
        }
        if self.player_update_interval == 0 {
            warnings.push(ConfigWarning::new(
                "player_update_interval",
                "player updates are disabled because the interval is 0",
            ));
        }
        if self.audio.crossfade.enabled && self.audio.crossfade.duration == 0 {
            warnings.push(ConfigWarning::new(
                "audio.crossfade.duration",
                "crossfade is enabled with a zero duration",
            ));
        }
        if self.metrics.enabled && self.metrics.password.is_none() {
            warnings.push(ConfigWarning::new(
                "metrics.password",
                "metrics uses the main server password because no metrics password is set",
            ));
        }
        if self.cluster.enabled && self.cluster.redis_url.is_none() {
            warnings.push(ConfigWarning::new(
                "cluster.redis_url",
                "clustering is enabled but no redis_url is configured",
            ));
        }
        if self.plugins.enabled && self.plugins.paths.is_empty() {
            warnings.push(ConfigWarning::new(
                "plugins.paths",
                "plugins are enabled but no plugin paths are configured",
            ));
        }
        if self.sources.spotify.enabled && self.sources.spotify.client_id.is_empty() {
            warnings.push(ConfigWarning::new(
                "sources.spotify.client_id",
                "Spotify is enabled but client_id is empty; search/resolve will fail",
            ));
        }
        if self.sources.spotify.enabled && self.sources.spotify.client_secret.is_empty() {
            warnings.push(ConfigWarning::new(
                "sources.spotify.client_secret",
                "Spotify is enabled but client_secret is empty; search/resolve will fail",
            ));
        }
        if self.sources.youtube.refresh_tokens.is_empty() && self.sources.youtube.get_oauth_token {
            warnings.push(ConfigWarning::new(
                "sources.youtube.refresh_tokens",
                "OAuth token acquisition is enabled but no refresh tokens are provided",
            ));
        }

        warnings
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub path: &'static str,
    pub message: &'static str,
}

impl ConfigWarning {
    pub fn new(path: &'static str, message: &'static str) -> Self {
        Self { path, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_contains_core_nodelink_sections() {
        let config = NodeLinkConfig::default();

        assert!(config.sources.youtube.enabled);
        assert!(config.sources.spotify.enabled);
        assert!(config.lyrics.genius.enabled);
        assert!(config.meanings.wikipedia.enabled);
        assert!(config.filters.enabled.equalizer);
        assert_eq!(config.audio.encryption, "aead_aes256_gcm_rtpsize");
        assert_eq!(config.sponsorblock.skip_margin_ms, 150);
    }

    #[test]
    fn validation_reports_dangerous_values() {
        let mut config = NodeLinkConfig::default();
        config.server.password = String::new();
        config.default_search_source.clear();
        config.audio.crossfade.enabled = true;
        config.audio.crossfade.duration = 0;

        let warnings = config.validate();
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "server.password"));
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "default_search_source"));
        assert!(warnings
            .iter()
            .any(|warning| warning.path == "audio.crossfade.duration"));
    }
}
