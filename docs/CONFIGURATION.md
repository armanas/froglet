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
| `FROGLET_LIGHTNING_MODE` | `mock` | Lightning mode: `mock` or `lnd_rest`. Required when payment backend is `lightning` |
| `FROGLET_LIGHTNING_REST_URL` | *(none)* | LND REST API URL. Required when mode is `lnd_rest` |
| `FROGLET_LIGHTNING_TLS_CERT_PATH` | *(none)* | Path to the LND TLS certificate. Required for `https://` REST URLs |
| `FROGLET_LIGHTNING_MACAROON_PATH` | *(none)* | Path to the LND macaroon file. Required when mode is `lnd_rest` |
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
| `FROGLET_STRIPE_SECRET_KEY` | *(required)* | Stripe test secret API key for the public local helper (must start with `sk_test_`) |
| `FROGLET_STRIPE_API_VERSION` | `2026-03-04.preview` | Stripe API version (required for MPP features) |

## Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_EXECUTION_TIMEOUT_SECS` | `10` | Maximum WASM execution wall-clock time (1-300) |
| `FROGLET_WASM_CONCURRENCY_LIMIT` | `16` | Maximum concurrent WASM executions |
| `FROGLET_WASM_MODULE_CACHE_CAPACITY` | `128` | Number of compiled WASM modules to cache |
| `FROGLET_WASM_POLICY_PATH` | *(none)* | Path to a TOML WASM policy file for host capabilities (HTTP, SQLite) |

## Confidential Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_CONFIDENTIAL_POLICY_PATH` | *(none)* | Path to a TOML confidential policy file |
| `FROGLET_CONFIDENTIAL_SESSION_TTL_SECS` | `300` | Confidential session time-to-live (30-3600) |

## Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_DATA_ROOT` | `./data` | Root data directory (also accepts legacy `FROGLET_DATA_DIR`) |
| `FROGLET_DB_PATH` | `<data_root>/node.db` | SQLite database path |
| `FROGLET_HOST_READABLE_CONTROL_TOKEN` | `false` | Make the provider control token readable on the host filesystem |

## Marketplace

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_MARKETPLACE_URL` | *(none)* | Marketplace URL for provider auto-registration and runtime discovery. Use the default public marketplace or any compatible marketplace endpoint. |

## MCP Server (integrations/mcp/froglet)

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PROVIDER_URL` | *(required)* | Provider base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_RUNTIME_URL` | *(required)* | Runtime base URL (fallback: `FROGLET_BASE_URL`) |
| `FROGLET_PROVIDER_AUTH_TOKEN_PATH` | *(none)* | Provider auth token file (fallback: `FROGLET_AUTH_TOKEN_PATH`) |
| `FROGLET_RUNTIME_AUTH_TOKEN_PATH` | *(none)* | Runtime auth token file (fallback: `FROGLET_AUTH_TOKEN_PATH`) |
| `FROGLET_REQUEST_TIMEOUT_MS` | `10000` | HTTP request timeout in milliseconds |
| `FROGLET_DEFAULT_SEARCH_LIMIT` | `20` | Default search result limit |
| `FROGLET_MAX_SEARCH_LIMIT` | `100` | Maximum search result limit |
