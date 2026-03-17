ARG RUST_VERSION=1.90

FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /app

COPY . .

RUN cargo build --release --locked --bin froglet --bin marketplace

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

FROM runtime-base AS froglet
COPY --from=builder /app/target/release/froglet /usr/local/bin/froglet

ENV FROGLET_DATA_DIR=/data \
    FROGLET_IDENTITY_AUTO_GENERATE=true \
    FROGLET_LISTEN_ADDR=0.0.0.0:8080 \
    FROGLET_RUNTIME_LISTEN_ADDR=127.0.0.1:8081 \
    FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:8082 \
    FROGLET_TOR_BINARY=tor

VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["froglet"]

FROM runtime-base AS marketplace
COPY --from=builder /app/target/release/marketplace /usr/local/bin/marketplace

ENV FROGLET_MARKETPLACE_LISTEN_ADDR=0.0.0.0:9090 \
    FROGLET_MARKETPLACE_DB_PATH=/data/marketplace.db

VOLUME ["/data"]
EXPOSE 9090
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["marketplace"]
