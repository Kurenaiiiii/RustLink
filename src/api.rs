use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    constants::PATH_VERSION,
    player::worker::WorkerCommand,
    sources::{PlaylistData, PlaylistInfo, SourceResult},
    state::{Player, Session, SharedState},
    tracks::{decode_track, encode_track, TrackData, TrackInfo},
};

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/version", get(version))
        .route(&format!("/{PATH_VERSION}/version"), get(version))
        .route(&format!("/{PATH_VERSION}/info"), get(info))
        .route(&format!("/{PATH_VERSION}/stats"), get(stats))
        .route(&format!("/{PATH_VERSION}/metrics"), get(metrics))
        .route(
            &format!("/{PATH_VERSION}/decodetrack"),
            get(decode_track_route),
        )
        .route(
            &format!("/{PATH_VERSION}/decodetracks"),
            post(decode_tracks_route),
        )
        .route(
            &format!("/{PATH_VERSION}/encodetrack"),
            post(encode_track_route),
        )
        .route(
            &format!("/{PATH_VERSION}/encodedtracks"),
            post(encoded_tracks_route),
        )
        .route(&format!("/{PATH_VERSION}/loadtracks"), get(load_tracks))
        .route(&format!("/{PATH_VERSION}/loadstream"), get(load_stream))
        .route(&format!("/{PATH_VERSION}/trackstream"), get(track_stream))
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id"),
            get(get_session).patch(patch_session),
        )
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players"),
            get(get_players),
        )
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id"),
            get(get_player).patch(patch_player).delete(delete_player),
        )
        .route("/v4/routeplanner/status", get(routeplanner_status))
        .route(
            "/v4/routeplanner/free/address",
            post(routeplanner_free_address),
        )
        .route("/v4/routeplanner/free/all", post(routeplanner_free_all))
        .route("/v4/workers", get(workers_list).patch(workers_patch))
        .route("/v4/workers/:guild_id", delete(worker_terminate))
        .route("/v4/youtube/config", get(youtube_config_get).patch(youtube_config_patch))
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id/sponsorblock"),
            get(sponsorblock_get).patch(sponsorblock_patch).delete(sponsorblock_delete),
        )
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id/mixer"),
            get(get_mixer).patch(patch_mixer).post(post_mix),
        )
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id/mixer/layers"),
            post(post_mixer_layer),
        )
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id/mixer/layers/:layer_id"),
            patch(patch_mixer_layer).delete(delete_mixer_layer),
        )
        .route(&format!("/{PATH_VERSION}/loadlyrics"), get(load_lyrics))
        .route(&format!("/{PATH_VERSION}/loadchapters"), get(load_chapters))
        .route(&format!("/{PATH_VERSION}/meaning"), get(load_meaning))
        .route(&format!("/{PATH_VERSION}/connection"), get(connection_status))
        .route(&format!("/{PATH_VERSION}/profiler"), get(profiler))
        .route("/v4/youtube/oauth", get(youtube_oauth_get).post(youtube_oauth))
        .route(
            &format!("/{PATH_VERSION}/sessions/:session_id/players/:guild_id/lyrics/subscribe"),
            post(lyrics_subscribe).delete(lyrics_unsubscribe),
        )
        .route("/v4/websocket", get(crate::server::ws::websocket_handler))
        .route("/v4/profiler/socket", get(profiler_websocket))
        .route("/v4/profiler/file", get(profiler_file))
        .route("/v4/profiler/ui", get(profiler_ui))
        .with_state(state)
}

pub fn router_with_middleware(state: SharedState) -> Router {
    crate::middleware::apply_middleware(router(state.clone()), &state.config, Some(state.clone()))
}

fn authorized(headers: &HeaderMap, state: &SharedState) -> bool {
    let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    value == state.config.server.password
        || value == format!("Bearer {}", state.config.server.password)
}

fn require_auth(headers: &HeaderMap, state: &SharedState) -> Result<(), Response> {
    if authorized(headers, state) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "Unauthorized").into_response())
    }
}

async fn version(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/version");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    ([(header::CONTENT_TYPE, "text/plain")], "3.8.0").into_response()
}

async fn info(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/v4/info");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let source_managers = state.sources.source_names().await;
    let plugins = state.plugin_manager.get_loaded_plugins();

    Json(json!({
        "version": {
            "semver": "3.8.0",
            "major": 3,
            "minor": 8,
            "patch": 0,
            "prerelease": [],
            "build": []
        },
        "buildTime": 0,
        "git": {
            "branch": "rust-rewrite",
            "commit": "unknown",
            "commitTime": 0
        },
        "node": env!("CARGO_PKG_RUST_VERSION"),
        "voice": {
            "name": "rustlink-voice",
            "version": env!("CARGO_PKG_VERSION")
        },
        "isNodelink": true,
        "sourceManagers": source_managers,
        "filters": [
            "tremolo", "vibrato", "lowpass", "highpass",
            "rotation", "karaoke", "distortion", "channelMix",
            "equalizer", "chorus", "compressor", "echo",
            "phaser", "timescale", "spatial"
        ],
        "plugins": plugins
    }))
    .into_response()
}

async fn stats(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/v4/stats");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let players: usize = state.sessions.iter().map(|s| s.players.len()).sum();
    let mut playing_players = 0usize;
    let mut frames_sent: u64 = 0;
    let mut frames_nulled: u64 = 0;
    let mut frames_deficit: u64 = 0;

    for e in state.player_states.iter() {
        let ls = e.value().read().await;
        if !ls.paused {
            playing_players += 1;
        }
        frames_sent += ls.frames_sent;
        frames_nulled += ls.frames_nulled;
        frames_deficit += ls.frames_deficit;
    }

    let uptime = state.start_time.elapsed().as_millis() as u64;

    let snapshot = state.stats_manager.get_snapshot().await;

    Json(json!({
        "players": players,
        "playingPlayers": playing_players,
        "uptime": uptime,
        "memory": { "free": 0, "used": 0, "allocated": 0, "reservable": 0 },
        "cpu": {
            "cores": std::thread::available_parallelism().map(|c| c.get()).unwrap_or(1),
            "systemLoad": 0.0,
            "processLoad": 0.0
        },
        "frameStats": {
            "sent": frames_sent,
            "nulled": frames_nulled,
            "deficit": frames_deficit
        },
        "detailedStats": snapshot
    }))
    .into_response()
}

