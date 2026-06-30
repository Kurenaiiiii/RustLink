use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::sources::{SourceProvider, SourceResult, TrackUrlResult};
use crate::sources::youtube_cipher::CipherManager;
use crate::sources::youtube_clients::ClientKind;
use crate::sources::youtube_oauth::YouTubeOAuth;
use crate::sources::youtube_potoken::PoTokenManager;
use crate::sources::youtube_sabr::{FormatEntry as SabrFormatEntry, SabrManager};
use crate::tracks::{encode_track, Chapter, TrackData, TrackInfo};

const INNERTUBE_API_KEY: &str = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";

pub struct YouTubeSource {
    client: Client,
    hl: String,
    gl: String,
    allow_itag: Vec<u32>,
    oauth: Option<Arc<Mutex<YouTubeOAuth>>>,
    enabled_clients: Vec<ClientKind>,
    cipher: CipherManager,
    potoken: PoTokenManager,
    sabr: SabrManager,
}

impl YouTubeSource {
    pub fn new(
        hl: String,
        gl: String,
        allow_itag: Vec<u32>,
        refresh_tokens: Vec<String>,
        potoken: Option<String>,
        po_token_endpoint: Option<String>,
    ) -> Self {
        let oauth = if refresh_tokens.is_empty() {
            None
        } else {
            Some(Arc::new(Mutex::new(YouTubeOAuth::new(refresh_tokens))))
        };

        let http_client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    reqwest::header::ACCEPT_LANGUAGE,
                    "en-US,en;q=0.9".parse().unwrap(),
                );
                h
            })
            .build()
            .unwrap();

        Self {
            client: http_client.clone(),
            hl,
            gl,
            allow_itag,
            oauth,
            enabled_clients: vec![
                ClientKind::Android,
                ClientKind::Web,
                ClientKind::Tv,
                ClientKind::Ios,
                ClientKind::Music,
                ClientKind::WebEmbedded,
                ClientKind::WebRemix,
                ClientKind::AndroidVR,
                ClientKind::TvCast,
                ClientKind::WebParentTools,
                ClientKind::TvEmbedded,
            ],
            cipher: CipherManager::new(),
            potoken: PoTokenManager::new(potoken, po_token_endpoint, http_client.clone()),
            sabr: SabrManager::new(http_client.clone()),
        }
    }

    pub fn oauth_enabled(&self) -> bool {
        self.oauth.is_some()
    }

    /// Spawns background tasks for visitor data rotation and PoToken polling.
    pub fn start_background_tasks(&self) {
        let potoken = self.potoken.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(600));
            loop {
                interval.tick().await;
                potoken.refresh_visitor_data();
            }
        });
        self.potoken.start_polling();
    }

    fn innertube_url(&self, endpoint: &str, client: ClientKind) -> String {
        format!("{}/youtubei/v1/{}?key={}", client.api_endpoint(), endpoint, INNERTUBE_API_KEY)
    }

    async fn innertube_with_client(
        &self,
        endpoint: &str,
        payload: Value,
        client_kind: ClientKind,
    ) -> anyhow::Result<Value> {
        let url = self.innertube_url(endpoint, client_kind);
        let vd = self.potoken.visitor_data();
        let context = client_kind.build_context(&self.hl, &self.gl, Some(vd.as_str()));

        let mut body = json!({ "context": context });
        if let Some(payload_obj) = payload.as_object() {
            if let Some(obj) = body.as_object_mut() {
                for (k, v) in payload_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        // Inject signatureTimestamp for clients that need player script
        if client_kind.requires_player_script() {
            if let Some(sts) = self.cipher.get_signature_timestamp().await {
                if let Some(pb_ctx) = body
                    .pointer_mut("/playbackContext/contentPlaybackContext")
                {
                    if let Some(map) = pb_ctx.as_object_mut() {
                        map.insert("signatureTimestamp".into(), json!(sts));
                    }
                }
            }
        }

        // Inject playerParams for clients that need it
        if let Some(pp) = client_kind.player_params() {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("playerParams".into(), json!(pp));
            }
        }

        // Inject poToken if available
        if let Some(token) = self.potoken.try_generate().await {
            if let Some(obj) = body.as_object_mut() {
                obj.insert("serviceIntegrityDimensions".into(), json!({
                    "poToken": token
                }));
            }
        }

        let mut req = self.client.post(&url).json(&body);

        if let Some(ref oauth) = self.oauth {
            if client_kind.supports_oauth() {
                if let Ok(mut guard) = oauth.try_lock() {
                    let headers = guard.get_auth_headers().await;
                    for (k, v) in &headers {
                        req = req.header(k.as_str(), v.as_str());
                    }
                }
            }
        }

        // Per-client headers
        for (key, value) in client_kind.search_headers() {
            req = req.header(key, value);
        }

        let resp = req.send().await?;
        let data: Value = resp.json().await?;
        Ok(data)
    }

    fn parse_video_renderer(&self, video: &Value) -> Option<TrackData> {
        let id = video["videoId"].as_str()?;
        let title = video
            .pointer("/title/runs/0/text")
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown Title");
        let author = video
            .pointer("/longBylineText/runs/0/text")
            .or_else(|| video.pointer("/shortBylineText/runs/0/text"))
            .or_else(|| video.pointer("/ownerText/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("Unknown Artist");
        let length_text = video
            .pointer("/lengthText/simpleText")
            .or_else(|| video.pointer("/lengthText/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("0:00");
        let length_ms = parse_duration(length_text).unwrap_or(0);
        let thumbnail = video
            .pointer("/thumbnail/thumbnails/0/url")
            .and_then(|t| t.as_str())
            .map(|t| {
                if t.starts_with("//") {
                    format!("https:{}", t)
                } else {
                    t.to_owned()
                }
            });

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: id.to_owned(),
                is_seekable: true,
                author: author.to_owned(),
                length: length_ms,
                is_stream: false,
                position: 0,
                title: title.to_owned(),
                uri: Some(format!("https://www.youtube.com/watch?v={}", id)),
                artwork_url: thumbnail.or_else(|| {
                    Some(format!(
                        "https://img.youtube.com/vi/{}/maxresdefault.jpg",
                        id
                    ))
                }),
                isrc: None,
                source_name: "youtube".into(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Some(track)
    }

    async fn resolve_video(&self, video_id: &str) -> anyhow::Result<Option<TrackData>> {
        let data = self
            .innertube_with_client(
                "player",
                json!({
                    "videoId": video_id,
                    "playbackContext": {
                        "contentPlaybackContext": {
                            "html5Preference": "HTML5_PREF_WANTS"
                        }
                    }
                }),
                ClientKind::Android,
            )
            .await?;

        let video_details = match data.get("videoDetails") {
            Some(d) => d,
            None => return Ok(None),
        };

        let title = video_details["title"].as_str().unwrap_or("Unknown Title");
        let author = video_details["author"].as_str().unwrap_or("Unknown Artist");
        let length_str = video_details["lengthSeconds"].as_str().unwrap_or("0");
        let length_ms = length_str.parse::<i64>().unwrap_or(0) * 1000;
        let thumb = video_details
            .pointer("/thumbnail/thumbnails/0/url")
            .and_then(|t| t.as_str());
        let is_live = video_details["isLive"].as_bool().unwrap_or(false);

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: video_id.to_owned(),
                is_seekable: !is_live,
                author: author.to_owned(),
                length: if is_live { i64::MAX } else { length_ms },
                is_stream: is_live,
                position: 0,
                title: title.to_owned(),
                uri: Some(format!("https://www.youtube.com/watch?v={}", video_id)),
                artwork_url: thumb.map(|t| t.to_owned()),
                isrc: None,
                source_name: "youtube".into(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };

        let captions = extract_captions(&data);
        if let Some(caps) = captions {
            track.plugin_info = json!({
                "captions": serde_json::to_string(&caps).unwrap_or_default()
            });
        }

        track.encoded = Some(encode_track(&track));
        Ok(Some(track))
    }

    async fn resolve_playlist(&self, playlist_id: &str) -> anyhow::Result<Option<SourceResult>> {
        let client = ClientKind::WebRemix;
        let mut payload = json!({ "browseId": format!("VL{playlist_id}") });
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("enablePersistentPlaylistPanel".into(), json!(true));
            obj.insert("isAudioOnly".into(), json!(true));
        }
        let data = self
            .innertube_with_client("browse", payload, client)
            .await?;

        // Extract playlist title from header
        let playlist_title = data
            .pointer("/header/playlistHeaderRenderer/title/simpleText")
            .or_else(|| data.pointer("/header/playlistHeaderRenderer/title/runs/0/text"))
            .and_then(|t| t.as_str())
            .unwrap_or("YouTube Playlist")
            .to_string();

        let mut tracks: Vec<TrackData> = Vec::new();

        // Parse initial page
        let videos_path = "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/itemSectionRenderer/contents/0/playlistVideoListRenderer/contents";
        if let Some(arr) = data.pointer(videos_path).and_then(|c| c.as_array()) {
            for item in arr {
                if let Some(video) = item.get("playlistVideoRenderer") {
                    if let Some(track) = self.parse_video_renderer(video) {
                        tracks.push(track);
                    }
                }
            }
        }

        if tracks.is_empty() && data.pointer(videos_path).is_none() {
            // Try alternative path for music playlists
            let alt_path = "/contents/twoColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents/0/musicPlaylistShelfRenderer/contents";
            if let Some(arr) = data.pointer(alt_path).and_then(|c| c.as_array()) {
                for item in arr {
                    if let Some(video) = item.get("musicResponsiveListItemRenderer") {
                        if let Some(track) = self.parse_music_list_item(Some(video)) {
                            tracks.push(track);
                        }
                    }
                }
            }
        }

        // Fetch continuation pages
        if !tracks.is_empty() {
            let max_pages = 10;
            let mut continuation_token = extract_continuation_token(data.pointer(videos_path));

            for _ in 0..max_pages {
                let token = match continuation_token {
                    Some(t) => t,
                    None => break,
                };

                match self
                    .innertube_with_client("next", json!({ "continuation": token }), ClientKind::Web)
                    .await
                {
                    Ok(page_data) => {
                        let page_tracks = self.parse_playlist_continuation_page(&page_data);
                        if page_tracks.is_empty() {
                            break;
                        }
                        tracks.extend(page_tracks);
                        continuation_token = extract_continuation_token_from_next(&page_data);
                    }
                    Err(e) => {
                        warn!(target: "YouTube", "Playlist continuation error: {e}");
                        break;
                    }
                }
            }
        }

        if tracks.is_empty() {
            return Ok(None);
        }

        let encoded = tracks[0].encoded.clone().unwrap_or_default();
        Ok(Some(SourceResult::Playlist {
            data: crate::sources::PlaylistData {
                encoded,
                info: crate::sources::PlaylistInfo {
                    name: playlist_title,
                    selected_track: 0,
                },
                plugin_info: json!({}),
                tracks,
            },
        }))
    }

    fn parse_playlist_continuation_page(&self, data: &Value) -> Vec<TrackData> {
        let items_path = "/onResponseReceivedCommands/0/appendContinuationItemsAction/contents";

        let mut tracks: Vec<TrackData> = Vec::new();
        if let Some(arr) = data.pointer(items_path).and_then(|c| c.as_array()) {
            for item in arr {
                if let Some(video) = item.get("playlistVideoRenderer") {
                    if let Some(track) = self.parse_video_renderer(video) {
                        tracks.push(track);
                    }
                }
            }
        }
        tracks
    }

    async fn extract_best_audio_url(&self, data: &Value, needs_cipher: bool) -> Option<FormatInfo> {
        // Check for live stream HLS manifest first
        let is_live = data
            .pointer("/videoDetails/isLive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_live {
            if let Some(hls_url) = data
                .pointer("/streamingData/hlsManifestUrl")
                .and_then(|v| v.as_str())
            {
                return Some(FormatInfo {
                    url: hls_url.to_owned(),
                    format: "hls".into(),
                    protocol: "hls".into(),
                    is_live: true,
                });
            }
        }

        let adaptive = data.pointer("/streamingData/adaptiveFormats")?.as_array()?;

        let preferred: Vec<u32> = if self.allow_itag.is_empty() {
            vec![251, 140, 250, 249, 139]
        } else {
            self.allow_itag.clone()
        };

        for itag in &preferred {
            if let Some(format_entry) = adaptive.iter().find(|f| {
                f.get("itag").and_then(|v| v.as_u64()).map(|v| v as u32) == Some(*itag)
            }) {
                let format_name = itag_to_format(*itag);

                if let Some(url) = format_entry.get("url").and_then(|v| v.as_str()) {
                    return Some(FormatInfo {
                        url: url.to_owned(),
                        format: format_name.to_string(),
                        protocol: "https".into(),
                        is_live,
                    });
                }

                if let Some(cipher_str) = format_entry
                    .get("signatureCipher")
                    .or_else(|| format_entry.get("cipher"))
                    .and_then(|v| v.as_str())
                {
                    let url = if needs_cipher {
                        if let Some(ops) = self.cipher.get_decipher_ops().await {
                            self.cipher.resolve_local_url(cipher_str, &ops)
                        } else {
                            decipher_url_basic(cipher_str)
                        }
                    } else {
                        decipher_url_basic(cipher_str)
                    };

                    if let Some(url) = url {
                        return Some(FormatInfo {
                            url,
                            format: format_name.to_string(),
                            protocol: "https".into(),
                            is_live,
                        });
                    }
                }
            }
        }

        None
    }

    fn extract_sabr_config(
        &self,
        data: &Value,
        client: &ClientKind,
        po_token: Option<String>,
        visitor_data: &str,
    ) -> Option<TrackUrlResult> {
        let streaming_data = data.get("streamingData")?;
        let server_abr_url = streaming_data
            .get("serverAbrStreamingUrl")
            .or_else(|| streaming_data.get("server_abr_streaming_url"))
            .and_then(|v| v.as_str())?;

        info!(target: "YouTube", "SABR URL found for client {}. Using SABR protocol.", client.display_name());

        // Extract ustreamer config (base64 string from player response)
        let ustreamer_config = data
            .pointer("/playerConfig/mediaCommonConfig/mediaUstreamerRequestConfig/videoPlaybackUstreamerConfig")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Extract formats from adaptiveFormats + formats
        let mut formats: Vec<SabrFormatEntry> = Vec::new();
        for src in &["adaptiveFormats", "formats"] {
            if let Some(arr) = streaming_data.get(*src).and_then(|v| v.as_array()) {
                for f in arr {
                    let itag = f.get("itag").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    if itag == 0 { continue; }
                    formats.push(SabrFormatEntry {
                        itag,
                        mime_type: f.get("mimeType").or_else(|| f.get("mime_type")).and_then(|v| v.as_str()).map(|s| s.to_string()),
                        xtags: f.get("xtags").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        last_modified: f.get("lastModified").or_else(|| f.get("last_modified_ms")).and_then(|v| v.as_str()).map(|s| s.to_string()),
                        audio_track_id: f.get("audioTrack").and_then(|t| t.get("id")).and_then(|v| v.as_str()).map(|s| s.to_string()),
                        bitrate: f.get("bitrate").and_then(|v| v.as_i64()).map(|v| v as i32),
                    });
                }
            }
        }

        let client_name = client.client_id() as i32;
        let client_version = client.sabr_client_version().unwrap_or_else(|| client.client_version()).to_string();

        // Build JSON additional_data for the player, including PoToken and ustreamer config
        let additional_data = json!({
            "sabr": {
                "serverAbrStreamingUrl": server_abr_url,
                "clientName": client_name,
                "clientVersion": client_version,
                "formats": formats.iter().map(|f| json!({
                    "itag": f.itag,
                    "mimeType": f.mime_type,
                    "xtags": f.xtags,
                    "lastModified": f.last_modified,
                    "audioTrackId": f.audio_track_id,
                    "bitrate": f.bitrate,
                })).collect::<Vec<_>>(),
                "ustreamerConfig": ustreamer_config,
                "poToken": po_token,
                "visitorData": visitor_data,
            }
        });

        Some(TrackUrlResult {
            url: Some(server_abr_url.to_string()),
            protocol: Some("sabr".to_string()),
            format: json!("sabr"),
            new_track: None,
            additional_data,
            exception: None,
        })
    }

    fn parse_web_search_results(&self, data: &Value) -> Vec<TrackData> {
        let items_path = "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents/0/itemSectionRenderer/contents";

        let mut tracks: Vec<TrackData> = Vec::new();
        if let Some(arr) = data.pointer(items_path).and_then(|i| i.as_array()) {
            for item in arr {
                if let Some(video) = item.get("videoRenderer") {
                    if let Some(track) = self.parse_video_renderer(video) {
                        tracks.push(track);
                    }
                } else if let Some(playlist) = item.get("playlistRenderer") {
                    if let Some(track) = Self::parse_playlist_renderer(playlist) {
                        tracks.push(track);
                    }
                }
            }
        }
        tracks
    }

    fn parse_search_continuation_page(&self, data: &Value) -> Vec<TrackData> {
        let items_path = "/onResponseReceivedCommands/0/appendContinuationItemsAction/contents";

        let mut tracks: Vec<TrackData> = Vec::new();
        if let Some(arr) = data.pointer(items_path).and_then(|c| c.as_array()) {
            for item in arr {
                if let Some(video) = item.get("videoRenderer") {
                    if let Some(track) = self.parse_video_renderer(video) {
                        tracks.push(track);
                    }
                } else if let Some(playlist) = item.get("playlistRenderer") {
                    if let Some(track) = Self::parse_playlist_renderer(playlist) {
                        tracks.push(track);
                    }
                }
            }
        }
        tracks
    }

    fn parse_playlist_renderer(item: &Value) -> Option<TrackData> {
        let playlist_id = item.get("playlistId").and_then(|v| v.as_str())?;
        let title = item
            .pointer("/title/runs/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Playlist");
        let author = item
            .pointer("/shortBylineText/runs/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Channel");
        let thumbnail = item
            .pointer("/thumbnail/thumbnails/0/url")
            .and_then(|v| v.as_str())
            .map(|t| {
                if t.starts_with("//") {
                    format!("https:{}", t)
                } else {
                    t.to_owned()
                }
            });

        let playlist_uri = format!("https://www.youtube.com/playlist?list={playlist_id}");
        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: playlist_uri.clone(),
                is_seekable: true,
                author: author.to_owned(),
                length: 0,
                is_stream: false,
                position: 0,
                title: title.to_owned(),
                uri: Some(playlist_uri),
                artwork_url: thumbnail,
                isrc: None,
                source_name: "youtube".into(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Some(track)
    }

    fn parse_music_search_results(&self, data: &Value) -> Vec<TrackData> {
        let shelf_path = "/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents";

        let mut tracks: Vec<TrackData> = Vec::new();

        if let Some(contents) = data.pointer(shelf_path).and_then(|c| c.as_array()) {
            for section in contents {
                if let Some(items) = section
                    .pointer("/musicShelfRenderer/contents")
                    .and_then(|c| c.as_array())
                {
                    for item in items {
                        if let Some(track) =
                            self.parse_music_list_item(item.get("musicResponsiveListItemRenderer"))
                        {
                            tracks.push(track);
                        }
                    }
                }
            }
        }

        tracks
    }

    fn parse_music_list_item(&self, item: Option<&Value>) -> Option<TrackData> {
        let item = item?;

        let id = item.get("videoId").and_then(|v| v.as_str())?;

        let title = item
            .pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown Title");

        let author = item
            .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs")
            .and_then(|runs| {
                let mut parts = Vec::new();
                for run in runs.as_array()? {
                    if let Some(text) = run.get("text").and_then(|t| t.as_str()) {
                        parts.push(text);
                    }
                }
                Some(parts.concat())
            })
            .unwrap_or_else(|| {
                item.pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown Artist")
                    .to_string()
            });

        let length_ms = item
            .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text")
            .and_then(|v| v.as_str())
            .and_then(parse_duration)
            .unwrap_or(0);

        let thumbnail = item
            .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails/0/url")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.pointer("/thumbnail/thumbnails/0/url")
                    .and_then(|v| v.as_str())
            })
            .map(|t| {
                if t.starts_with("//") {
                    format!("https:{}", t)
                } else {
                    t.to_owned()
                }
            });

        let mut track = TrackData {
            encoded: None,
            info: TrackInfo {
                identifier: id.to_owned(),
                is_seekable: true,
                author: author.to_owned(),
                length: length_ms,
                is_stream: false,
                position: 0,
                title: title.to_owned(),
                uri: Some(format!("https://www.youtube.com/watch?v={}", id)),
                artwork_url: thumbnail.or_else(|| {
                    Some(format!(
                        "https://img.youtube.com/vi/{}/maxresdefault.jpg",
                        id
                    ))
                }),
                isrc: None,
                source_name: "youtube".into(),
                chapters: None,
            },
            plugin_info: json!({}),
            user_data: json!({}),
            details: Vec::new(),
            message_flags: 0,
        };
        track.encoded = Some(encode_track(&track));
        Some(track)
    }

    async fn try_player_with_auth(&self, video_id: &str, client_kind: ClientKind) -> Option<FormatInfo> {
        let data = self
            .innertube_with_client(
                "player",
                json!({
                    "videoId": video_id,
                    "playbackContext": {
                        "contentPlaybackContext": {
                            "html5Preference": "HTML5_PREF_WANTS"
                        }
                    }
                }),
                client_kind,
            )
            .await
            .ok()?;

        if check_playability(&data).is_some() {
            return None;
        }

        self.extract_best_audio_url(&data, client_kind.requires_player_script())
            .await
    }
}

fn extract_continuation_token(contents: Option<&Value>) -> Option<String> {
    let arr = contents?.as_array()?;
    let last = arr.last()?;
    last.pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn extract_continuation_token_from_next(data: &Value) -> Option<String> {
    let contents = data
        .pointer("/onResponseReceivedCommands/0/appendContinuationItemsAction/contents")?
        .as_array()?;
    let last = contents.last()?;
    last.pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn check_playability(data: &Value) -> Option<String> {
    let status = data
        .pointer("/playabilityStatus/status")
        .and_then(|v| v.as_str())?;

    match status {
        "OK" => None,
        _ => {
            let reason = data
                .pointer("/playabilityStatus/reason")
                .and_then(|v| v.as_str())
                .unwrap_or(status);
            Some(format!("{status}: {reason}"))
        }
    }
}

struct FormatInfo {
    url: String,
    format: String,
    protocol: String,
    is_live: bool,
}

fn itag_to_format(itag: u32) -> &'static str {
    match itag {
        249 | 250 | 251 => "opus",
        139 | 140 => "aac",
        255 | 257 => "opus",
        256 | 258 => "aac",
        171 | 172 => "vorbis",
        34 | 35 => "mp3",
        36 | 18 | 22 => "aac",
        _ => "opus",
    }
}

fn decipher_url_basic(cipher_str: &str) -> Option<String> {
    let params: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(cipher_str.as_bytes())
            .into_owned()
            .collect();

    let url = params.get("url")?;
    let s = params.get("s")?;
    let sp = params.get("sp").map(|s| s.as_str()).unwrap_or("signature");

    if url.contains(sp) {
        return Some(url.to_owned());
    }

    let sep = if url.contains('?') { "&" } else { "?" };
    Some(format!("{url}{sep}{sp}={s}"))
}

fn parse_duration(dur: &str) -> Option<i64> {
    let parts: Vec<&str> = dur.split(':').collect();
    match parts.len() {
        3 => {
            let h = parts[0].parse::<i64>().ok()?;
            let m = parts[1].parse::<i64>().ok()?;
            let s = parts[2].parse::<i64>().ok()?;
            Some((h * 3600 + m * 60 + s) * 1000)
        }
        2 => {
            let m = parts[0].parse::<i64>().ok()?;
            let s = parts[1].parse::<i64>().ok()?;
            Some((m * 60 + s) * 1000)
        }
        _ => None,
    }
}

#[async_trait]
impl SourceProvider for YouTubeSource {
    fn name(&self) -> &'static str {
        "youtube"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["yt", "ytsearch"]
    }

    fn search_terms(&self) -> &'static [&'static str] {
        &["ytsearch", "ytmsearch", "youtube"]
    }

    async fn search(&self, query: &str, search_type: Option<&str>) -> anyhow::Result<SourceResult> {
        info!(target: "YouTube", "Searching: {} (type={:?})", query, search_type);

        let is_music = search_type == Some("ytmsearch");
        let client = if is_music { ClientKind::Music } else { ClientKind::Web };

        let payload = {
            let mut payload = json!({ "query": query });
            if let Some(params) = client.search_params("tracks") {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert("params".into(), json!(params));
                }
            }
            payload
        };

        let data = match self.innertube_with_client(
            "search",
            payload,
            client,
        ).await {
            Ok(d) => d,
            Err(e) => {
                warn!(target: "YouTube", "Search API error: {e}");
                return Ok(SourceResult::Empty);
            }
        };

        let mut tracks = if is_music {
            self.parse_music_search_results(&data)
        } else {
            self.parse_web_search_results(&data)
        };

        // Search continuation (non-music only)
        if !is_music && !tracks.is_empty() {
            let items_path = "/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents/0/itemSectionRenderer/contents";
            let mut continuation_token =
                extract_continuation_token(data.pointer(items_path));

            for _ in 0..5 {
                let token = match continuation_token {
                    Some(t) => t,
                    None => break,
                };

                match self
                    .innertube_with_client("next", json!({ "continuation": token }), ClientKind::Web)
                    .await
                {
                    Ok(page_data) => {
                        let page_tracks = self.parse_search_continuation_page(&page_data);
                        if page_tracks.is_empty() {
                            break;
                        }
                        tracks.extend(page_tracks);
                        continuation_token = extract_continuation_token_from_next(&page_data);
                    }
                    Err(e) => {
                        warn!(target: "YouTube", "Search continuation error: {e}");
                        break;
                    }
                }
            }
        }

        if tracks.is_empty() {
            return Ok(SourceResult::Empty);
        }
        Ok(SourceResult::Search { data: tracks })
    }

    async fn resolve(&self, query: &str, _kind: Option<&str>) -> anyhow::Result<SourceResult> {
        // Check for playlist first
        if let Some(playlist_id) = extract_playlist_id(query) {
            info!(target: "YouTube", "Resolving playlist: {}", playlist_id);
            match self.resolve_playlist(&playlist_id).await {
                Ok(Some(pl)) => return Ok(pl),
                Ok(None) => warn!(target: "YouTube", "Playlist resolve returned no results"),
                Err(e) => warn!(target: "YouTube", "Playlist resolve error: {e}"),
            }
        }

        let video_id = extract_video_id(query);
        let video_id = match video_id {
            Some(id) => id,
            None => return Ok(SourceResult::Empty),
        };

        info!(target: "YouTube", "Resolving video: {}", video_id);

        match self.resolve_video(&video_id).await {
            Ok(Some(track)) => Ok(SourceResult::Track(track)),
            Ok(None) => Ok(SourceResult::Empty),
            Err(e) => {
                warn!(target: "YouTube", "Resolve error: {e}");
                Ok(SourceResult::Empty)
            }
        }
    }

    async fn get_track_url(&self, track: &TrackInfo) -> anyhow::Result<TrackUrlResult> {
        let video_id = &track.identifier;
        let mut last_error = String::new();
        let mut tried_login = false;

        for &client in &self.enabled_clients {
            if !client.can_provide_track_url() {
                continue;
            }
            let data = match self
                .innertube_with_client(
                    "player",
                    json!({
                        "videoId": video_id,
                        "playbackContext": {
                            "contentPlaybackContext": {
                                "html5Preference": "HTML5_PREF_WANTS"
                            }
                        }
                    }),
                    client,
                )
                .await
            {
                Ok(d) => d,
                Err(e) => {
                    let err_msg = format!("{} request error: {e}", client.display_name());
                    warn!(target: "YouTube", "{}", err_msg);
                    last_error = err_msg;
                    continue;
                }
            };

            // Check playability status
            let playability = check_playability(&data);
            if let Some(reason) = &playability {
                last_error = format!("{}: {}", client.display_name(), reason);
                warn!(target: "YouTube", "{}", last_error);

                // If LOGIN_REQUIRED and OAuth available, retry with TV only
                if reason.contains("LOGIN_REQUIRED") && !tried_login && self.oauth.is_some() {
                    tried_login = true;
                    if let Some(info) = self
                        .try_player_with_auth(video_id, ClientKind::Tv)
                        .await
                    {
                        return Ok(TrackUrlResult {
                            url: Some(info.url),
                            protocol: Some(info.protocol),
                            format: json!(info.format),
                            new_track: None,
                            additional_data: json!({}),
                            exception: None,
                        });
                    }
                }

                continue;
            }

            // Check for SABR streaming URL
            let po_token = self.potoken.get_token().await;
            let visitor_data = self.potoken.visitor_data();
            if let Some(sabr_result) = self.extract_sabr_config(&data, &client, po_token, &visitor_data) {
                return Ok(sabr_result);
            }

            let needs_cipher = client.requires_player_script();
            if let Some(info) = self.extract_best_audio_url(&data, needs_cipher).await {
                return Ok(TrackUrlResult {
                    url: Some(info.url),
                    protocol: Some(info.protocol),
                    format: json!(info.format),
                    new_track: None,
                    additional_data: json!({}),
                    exception: None,
                });
            }

            last_error = format!("{}: no playable format", client.display_name());
            warn!(target: "YouTube", "{} for {video_id}", last_error);
        }

        warn!(target: "YouTube", "No playable audio format found for {video_id}");
        Ok(TrackUrlResult {
            url: None,
            protocol: None,
            format: json!({}),
            new_track: None,
            additional_data: json!({}),
            exception: Some(if last_error.is_empty() {
                "No playable audio format found".into()
            } else {
                last_error
            }),
        })
    }

    async fn get_chapters(&self, track: &TrackInfo) -> anyhow::Result<Vec<Chapter>> {
        let data = self
            .innertube_with_client(
                "player",
                json!({
                    "videoId": &track.identifier,
                }),
                ClientKind::Web,
            )
            .await?;

        // Try chapters from playerOverlays
        if let Some(markers) = data
            .pointer("/playerOverlays/playerOverlayRenderer/decoratedPlayerBarRenderer/decoratedPlayerBarRenderer/multiMarkersPlayerBarRenderer/markersMap")
            .and_then(|v| v.as_array())
        {
            for marker_map in markers {
                if marker_map["key"].as_str() == Some("DESCRIPTION_CHAPTERS") {
                    if let Some(chapters) = marker_map["value"]["chapters"].as_array() {
                        let mut result = Vec::new();
                        for ch in chapters {
                            let title = ch["title"]["simpleText"].as_str().map(|s| s.to_string())
                                .or_else(|| {
                                    ch["title"]["runs"].as_array().map(|runs| {
                                        runs.iter().filter_map(|r| r["text"].as_str()).collect::<Vec<_>>().join(" ")
                                    })
                                })
                                .unwrap_or_default();
                            let start_ms = ch["chapterRenderer"]["timeRangeStartMillis"]
                                .as_u64()
                                .unwrap_or(0);
                            let end_ms = ch["chapterRenderer"]["timeRangeEndMillis"]
                                .as_u64()
                                .unwrap_or(0);
                            result.push(Chapter { title, start: start_ms, end: end_ms });
                        }
                        if !result.is_empty() {
                            return Ok(result);
                        }
                    }
                }
            }
        }

        // Fallback: parse description for timestamp-based chapters
        if let Some(desc) = data["videoDetails"]["shortDescription"].as_str() {
            let chapters = parse_chapters_from_description(desc, track.length);
            if !chapters.is_empty() {
                return Ok(chapters);
            }
        }

        Ok(Vec::new())
    }
}

fn parse_chapters_from_description(desc: &str, track_length_ms: i64) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    for line in desc.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Find first timestamp-like pattern (e.g., 0:00, 1:30, 12:45, 1:23:45)
        let mut ts_end = 0;
        let mut found = false;
        let bytes = line.as_bytes();
        for i in 0..bytes.len() {
            if bytes[i].is_ascii_digit() {
                // check for mm:ss or hh:mm:ss pattern
                let mut j = i;
                let mut colons = 0;
                while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b':') {
                    if bytes[j] == b':' {
                        colons += 1;
                    }
                    j += 1;
                }
                if colons >= 1 && colons <= 2 && j - i >= 4 {
                    let after = if j < bytes.len() { bytes[j] } else { b' ' };
                    if after == b' ' || after == b'\t' || after == b'-' || after == b':' {
                        ts_end = j;
                        found = true;
                        break;
                    }
                }
            }
        }
        if !found || ts_end >= line.len() {
            continue;
        }
        let time_str = &line[..ts_end];
        let title = line[ts_end..].trim().trim_start_matches(&[' ', '\t', '-', ':'][..]).trim().to_string();
        if title.is_empty() {
            continue;
        }
        if let Some(start_ms) = parse_timestamp_ms(time_str) {
            chapters.push(Chapter {
                title,
                start: start_ms,
                end: 0,
            });
        }
    }
    chapters.sort_by_key(|c| c.start);
    chapters.dedup_by_key(|c| c.start);
    for i in 0..chapters.len() {
        let end = if i + 1 < chapters.len() {
            chapters[i + 1].start
        } else {
            track_length_ms.max(0) as u64
        };
        chapters[i].end = end;
    }
    chapters
}

