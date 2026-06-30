# RustLink

> A high-performance [Lavalink](https://lavalink.dev) v4 compatible audio server — Rust port of [NodeLink v3](https://github.com/PerformanC/NodeLink). Native audio processing, zero JavaScript dependencies, thread-safe concurrency.

---

## Features

**Audio Engine**
- Symphonia decoding (MP3, FLAC, OGG, WAV, AAC, Opus) → Rubato resampling → Opus encoding
- EBU R128 loudness normalizer (BS.1770-4 weighting, lookahead gain, smooth transitions)
- Multi-layer `AudioMixer` with constant-power panning
- Configurable fading (tape/volume curves) for track start, end, pause, resume
- Crossfade via ring-buffer: capture last N frames, fade-out old + fade-in new, mix between tracks
- Real-time filter chain: equalizer, karaoke, timescale, tremolo, vibrato, lowpass, highpass, rotation, distortion, channel mix, chorus, compressor, echo, phaser

**Music Sources (40+)**

| Source | Search | Resolve | Stream | Auth Required |
|--------|--------|---------|--------|---------------|
| YouTube | ✓ | ✓ | ✓ | Optional (OAuth/PO token) |
| Spotify | ✓ | ✓ | ✓ | Client ID + Secret |
| SoundCloud | ✓ | ✓ | ✓ | — |
| Apple Music | ✓ | ✓ | ✓ | Media API Token (JWT) |
| Deezer | ✓ | ✓ | ✓ | ARL cookie |
| Bandcamp | ✓ | ✓ | ✓ | — |
| Twitch | ✓ | ✓ | ✓ | — |
| Vimeo | ✓ | ✓ | ✓ | — |
| Bilibili | ✓ | — | ✓ | SESSDATA cookie |
| NicoNico | ✓ | — | ✓ | — |
| Mixcloud | ✓ | ✓ | ✓ | — |
| Reddit | ✓ | — | ✓ | — |
| Tumblr | ✓ | — | ✓ | — |
| Twitter/X | ✓ | — | ✓ | — |
| Instagram | ✓ | — | ✓ | — |
| Telegram | ✓ | — | ✓ | — |
| Google Drive | ✓ | — | ✓ | — |
| Amazon Music | ✓ | — | ✓ | — |
| HTTP (raw URL) | — | — | ✓ | — |
| Local filesystem | — | — | ✓ | — |
| Tidal | ✓ | — | ✓ | Token |
| Qobuz | ✓ | — | ✓ | User token |
| JioSaavn | ✓ | — | ✓ | — |
| Yandex Music | ✓ | — | ✓ | Access token |
| Gaana | ✓ | — | ✓ | — |
| Audius | ✓ | — | ✓ | App name + API key |
| AudioMack | ✓ | — | ✓ | — |
| Pandora | ✓ | — | ✓ | CSRF token |
| VK Music | ✓ | — | ✓ | User token |
| EternalBox | ✓ | — | ✓ | — |
| Anghami | — | — | ✓ | Cookies |
| Shazam | ✓ | — | — | — |
| Last.fm | ✓ | — | — | API key |
| Pinterest | ✓ | — | ✓ | — |
| RSS/Podcasts | ✓ | — | ✓ | — |
| iHeartRadio | ✓ | — | ✓ | — |
| Bluesky | ✓ | — | ✓ | — |
| Kwai | ✓ | — | ✓ | — |
| Songlink/Odesli | ✓ | — | — | — |
| Netease | ✓ | — | ✓ | — |
| Monochrome | ✓ | — | ✓ | — |
| Flowery TTS | — | — | ✓ | — |
| LazyPy TTS | — | — | ✓ | — |
| Piper TTS | — | — | ✓ | — |

**Lyrics** — Synced (LRC) and plain lyrics from YouTube, Genius, Musixmatch, Deezer, LRCLib, Letrasmus, Bilibili, Yandex Music, Monochrome

**SponsorBlock** — Skip sponsored segments, intros, outros, and more on YouTube tracks

**Voice Resilience** — Discord voice heartbeat (`op 3`), WebSocket resume/reconnect, configurable stuck-track threshold

**REST API** — Full Lavalink v4 API: sessions, players, track loading, decoding, encoding, routeplanner, stats, metrics, plugins, lyrics, meaning

**Performance**
- In-process worker mode (no IPC overhead) or multi-process clustering
- Optional auto-scaling of workers based on CPU/lag utilization
- In-memory LRU track cache with configurable TTL
- Connection quality-based routing to best voice server

---

## Quick Start

### Download

Grab the latest binary from [Releases](../../releases) or build from source:

```bash
git clone https://github.com/your-org/rustlink.git
cd rustlink
cargo build --release
```

### Configure

Copy the example config and edit:

```bash
cp rustlink.toml rustlink.local.toml
# Edit rustlink.local.toml with your settings
```

Minimal config:
```toml
[server]
password = "your-password-here"

[sources.spotify]
enabled = true
client_id = "your-spotify-client-id"
client_secret = "your-spotify-client-secret"
```

RustLink loads `rustlink.toml` by default. You can also set any value via environment variables:

```bash
export RUSTLINK__SERVER__PORT=2334
export NODELINK__SOURCES__YOUTUBE__HL=ja
```

### Run

```bash
./rustlink
```

Or with Docker (if you build the image):
```bash
docker compose up -d
```

### Connect

Point your Discord bot (using Lavalink v4 client) to `ws://your-server:2333` with the password you configured.

---

## Configuration Reference

RustLink is configured via `rustlink.toml` (TOML format). Every field has a sensible default — you only need to set what you want to override.

### `[server]`

| Key | Default | Description |
|-----|---------|-------------|
| `host` | `"0.0.0.0"` | Bind address |
| `port` | `2333` | Lavalink-compatible port |
| `password` | `"youshallnotpass"` | Auth password (required by Lavalink protocol) |
| `max_body_size` | `10485760` | Max HTTP request body (bytes) |

### `[logging]`

| Key | Default | Description |
|-----|---------|-------------|
| `level` | `"info"` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `file.enabled` | `false` | Write logs to file in addition to stdout |
| `file.path` | `"logs"` | Log directory |
| `file.rotation` | `"daily"` | Rotation: `daily`, `hourly`, `never` |
| `file.ttl_days` | `7` | Delete logs older than N days |

`[logging.debug]` — Enable verbose debug per subsystem:
`all`, `request`, `session`, `player`, `filters`, `sources`, `lyrics`, `youtube`, `youtube-cipher`, `sabr`, `potoken`

### `[connection]`

| Key | Default | Description |
|-----|---------|-------------|
| `interval` | `2147483647` | Health check interval (ms, effectively disabled) |
| `timeout` | `2000` | Health check timeout (ms) |
| `thresholds.bad` | `1` | Penalty threshold for bad connections |
| `thresholds.average` | `5` | Penalty threshold for average connections |

### `[metrics]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Enable Prometheus `/metrics` endpoint |
| `username` | `"admin"` | Basic auth username |
| `password` | `null` | Falls back to server password if null |
| `interval` | `5000` | Collection interval (ms) |

### `[rate_limit]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Master switch |
| `global.max_requests` | `1000` | Global limit |
| `perIp.max_requests` | `100` | Per-IP limit |
| `perUserId.max_requests` | `50` | Per-user limit |
| `perGuildId.max_requests` | `20` | Per-guild limit |
| `ignore_paths` | `[]` | Paths exempt from rate limiting |
| `ignore.user_ids` / `guild_ids` / `ips` | `[]` | Entities exempt from rate limiting |

Time windows are configurable per tier via `time_window_ms`.

### `[dos_protection]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `true` | Master switch |
| `max_body_size` | `10485760` | Max body size (bytes) |
| `max_requests_per_second` | `50` | Requests/sec threshold |
| `thresholds.burst_requests` | `50` | Burst limit |
| `thresholds.time_window_ms` | `10000` | Burst window |
| `mitigation.delay_ms` | `500` | Extra delay when throttled |
| `mitigation.block_duration_ms` | `300000` | Block duration (5 min) |

### `[audio]`

| Key | Default | Description |
|-----|---------|-------------|
| `quality` | `"high"` | Opus quality: `low`, `medium`, `high`, `best` |
| `encryption` | `"aead_aes256_gcm_rtpsize"` | Encryption mode |
| `resampling_quality` | `"best"` | `fast`, `medium`, `best` |
| `loudness_normalizer` | `true` | EBU R128 normalization |
| `lookahead_ms` | `200` | Loudness lookahead |
| `gate_threshold_lufs` | `-60.0` | Silence gate |

`[audio.fading.*]` — Per-event fade config (duration, curve, type)

`[audio.crossfade]` — Crossfade between tracks (duration, curve, mode, buffer)

### `[filters.enabled]`

Toggle individual audio filters on/off. All default to `true`.

### `[sponsorblock]`

| Key | Default | Description |
|-----|---------|-------------|
| `api` | `"https://sponsor.ajay.app"` | SponsorBlock API |
| `categories` | 8 categories | Segment types to skip |
| `action_types` | `["skip"]` | `skip`, `mute`, `full`, `poi` |
| `skip_margin_ms` | `150` | Padding around segments |

### `[lyrics.*]`

Per-source toggle for each lyrics provider. `fallback_source` defaults to `"genius"`.

### `[voiceReceive]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Receive audio from voice channels |
| `format` | `"opus"` | Output format: `opus` or `pcm_s16le` |

### `[routePlanner]`

| Key | Default | Description |
|-----|---------|-------------|
| `strategy` | `"RotateOnBan"` | `RotateOnBan`, `LoadBalance`, `NanoIp`, `RotatingNanoIp` |
| `bannedIpCooldown` | `600000` | Cooldown before retrying banned IP (ms) |

### `[cluster]`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Multi-worker mode |
| `workers` | `0` | Worker count (0 = auto) |
| `process_mode` | `"in-process"` | `in-process` or `multi-process` |
| `redis_url` | `null` | Redis URL for multi-node clustering |

### `[sources.*]`

Each music source has `enabled = true/false`. Sources with credentials or special config:

**YouTube** — `allow_itag`, `potoken`, `hl`, `gl`, `proxies`, `ciphers`, client selection

**Spotify** — `client_id`, `client_secret`, `market`, load limits, concurrency

**Apple Music** — `mediaApiToken` (JWT), `market`, load limits

**Deezer** — `arl` (cookie), `decryptionKey`

**See `rustlink.toml` for the full documented reference with all ~150 options.**

---

## API Endpoints

RustLink implements the full Lavalink v4 REST API:

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/v4/info` | Server info & version |
| `GET` | `/v4/decodetrack` | Decode base64 track |
| `GET` | `/v4/encodetrack` | Encode track to base64 |
| `GET` | `/v4/loadtracks?identifier=` | Load tracks from query/URL |
| `GET` | `/v4/websocket` | WebSocket (Lavalink protocol) |
| `PATCH` | `/v4/sessions/:id` | Update session config |
| `GET` | `/v4/sessions/:id/players` | List players |
| `GET/PATCH/DELETE` | `/v4/sessions/:id/players/:guild` | Player operations |
| `GET` | `/v4/stats` | Server stats |
| `GET` | `/v4/routeplanner/status` | Route planner status |
| `GET` | `/v4/version` | Version info |
| `GET` | `/metrics` | Prometheus metrics (if enabled) |

WebSocket opcodes: `voiceUpdate`, `play`, `stop`, `pause`, `seek`, `volume`, `filters`, `destroy`, `configureResuming`, `mixer`

---

## Building

### Prerequisites

- **Rust** 1.82+ (`rustup install stable && rustup default stable`)
- **CMake** 3.5+ (for `audiopus-sys`)
- **C toolchain** (MSVC on Windows, GCC/Clang on Linux)

### Linux / Pterodactyl

```bash
# Install dependencies
apt update && apt install -y build-essential cmake pkg-config libopus-dev

# Build
cargo build --release

# Binary at: target/release/rustlink
# Worker binaries: target/release/playback-worker, target/release/source-worker
```

### Cross-compile (Windows → Linux)

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

---

## Architecture

```
┌───────────────────────────────────────────────────────────────┐
│                         RustLink                              │
│  ┌───────────┐  ┌──────────────┐  ┌────────────────────────┐│
│  │ REST API   │  │ WebSocket    │  │  Worker Manager        ││
│  │ (Axum)     │  │ (tokio-tung) │  │  ┌──────────────────┐ ││
│  └─────┬─────┘  └──────┬───────┘  │  │ In-process │ IPC  │ ││
│        │               │          │  └────────┬─────────┘ ││
│        └───────┬───────┘           └──────────┼───────────┘│
│                │                              │            │
│         ┌──────▼──────┐                ┌──────▼───────┐    │
│         │  AppState   │                │  Workers     │    │
│         │  (Shared)   │◄───────────────│  (Audio      │    │
│         │             │                │   Processing)│    │
│         └──────┬──────┘                └──────────────┘    │
│                │                                            │
│         ┌──────▼──────┐                                     │
│         │  Sources    │  YouTube, Spotify, SoundCloud...    │
│         │  (Reqwest)  │   + lyrics, sponsorblock, etc.     │
│         └─────────────┘                                     │
└───────────────────────────────────────────────────────────────┘
```

- **In-process mode** (default): Everything runs in one process — no IPC overhead, simplest setup.
- **Multi-process mode**: Spawns separate `playback-worker` and `source-worker` processes. Communication via named pipes (Windows) or Unix sockets (Linux/macOS). Workers are pooled and auto-scaled.
- **Multi-node mode**: Uses Redis pub/sub to synchronize state across multiple RustLink instances.

---

## Performance Tuning

**For low-latency:**
```toml
[audio]
quality = "best"
resampling_quality = "best"
fading.enabled = false
crossfade.enabled = false
loudness_normalizer = false
```

**For high concurrency (many guilds):**
```toml
[cluster]
enabled = true
workers = 4
process_mode = "multi-process"

[cluster.scaling]
maxPlayersPerWorker = 50
```

**For minimal memory:**
```toml
max_search_results = 10
max_album_playlist_length = 100
track_stuck_threshold_ms = 5000
zombie_threshold_ms = 30000
```

---

## Troubleshooting

**"No such file or directory" for workers** — Multi-process mode requires the worker binaries next to the main binary. Use `process_mode = "in-process"` for single-binary setup.

**YouTube 404 / age-restricted** — Set up a PO token generator and configure `potoken` + `po_token_endpoint` in `[sources.youtube]`.

**Spotify search fails** — Verify `client_id` and `client_secret` are set. They're required even for basic search.

**High memory usage** — Reduce `max_search_results`, `cache_encryption_key` cache size, or lower `max_album_playlist_length`.

**Audio crackling** — Lower `[audio].quality` to `"high"` or `"medium"`, or reduce lookahead.

**Crossfade not working** — Ensure `[audio.crossfade].enabled = true` and `duration > 0`. Crossfade requires minimum buffer (`min_buffer_ms`).

---

## License

This project is licensed under the **GNU Affero General Public License v3.0** — see [LICENSE](LICENSE).

RustLink is a Rust port of [NodeLink v3](https://github.com/PerformanC/NodeLink) by PerformanC, which is also AGPL-3.0.
