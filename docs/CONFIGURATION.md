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
| `FROGLET_TOR_BACKEND_LISTEN_ADDR` | `127.0.0.1:8082` | Loopback backend listener fronted by the Tor hidden service and the relay tunnel |
| `FROGLET_TOR_STARTUP_TIMEOUT_SECS` | `90` | Seconds to wait for Tor to bootstrap (5-300) |
| `FROGLET_RELAY_URL` | *(none at daemon level; bootstrap plans `wss://relay.froglet.dev/v1/tunnel`)* | Relay control endpoint per [RELAY.md](RELAY.md); must be `wss://` unless loopback `ws://`. URL plus suffix reserve an identity-derived endpoint but do not open WSS without an exact durable publication grant. |
| `FROGLET_RELAY_PUBLIC_SUFFIX` | *(none at daemon level; bootstrap plans `relay.froglet.dev`)* | DNS suffix used to derive the exact public HTTPS endpoint. Configure it together with `FROGLET_RELAY_URL`; set both empty in bootstrap to opt out. |
| `FROGLET_SHARE_SITE_ORIGIN` | `https://froglet.dev/` | Optional first-party HTTPS site origin for relay publication share links, such as `https://candidate.froglet.dev/` during private qualification. Other origins, credentials, paths, ports, queries, and fragments are ignored. |

## Identity

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_IDENTITY_AUTO_GENERATE` | `true` | Auto-generate a secp256k1 keypair on first run |

The following first-boot inputs are reserved for custody and Managed
Publication adapters. Existing seed files always win, so a restart cannot
silently replace an identity:

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_IDENTITY_SEED_HEX` | *(none)* | Provider identity seed, exactly 32 bytes as lowercase hex. |
| `FROGLET_NOSTR_PUBLICATION_IDENTITY_SEED_HEX` | *(none)* | Nostr publication identity seed, exactly 32 bytes as lowercase hex. |
| `FROGLET_NOSTR_PUBLICATION_CREATED_AT_EPOCH_SECONDS` | *(none)* | Original non-negative creation time for a first-boot Nostr publication seed. Managed handoff binds this value so linked-identity signatures remain stable. |

## Pricing

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PRICE_EVENTS_QUERY` | `0` | Whole minor units per events query: sats with Lightning, USD cents with Stripe-only (0 = free) |
| `FROGLET_PRICE_EXEC_WASM` | `0` | Whole minor units per execution: sats with Lightning, USD cents with Stripe-only (0 = free) |

The Stripe and x402 runtime adapters reuse the configured numeric price in
backend-native units. They do not perform FX conversion. When Lightning and
Stripe are both configured, the default built-in offer uses Lightning; use an
explicit USD/Stripe publication for a separate fiat offer.

## Provider access, pause controls, and usage allowances

These settings are local operator policy, outside the signed Kernel artifacts.
They take effect only after deploying a build that supports them and restarting
the provider with the settings loaded.

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PROVIDER_ACCESS_MODE` | `open` | `open` preserves legacy access; `private` requires the provider-control bearer token for new work; `invite` requires an approved access token; `trial` permits public work within explicit cumulative limits; `paid` additionally requires the existing payment policy. |
| `FROGLET_PROVIDER_INVITE_HASH_FILE` | *(none)* | Private regular file (0600 on Unix), one lowercase SHA-256 access-token hash per line, at most 1000 hashes / 64 KiB. Optional legacy invitations. New invitations can be issued and revoked immediately through the authenticated API or native MCP/CLI. Raw tokens belong only with invited clients. |
| `FROGLET_PROVIDER_MIN_FREE_BYTES` | `0` | Reject new work when available filesystem space falls below this reserve. `0` disables this check. Check failures refuse admission. |
| `FROGLET_PROVIDER_MAX_DATABASE_BYTES` | *(none)* | Reject new work at this high-water size for the node database plus WAL/SHM files. In-flight writes can exceed the threshold; leave headroom for recovery. |
| `FROGLET_PROVIDER_REQUIRE_PAYMENT` | `false` | Reject free, mock-paid, and success-fee-only execution. Require a nonzero upfront fee through configured phoenixd, real LND, or live Stripe. |
| `FROGLET_FILE_MAX_TOTAL_DOWNLOADS` | *(disabled)* | Positive persistent download reservation ceiling, at most 1,000,000. All three file limits are required to enable downloads. |
| `FROGLET_FILE_MAX_TOTAL_BYTES` | *(disabled)* | Positive persistent file egress reservation ceiling. A GET reserves the entire snapshot; failed transfers are not refunded. |
| `FROGLET_FILE_MAX_STORAGE_BYTES` | *(disabled)* | Positive byte ceiling for immutable `.file` snapshot packages; publication refuses insufficient space. |
| `FROGLET_PROVIDER_MAX_TOTAL_DEALS` | *(none)* | Cumulative number of admitted deals/probes in this database. Required for every protected access mode; `0` prevents new admissions. |
| `FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS` | *(none)* | Cumulative sum of admitted maximum runtimes in milliseconds. Required for every protected access mode; `0` prevents new execution admissions. |
| `FROGLET_PROVIDER_MAX_TOTAL_QUOTES` | *(none)* | Cumulative quote requests and confidential-session openings, counted before wallet/resource work. Required for every protected access mode; `0` prevents new quotes/sessions. |

