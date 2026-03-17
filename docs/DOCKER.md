# Froglet Docker Guide

Status: practical packaging guidance for the official container assets in this
repo

## 1. Included Assets

The repo now ships:

- a multi-stage [../Dockerfile](../Dockerfile) with `froglet` and `marketplace`
  targets
- a starter [../compose.yaml](../compose.yaml) that runs a priced Froglet node
  with the reference marketplace
- a dedicated [../compose.full-runtime.yaml](../compose.full-runtime.yaml) for
  host-local bots that need the privileged runtime listener

The container images default to:

- `FROGLET_DATA_DIR=/data`
- `FROGLET_IDENTITY_AUTO_GENERATE=true`
- `FROGLET_LISTEN_ADDR=0.0.0.0:8080`
- `FROGLET_PUBLIC_BASE_URL=http://127.0.0.1:8080`
- `FROGLET_RUNTIME_LISTEN_ADDR=127.0.0.1:8081`
- `FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=false`
- `FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:8082`
- `FROGLET_MARKETPLACE_LISTEN_ADDR=0.0.0.0:9090`
- `FROGLET_MARKETPLACE_DB_PATH=/data/marketplace.db`

The image includes the external `tor` binary so `tor` or `dual` transport modes
can be enabled without rebuilding the image.

## 2. Quick Start

Build and start the starter stack:

```bash
docker compose up --build
```

That brings up:

- Froglet on `http://127.0.0.1:8080`
- the reference marketplace on `http://127.0.0.1:9090`
- mock-Lightning pricing for `execute.wasm`

The starter stack publishes only loopback host ports by default.

`FROGLET_PUBLIC_BASE_URL` keeps marketplace and descriptor advertisements
host-reachable even though the process itself binds `0.0.0.0` inside the
container.

## 3. Full Runtime Mode for Host Bots

If you want OpenClaw or another host-local bot to use the privileged runtime
surface, use the dedicated full-runtime Compose file instead of the starter
stack:

```bash
mkdir -p ./data
docker compose -f compose.full-runtime.yaml up --build
```

That brings up:

- Froglet provider API on `http://127.0.0.1:8080`
- Froglet runtime API on `http://127.0.0.1:8081`
- reference marketplace on `http://127.0.0.1:9090`

It also bind-mounts `./data` into the container, so the runtime auth token is
available to host tools at:

- `./data/runtime/auth.token`

This layout is the easiest Docker path for full OpenClaw runtime mode because
the checked-in full-runtime OpenClaw config can point at the same host URLs and
token path as the direct host process flow.

The dedicated Compose file opts into non-loopback runtime binding inside the
container while still publishing only host loopback ports.

## 4. Single-Image Usage

Build the node image only:

```bash
docker build --target froglet -t froglet:local .
```

Run it with a persistent volume:

```bash
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -v froglet-data:/data \
  -e FROGLET_PRICE_EXEC_WASM=10 \
  -e FROGLET_PAYMENT_BACKEND=lightning \
  -e FROGLET_LIGHTNING_MODE=mock \
  froglet:local
```

Run it in host-accessible full-runtime mode. The `0.0.0.0` override for
`FROGLET_RUNTIME_LISTEN_ADDR` and explicit
`FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=true` opt-in are both mandatory — without
them the runtime either binds loopback inside the container or exits at startup:

```bash
mkdir -p ./data
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -p 127.0.0.1:8081:8081 \
  -v "$PWD/data:/data" \
  -e FROGLET_PRICE_EXEC_WASM=10 \
  -e FROGLET_PAYMENT_BACKEND=lightning \
  -e FROGLET_LIGHTNING_MODE=mock \
  -e FROGLET_RUNTIME_LISTEN_ADDR=0.0.0.0:8081 \
  -e FROGLET_RUNTIME_ALLOW_NON_LOOPBACK=true \
  froglet:local
```

Build the reference marketplace image:

```bash
docker build --target marketplace -t froglet-marketplace:local .
```

Run it with a persistent volume:

```bash
docker run --rm \
  -p 127.0.0.1:9090:9090 \
  -v marketplace-data:/data \
  froglet-marketplace:local
```

## 5. Runtime Boundary in Containers

The Froglet runtime listener is intentionally still loopback-only inside the
starter container layout.

Non-loopback runtime binding is now an explicit opt-in meant for the dedicated
full-runtime Docker path. Do not enable it for normal starter deployments.

That means:

- the public provider API is the normal host-published surface
- the privileged runtime API is not published by the starter Compose stack
- local bots should either use [../compose.full-runtime.yaml](../compose.full-runtime.yaml),
  run beside Froglet in the same container/network namespace, or use
  `docker exec`

To inspect the runtime auth token in the starter stack:

```bash
docker compose exec froglet cat /data/runtime/auth.token
```

## 6. Tor and Real Lightning

The image includes `tor`, but Tor is still optional. To enable it, pass the
same environment variables you would use outside Docker, for example:

```bash
docker run --rm \
  -p 127.0.0.1:8080:8080 \
  -v froglet-data:/data \
  -e FROGLET_NETWORK_MODE=dual \
  froglet:local
```

For `lnd_rest`, mount the TLS cert and macaroon into the container and point the
environment variables at those mounted paths.

## 7. Volume Ownership

The entrypoint creates and fixes ownership for `/data` before dropping
privileges to the dedicated `froglet` user.

That keeps named Docker volumes usable without manual `chown` steps while still
running the service process itself without root.