fn parse_timestamp_ms(ts: &str) -> Option<u64> {
    let parts: Vec<&str> = ts.split(':').collect();
    match parts.len() {
        3 => {
            let h = parts[0].parse::<u64>().ok()?;
            let m = parts[1].parse::<u64>().ok()?;
            let s = parts[2].parse::<u64>().ok()?;
            Some((h * 3600 + m * 60 + s) * 1000)
        }
        2 => {
            let m = parts[0].parse::<u64>().ok()?;
            let s = parts[1].parse::<u64>().ok()?;
            Some((m * 60 + s) * 1000)
        }
        _ => None,
    }
}

fn extract_captions(data: &Value) -> Option<Vec<Value>> {
    let tracks = data
        .pointer("/captions/playerCaptionsTracklistRenderer/captionTracks")?
        .as_array()?;

    let extracted: Vec<Value> = tracks
        .iter()
        .filter_map(|ct| {
            let language_code = ct["languageCode"].as_str()?;
            let base_url = ct["baseUrl"].as_str()?;
            Some(json!({
                "languageCode": language_code,
                "name": ct["name"]["simpleText"].as_str().unwrap_or(language_code),
                "isTranslatable": ct["isTranslatable"].as_bool().unwrap_or(false),
                "baseUrl": base_url,
                "kind": ct.get("kind").and_then(|k| k.as_str()).unwrap_or(""),
            }))
        })
        .collect();

    if extracted.is_empty() {
        None
    } else {
        Some(extracted)
    }
}