All three allowances are non-negative integers, at most `9223372036854775807`.
Admission uses a persistent, transactional ledger: concurrent requests cannot
overbook it; retries of an existing deal reuse its reservation. Failed or unpaid
reservations are not refunded. Restarting does not replenish an allowance.
Raise limits deliberately to admit additional work; do not replace the database
to reset usage. Limits apply to this database, not an entire cloud account or
independent provider instances. Already admitted work can finish after a limit
is lowered; this is a stop for **new** work, not cancellation of existing deals.

Inspect policy and counters with `GET /v1/provider/usage`, authenticated with the
provider control token. Paid-only mode hides ineligible offers from public
discovery and rejects old free quotes. Legacy direct query/execution/job routes
require signed deals when payment or an execution allowance is enabled.
Event publication requires the owner control token whenever an access policy or
cumulative allowance is enabled; a trial/invite cannot bypass admission by
writing events. Public canary
executions consume an execution reservation too.

For operation while payment selection is deferred, use `FROGLET_PAYMENT_BACKEND=none`,
`FROGLET_PROVIDER_REQUIRE_PAYMENT=false`, and `FROGLET_PROVIDER_ACCESS_MODE=private`.
Explicitly set all three cumulative allowances. Zero pauses admission; positive
values permit a finite amount of owner-authorized work. Private/invite/trial
modes refuse startup without these allowances. Configuration alone does not
alter already-running or previously deployed nodes.

Invited callers send `X-Froglet-Access-Token` only on quote/deal requests.
The native client reads it from a private file and forwards it through the local
authenticated runtime over HTTPS to a remote provider. Issue/list/revoke via
`froglet-node safeguards invite-create|invite-list|invite-revoke` or the matching
native MCP actions. Database invitations have an expiry (within 30 days), a
finite request allowance (2–10000), and immediate revocation. The provider stores
only hashes. A normal invocation uses two requests; failures count too. Scope
is the entire provider, not one service. Only operator controls issue credentials.
See [HTTP_SERVICES.md](HTTP_SERVICES.md) for the complete flow and file rules.

Legacy hash-file invitations remain supported. Restart after editing that file;
the revoke API can immediately revoke a legacy hash using a persistent tombstone.
Never put access credentials in service URLs or QR codes. The provider-control
token is for the owner only. Standard discovery and completed-deal recovery remain
available under independent bounded quotas.

The operator CLI uses `FROGLET_DAEMON_URL` and the existing provider-control token
file (or `FROGLET_PROVIDER_CONTROL_TOKEN_PATH`):

```sh
froglet-node safeguards status --json
froglet-node safeguards pause --reason "operator maintenance"
froglet-node safeguards resume
froglet-node safeguards prune-cache
```