async fn metrics(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if !state.config.metrics.enabled {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let mut body = String::new();

    let players: usize = state.sessions.iter().map(|s| s.players.len()).sum();
    let mut playing_players = 0usize;
    let mut frames_sent: u64 = 0;

    for e in state.player_states.iter() {
        let ls = e.value().blocking_read();
        if !ls.paused {
            playing_players += 1;
        }
        frames_sent += ls.frames_sent;
    }

    body.push_str(&format!(
        "nodelink_players_total {} {}\n",
        "# HELP nodelink_players_total Total number of players",
        players
    ));
    body.push_str(&format!(
        "nodelink_players_playing {} {}\n",
        "# HELP nodelink_players_playing Number of players currently playing",
        playing_players
    ));
    body.push_str(&format!(
        "nodelink_frames_sent_total {} {}\n",
        "# HELP nodelink_frames_sent_total Total audio frames sent",
        frames_sent
    ));
    let uptime = state.start_time.elapsed().as_secs();
    body.push_str(&format!(
        "nodelink_uptime_seconds {} {}\n",
        "# HELP nodelink_uptime_seconds Server uptime in seconds",
        uptime
    ));

    for item in state.api_requests.iter() {
        body.push_str(&format!(
            "nodelink_api_requests_total{{endpoint=\"{}\"}} {}\n",
            item.key(),
            item.value()
        ));
    }

    body.push_str("# EOF\n");
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body).into_response()
}

#[derive(Debug, Deserialize)]
struct DecodeTrackQuery {
    encoded_track: Option<String>,
    #[serde(rename = "encodedTrack")]
    encoded_track_camel: Option<String>,
}

async fn decode_track_route(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<DecodeTrackQuery>,
) -> Response {
    state.increment_api_request("/v4/decodetrack");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let Some(encoded) = query.encoded_track.or(query.encoded_track_camel) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": "encodedTrack is required"})),
        )
            .into_response();
    };

    match decode_track(&encoded) {
        Ok(track) => Json(track).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": format!("failed to decode track: {error}")})),
        )
            .into_response(),
    }
}

async fn decode_tracks_route(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Vec<String>>,
) -> Response {
    state.increment_api_request("/v4/decodetracks");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let decoded: Result<Vec<_>, _> = body.iter().map(|track| decode_track(track)).collect();
    match decoded {
        Ok(tracks) => Json(tracks).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": format!("failed to decode tracks: {error}")})),
        )
            .into_response(),
    }
}

async fn encode_track_route(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(mut body): Json<TrackData>,
) -> Response {
    state.increment_api_request("/v4/encodetrack");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let encoded = encode_track(&body);
    body.encoded = Some(encoded.clone());
    Json(json!({ "encoded": encoded, "track": body })).into_response()
}

async fn encoded_tracks_route(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<Vec<TrackData>>,
) -> Response {
    state.increment_api_request("/v4/encodedtracks");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let encoded: Vec<_> = body.iter().map(encode_track).collect();
    Json(encoded).into_response()
}

#[derive(Debug, Deserialize)]
struct LoadTracksQuery {
    identifier: String,
}

enum LoadTarget {
    Url(String),
    Search { source: String, query: String },
    DefaultSearch(String),
}

fn parse_load_identifier(identifier: &str) -> LoadTarget {
    if identifier.starts_with("http://") || identifier.starts_with("https://") {
        return LoadTarget::Url(identifier.to_owned());
    }

    if let Some((source, query)) = identifier.split_once(':') {
        let windows_drive = source.len() == 1
            && query
                .chars()
                .next()
                .map(|ch| ch == '\\' || ch == '/')
                .unwrap_or(false);

        if !windows_drive && !query.starts_with("//") && !source.is_empty() && !query.is_empty() {
            return LoadTarget::Search {
                source: source.to_owned(),
                query: query.to_owned(),
            };
        }
    }

    if identifier.starts_with('/') || identifier.starts_with('\\') {
        return LoadTarget::Search {
            source: "local".into(),
            query: identifier.to_owned(),
        };
    }

    LoadTarget::DefaultSearch(identifier.to_owned())
}

async fn load_tracks(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadTracksQuery>,
) -> Response {
    state.increment_api_request("/v4/loadtracks");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let cache_key = query.identifier.trim().to_string();
    if let Some(cached) = state.track_cache_manager.get(&cache_key).await {
        return Json(cached).into_response();
    }

    let result = match parse_load_identifier(query.identifier.trim()) {
        LoadTarget::Url(url) => state.sources.resolve(&url).await,
        LoadTarget::Search { source, query } => state.sources.search(&source, &query).await,
        LoadTarget::DefaultSearch(query) => {
            state
                .sources
                .search_with_default(
                    state
                        .config
                        .default_search_source
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("ytsearch"),
                    &query,
                )
                .await
        }
    };

    match result {
        Ok(SourceResult::Error(exception)) => Json(json!({
            "loadType": "error",
            "data": {
                "message": exception,
                "severity": "common",
                "cause": "Unknown"
            }
        }))
        .into_response(),
        Ok(other) => {
            let value = serde_json::to_value(&other).unwrap_or_default();
            state.track_cache_manager.set(cache_key, value.clone()).await;
            Json(value).into_response()
        }
        Err(e) => Json(json!({
            "loadType": "error",
            "data": {
                "message": e.to_string(),
                "severity": "fault",
                "cause": "Unknown"
            }
        }))
        .into_response(),
    }
}

async fn load_stream(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadTracksQuery>,
) -> Response {
    state.increment_api_request("/v4/loadstream");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.enable_load_stream_endpoint {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let url = query.identifier.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Json(json!({
            "loadType": "error",
            "data": {
                "message": "loadstream requires an http:// or https:// URL",
                "severity": "common",
                "cause": "Invalid URL"
            }
        }))
        .into_response();
    }

    match state.sources.resolve(url).await {
        Ok(SourceResult::Error(exception)) => Json(json!({
            "loadType": "error",
            "data": {
                "message": exception,
                "severity": "common",
                "cause": "Unknown"
            }
        }))
        .into_response(),
        Ok(other) => Json(serde_json::to_value(&other).unwrap_or_default()).into_response(),
        Err(e) => Json(json!({
            "loadType": "error",
            "data": {
                "message": e.to_string(),
                "severity": "fault",
                "cause": "Unknown"
            }
        }))
        .into_response(),
    }
}

