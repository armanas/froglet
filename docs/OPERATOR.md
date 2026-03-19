# Operator Guide

Froglet now has one supported runtime shape:

- local `froglet-runtime`
- remote `froglet-provider`
- remote `froglet-discovery`

## Local Development

Use the local three-role stack:

```bash
docker compose up --build
```

Important paths and ports:

- provider: `http://127.0.0.1:8080`
- runtime: `http://127.0.0.1:8081`
- discovery: `http://127.0.0.1:9090`
- runtime token: `./data/runtime/auth.token`

## Wallet Modes

- `FROGLET_PAYMENT_BACKEND=none`
- `FROGLET_PAYMENT_BACKEND=lightning` with `FROGLET_LIGHTNING_MODE=mock`
- `FROGLET_PAYMENT_BACKEND=lightning` with `FROGLET_LIGHTNING_MODE=lnd_rest`

Use `GET /v1/runtime/wallet/balance` to confirm runtime wallet visibility.

## Runtime Auth

Privileged runtime calls require the bearer token at `./data/runtime/auth.token`.

The token belongs only to the local runtime surface. Do not send it to provider or discovery services.

## Provider Publication

Provider publication to discovery is provider configuration:

- `FROGLET_DISCOVERY_MODE=reference`
- `FROGLET_DISCOVERY_URL=...`
- `FROGLET_DISCOVERY_PUBLISH=true`

Bots do not trigger provider publication.

## Archive Export

Use:

- `GET /v1/runtime/archive/deal/:deal_id`
- `GET /v1/runtime/archive/job/:job_id`

Archive export is the main retained evidence surface for operators.