Status reports **effective running configuration**, remaining cumulative allowances,
active deals, process slots, storage checks, and process-lifetime rate-limit
decisions. Pause is persistent and stops new work; it does not cancel admitted
work. Resume never resets usage. Cache maintenance removes only derived CSV
indexes older than 24 hours (up to 10,000 directory entries per call). It never
removes signed artifacts, original datasets, receipts, results, or accounting.
Retain those records and set finite admission allowances and storage reserves.

Public new-work, recovery, authenticated operator, and health requests use separate
bounded quota buckets. These are application controls; they cannot guarantee
availability against saturated network links or exhausted host resources.

Builtin handlers run in supervised child processes on Unix. Timeout, cancellation,
and supervisor death terminate the worker process group; input/output pipes are
bounded. Linux also applies CPU/address-space limits. macOS requires a container
or host limit for a hard memory ceiling. A handler without `worker_spec()` is
refused rather than run in-process. A custom host binary must implement the
`__builtin-worker` entrypoint using `builtin_worker::read_request()` and
`write_result()`, dispatch only trusted registered handlers, and explicitly
allowlist any worker environment (never forward the provider wallet/control keys).
Standard and native-data workers are implemented by `froglet-node`. Native data
queries reopen and validate their source in the child and reuse the disposable
on-disk CSV index. This trades process-start overhead for enforceable termination.
Integration tests in other workspaces can set `FROGLET_TEST_BUILTIN_WORKER` to an
absolute prebuilt worker binary; this override applies only to Cargo executables
under `deps/`, not running production binaries.

Use `scripts/setup-payment.sh lightning --mode phoenixd --paid-only` or
`scripts/setup-payment.sh stripe --paid-only` after securely providing the
receiving-account configuration, positive built-in prices, and all three
allowances. The script writes a mode-0600 environment snippet; load it in the
actual service environment. It also sets a zero buyer spend budget, conservative
request/concurrency limits, and a five-second execution timeout by default.
Stripe requires a live key and `FROGLET_STRIPE_LIVE_CONFIRM=fresh`; the sandbox
SPT helper cannot supply a production customer's payment credential.

Existing named publications retain their signed prices. Republish each with
appropriate upfront terms before re-enabling it. No automatic currency conversion
is performed. Verify the signed quote's currency and amount before accepting
real money. The current x402 adapter is for direct HTTP requests and is not a
payment rail for these signed deals; paid-only setup rejects it.

These are workload limits, **not a monetary cap on a cloud bill**. Hosting,
storage, ingress/proxy handling, network egress, and payment-provider overhead
can still cost money while execution is stopped. Apply infrastructure limits,
edge rate limits, storage/log retention, and a reviewed shutdown procedure
separately. Do not describe billing alerts as a guaranteed spending ceiling.

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
| `FROGLET_REQUESTER_SPEND_BUDGET_MSAT` | *(none)* | Cumulative spend budget (msat) across all paid deals this node creates as a buyer. **Required for paid deals — when unset, paid deals are refused (fail-closed)**; free deals are unaffected. Tracked in a persistent ledger; inspect with `GET /v1/runtime/spend`, archive committed spend with `POST /v1/runtime/spend/reset` |
| `FROGLET_REQUESTER_MAX_DEAL_MSAT` | *(none)* | Hard cap (msat) on any single deal's quoted total (base + success fee). Optional extra guard on top of the cumulative budget |
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
| `FROGLET_X402_FACILITATOR_URL` | *(required)* | Authenticated x402 v2 facilitator (or operator-managed authenticated proxy) endpoint for `/verify` and `/settle`. Froglet intentionally has no unauthenticated CDP default: CDP mainnet requires API authentication, while the public x402.org facilitator is testnet-only. |
| `FROGLET_X402_WALLET_ADDRESS` | *(required)* | Your Base wallet address to receive USDC payments |
| `FROGLET_X402_NETWORK` | `base` | Chain network identifier (`base` only in the current public implementation) |

