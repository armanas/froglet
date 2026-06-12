# Configuration Reference

Froglet is configured entirely through environment variables. All variables use
the `FROGLET_` prefix. Unset variables fall back to sensible defaults.

## Node Role

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_NODE_ROLE` | `provider` | Node role: `provider`, `runtime`, or `dual` |

## Network

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_LISTEN_ADDR` | `127.0.0.1:8080` | Provider HTTP listen address |
| `FROGLET_RUNTIME_LISTEN_ADDR` | `127.0.0.1:8081` | Runtime HTTP listen address (loopback only unless overridden) |
| `FROGLET_RUNTIME_ALLOW_NON_LOOPBACK` | `false` | Allow the runtime socket on non-loopback interfaces. **Use with caution** |
| `FROGLET_PUBLIC_BASE_URL` | *(none)* | Publicly reachable base URL advertised in the descriptor (e.g. `https://node.example.com:8080`) |
| `FROGLET_NETWORK_MODE` | `clearnet` | Transport mode: `clearnet`, `tor`, or `dual` |
| `FROGLET_HTTP_CA_CERT_PATH` | *(none)* | Path to a custom CA certificate bundle (PEM) for outbound HTTPS |

## Tor Sidecar

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_TOR_BINARY` | `tor` | Path to the Tor binary |
| `FROGLET_TOR_BACKEND_LISTEN_ADDR` | `127.0.0.1:8082` | Tor backend listener (must be loopback) |
| `FROGLET_TOR_STARTUP_TIMEOUT_SECS` | `90` | Seconds to wait for Tor to bootstrap (5-300) |

## Identity

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_IDENTITY_AUTO_GENERATE` | `true` | Auto-generate a secp256k1 keypair on first run |

