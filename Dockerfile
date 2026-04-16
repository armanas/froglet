ARG RUST_VERSION=1.91

FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /app

COPY . .

RUN cargo build --release --locked --bin froglet-node -p froglet

FROM debian:bookworm-slim AS runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl gosu tor \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --gid 10001 froglet \
    && useradd --uid 10001 --gid froglet --home-dir /nonexistent --shell /usr/sbin/nologin froglet \
    && mkdir -p /data \
    && chown froglet:froglet /data

COPY scripts/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod 0755 /usr/local/bin/docker-entrypoint.sh

WORKDIR /app

FROM runtime-base AS python-runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends python3 \
    && rm -rf /var/lib/apt/lists/*

# ── froglet-node (provider mode) ──────────────────────────────────
FROM python-runtime-base AS provider
COPY --from=builder /app/target/release/froglet-node /usr/local/bin/froglet-node

ENV FROGLET_NODE_ROLE=provider \
    FROGLET_DATA_DIR=/data \
    FROGLET_IDENTITY_AUTO_GENERATE=true \
    FROGLET_LISTEN_ADDR=0.0.0.0:8080 \
    FROGLET_RUNTIME_LISTEN_ADDR=127.0.0.1:0 \
    FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:8082 \
    FROGLET_TOR_BINARY=tor

VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["froglet-node"]

# ── froglet-node (runtime/requester mode) ─────────────────────────
FROM runtime-base AS runtime
COPY --from=builder /app/target/release/froglet-node /usr/local/bin/froglet-node

ENV FROGLET_NODE_ROLE=runtime \
    FROGLET_DATA_DIR=/data \
    FROGLET_IDENTITY_AUTO_GENERATE=true \
    FROGLET_LISTEN_ADDR=127.0.0.1:0 \
    FROGLET_RUNTIME_LISTEN_ADDR=0.0.0.0:8081 \
    FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=true \
    FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:0 \
    FROGLET_TOR_BINARY=tor

VOLUME ["/data"]
EXPOSE 8081
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["froglet-node"]

# ── froglet-node (dual mode — both provider + runtime) ────────────
FROM python-runtime-base AS dual
COPY --from=builder /app/target/release/froglet-node /usr/local/bin/froglet-node

ENV FROGLET_NODE_ROLE=dual \
    FROGLET_DATA_DIR=/data \
    FROGLET_IDENTITY_AUTO_GENERATE=true \
    FROGLET_LISTEN_ADDR=0.0.0.0:8080 \
    FROGLET_RUNTIME_LISTEN_ADDR=0.0.0.0:8081 \
    FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=true \
    FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:8082 \
    FROGLET_TOR_BINARY=tor

VOLUME ["/data"]
EXPOSE 8080 8081
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["froglet-node"]