## Stripe MPP

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_STRIPE_SECRET_KEY` | *(required)* | Stripe secret API key for MPP. Use `sk_test_...` by default; live keys require the explicit confirmation below. |
| `FROGLET_STRIPE_LIVE_CONFIRM` | *(none)* | Set to `fresh` only for an operator-approved provider-side live Stripe setup/proof with `sk_live_...`. The daemon enforces this for `FROGLET_STRIPE_SECRET_KEY`. The SPT test helper always refuses live keys. |
| `FROGLET_STRIPE_API_VERSION` | `2026-04-22.preview` | Stripe API version (required for MPP features) |
| `FROGLET_STRIPE_WEBHOOK_SECRET` | *(none)* | Optional Stripe webhook endpoint signing secret (`whsec_...`) for `POST /v1/webhooks/stripe` |
| `FROGLET_STRIPE_SPT_TEST_HELPER_ENABLED` | `false` | Explicitly enables Stripe's **seller-side sandbox test helper**. It is not a production buyer credential flow. |
| `FROGLET_BUYER_STRIPE_SECRET_KEY` | *(none)* | Seller sandbox `sk_test_...` key used only by the explicitly enabled SPT test helper. `sk_live_...` is always rejected. |
| `FROGLET_BUYER_STRIPE_PAYMENT_METHOD` | *(none)* | Test payment method (`pm_...`) used by the sandbox helper. Stripe's documented helper does not accept Froglet's former customer-ID shortcut. |
| `FROGLET_BUYER_STRIPE_SELLER_NETWORK_ID` | *(none)* | Required seller network scope for a sandbox SPT, for example `internal`. |
| `FROGLET_BUYER_STRIPE_SELLER_EXTERNAL_ID` | *(none)* | Optional seller, cart, or connected-account scope for a sandbox SPT. |

For production Stripe-priced deals, the requester or its authorized
agentic-commerce platform supplies
`payment: {"kind":"stripe_mpp","token":"spt_..."}`. Froglet never calls the
seller test-helper endpoint with a live key and does not require a buyer Stripe
secret for this flow. The provider's configured Stripe account is the direct
payee; Froglet does not route a marketplace payout or take a platform fee.

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
| `FROGLET_PROCESS_PIDS_LIMIT` | `128` | PID/process cap applied to container `--pids-limit`; sandboxed inline Python is fixed to one process/thread |
| `FROGLET_PROCESS_CPU_LIMIT` | `1.0` | CPU share limit applied to container `--cpus`; inline Python receives an `RLIMIT_CPU` derived from its execution deadline |

### GPU

The current reference OCI worker does not attach or account for GPU devices.
GPU execution is refused for every runtime, including OCI, and no `compute.gpu`
capabilities are advertised. `/v1/node/capabilities` reports GPU execution as
disabled. The following retained configuration fields describe operator-supplied
inventory only; setting them does not enable execution:

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_GPU_ENABLED` | `false` | Retained inventory configuration flag |
| `FROGLET_GPU_COUNT` | `1` when configured, otherwise `0` | Declared GPU count |
| `FROGLET_GPU_VENDOR` | *(none)* | Optional vendor label |
| `FROGLET_GPU_MODEL` | *(none)* | Optional model label |
| `FROGLET_GPU_MEMORY_MB` | *(none)* | Declared GPU memory in MB |
| `FROGLET_GPU_CONTAINER_RUNTIME` | `docker` when configured | Declared runtime |

The earlier 2026-05-01 Docker/GCP T4 smoke described a previous execution path.
It does not qualify the current out-of-process OCI worker. GPU support requires
worker device attachment, resource accounting and a new hardware smoke before
capability advertisement can be restored. `scripts/gpu_smoke.sh` remains a
qualification tool, not evidence that this build supports GPU execution.