fn parse_m3u(body: &str, base_url: &str) -> Vec<TrackData> {
    let mut tracks = Vec::new();
    let mut pending_title: Option<String> = None;
    let mut pending_duration: i64 = -1;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            if let Some(extinf) = line.strip_prefix("#EXTINF:") {
                let (duration_str, title) = if let Some(comma_pos) = extinf.find(',') {
                    (Some(&extinf[..comma_pos]), Some(extinf[comma_pos + 1..].trim().to_owned()))
                } else {
                    (None, None)
                };
                pending_duration = duration_str
                    .and_then(|s| s.split(&['-', ':'][..]).next().unwrap_or(s).parse().ok())
                    .unwrap_or(-1);
                pending_title = title;
            }
            continue;
        }

        let track_url = if line.starts_with("http://") || line.starts_with("https://") {
            line.to_owned()
        } else {
            format!("{}/{}", base_url.trim_end_matches('/'), line.trim_start_matches('/'))
        };

        let title = pending_title.take().unwrap_or_else(|| {
            track_url.rsplit('/').next().unwrap_or("Unknown").to_owned()
        });

        let author = if let Some(dash) = title.find(" - ") {
            let (a, _) = title.split_at(dash);
            a.to_owned()
        } else {
            "unknown".into()
        };

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: track_url.clone(),
                is_seekable: false,
                author,
                length: pending_duration,
                is_stream: true,
                position: 0,
                title,
                uri: Some(track_url),
                artwork_url: None,
                isrc: None,
                source_name: "http".into(),
                chapters: None,
            },
            plugin_info: serde_json::json!({}),
            user_data: serde_json::json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        tracks.push(track);

        pending_duration = -1;
    }

    tracks
}

async fn track_stream(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadTracksQuery>,
) -> Response {
    state.increment_api_request("/v4/trackstream");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if !state.config.enable_track_stream_endpoint {
        return (StatusCode::NOT_FOUND, "Not Found").into_response();
    }

    let url = query.identifier.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Json(json!({
            "loadType": "error",
            "data": {
                "message": "trackstream requires an http:// or https:// URL",
                "severity": "common",
                "cause": "Invalid URL"
            }
        }))
        .into_response();
    }

    // Fetch the URL and check if it's a playlist
    let response = match reqwest::get(url).await {
        Ok(r) => r,
        Err(e) => {
            return Json(json!({
                "loadType": "error",
                "data": {
                    "message": format!("Failed to fetch URL: {e}"),
                    "severity": "fault",
                    "cause": "Network Error"
                }
            }))
            .into_response();
        }
    };

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let body = match response.text().await {
        Ok(b) => b,
        Err(e) => {
            return Json(json!({
                "loadType": "error",
                "data": {
                    "message": format!("Failed to read response body: {e}"),
                    "severity": "fault",
                    "cause": "Network Error"
                }
            }))
            .into_response();
        }
    };

    // Detect playlist formats
    let is_playlist = content_type.contains("mpegurl")
        || content_type.contains("x-mpegurl")
        || content_type.contains("audio/x-scpls")
        || body.trim_start().starts_with("#EXTM3U")
        || body.trim_start().starts_with("[playlist]");

    if is_playlist {
        let tracks = parse_m3u(&body, url);
        if tracks.is_empty() {
            return Json(json!({
                "loadType": "empty",
                "data": {}
            }))
            .into_response();
        }
        let first = tracks[0].clone();
        let playlist_name = body
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("#PLAYLIST:"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_else(|| "Stream".into());
        let playlist = PlaylistData {
            encoded: first.encoded.clone().unwrap_or_default(),
            info: PlaylistInfo {
                name: playlist_name,
                selected_track: 0,
            },
            plugin_info: serde_json::json!({}),
            tracks,
        };
        Json(json!({
            "loadType": "playlist",
            "data": playlist
        }))
        .into_response()
    } else {
        // Single track stream — delegate to resolve
        match state.sources.resolve(url).await {
            Ok(SourceResult::Error(exception)) => Json(json!({
                "loadType": "error",
                "data": {
                    "message": exception,
                    "severity": "common",
                    "cause": "Unknown"
                }
            }))
            .into_response(),
            Ok(other) => {
                Json(serde_json::to_value(&other).unwrap_or_default()).into_response()
            }
            Err(e) => Json(json!({
                "loadType": "error",
                "data": {
                    "message": e.to_string(),
                    "severity": "fault",
                    "cause": "Unknown"
                }
            }))
            .into_response(),
        }
    }
}

async fn get_session(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    if let Some(session) = state.sessions.get(&session_id) {
        Json(session.clone()).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "Session not found"})),
        )
            .into_response()
    }
}

#[derive(Debug, Deserialize)]
struct PatchSessionPayload {
    resuming: Option<bool>,
    timeout: Option<u64>,
}

async fn patch_session(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(payload): Json<PatchSessionPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let mut session = state.sessions.entry(session_id.clone()).or_insert(Session {
        id: session_id,
        user_id: "0".into(),
        resuming: false,
        timeout: 60,
        players: Vec::new(),
    });

    if let Some(resuming) = payload.resuming {
        session.resuming = resuming;
    }
    if let Some(timeout) = payload.timeout {
        session.timeout = timeout;
    }

    Json(session.clone()).into_response()
}

async fn get_players(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let players = state
        .sessions
        .get(&session_id)
        .map(|session| session.players.clone())
        .unwrap_or_default();
    Json(players).into_response()
}

