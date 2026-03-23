# Froglet

[![CI](https://github.com/armanas/froglet/actions/workflows/ci.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/ci.yml)
[![Release](https://github.com/armanas/froglet/actions/workflows/release.yml/badge.svg)](https://github.com/armanas/froglet/actions/workflows/release.yml)

Froglet is a signed-deal execution system with one supported topology:

- `froglet-runtime`: local requester runtime for bots
- `froglet-provider`: remote execution provider
- `froglet-discovery`: remote reference discovery

Bots talk only to the local runtime. The runtime discovers providers, requests quotes, signs requester deals, submits them to remote providers, tracks requester-side state, exposes payment intent, and accepts results.

## Binaries

| Binary | Purpose |
|---|---|
| `froglet-runtime` | Local bot-facing runtime on loopback |
| `froglet-provider` | Public provider API |
| `froglet-discovery` | Public reference discovery |

## Quick Start

Start discovery:

```bash
cargo run --bin froglet-discovery
```

Start a provider and publish it to discovery:

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_DISCOVERY_PUBLISH=true \
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-provider
```

Start a local runtime for bots:

```bash
FROGLET_DISCOVERY_MODE=reference \
FROGLET_DISCOVERY_URL=http://127.0.0.1:9090 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet-runtime
```

## Docker

The default Compose file is a local three-role stack:

```bash
docker compose up --build
```

That publishes:

- discovery on `http://127.0.0.1:9090`
- provider on `http://127.0.0.1:8080`
- runtime on `http://127.0.0.1:8081`
- runtime token on `./data/runtime/auth.token`

Role-specific Compose files are also included:

- `compose.discovery.yaml`
- `compose.provider.yaml`
- `compose.runtime.yaml`

See [docs/DOCKER.md](docs/DOCKER.md).

## Bot Surface

Supported runtime routes:

- `GET /v1/runtime/wallet/balance`
- `POST /v1/runtime/search`
- `GET /v1/runtime/providers/:provider_id`
- `POST /v1/runtime/deals`
- `GET /v1/runtime/deals/:deal_id`
- `GET /v1/runtime/deals/:deal_id/payment-intent`
- `POST /v1/runtime/deals/:deal_id/accept`
- `GET /v1/runtime/archive/:subject_kind/:subject_id`

Supported provider routes:

- `GET /v1/provider/descriptor`
- `GET /v1/provider/offers`
- `POST /v1/provider/quotes`
- `POST /v1/provider/deals`
- `GET /v1/provider/deals/:deal_id`
- `POST /v1/provider/deals/:deal_id/accept`
- `GET /v1/provider/deals/:deal_id/invoice-bundle`
- `GET /v1/provider/confidential/profiles/:artifact_hash`
- `POST /v1/provider/confidential/sessions`
- `GET /v1/provider/confidential/sessions/:session_id`

Supported discovery routes:

- `POST /v1/discovery/search`
- `GET /v1/discovery/providers/:provider_id`

Verification routes remain public:

- `POST /v1/invoice-bundles/verify`
- `POST /v1/curated-lists/verify`
- `POST /v1/nostr/events/verify`
- `POST /v1/receipts/verify`

## OpenClaw and NemoClaw

The public Froglet OpenClaw plugin is runtime-only and shared by OpenClaw and
NemoClaw. The plugin contract is the same in both products. Profile-specific
differences are limited to plugin load path, runtime URL, runtime token path,
and non-Froglet top-level config such as model/provider settings.

Supported profiles:

| Profile | Runtime placement | Notes |
| --- | --- | --- |
| `openclaw-local` | local host runtime | baseline local OpenClaw workflow |
| `nemoclaw-local-runtime` | runtime inside the sandbox | compatibility path when the sandbox-local runtime is intentional |
| `nemoclaw-hosted-runtime` | runtime on the consumer host over HTTPS | supported NemoClaw baseline |

The checked-in example JSON files under [`integrations/openclaw/froglet/examples`](/Users/armanas/Projects/github.com/armanas/froglet/integrations/openclaw/froglet/examples) are complete user-edited configs, not rendered fragments.

The Froglet-owned plugin keys are:

- `runtimeUrl`
- `runtimeAuthTokenPath`

Tool surface:

- `froglet_search`
- `froglet_get_provider`
- `froglet_events_query`
- `froglet_buy`
- `froglet_mock_pay`
- `froglet_wait_deal`
- `froglet_payment_intent`
- `froglet_accept_result`
- `froglet_wallet_balance`

Minimal `froglet_buy` request for the standard `execute.wasm` path:

```json
{
  "request": {
    "provider": { "provider_id": "provider-1" },
    "offer_id": "execute.wasm",
    "submission": { "wasm_module_hex": "<valid_wasm_module_hex>" }
  }
}
```

For accept flows, `froglet_wait_deal` must be called with `wait_statuses` including `result_ready`; the default wait behavior only stops on terminal statuses.

OpenClaw setup is documented in [docs/OPENCLAW.md](docs/OPENCLAW.md). NemoClaw setup is documented in [docs/NEMOCLAW.md](docs/NEMOCLAW.md).

## Confidential Execution

Confidential execution is an additive provider/runtime extension. It does not change the requester-runtime topology above. See [docs/CONFIDENTIAL.md](docs/CONFIDENTIAL.md).

## More Docs

- [docs/BOT_RUNTIME_ALPHA.md](docs/BOT_RUNTIME_ALPHA.md)
- [docs/RUNTIME.md](docs/RUNTIME.md)
- [docs/DOCKER.md](docs/DOCKER.md)
- [docs/OPENCLAW.md](docs/OPENCLAW.md)
- [docs/NEMOCLAW.md](docs/NEMOCLAW.md)
- [examples/README.md](examples/README.md)