## Confidential Execution

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_CONFIDENTIAL_POLICY_PATH` | *(none)* | Reserved confidential-policy path. This build rejects it because only mock attestation/key-release reference providers exist |
| `FROGLET_CONFIDENTIAL_SESSION_TTL_SECS` | `300` | Confidential session time-to-live (30-3600) |
| `FROGLET_CONFIDENTIAL_SESSION_QUOTA_PER_IDENTITY` | `20` | Confidential session openings allowed per identity/window |

## Public Write Quotas

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_PUBLIC_REQUEST_QUOTA` | `6000` | Global requests across all routes on the public listener per public-write window (1-100000). Shared across origins, so changing IP addresses does not bypass it. In-memory: resets on process restart. |
| `FROGLET_HOSTED_TRIAL_DEAL_QUOTA_PER_IDENTITY` | `10` | Hosted trial deal creations allowed per request-origin identity/window |
| `FROGLET_HOSTED_TRIAL_SESSION_QUOTA_PER_IDENTITY` | `20` | Hosted trial session creations allowed per origin identity/window |
| `FROGLET_EVENT_PUBLISH_QUOTA_PER_IDENTITY` | `60` | Public event publishes allowed per request-origin identity/window |
| `FROGLET_QUOTE_QUOTA_PER_IDENTITY` | `60` | Provider quote creations allowed per request-origin identity/window |
| `FROGLET_TRUST_FORWARD_PUBLIC_QUOTA_HEADERS` | `false` | Trust normalized `Forwarded`/`X-Forwarded-For`/`X-Real-IP`/`CF-Connecting-IP` values for public quota identity. Leave false unless a trusted proxy strips client-supplied copies first. |
| `FROGLET_HOSTED_TRIAL_DEAL_QUOTA_WINDOW_SECS` | `900` | Hosted trial quota window in seconds |
| `FROGLET_PUBLIC_WRITE_QUOTA_WINDOW_SECS` | `900` | Public event/quote/confidential quota window in seconds |
| `FROGLET_HOSTED_TRIAL_ALLOWED_SERVICE_IDS` | `demo.add,demo.echo,demo.fetch-witness,demo.hash-verify,demo.notarize` | Comma-separated free local service IDs accepted by hosted-trial deal creation |

Public write quotas use a request-origin identity, not body fields such as
`requester_id` or event public keys. Keep
`FROGLET_TRUST_FORWARD_PUBLIC_QUOTA_HEADERS=false` unless a trusted proxy strips
client-supplied forwarding headers before forwarding to the node.
Identity-quota maps hold at most 10,000 active identities; new identities are
refused when full until expired buckets can be reclaimed. The global request
limit returns HTTP 429, including on public status/payment routes; size it for
legitimate polling and apply edge abuse controls before traffic reaches the VM.

## Storage

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_DATA_ROOT` | `./data` | Root data directory (also accepts legacy `FROGLET_DATA_DIR`) |
| `FROGLET_DB_PATH` | `<data_root>/node.db` | SQLite database path |
| `FROGLET_HOST_READABLE_CONTROL_TOKEN` | `false` | Make the provider control token readable on the host filesystem |
| `FROGLET_PROVIDER_ARTIFACT_ROOT` | *(none)* | Directory root for provider-control `artifact_path` publication. When unset, daemon-local `artifact_path` inputs are rejected; prefer inline source/module bytes for agent-driven publication. |

`FROGLET_PROVIDER_ARTIFACT_ROOT` is required only for trusted operator workflows
that intentionally publish daemon-local files. Agent-driven publication should
send `inline_source` or `wasm_module_hex` instead of absolute host paths.

## Managed Publication

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_MANAGED_TARGETS_FILE` | *(none)* | Absolute path to the bounded private target/profile registry used by `hosting.default = "managed"`. |
| `FROGLET_MANAGED_BUNDLE_PATH` | *(none)* | Managed-runner-only path to the canonical runtime bundle. Must be paired with `FROGLET_MANAGED_CAPSULE_BASE64`. |
| `FROGLET_MANAGED_CAPSULE_BASE64` | *(none)* | Managed-runner-only, provider-private exact plan/Descriptor/Offer/Revision/fixture capsule. Startup fails closed when only one bundle input is present. |