async fn get_player(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((session_id, guild_id)): Path<(String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    if let Some(session) = state.sessions.get(&session_id) {
        if let Some(player) = session.players.iter().find(|p| p.guild_id == guild_id) {
            let mut player = player.clone();
            // Overlay live state from worker
            if let Some(live) = state.player_states.get(&guild_id) {
                let ls = live.value().read().await;
                player.state.position = ls.position;
                player.state.time = ls.position;
                player.state.connected = ls.connected;
                player.state.ping = ls.ping;
                player.voice = ls.voice.clone();
                player.paused = ls.paused;
                player.volume = ls.volume;
            }
            return Json(player).into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({"message": "Player not found"})),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct VoicePayload {
    token: String,
    endpoint: String,
    #[serde(rename = "sessionId")]
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct PatchPlayerPayload {
    track: Option<Value>,
    volume: Option<u32>,
    paused: Option<bool>,
    filters: Option<Value>,
    voice: Option<VoicePayload>,
}

async fn patch_player(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((session_id, guild_id)): Path<(String, String)>,
    Json(payload): Json<PatchPlayerPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let user_id = state
        .sessions
        .get(&session_id)
        .map(|s| s.user_id.clone())
        .unwrap_or_else(|| "0".into());

    let mut session = state.sessions.entry(session_id.clone()).or_insert(Session {
        id: session_id,
        user_id: user_id.clone(),
        resuming: false,
        timeout: 60,
        players: Vec::new(),
    });

    // Handle voice update from REST
    if let Some(ref voice) = payload.voice {
        state.plugin_manager.on_voice_server_update(&guild_id, &voice.endpoint, &voice.token).await;
        let workers = state.workers.read().await;
        if let Some(tx) = workers.get(&guild_id) {
            let _ = tx
                .send(WorkerCommand::VoiceUpdate {
                    session_id: voice.session_id.clone(),
                    user_id: user_id.clone(),
                    token: voice.token.clone(),
                    endpoint: voice.endpoint.clone(),
                })
                .await;
        }
    }

    // Clone track before potential move into session
    let track_for_worker = payload.track.clone();

    if let Some(player) = session.players.iter_mut().find(|p| p.guild_id == guild_id) {
        if let Some(t) = payload.track {
            player.track = Some(t);
        }
        if let Some(volume) = payload.volume {
            player.volume = volume;
        }
        if let Some(paused) = payload.paused {
            player.paused = paused;
        }
        if let Some(ref filters) = payload.filters {
            player.filters = filters.clone();
        }
    } else {
        let player = Player {
            guild_id: guild_id.clone(),
            track: payload.track.clone(),
            volume: payload.volume.unwrap_or(100),
            paused: payload.paused.unwrap_or(false),
            state: Default::default(),
            voice: Default::default(),
            filters: payload.filters.clone().unwrap_or(serde_json::Value::Object(Default::default())),
        };
        session.players.push(player);
    }

    // Send commands to the worker if it exists
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        if let Some(ref track_val) = track_for_worker {
            if let Some(encoded) = track_val.get("encoded").and_then(|e| e.as_str()) {
                let _ = tx
                    .send(WorkerCommand::Play {
                        encoded_track: encoded.to_string(),
                        no_replace: false,
                    })
                    .await;
            }
        }
        if let Some(paused) = payload.paused {
            let _ = tx.send(WorkerCommand::Pause(paused)).await;
        }
        if let Some(volume) = payload.volume {
            let _ = tx.send(WorkerCommand::Volume(volume as u16)).await;
        }
        if let Some(ref filters) = payload.filters {
            let _ = tx.send(WorkerCommand::Filters(filters.clone())).await;
        }
    }

    let player = session
        .players
        .iter()
        .find(|p| p.guild_id == guild_id)
        .cloned()
        .unwrap();
    Json(player).into_response()
}

async fn delete_player(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((session_id, guild_id)): Path<(String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    if let Some(mut session) = state.sessions.get_mut(&session_id) {
        session.players.retain(|player| player.guild_id != guild_id);
    }

    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx.send(WorkerCommand::Destroy).await;
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn get_mixer(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Some(live) = state.player_states.get(&guild_id) {
        let ls = live.value().read().await;
        return Json(ls.mixer.clone().unwrap_or(json!([]))).into_response();
    }
    Json(json!([])).into_response()
}

#[derive(Debug, Deserialize)]
struct PatchMixerPayload {
    action: String,
    #[serde(default)]
    layer_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    volume: Option<f32>,
    #[serde(default)]
    pan: Option<f32>,
    #[serde(default)]
    mute: Option<bool>,
    #[serde(default)]
    solo: Option<bool>,
    #[serde(default)]
    url: Option<String>,
}

async fn patch_mixer(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
    Json(payload): Json<PatchMixerPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let cmd = match payload.action.as_str() {
            "addLayer" => WorkerCommand::MixerAddLayer {
                name: payload.name.unwrap_or_default(),
                volume: payload.volume.unwrap_or(1.0),
                pan: payload.pan.unwrap_or(0.0),
            },
            "removeLayer" => WorkerCommand::MixerRemoveLayer {
                layer_id: payload.layer_id.unwrap_or_default(),
            },
            "updateLayer" => WorkerCommand::MixerUpdateLayer {
                layer_id: payload.layer_id.unwrap_or_default(),
                name: payload.name,
                volume: payload.volume,
                pan: payload.pan,
                mute: payload.mute,
                solo: payload.solo,
            },
            "setUrl" => WorkerCommand::MixerSetUrl {
                layer_id: payload.layer_id.unwrap_or_default(),
                url: payload.url,
            },
            _ => WorkerCommand::MixerList,
        };
        let _ = tx.send(cmd).await;
    }
    // Re-read mixer state after update
    if let Some(live) = state.player_states.get(&guild_id) {
        let ls = live.value().read().await;
        return Json(ls.mixer.clone().unwrap_or(json!([]))).into_response();
    }
    Json(json!([])).into_response()
}

async fn post_mixer_layer(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
    Json(payload): Json<PatchMixerPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer/layers");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx
            .send(WorkerCommand::MixerAddLayer {
                name: payload.name.unwrap_or_default(),
                volume: payload.volume.unwrap_or(1.0),
                pan: payload.pan.unwrap_or(0.0),
            })
            .await;
    }
    // Re-read mixer state after add
    if let Some(live) = state.player_states.get(&guild_id) {
        let ls = live.value().read().await;
        return Json(ls.mixer.clone().unwrap_or(json!([]))).into_response();
    }
    Json(json!([])).into_response()
}

async fn delete_mixer_layer(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id, layer_id)): Path<(String, String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer/layers/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx
            .send(WorkerCommand::MixerRemoveLayer { layer_id })
            .await;
    }
    // Re-read mixer state after remove
    if let Some(live) = state.player_states.get(&guild_id) {
        let ls = live.value().read().await;
        return Json(ls.mixer.clone().unwrap_or(json!([]))).into_response();
    }
    Json(json!([])).into_response()
}

#[derive(Debug, Deserialize)]
struct PostMixPayload {
    #[serde(default)]
    track: Option<MixTrackPayload>,
    #[serde(default)]
    volume: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct MixTrackPayload {
    #[serde(default)]
    encoded: Option<String>,
}

async fn post_mix(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
    Json(payload): Json<PostMixPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let name = payload
        .track
        .and_then(|t| t.encoded)
        .unwrap_or_default();
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx
            .send(WorkerCommand::MixerAddLayer {
                name,
                volume: payload.volume.unwrap_or(1.0),
                pan: 0.0,
            })
            .await;
    }
    StatusCode::CREATED.into_response()
}

async fn patch_mixer_layer(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id, layer_id)): Path<(String, String, String)>,
    Json(payload): Json<PatchMixerPayload>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/mixer/layers/:id");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx
            .send(WorkerCommand::MixerUpdateLayer {
                layer_id,
                name: payload.name,
                volume: payload.volume,
                pan: payload.pan,
                mute: payload.mute,
                solo: payload.solo,
            })
            .await;
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn routeplanner_free_address(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let address = body["address"].as_str().unwrap_or("");
    if !address.is_empty() {
        let mut failing = state.route_planner.failing.lock().await;
        failing.remove(address);
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn routeplanner_free_all(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let mut failing = state.route_planner.failing.lock().await;
    failing.clear();
    StatusCode::NO_CONTENT.into_response()
}

async fn routeplanner_status(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let rp = &state.route_planner;
    let failing = rp.failing.lock().await;
    let blocked = rp.blocked.lock().await;

    let failing_addrs: Vec<serde_json::Value> = failing
        .iter()
        .map(|(addr, reason)| {
            json!({
                "address": addr,
                "failingTimestamp": 0,
                "failingTime": reason,
                "unavailableSince": 0
            })
        })
        .collect();

    let blocked_addrs: Vec<serde_json::Value> = blocked
        .iter()
        .map(|addr| {
            json!({
                "address": addr,
                "blockedTimestamp": 0,
                "blocked": true
            })
        })
        .collect();

    Json(json!({
        "class": "RotatingNanoIpRoutePlanner",
        "details": {
            "ipBlock": {
                "type": rp.ip_block_type,
                "size": rp.ip_block_size
            },
            "failingAddresses": failing_addrs,
            "blockedAddresses": blocked_addrs,
            "rotating": rp.rotating,
            "currentAddressIndex": rp.current_index,
            "addresses": rp.addresses
        }
    }))
    .into_response()
}

async fn workers_list(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    let list: Vec<serde_json::Value> = workers
        .iter()
        .map(|(guild_id, _)| json!({ "guildId": guild_id }))
        .collect();
    Json(list).into_response()
}

#[derive(Deserialize)]
struct WorkersPatchPayload {
    id: Option<String>,
    pid: Option<u32>,
}

async fn workers_patch(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(payload): Json<WorkersPatchPayload>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    if let Some(id) = payload.id {
        state.worker_manager.remove_worker(&id).await;
        Json(json!({"killed": true, "id": id, "clusterId": null, "pid": null})).into_response()
    } else {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": "Either id or pid must be provided"})),
        )
            .into_response()
    }
}

async fn worker_terminate(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path(guild_id): Path<String>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let workers = state.workers.read().await;
    if let Some(tx) = workers.get(&guild_id) {
        let _ = tx.send(WorkerCommand::Destroy).await;
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"message": "Worker not found"})),
        )
            .into_response()
    }
}

