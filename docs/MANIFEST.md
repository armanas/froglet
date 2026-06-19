# Froglet Manifests

Status: normative spec for `froglet.toml` (project-level) and `froglet-service.toml` v3 (per-service).

This document defines the two manifest files that make a Froglet service
authorable and publishable in one command (`froglet-node publish`) or one MCP
call (`marketplace_publish`). The manifest is the durable contract between
the author and the publish engine: change the file, re-publish, the new offer
is signed and registered.

A v2 sample lives at `data/projects/test_project_1/froglet-service.toml`. v3
is a superset of v2 — every v2 manifest loads in v3 with deprecation warnings
on missing v3 sections.

## File layout

```
my-project/
├── froglet.toml                 # project-level (defaults, identity, marketplace)
├── services/
│   ├── translator/
│   │   ├── froglet-service.toml # per-service overrides
│   │   └── handler.py
│   └── echo/
│       ├── froglet-service.toml
│       └── handler.py
```

A project can hold one service or many; the layout is unenforced. The publish
engine resolves the project manifest by walking upward from a service
manifest's directory until it finds `froglet.toml`, or falls back to defaults
if none exists.

---

## `froglet.toml` — project-level

Optional. Holds defaults inherited by every service in the project, plus the
project's signing identity strategy. If absent, every service manifest must
specify its own values (or accept engine defaults).

```toml
schema_version = "froglet/v1"

[project]
name = "my-project"
description = "Multi-service Froglet project for translation + summarization"

[project.identity]
# How the publish engine resolves the signing key. Three modes:
#   "auto"          — auto-generate at FROGLET_DATA_DIR on first publish
#   "env:NAME"      — read 64-hex seed from environment variable NAME
#   "file:PATH"     — read 64-hex seed from a file
strategy = "auto"

[project.marketplace]
# Default marketplace URL. Per-service [marketplace] overrides this.
url = "https://marketplace.froglet.dev"

[project.defaults]
# Inherited by every service in this project unless the service overrides.
runtime = "python"
hosting = "tor"
settlement = "none"
```

### Required fields

- `schema_version` — must equal `"froglet/v1"` exactly
- `project.name` — non-empty string, lowercase + digits + hyphens, ≤63 chars

### Optional with defaults

| Field | Default | Notes |
|---|---|---|
| `project.description` | `""` | |
| `project.identity.strategy` | `"auto"` | |
| `project.marketplace.url` | `"https://marketplace.froglet.dev"` | |
| `project.defaults.runtime` | `"python"` | |
| `project.defaults.hosting` | `"local"` | Local is private; choose `"tor"` for the easiest public path |
| `project.defaults.settlement` | `"none"` | |

---

## `froglet-service.toml` v3 — per-service

Required for every service. Defines the executable artifact, hosting choice,
settlement, and limits.

```toml
schema_version = "froglet-service/v3"

# Identity
project_id = "my-project"           # must match enclosing froglet.toml
service_id = "translator-en-es"     # unique within project
offer_id = "translator-en-es"       # defaults to service_id
summary = "Translate EN→ES via Claude"

# Runtime / packaging
runtime = "python"                  # python | wasm | container | builtin
package_kind = "inline_source"      # inline_source | inline_module | oci_image
entrypoint_kind = "handler"         # handler (python default) | script | module
entrypoint = "handler.py"
contract_version = "froglet.python.handler_json.v1"  # defaulted per runtime
mode = "sync"                       # sync | async
source_kind = "python"              # informational
publication_state = "active"        # active | hidden

# Hosting (NEW in v3)
[hosting]
default = "tor"                     # local | tor | self | managed | fly

[hosting.local]
# no config

[hosting.tor]
# auto-spawn local tor hidden service; no config

[hosting.self]
url = "https://my-existing-app.fly.dev"

# 1B (deferred):
# [hosting.managed]
# slug = "translator"
# [hosting.fly]
# app = "my-translator"
# region = "iad"

# Settlement (NEW in v3)
[settlement]
method = "none"                     # "none" | "lightning" | "stripe"

# Marketplace override (NEW in v3, optional)
[marketplace]
url = "https://marketplace.froglet.dev"

# Execution limits (NEW in v3)
[limits]
max_input_bytes = 16384
max_runtime_ms = 5000
max_memory_bytes = 16777216
max_output_bytes = 16384
fuel_limit = 0

# Pricing
[price]
sats = 0                            # 0 for free; ignored if settlement.method = "none"
# currency = "sat"                  # "sat" (default) = satoshis via Lightning
#                                   # "usd"           = US cents via Stripe
#                                   # currency="usd" requires a Stripe payment backend

# Optional I/O schemas (JSON Schema, free shape)
[input_schema]
# example: type = "object", required = ["text"], ...

[output_schema]
# example: type = "object", required = ["translated"], ...
```