The target registry is operator-private JSON. Service manifests select only a
neutral `target` and `profile`; cloud account IDs, hostnames, credentials, DNS
providers, and adapter configuration do not enter the manifest or Kernel. Each
profile binds:

- an absolute `froglet-operator` binary and adapter-config path;
- exact base-runner manifest/config bytes and release digest;
- one private OCI output repository plus anonymous, Basic, or Bearer registry
  authentication sourced from environment variables;
- the exact HTTPS public origin and a provider-neutral deployment template;
- logical secrets for the capsule, Provider seed, and Nostr publication seed.

A minimal SSH + OCI profile has this shape (digests and absolute paths are
placeholders, and the separately private operator adapter config owns the SSH
host, key, remote root, edge image, and logical-secret resolver):

```json
{
  "schema_version": "froglet.managed-target-registry.v1",
  "targets": {
    "independent-host": {
      "small": {
        "operator_binary": "/opt/froglet/bin/froglet-operator",
        "adapter": "ssh-oci",
        "adapter_config_path": "/etc/froglet/operator/ssh-small.json",
        "output_repository": "registry.example/private/services",
        "base_runner_image": {
          "repository": "registry.example/private/froglet-runner",
          "digest": "sha256:<64 lowercase hex>"
        },
        "release_bundle_digest": "sha256:<64 lowercase hex>",
        "base_manifest_path": "/var/lib/froglet/releases/runner-manifest.json",
        "base_config_path": "/var/lib/froglet/releases/runner-config.json",
        "public_url": "https://service.example",
        "provision": false,
        "deployment": {
          "deployment_id": "froglet-service",
          "environment": {
            "FROGLET_MANAGED_BUNDLE_PATH": "/opt/froglet/managed/bundle.json",
            "FROGLET_PUBLIC_BASE_URL": "https://service.example"
          },
          "secrets": [
            {
              "environment_name": "FROGLET_MANAGED_CAPSULE_BASE64",
              "reference": "secret://froglet/service/capsule"
            },
            {
              "environment_name": "FROGLET_IDENTITY_SEED_HEX",
              "reference": "secret://froglet/provider/identity"
            },
            {
              "environment_name": "FROGLET_NOSTR_PUBLICATION_IDENTITY_SEED_HEX",
              "reference": "secret://froglet/provider/nostr-publication-identity"
            }
          ],
          "resources": {
            "cpu_millis": 500,
            "memory_bytes": 536870912,
            "architecture": "amd64"
          },
          "ports": [
            { "name": "froglet", "container_port": 8080, "protocol": "tcp" }
          ],
          "persistent_volumes": [],
          "health_check": {
            "port_name": "froglet",
            "path": "/healthz",
            "interval_seconds": 30,
            "timeout_seconds": 5
          },
          "ingress": {
            "port_name": "froglet",
            "transport": "https",
            "requested_hostname": "service.example"
          },
          "observability": { "structured_logs": true },
          "lifecycle": { "rollback_required": true }
        },
        "capsule_source_environment": "FROGLET_MANAGED_CAPSULE_SOURCE",
        "registry_auth": {
          "kind": "bearer",
          "token_environment": "FROGLET_REGISTRY_TOKEN"
        },
        "registry_insecure_http": false
      }
    }
  }
}
```

The deployment template must set
`FROGLET_MANAGED_BUNDLE_PATH=/opt/froglet/managed/bundle.json` and
`FROGLET_PUBLIC_BASE_URL` to the profile's exact `public_url`. Froglet inserts
the Nostr identity creation time into the approved desired state. Secret values
never enter the plan, command line, or durable operation record. Base layers
and the output image must use one registry in schema v1 so the Distribution API
can mount the digest-pinned layer without downloading it through the authoring
node.

