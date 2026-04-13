# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM rust:1.82-bookworm AS builder

WORKDIR /build

# Cache dependency compilation by copying manifests first
COPY Cargo.toml Cargo.lock ./
COPY interlink-core/Cargo.toml interlink-core/Cargo.toml
COPY circuits/Cargo.toml circuits/Cargo.toml
COPY relayer/Cargo.toml relayer/Cargo.toml

# Create stub source files so cargo can resolve the workspace
RUN mkdir -p interlink-core/src circuits/src relayer/src relayer/src/bin \
    && echo "pub fn stub() {}" > interlink-core/src/lib.rs \
    && echo "pub fn stub() {}" > circuits/src/lib.rs \
    && echo "pub fn stub() {}" > relayer/src/lib.rs \
    && echo "fn main() {}" > relayer/src/main.rs \
    && echo "fn main() {}" > relayer/src/bin/export_vk.rs \
    && echo "fn main() {}" > relayer/src/bin/benchmark.rs \
    && echo "fn main() {}" > relayer/src/bin/load_test.rs

# Build dependencies only (cached unless Cargo.toml/lock change)
RUN cargo build --release --bin relayer 2>/dev/null || true

# Copy real source and rebuild
COPY interlink-core/ interlink-core/
COPY circuits/ circuits/
COPY relayer/ relayer/

# Touch source files to invalidate the stub build
RUN touch interlink-core/src/lib.rs circuits/src/lib.rs relayer/src/lib.rs relayer/src/main.rs

RUN cargo build --release --bin relayer

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --shell /bin/bash relayer

COPY --from=builder /build/target/release/relayer /usr/local/bin/relayer

USER relayer
WORKDIR /home/relayer

# Required env vars (see .env.example):
#   EVM_WS_RPC_URL, EVM_HTTP_RPC_URL, SOLANA_RPC_URL,
#   GATEWAY_ADDRESS, HUB_PROGRAM_ID, KEYPAIR_PATH

ENV RUST_LOG=relayer=info
ENV LOG_FORMAT=json
ENV API_ADDR=0.0.0.0:8080

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -sf http://localhost:8080/health || exit 1

ENTRYPOINT ["relayer"]