## Pricing

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PRICE_EVENTS_QUERY` | `0` | Price in sats per events query (0 = free) |
| `FROGLET_PRICE_EXEC_WASM` | `0` | Price in sats per WASM execution (0 = free) |

The current public Stripe and x402 runtime adapters reuse that configured
numeric price directly on the local `/v1/node/*` flow. They do not perform FX
conversion from sats into backend-native fiat or token units.

## Payment & Lightning

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PAYMENT_BACKEND` | `none` | Payment backends (comma-separated): `none`, `lightning`, `x402`, `stripe`. Example: `lightning,x402`. Auto-set to `lightning` when any price > 0 |
| `FROGLET_LIGHTNING_MODE` | `mock` | Lightning mode: `mock`, `lnd_rest` (hold-invoice escrow), or `phoenixd` (self-custodial prepaid). Required when payment backend is `lightning` |
| `FROGLET_LIGHTNING_PHOENIXD_URL` | *(none)* | phoenixd HTTP API URL (default `http://127.0.0.1:9740`). Required when mode is `phoenixd` |
| `FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD` | *(none)* | phoenixd `http-password` (HTTP Basic auth; from `~/.phoenix/phoenix.conf`). Required when mode is `phoenixd` |
| `FROGLET_LIGHTNING_PHOENIXD_REQUEST_TIMEOUT_SECS` | `15` | HTTP request timeout for phoenixd calls (1-60) |
| `FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM` | *(none)* | Set to `1` to allow a non-loopback `phoenixd` URL (a real-funds node); loopback URLs do not require it |
| `FROGLET_LIGHTNING_BUYER_PHOENIXD_URL` | *(none)* | Buyer-side phoenixd URL used to pay prepaid invoices when this node buys services |
| `FROGLET_LIGHTNING_BUYER_PHOENIXD_HTTP_PASSWORD` | *(none)* | Buyer-side phoenixd `http-password`. Required when `FROGLET_LIGHTNING_BUYER_PHOENIXD_URL` is set |
| `FROGLET_LIGHTNING_REST_URL` | *(none)* | LND REST API URL. Required when mode is `lnd_rest` |
| `FROGLET_LIGHTNING_TLS_CERT_PATH` | *(none)* | Path to the LND TLS certificate. Required for `https://` REST URLs |
| `FROGLET_LIGHTNING_TLS_CERT_B64` | *(none)* | Docker-only convenience input. Base64 PEM decoded by `docker-entrypoint.sh` into `FROGLET_LIGHTNING_TLS_CERT_PATH` when the path is unset |
| `FROGLET_LIGHTNING_MACAROON_PATH` | *(none)* | Path to the LND macaroon file. Required when mode is `lnd_rest` |
| `FROGLET_LIGHTNING_MACAROON_B64` | *(none)* | Docker-only convenience input. Base64 raw macaroon decoded by `docker-entrypoint.sh` into `FROGLET_LIGHTNING_MACAROON_PATH` when the path is unset |
| `FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS` | `5` | HTTP request timeout for LND REST calls (1-30) |
| `FROGLET_LIGHTNING_DESTINATION_IDENTITY` | *(none)* | Override Lightning destination node identity |
| `FROGLET_LIGHTNING_BASE_INVOICE_EXPIRY_SECS` | `300` | Base invoice expiry (60-3600) |
| `FROGLET_LIGHTNING_SUCCESS_HOLD_EXPIRY_SECS` | `300` | Success hold invoice expiry (60-3600) |
| `FROGLET_LIGHTNING_MIN_FINAL_CLTV_EXPIRY` | `18` | Minimum CLTV delta for invoices (1-144) |
| `FROGLET_LIGHTNING_SYNC_INTERVAL_MS` | `1000` | Settlement sync polling interval (100-60000) |

## x402 (USDC on Base)

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_X402_FACILITATOR_URL` | `https://api.cdp.coinbase.com/platform/v2/x402` | x402 facilitator endpoint for verify/settle |
| `FROGLET_X402_WALLET_ADDRESS` | *(required)* | Your Base wallet address to receive USDC payments |
| `FROGLET_X402_NETWORK` | `base` | Chain network identifier (`base` only in the current public implementation) |

## Stripe MPP

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_STRIPE_SECRET_KEY` | *(required)* | Stripe test secret API key for the public local helper (must use a Stripe test key, not a live key) |
| `FROGLET_STRIPE_API_VERSION` | `2026-03-04.preview` | Stripe API version (required for MPP features) |
| `FROGLET_STRIPE_WEBHOOK_SECRET` | *(none)* | Optional Stripe webhook endpoint signing secret (`whsec_...`) for `POST /v1/webhooks/stripe` |

## Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_EXECUTION_TIMEOUT_SECS` | `10` | Maximum WASM execution wall-clock time (1-300) |
| `FROGLET_WASM_CONCURRENCY_LIMIT` | `16` | Maximum concurrent WASM executions |
| `FROGLET_WASM_MODULE_CACHE_CAPACITY` | `128` | Number of compiled WASM modules to cache |
| `FROGLET_WASM_POLICY_PATH` | *(none)* | Path to a TOML WASM policy file for host capabilities (HTTP, SQLite) |
| `FROGLET_PROCESS_CONCURRENCY` | `4` | Maximum concurrent Python/container process executions |
| `FROGLET_PROCESS_OUTPUT_MAX_BYTES` | `1048576` | Maximum captured stdout/stderr bytes per process stream |
| `FROGLET_PROCESS_MEMORY_MAX_BYTES` | `536870912` | Memory cap applied to Python rlimits and container `--memory` |
| `FROGLET_PROCESS_PIDS_LIMIT` | `128` | PID/process cap applied to Python rlimits and container `--pids-limit` |
| `FROGLET_PROCESS_CPU_LIMIT` | `1.0` | CPU limit applied to container `--cpus` |

### GPU

GPU support is opt-in and provider-local. A provider only advertises GPU
capabilities when explicitly configured; the hosted proof remains free-only and
does not prove GPU execution.

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_GPU_ENABLED` | `false` | Enable GPU capability advertisement and GPU-gated container execution |
| `FROGLET_GPU_COUNT` | `1` when enabled, otherwise `0` | Number of GPUs available to this provider |
| `FROGLET_GPU_VENDOR` | *(none)* | Optional vendor label, for example `nvidia` |
| `FROGLET_GPU_MODEL` | *(none)* | Optional model label shown in `/v1/node/capabilities` |
| `FROGLET_GPU_MEMORY_MB` | *(none)* | Optional GPU memory per provider in MB |
| `FROGLET_GPU_CONTAINER_RUNTIME` | `docker` when enabled | Container runtime expected to expose GPUs. Current execution wiring supports Docker with `--gpus all` |

Publishing a service with `capabilities: ["compute.gpu"]` fails unless
`FROGLET_GPU_ENABLED=1` is set. When GPU is enabled, the generic compute offer
advertises the provider GPU capabilities so direct container workloads can
request them. Runtime invocation grants GPU access only when the requested
capability is listed in the signed offer. Non-GPU providers, and non-container
workloads that request GPU, return clear errors instead of silently falling back
to CPU.

Verified smoke: on 2026-05-01, a self-hosted GCP `nvidia-tesla-t4` VM ran a
digest-pinned container through `POST /v1/runtime/deals` with
`requested_access: ["compute.gpu"]`. The signed quote granted `compute.gpu`,
Docker was invoked with `--gpus all`, the container observed `Tesla T4` plus
`FROGLET_GPU_CAPABILITIES=["compute.gpu"]`, and the signed receipt recorded
`deal_state: "succeeded"` with deal id
`63bcf0c2b5a9c60ca3799d8e6be910fa`. This proves the single-node Docker GPU
path, not cross-provider scheduling, marketplace routing, or production
capacity management.

Run the same proof on a GPU host with Docker NVIDIA support:

```bash
FROGLET_GPU_SMOKE_EXPECTED_GPU="Tesla T4" ./scripts/gpu_smoke.sh
```

The script writes the request, quote/deal/receipt, Docker invocation log, and
GPU probe output under its printed evidence directory.

## Confidential Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_CONFIDENTIAL_POLICY_PATH` | *(none)* | Path to a TOML confidential policy file |
| `FROGLET_CONFIDENTIAL_SESSION_TTL_SECS` | `300` | Confidential session time-to-live (30-3600) |
| `FROGLET_CONFIDENTIAL_SESSION_QUOTA_PER_IDENTITY` | `20` | Confidential session openings allowed per identity/window |

## Public Write Quotas

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_HOSTED_TRIAL_DEAL_QUOTA_PER_IDENTITY` | `10` | Hosted trial deal creations allowed per identity/window |
| `FROGLET_HOSTED_TRIAL_SESSION_QUOTA_PER_IDENTITY` | `20` | Hosted trial session creations allowed per origin identity/window |
| `FROGLET_EVENT_PUBLISH_QUOTA_PER_IDENTITY` | `60` | Public event publishes allowed per event signer/window |
| `FROGLET_QUOTE_QUOTA_PER_IDENTITY` | `60` | Provider quote creations allowed per requester/window |
| `FROGLET_HOSTED_TRIAL_DEAL_QUOTA_WINDOW_SECS` | `900` | Hosted trial quota window in seconds |
| `FROGLET_PUBLIC_WRITE_QUOTA_WINDOW_SECS` | `900` | Public event/quote/confidential quota window in seconds |

## Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_DATA_ROOT` | `./data` | Root data directory (also accepts legacy `FROGLET_DATA_DIR`) |
| `FROGLET_DB_PATH` | `<data_root>/node.db` | SQLite database path |
| `FROGLET_HOST_READABLE_CONTROL_TOKEN` | `false` | Make the provider control token readable on the host filesystem |

## Marketplace

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_MARKETPLACE_URL` | *(none)* | Marketplace URL for runtime discovery and provider self-registration. Use `https://marketplace.froglet.dev` for the default public marketplace. |

## MCP Server (integrations/mcp/froglet)

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PROFILE` | `local` | MCP profile for a local/self-hosted provider and runtime |
| `FROGLET_PROVIDER_URL` | `http://127.0.0.1:8080` | Provider base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_RUNTIME_URL` | `http://127.0.0.1:8081` | Runtime base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_PROVIDER_AUTH_TOKEN_PATH` | *(none)* | Provider auth token file for local provider actions (fallback: `FROGLET_AUTH_TOKEN_PATH`) |
| `FROGLET_RUNTIME_AUTH_TOKEN_PATH` | *(none)* | Runtime auth token file for local runtime actions (fallback: `FROGLET_AUTH_TOKEN_PATH`) |
| `FROGLET_REQUEST_TIMEOUT_MS` | `10000` | HTTP request timeout in milliseconds |
| `FROGLET_DEFAULT_SEARCH_LIMIT` | `10` | Default search result limit |
| `FROGLET_MAX_SEARCH_LIMIT` | `50` | Maximum search result limit |
| `FROGLET_EGRESS_MODE` | lenient | `strict` applies DNS-pinning and SSRF validation to operator-configured provider/runtime URLs; lenient keeps local and Docker dev URLs working |

The published MCP package is `froglet-mcp`:

```bash
npx froglet-mcp
```

Equivalent explicit local launch: `FROGLET_PROFILE=local npx froglet-mcp`.

`plan_install`, `get_install_guide`, and `plan_use_case` do not require local token files.
Provider/runtime actions require the matching URL and token-path configuration.
The no-install public hosted proof is intentionally outside the installed MCP
surface; use `https://froglet.dev/llms.txt` for that HTTP flow.
