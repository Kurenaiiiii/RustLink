# syntax=docker/dockerfile:1.6
# RustLink — single-binary Lavalink server (MIT)
# Builds `rustlink` (88M) and bakes `ffmpeg` for YouTube SABR fMP4

# ── Builder ──────────────────────────────────────────────────────────
FROM rust:1.82-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config cmake clang libssl-dev perl && rm -rf /var/lib/apt/lists/*

WORKDIR /app
# Leverage Docker layer cache: copy Cargo manifests first
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
# Dummy build to cache deps (will be rebuilt with real src)
RUN mkdir -p src/bin && touch src/bin/playback_worker.rs src/bin/source_worker.rs
RUN cargo build --release --locked || true

# Copy the real source and build the actual binary
COPY . .
RUN cargo build --release --locked && strip target/release/rustlink

# ── Runner ───────────────────────────────────────────────────────────
FROM debian:bookworm-slim

# ffmpeg is REQUIRED for YouTube SABR (fMP4 dash+sidx+moof). Without it
# RustLink will refuse to start (see src/main.rs:check_required_binaries).
# This is an external binary, not a linked library, so MIT is preserved.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg ca-certificates curl tini \
    && rm -rf /var/lib/apt/lists/* \
    && ffmpeg -version | head -1

WORKDIR /app
COPY --from=builder /app/target/release/rustlink ./rustlink
COPY rustlink.toml ./rustlink.toml
COPY README.md LICENSE ./

# Lavalink-compatible port (configurable via RUSTLINK__SERVER__PORT / NODELINK__SERVER__PORT)
EXPOSE 2333 8211 3000

ENV RUSTLINK__SERVER__HOST=0.0.0.0 \
    RUSTLINK__SERVER__PORT=2333 \
    RUST_LOG=info

# Use tini as init to handle SIGTERM correctly
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["./rustlink"]