async fn youtube_config_get(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let has_oauth = !state.config.sources.youtube.refresh_tokens.is_empty();
    Json(json!({
        "enabled": state.config.sources.youtube.enabled,
        "allowSearch": state.config.sources.youtube.enabled,
        "allowDirectVideo": true,
        "allowDirectPlaylist": true,
        "oauth": if has_oauth {
            json!({"enabled": true, "hasTokens": true})
        } else {
            json!(null)
        },
        "clients": {
            "web": null,
            "android": null,
            "ios": null
        }
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct YoutubeConfigPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub allow_search: Option<bool>,
    #[serde(default)]
    pub allow_direct_video: Option<bool>,
    #[serde(default)]
    pub allow_direct_playlist: Option<bool>,
}

async fn sponsorblock_get(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let categories = state
        .sponsorblock
        .get(&guild_id)
        .map(|v| v.clone())
        .unwrap_or_default();
    Json(json!({
        "guildId": guild_id,
        "categories": categories,
        "enabled": !categories.is_empty()
    }))
    .into_response()
}

async fn sponsorblock_patch(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
    Json(payload): Json<SponsorblockPayload>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let categories = payload.categories.unwrap_or_default();
    state.sponsorblock.insert(guild_id.clone(), categories.clone());
    Json(json!({
        "guildId": guild_id,
        "categories": categories,
        "enabled": !categories.is_empty()
    }))
    .into_response()
}

async fn sponsorblock_delete(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    state.sponsorblock.remove(&guild_id);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
struct SponsorblockPayload {
    #[serde(default)]
    pub categories: Option<Vec<String>>,
}

async fn youtube_config_patch(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(payload): Json<YoutubeConfigPatch>,
) -> Response {
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let has_oauth = !state.config.sources.youtube.refresh_tokens.is_empty();
    Json(json!({
        "enabled": payload.enabled.unwrap_or(state.config.sources.youtube.enabled),
        "allowSearch": payload.allow_search.unwrap_or(state.config.sources.youtube.enabled),
        "allowDirectVideo": payload.allow_direct_video.unwrap_or(true),
        "allowDirectPlaylist": payload.allow_direct_playlist.unwrap_or(true),
        "oauth": if has_oauth {
            json!({"enabled": true, "hasTokens": true})
        } else {
            json!(null)
        },
        "clients": {
            "web": null,
            "android": null,
            "ios": null
        }
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct LoadLyricsQuery {
    #[serde(rename = "trackTitle")]
    track_title: String,
    #[serde(rename = "trackAuthor")]
    track_author: String,
    #[serde(rename = "albumName")]
    album_name: Option<String>,
}

async fn load_lyrics(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadLyricsQuery>,
) -> Response {
    state.increment_api_request("/v4/loadlyrics");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    match crate::lyrics::fetch_lyrics(&query.track_title, &query.track_author, query.album_name.as_deref(), None).await {
        Ok(Some(lyrics)) => {
            let synced: Vec<serde_json::Value> = lyrics.synced_lyrics.iter().map(|l| {
                json!({"time": l.time, "text": l.text})
            }).collect();
            Json(json!({
                "lyrics": true,
                "source": lyrics.source,
                "title": lyrics.title,
                "artist": lyrics.artist,
                "album": lyrics.album,
                "syncedLyrics": synced,
                "plainLyrics": lyrics.plain_lyrics,
            })).into_response()
        }
        Ok(None) => Json(json!({
            "lyrics": false,
            "message": "No lyrics found"
        })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": format!("Failed to fetch lyrics: {e}")})),
        ).into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct LoadChaptersQuery {
    #[serde(rename = "encodedTrack")]
    encoded_track: String,
}

async fn load_chapters(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadChaptersQuery>,
) -> Response {
    state.increment_api_request("/v4/loadchapters");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let track = match crate::tracks::decode_track(&query.encoded_track) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": format!("Invalid encoded track: {e}")})),
            )
                .into_response();
        }
    };

    if track.info.source_name != "youtube" {
        return Json(json!([])).into_response();
    }

    match state.sources.get_chapters(&track.info).await {
        Ok(chapters) => Json(json!(chapters)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"message": format!("Failed to load chapters: {e}")})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct LoadMeaningQuery {
    #[serde(rename = "encodedTrack")]
    encoded_track: String,
    lang: Option<String>,
}

async fn load_meaning(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<LoadMeaningQuery>,
) -> Response {
    state.increment_api_request("/v4/meaning");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let track = match crate::tracks::decode_track(&query.encoded_track) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"message": format!("Invalid encoded track: {e}")})),
            )
                .into_response();
        }
    };

    let language = query.lang.as_deref().unwrap_or("en");

    let source_name = if track.info.source_name == "letrasmus" {
        Some(track.info.source_name.as_str())
    } else {
        None
    };

    match state.meaning_manager.load_meaning(&track.info.title, &track.info.author, language, source_name).await {
        Ok(Some(meaning)) => Json(json!({
            "loadType": meaning.load_type,
            "data": meaning.data
        }))
        .into_response(),
        Ok(None) => Json(json!({
            "loadType": "empty",
            "data": {}
        }))
        .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"message": format!("Failed to load meaning: {e}")})),
            )
                .into_response(),
    }
}