fn extract_video_id(input: &str) -> Option<String> {
    if input.len() == 11 && input.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Some(input.to_owned());
    }

    if let Some(url) = url::Url::parse(input).ok() {
        if url.host_str()? == "youtu.be" {
            return url.path_segments()?.next().map(|s| s.to_owned());
        }
        if url
            .host_str()?
            .contains("youtube.com")
        {
            if let Some(v) = url.query_pairs().find(|(k, _)| k == "v") {
                return Some(v.1.to_string());
            }
        }
    }

    None
}

fn extract_playlist_id(input: &str) -> Option<String> {
    // Try URL extraction first
    if let Ok(url) = url::Url::parse(input) {
        if let Some(host) = url.host_str() {
            if host.contains("youtube.com") || host == "youtu.be" {
                if let Some(list) = url.query_pairs().find(|(k, _)| k == "list") {
                    let id = list.1.to_string();
                    if !id.is_empty() && id.len() <= 64 {
                        return Some(id);
                    }
                }
            }
        }
    }

    // Accept bare playlist/Mix IDs (not URLs, not video IDs)
    if !input.contains('/') && !input.contains('.') {
        let trimmed = input.trim();
        let is_playlist_pattern = trimmed.starts_with("PL")
            || trimmed.starts_with("RD")
            || trimmed.starts_with("OL")
            || trimmed.starts_with("FL")
            || trimmed.starts_with("LL")
            || trimmed.starts_with("UU");
        if is_playlist_pattern && trimmed.len() >= 13 && trimmed.len() <= 64 {
            return Some(trimmed.to_owned());
        }
    }

    None
}