Planning is read-only. Approval binds the deterministic private OCI manifest,
operator provision/deploy/compensation plans, endpoint, Provider identity,
recurring-cost disclosure, and target/profile. Activation durably records each
external phase, uploads through the registry-neutral OCI Distribution API,
passes the desired state over bounded stdin to the operator, imports the exact
paired identity and signed artifact chain, and requires an independently
verified remote canary before marketplace registration. Interrupted external
phases require status-based reconciliation; they are never blindly replayed.

## Marketplace

| Variable | Default | Description |
|----------|---------|-------------|
| `FROGLET_MARKETPLACE_URL` | *(none)* | Marketplace URL for runtime discovery and provider self-registration. Use `https://marketplace.froglet.dev` for the default public marketplace. |
| `FROGLET_MARKETPLACE_ALLOW_LOCAL` | `false` | Allow local/private marketplace URLs for explicit dev/test use. Production egress requires public HTTPS or approved onion transport. |

When `FROGLET_MARKETPLACE_ALLOW_LOCAL=false`, marketplace egress rejects
loopback, private-network, link-local, metadata, `.local`, `.internal`, and
plain public HTTP endpoints.

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
A complete install plan reads public GitHub release metadata and
the tag-specific bootstrap bytes so it can bind their hashes; it does not write
to the host. `get_install_guide` repeats those reads before accepting the
approval hash.
Provider/runtime actions require the matching URL and token-path configuration.
The no-install public hosted proof is intentionally outside the installed MCP
surface; use `https://froglet.dev/llms.txt` for that HTTP flow.

### Download-only file publications

The `froglet.builtin.file_download.v1` contract shares one immutable regular file,
up to 8 MiB. It is disabled until all three `FROGLET_FILE_*` ceilings above are
configured. Existing provider access, invitation, pause, storage and relay limits
still apply. Files require zero-fee terms and settlement `none`; payments are not
implemented for this path. Transfer counts and bytes survive restarts in the node
SQLite database. These limits are not a cloud hosting cost cap.

Prepare a private project using `froglet-node prepare-service --request FILE`.
The request contains absolute `source` and `destination` paths, `service_id`, a
public `summary`, and a `file` object with `filename`, optional `media_type`,
`expires_at` (Unix seconds, within 30 days), `max_downloads`, and
`max_transfer_bytes`. For example, the following fields select download mode
instead of parsing the source as a dataset:

```json
{
  "source": "/absolute/path/report.pdf",
  "destination": "/absolute/path/share-project",
  "service_id": "report-download",
  "summary": "Download the approved report",
  "file": {
    "filename": "report.pdf",
    "media_type": "application/pdf",
    "expires_at": 1790611200,
    "max_downloads": 10,
    "max_transfer_bytes": 83886080
  }
}
```

Choose a fresh expiry; the timestamp above is illustrative. Preparation makes a
private snapshot and manifest, then the existing build/plan/consent/publish flow
applies. Review the filename, summary, byte count, fingerprint and expiry as
public metadata. Source paths stay private. Edits to the source do not update
published bytes. Symlinks and special files are rejected; preparation also caps
retained file snapshots at 64 MiB per project.

The share page downloads only after a click and verifies size and SHA-256 before
offering **Save verified file**. Invitation tokens are entered separately and are
not placed in URLs or persistent browser storage. The native equivalent is:

```sh
froglet-node download --service-url SHARE_URL --destination /absolute/new/report.pdf --json
```

Add `--access-token-file /absolute/private/invitation.token` for an invited share.
The native command verifies signed metadata, refuses redirects and non-public
network addresses, checks bytes, and creates a new private file without
replacing an existing destination. First-party links use HTTPS relays; this
command does not provide a Tor-only route or anonymous transport.

`froglet-node safeguards status --json` includes file transfer reservations.
Pause/unpublish/revoke/expiry prevent new admission. An admitted transfer may
finish within its reserved bytes and deadline. `froglet-node safeguards
abort-files` stops active node responses; bytes already buffered by the relay or
received by a recipient cannot be recalled. Neither command resets allowances.
F1 is buffered and does not support ranges, resume, uploads or folder browsing.