async fn connection_status(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/v4/connection");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let status = state.connection_manager.current_status();
    let metrics = state.connection_manager.current_metrics();

    Json(json!({
        "status": status,
        "metrics": metrics,
    }))
    .into_response()
}

async fn profiler(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/v4/profiler");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let payload = crate::profiler::ProfilerPayload::default();
    let result = crate::profiler::collect_action_snapshot("status", &payload, &state).await;
    Json(result).into_response()
}

async fn profiler_websocket(
    ws: WebSocketUpgrade,
    Query(params): Query<std::collections::HashMap<String, String>>,
    State(state): State<SharedState>,
) -> Response {
    let code = params.get("code").cloned().unwrap_or_default();
    let scope = params.get("scope").cloned().unwrap_or_else(|| "all".to_string());
    let interval_ms: u64 = params.get("intervalMs").and_then(|s| s.parse().ok()).unwrap_or(2000);

    let config = crate::profiler::get_endpoint_config(&state.config);
    let remote_addr = None;

    if let Err(e) = crate::profiler::validate_access(&config, remote_addr, Some(&code)) {
        return (StatusCode::FORBIDDEN, e).into_response();
    }

    ws.on_upgrade(move |socket| handle_profiler_socket(socket, scope, interval_ms, state))
}

async fn handle_profiler_socket(mut socket: WebSocket, scope: String, interval_ms: u64, state: SharedState) {
    use futures::StreamExt;
    use tokio::time::{sleep, Duration};

    let mut last_action = String::new();
    let mut last_payload = crate::profiler::ProfilerPayload::default();
    last_payload.scope = Some(scope.clone());

    loop {
        tokio::select! {
            // Periodically push status snapshots
            _ = sleep(Duration::from_millis(interval_ms)) => {
                let action = if last_action.is_empty() { "status" } else { &last_action };
                let snapshot = crate::profiler::collect_action_snapshot(action, &last_payload, &state).await;
                let msg = serde_json::to_string(&snapshot).unwrap_or_default();
                if socket.send(Message::Text(msg.into())).await.is_err() {
                    break;
                }
            }
            // Handle incoming messages from the client
            msg = socket.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            let payload = crate::profiler::ProfilerPayload::from_json(&v);
                            let action = payload.action.clone().unwrap_or_else(|| "status".to_string());
                            last_action = action.clone();
                            if let Some(s) = &payload.scope {
                                last_payload.scope = Some(s.clone());
                            }

                            let snapshot = if action == "all" {
                                crate::profiler::collect_all_sequence(&payload, &state).await
                            } else if action == "allocTop" {
                                crate::profiler::collect_allocation_top_sites(&payload, &state).await
                            } else {
                                crate::profiler::collect_action_snapshot(&action, &payload, &state).await
                            };

                            let msg = serde_json::to_string(&snapshot).unwrap_or_default();
                            if socket.send(Message::Text(msg.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
struct ProfilerFileQuery {
    code: Option<String>,
    path: Option<String>,
    line: Option<usize>,
    context: Option<usize>,
}

async fn profiler_file(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<ProfilerFileQuery>,
) -> Response {
    state.increment_api_request("/v4/profiler/file");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let path = match &query.path {
        Some(p) => p,
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({"message": "path is required"}))).into_response();
        }
    };

    if path.contains("..") {
        return (StatusCode::BAD_REQUEST, Json(json!({"message": "directory traversal not allowed"}))).into_response();
    }

    let line = query.line.unwrap_or(1);
    let context = query.context.unwrap_or(8).min(60);

    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return (StatusCode::NOT_FOUND, Json(json!({"message": "file not found"}))).into_response(),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start = if line > context { line - context } else { 1 };
    let end = std::cmp::min(line + context, total_lines);

    let snippet: Vec<serde_json::Value> = lines[(start - 1)..end]
        .iter()
        .enumerate()
        .map(|(i, text)| json!({"number": start + i, "text": text}))
        .collect();

    Json(json!({
        "path": path,
        "line": line,
        "start": start,
        "end": end,
        "totalLines": total_lines,
        "snippet": snippet,
    }))
    .into_response()
}

async fn profiler_ui(
    State(state): State<SharedState>,
    headers: HeaderMap,
) -> Response {
    state.increment_api_request("/v4/profiler/ui");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    let html = r###"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8"><title>RustLink Profiler</title>
<style>
*{margin:0;padding:0;box-sizing:border-box}
body{font-family:monospace;background:#1a1a2e;color:#eee;padding:20px}
h1{color:#e94560;margin-bottom:20px}
#status{padding:10px;background:#16213e;border-radius:4px;min-height:200px}
</style></head>
<body>
<h1>RustLink Profiler</h1>
<div id="status">Connecting to profiler WebSocket...</div>
<script>
const ws=new URLSearchParams(location.search);
const socket=new WebSocket("ws://"+location.host+"/v4/profiler/socket?code="+(ws.get("code")||""));
socket.onmessage=function(e){document.getElementById("status").textContent=e.data};
socket.onerror=function(){document.getElementById("status").textContent="WebSocket connection failed"};
</script>
</body>
</html>"###;
    ([(header::CONTENT_TYPE, "text/html")], html).into_response()
}

fn memory_stats() -> Option<MemoryStats> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let vm_rss = status.lines().find_map(|l| {
            if l.starts_with("VmRSS:") {
                l.split_whitespace().nth(1)?.parse::<u64>().ok()
            } else {
                None
            }
        })?;
        let vm_size = status.lines().find_map(|l| {
            if l.starts_with("VmSize:") {
                l.split_whitespace().nth(1)?.parse::<u64>().ok()
            } else {
                None
            }
        })?;
        Some(MemoryStats {
            physical_mem: vm_rss * 1024,
            virtual_mem: vm_size * 1024,
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

struct MemoryStats {
    physical_mem: u64,
    virtual_mem: u64,
}

#[derive(Deserialize)]
struct YoutubeOAuthQuery {
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
}

async fn youtube_oauth_get(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Query(query): Query<YoutubeOAuthQuery>,
) -> Response {
    state.increment_api_request("/v4/youtube/oauth");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }

    let refresh_token = query
        .refresh_token
        .or_else(|| state.config.sources.youtube.refresh_tokens.first().cloned());

    let Some(refresh_token) = refresh_token else {
        return (StatusCode::BAD_REQUEST, Json(json!({
            "error": "no_refresh_token",
            "message": "No refresh token provided and none configured"
        }))).into_response();
    };

    let client = reqwest::Client::new();
    let params = [
        ("client_id", "861556708454-d6dlm3lh05idd8npek18k6be8ba3oc68.apps.googleusercontent.com"),
        ("client_secret", "SboVhoG9s0rNafixCSGGKXAT"),
        ("refresh_token", &refresh_token),
        ("grant_type", "refresh_token"),
    ];

    match client
        .post("https://www.youtube.com/o/oauth2/token")
        .form(&params)
        .send()
        .await
    {
        Ok(resp) => {
            let http_status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            match resp.json::<Value>().await {
                Ok(body) => {
                    if http_status.is_success() {
                        (http_status, Json(body)).into_response()
                    } else {
                        (StatusCode::BAD_GATEWAY, Json(json!({
                            "error": "oauth_error",
                            "status": http_status.as_u16(),
                            "body": body
                        }))).into_response()
                    }
                }
                Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({
                    "error": "invalid_response",
                    "message": e.to_string()
                }))).into_response(),
            }
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(json!({
            "error": "request_failed",
            "message": e.to_string()
        }))).into_response(),
    }
}

async fn youtube_oauth(State(state): State<SharedState>, headers: HeaderMap) -> Response {
    state.increment_api_request("/v4/youtube/oauth");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    Json(json!({
        "status": "not_implemented",
        "message": "YouTube OAuth requires browser-based flow — use config refresh_tokens instead"
    }))
    .into_response()
}

async fn lyrics_subscribe(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/lyrics/subscribe");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    state.lyrics_subscribers.lock().await.insert(guild_id.clone());
    Json(json!({"subscribed": true, "guildId": guild_id})).into_response()
}

async fn lyrics_unsubscribe(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Path((_session_id, guild_id)): Path<(String, String)>,
) -> Response {
    state.increment_api_request("/v4/sessions/:id/players/:id/lyrics/subscribe");
    if let Err(response) = require_auth(&headers, &state) {
        return response;
    }
    state.lyrics_subscribers.lock().await.remove(&guild_id);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn app() -> Router {
        router(crate::state::AppState::new(crate::config::NodeLinkConfig::default()).await)
    }

    async fn app_with_config(config: crate::config::NodeLinkConfig) -> Router {
        router(crate::state::AppState::new(config).await)
    }

    fn authed_request(uri: &str) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, "youshallnotpass")
            .body(Body::empty())
            .unwrap()
    }

    fn authed_json_request(
        method: axum::http::Method,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, "youshallnotpass")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn version_requires_auth() {
        let response = app()
            .await
            .oneshot(
                axum::http::Request::builder()
                    .uri("/version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn version_returns_rust_marker() {
        let response = app()
            .await
            .oneshot(authed_request("/v4/version"))
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();

        assert_eq!(&bytes[..], b"3.8.0");
    }

    #[tokio::test]
    async fn loadtracks_reports_missing_provider() {
        let response = app()
            .await
            .oneshot(authed_request("/v4/loadtracks?identifier=nonexistent:test"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["loadType"], "empty");
    }

    #[tokio::test]
    async fn sessions_can_be_resumable() {
        let response = app()
            .await
            .oneshot(authed_json_request(
                axum::http::Method::PATCH,
                "/v4/sessions/session-a",
                json!({ "resuming": true, "timeout": 120 }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["id"], "session-a");
        assert_eq!(body["resuming"], true);
        assert_eq!(body["timeout"], 120);
    }

    #[tokio::test]
    async fn players_can_be_patched_and_deleted() {
        let router = app().await;
        let response = router
            .clone()
            .oneshot(authed_json_request(
                axum::http::Method::PATCH,
                "/v4/sessions/session-a/players/123456789012345678",
                json!({ "volume": 250, "paused": true }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["guildId"], "123456789012345678");
        assert_eq!(body["volume"], 250);
        assert_eq!(body["paused"], true);

        let response = router
            .clone()
            .oneshot(authed_request("/v4/sessions/session-a/players"))
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body.as_array().unwrap().len(), 1);

        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .method(axum::http::Method::DELETE)
                    .uri("/v4/sessions/session-a/players/123456789012345678")
                    .header(header::AUTHORIZATION, "youshallnotpass")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn tracks_encode_and_decode_round_trip() {
        let track = json!({
            "encoded": null,
            "info": {
                "identifier": "abc123",
                "isSeekable": true,
                "author": "artist",
                "length": 1234,
                "isStream": false,
                "position": 0,
                "title": "title",
                "uri": "https://example.com/audio.mp3",
                "artworkUrl": null,
                "isrc": null,
                "sourceName": "http"
            },
            "pluginInfo": {},
            "userData": {}
        });

        let response = app()
            .await
            .oneshot(authed_json_request(
                axum::http::Method::POST,
                "/v4/encodetrack",
                track,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let encoded_body = response_json(response).await;
        let encoded = encoded_body["encoded"].as_str().unwrap();

        let response = app()
            .await
            .oneshot(authed_request(&format!(
                "/v4/decodetrack?encodedTrack={encoded}"
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let decoded = response_json(response).await;
        assert_eq!(decoded["info"]["identifier"], "abc123");
        assert_eq!(decoded["info"]["sourceName"], "http");
    }

    #[tokio::test]
    async fn loadtracks_resolves_local_files() {
        let base_path =
            std::env::temp_dir().join(format!("rustlink-local-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base_path).unwrap();
        let file_path = base_path.join("sample.mp3");
        std::fs::write(&file_path, [0xff, 0xfb, 0x90, 0x64]).unwrap();

        let mut config = crate::config::NodeLinkConfig::default();
        config.sources.local.base_path = base_path.to_string_lossy().to_string();
        config.default_search_source = vec!["local".into()];

        let response = app_with_config(config)
            .await
            .oneshot(authed_request("/v4/loadtracks?identifier=local:sample.mp3"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert_eq!(body["loadType"], "track");
        assert_eq!(body["info"]["title"], "sample.mp3");
        assert_eq!(body["info"]["sourceName"], "local");
        assert!(body["encoded"].as_str().unwrap().len() > 10);

        std::fs::remove_file(file_path).unwrap();
        std::fs::remove_dir(base_path).unwrap();
    }

    #[tokio::test]
    async fn loadtracks_resolves_google_tts_text() {
        let response = app()
            .await
            .oneshot(authed_request(
                "/v4/loadtracks?identifier=gtts:hello%20rustlink",
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = response_json(response).await;
        assert_eq!(body["loadType"], "track");
        assert_eq!(body["info"]["title"], "TTS: hello rustlink");
        assert_eq!(body["info"]["author"], "Google TTS");
        assert_eq!(body["info"]["sourceName"], "google-tts");
        assert!(body["info"]["uri"]
            .as_str()
            .unwrap()
            .contains("translate_tts"));
    }

    #[tokio::test]
    async fn loadstream_rejects_non_urls() {
        let response = app()
            .await
            .oneshot(authed_request("/v4/loadstream?identifier=not-a-url"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["loadType"], "error");
        assert!(body["data"]["message"].as_str().unwrap().contains("http://"));
    }

    #[tokio::test]
    async fn loadstream_returns_404_when_disabled() {
        let mut config = crate::config::NodeLinkConfig::default();
        config.enable_load_stream_endpoint = false;
        let response = app_with_config(config)
            .await
            .oneshot(authed_request("/v4/loadstream?identifier=https://example.com/audio.mp3"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn trackstream_rejects_non_urls() {
        let response = app()
            .await
            .oneshot(authed_request("/v4/trackstream?identifier=not-a-url"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["loadType"], "error");
        assert!(body["data"]["message"].as_str().unwrap().contains("http://"));
    }

    #[tokio::test]
    async fn trackstream_returns_404_when_disabled() {
        let mut config = crate::config::NodeLinkConfig::default();
        config.enable_track_stream_endpoint = false;
        let response = app_with_config(config)
            .await
            .oneshot(authed_request("/v4/trackstream?identifier=https://example.com/playlist.m3u"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn parse_m3u_basic() {
        let m3u = "#EXTM3U\n#EXTINF:123,Test Artist - Test Title\nhttps://example.com/track1.mp3\n";
        let tracks = parse_m3u(m3u, "https://example.com");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].info.title, "Test Artist - Test Title");
        assert_eq!(tracks[0].info.author, "Test Artist");
        assert_eq!(tracks[0].info.length, 123);
        assert_eq!(tracks[0].info.identifier, "https://example.com/track1.mp3");
        assert!(tracks[0].info.is_stream);
        assert!(tracks[0].encoded.is_some());
    }

    #[tokio::test]
    async fn parse_m3u_multiple() {
        let m3u = "#EXTM3U\n#EXTINF:30,First Track\nhttps://example.com/1.mp3\n#EXTINF:45,Second Track\nhttps://example.com/2.mp3\n";
        let tracks = parse_m3u(m3u, "https://example.com");
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].info.title, "First Track");
        assert_eq!(tracks[0].info.length, 30);
        assert_eq!(tracks[1].info.title, "Second Track");
        assert_eq!(tracks[1].info.length, 45);
    }

    #[tokio::test]
    async fn parse_m3u_relative_urls() {
        let m3u = "#EXTM3U\n#EXTINF:-1,Track\nrelative/path.mp3\n";
        let tracks = parse_m3u(m3u, "https://example.com/stream");
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].info.identifier, "https://example.com/stream/relative/path.mp3");
    }
}
