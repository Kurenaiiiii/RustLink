#![allow(dead_code)]

pub const SAMPLE_RATE: u32 = 48_000;
pub const PATH_VERSION: &str = "v4";
pub const MINIMUM_NODE_VERSION: &str = "22.22.2";
pub const REDIRECT_STATUS_CODES: &[u16] = &[301, 302, 303, 307, 308];
pub const DEFAULT_MAX_REDIRECTS: u32 = 5;
pub const HLS_SEGMENT_DOWNLOAD_CONCURRENCY_LIMIT: u32 = 5;
pub const DEFAULT_MAX_RESPONSE_BODY_BYTES: u64 = 32 * 1024 * 1024;

pub mod gateway_events {
    pub const WEBSOCKET_CLOSED: &str = "WebSocketClosedEvent";
    pub const TRACK_END: &str = "TrackEndEvent";
    pub const TRACK_START: &str = "TrackStartEvent";
    pub const TRACK_STUCK: &str = "TrackStuckEvent";
    pub const TRACK_EXCEPTION: &str = "TrackExceptionEvent";
    pub const SPONSORBLOCK_SEGMENTS_LOADED: &str = "SponsorBlockSegmentsLoadedEvent";
    pub const SPONSORBLOCK_SEGMENT_SKIPPED: &str = "SponsorBlockSegmentSkippedEvent";
    pub const PLAYER_UPDATE: &str = "playerUpdate";
    pub const CONNECTION_STATUS: &str = "ConnectionStatusEvent";
    pub const VOLUME_CHANGED: &str = "VolumeChangedEvent";
    pub const FILTERS_CHANGED: &str = "FiltersChangedEvent";
    pub const SEEK: &str = "SeekEvent";
    pub const PAUSE: &str = "PauseEvent";
    pub const PLAYER_CREATED: &str = "PlayerCreatedEvent";
    pub const PLAYER_DESTROYED: &str = "PlayerDestroyedEvent";
    pub const PLAYER_RECONNECTING: &str = "PlayerReconnectingEvent";
    pub const PLAYER_CONNECTED: &str = "PlayerConnectedEvent";
    pub const MIX_STARTED: &str = "MixStartedEvent";
    pub const MIX_ENDED: &str = "MixEndedEvent";
    pub const ETERNALBOX_INFO: &str = "EternalBoxInfoEvent";
    pub const ETERNALBOX_JUMP: &str = "EternalBoxJumpEvent";
    pub const STREAM_METADATA: &str = "StreamMetadataEvent";
}

pub mod end_reasons {
    pub const STOPPED: &str = "stopped";
    pub const FINISHED: &str = "finished";
    pub const LOAD_FAILED: &str = "loadFailed";
    pub const REPLACED: &str = "replaced";
    pub const CLEANUP: &str = "cleanup";
    pub const GAPLESS: &str = "gapless";
}

pub mod supported_formats {
    pub const OPUS: &str = "opus";
    pub const AAC: &str = "aac";
    pub const MPEG: &str = "mpeg";
    pub const FLAC: &str = "flac";
    pub const OGG_VORBIS: &str = "ogg-vorbis";
    pub const WAV: &str = "wav";
    pub const FLV: &str = "flv";
    pub const UNKNOWN: &str = "unknown";
}

pub fn normalize_format(type_str: Option<&str>) -> &'static str {
    let t = match type_str {
        Some(s) => s,
        None => return supported_formats::UNKNOWN,
    };
    let lower = t.to_lowercase();

    if lower.contains("opus") || lower.contains("webm") || lower.contains("weba") {
        return supported_formats::OPUS;
    }
    if lower.contains("aac")
        || lower.contains("mp4")
        || lower.contains("m4a")
        || lower.contains("m4v")
        || lower.contains("mov")
        || lower.contains("quicktime")
        || lower.contains("hls")
        || lower.contains("mpegurl")
        || lower.contains("fmp4")
        || lower.contains("mpegts")
    {
        return supported_formats::AAC;
    }
    if lower.contains("mpeg") || lower.contains("mp3") {
        return supported_formats::MPEG;
    }
    if lower.contains("flac") {
        return supported_formats::FLAC;
    }
    if lower.contains("ogg") || lower.contains("vorbis") {
        return supported_formats::OGG_VORBIS;
    }
    if lower.contains("wav") {
        return supported_formats::WAV;
    }
    if lower.contains("flv") {
        return supported_formats::FLV;
    }

    supported_formats::UNKNOWN
}