### Required fields

- `schema_version` — `"froglet-service/v3"` exactly (or `"froglet-service/v2"` for legacy)
- `service_id` — non-empty, lowercase + digits + hyphens, ≤63 chars
- `runtime` — one of `python` | `wasm` | `container` | `builtin`
- `package_kind` — one of `inline_source` | `inline_module` | `oci_image` | `builtin`
- `[hosting] default` — one of `local` | `tor` | `self` (v1A) | `managed` | `fly` (v1B)
- `[settlement] method` — one of `"none"` | `"lightning"` | `"stripe"`

### Conditional requirements

| Runtime | Package kind | Required additional |
|---|---|---|
| `python` | `inline_source` | `entrypoint` (relative path to source file) |
| `wasm` | `inline_module` | `entrypoint` (relative path to `.wasm` artifact) OR `module_bytes_hex` |
| `wasm` | `oci_image` | `oci.reference` AND `oci.digest` |
| `python` | `oci_image` | `oci.reference` AND `oci.digest` |
| `container` | `oci_image` | `oci.reference` AND `oci.digest` |
| `builtin` | `builtin` | **rejected at publish time** — builtins are reserved |

### Hosting-backend-specific fields

| Backend | Required | Notes |
|---|---|---|
| `local` | — | Private dev only; not registered with marketplace |
| `tor` | — | Engine auto-spawns `tor` daemon if available |
| `self` | `hosting.self.url` | Engine validates URL is reachable + serves `/v1/feed` |
| `managed` (1B) | — | Engine signs claim, requires public IP |
| `fly` (1B) | `hosting.fly.app`, `hosting.fly.region` | Engine wraps `flyctl deploy` |

### Default values supplied by engine

When omitted from the manifest:

| Field | Default |
|---|---|
| `offer_id` | `service_id` |
| `summary` | `"Froglet service {service_id}"` |
| `entrypoint_kind` | runtime-derived (`handler` for python, `module` for wasm, `image` for container) |
| `contract_version` | `"froglet.{runtime}.{package_kind}.v1"` form |
| `mode` | `"sync"` |
| `source_kind` | inferred from runtime + package_kind |
| `publication_state` | `"active"` |
| `limits.*` | provider state defaults (currently 64 KiB input, 30 s runtime, 16 MiB memory) |
| `price.sats` | `0` |
| `price.currency` | `"sat"` — satoshis, Lightning rail. Use `"usd"` for US cents on the Stripe rail (requires Stripe payment backend). |
| `marketplace.url` | inherited from `froglet.toml`, else `"https://marketplace.froglet.dev"` |

---

## Inheritance rules

Project-level fields cascade into the service manifest. The order of
resolution per field:

1. Service manifest explicit value (wins)
2. Project manifest `[project.defaults]` value
3. Engine built-in default

`project_id` in the service manifest must match `project.name` in the project
manifest when both are present. Mismatch is a validation error.

---

## Validation rules

The parser's `validate()` method enforces:

- **Schema version**: `"froglet/v1"` for project, `"froglet-service/v3"` for service.
  `"froglet-service/v2"` is accepted with a `Deprecated` warning per missing
  v3 section.
- **Identifier shape**: `name`, `project_id`, `service_id`, `offer_id` are
  lowercase ASCII + digits + interior hyphens, 1-63 chars, no leading/
  trailing hyphen.
- **Runtime + package_kind combos**: per the table above. Invalid combos
  rejected with a message naming the combo and the allowed alternatives.
- **Hosting backend gating**: `local` is rejected if `marketplace.url` is set
  (local services are private; marketplace registration would fail).
- **Settlement allowlist**: `[settlement] method` must be one of `"none"`,
  `"lightning"`, or `"stripe"`; any other value is rejected. `"stripe"`
  additionally requires `price.currency = "usd"`; `"lightning"` requires
  `price.currency = "sat"` or absent.
- **Limits sanity**: every limit must be `> 0`. `fuel_limit` may be `0` to
  mean "unlimited within max_runtime_ms".
- **Entrypoint reachability**: relative paths in `entrypoint` are resolved
  against the service manifest's directory; the path must exist when the
  publish engine is invoked.
- **No unknown fields**: extra top-level keys are rejected to catch typos
  early. Nested unknown keys inside `[input_schema]` / `[output_schema]` are
  preserved as-is.

---

## v2 → v3 migration

A v2 manifest is implicitly equivalent to a v3 manifest with:

- `[hosting] default = "local"` (private)
- `[settlement] method = "none"` (free)
- `[limits]` from engine defaults
- `[marketplace] url` from project default or built-in default

The parser emits one `Deprecation` warning per missing section, suggesting
the explicit v3 form. v2 is supported for one minor version cycle (through
v0.3); v0.4 drops v2 support.

---

## Reuse contract with `ProviderManagedOfferDefinition`

Every manifest field name mirrors the canonical signed-offer field name in
`src/api/types.rs::ProviderManagedOfferDefinition`. The publish engine does
not translate field names; it maps directly:

| Manifest path | Offer field |
|---|---|
| `service_id` | `offer_id` (when `offer_id` is omitted) |
| `runtime` | `runtime` |
| `package_kind` | `package_kind` |
| `entrypoint` | `entrypoint` |
| `entrypoint_kind` | `entrypoint_kind` |
| `contract_version` | `contract_version` |
| `mode` | `mode` |
| `publication_state` | `publication_state` |
| `limits.max_input_bytes` | `max_input_bytes` |
| `limits.max_runtime_ms` | `max_runtime_ms` |
| `limits.max_memory_bytes` | `max_memory_bytes` |
| `limits.max_output_bytes` | `max_output_bytes` |
| `limits.fuel_limit` | `fuel_limit` |
| `price.sats` | `price_sats` |
| `price.currency` | `price_currency` |
| `summary` | `summary` |
| `input_schema` | `input_schema` |
| `output_schema` | `output_schema` |

The 1:1 mapping is deliberate. Translation logic in the engine = bugs.

---

## Stripe service example

A Stripe-priced service uses `settlement.method = "stripe"` with
`price.currency = "usd"`. The integer in `price.sats` is US cents
(e.g. `500` = $5.00).

```toml
schema_version = "froglet-service/v3"
service_id     = "my-stripe-service"
runtime        = "python"
package_kind   = "inline_source"
entrypoint     = "handler.py"

[hosting]
default = "tor"

[settlement]
method = "stripe"

[price]
sats     = 500     # $5.00 in US cents
currency = "usd"   # required when settlement.method = "stripe"

[marketplace]
url = "https://marketplace.froglet.dev"
```

The `settlement.method = "stripe"` declaration is the manifest-level intent.
The signed offer's `settlement_method` field is stamped by the backend as
`"stripe_mpp.v1"` at publish time — do not set that field manually.
