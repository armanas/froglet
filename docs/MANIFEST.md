# Froglet Manifests

Status: authoring contract for `froglet.toml` (project-level) and
`froglet-service.toml` v4 (per-service). These manifests are outside the
Froglet Kernel; see [PUBLICATION_CONTRACT.md](PUBLICATION_CONTRACT.md).

This document defines the two manifest files that make a Froglet service
authorable and publishable in one command (`froglet-node publish`) or one MCP
call (`marketplace_publish`). The manifest is the durable contract between
the author and the publish engine: change the file, re-publish, the new offer
is signed and, for public hosting modes, submitted for registration.

A v2 sample lives at `data/projects/test_project_1/froglet-service.toml`. New
authoring uses v4. V3 remains readable for compatibility, and v2 loads with
deprecation warnings for its missing publication sections.

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
| `project.defaults.*` | omitted | Compatibility metadata for legacy v2 hosting only; v4 services declare runtime, hosting, and settlement explicitly |

---

## `froglet-service.toml` v4 — per-service

Required for every service. Defines the executable artifact, hosting choice,
settlement, and limits.

```toml
schema_version = "froglet-service/v4"

# Identity
project_id = "my-project"           # must match enclosing froglet.toml
service_id = "translator-en-es"     # unique within project
offer_id = "translator-en-es"       # defaults to service_id
summary = "Translate EN→ES via Claude"

# Runtime / packaging
runtime = "python"                  # python | wasm | container
package_kind = "inline_source"      # inline_source | inline_module | oci_image
entrypoint_kind = "handler"         # handler (python default) | script | module
entrypoint = "handler.py"
contract_version = "froglet.python.handler_json.v1"  # defaulted per runtime
mode = "sync"                       # sync | async
source_kind = "python"              # informational
publication_state = "active"        # active | hidden
capabilities = []                    # normalized, sorted, and deduplicated
verification = { input = { text = "hello" } } # private; required for public hosting

# Optional dependency lock for python+inline_source. Omit this section for a
# dependency-free, automatically locked bundle.
# [python]
# lock = "python-lock.json"           # relative to entrypoint's directory

# Optional author-declared data requirement. Provider binding material is
# resolved locally and must not appear here.
# [[mounts]]
# handle = "warehouse"
# kind = "postgres"                 # postgres | sqlite | object_store | redis
# read_only = true                   # defaults to true for publication

# Hosting
[hosting]
default = "relay"                   # local | relay | tor | self | managed

[hosting.local]
# no config

[hosting.relay]
# no service-level URL or credentials; the Froglet Node supplies its exact
# identity-derived Reachability Lease endpoint

[hosting.tor]
# auto-spawn local tor hidden service; no config

[hosting.self]
url = "https://my-existing-app.fly.dev"

# Provider-neutral managed authoring (adapter not yet implemented):
# [hosting.managed]
# slug = "translator"
# target = "regional-container"
# profile = "small-public"

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
sats = 0                            # must be 0 when settlement.method = "none"
# base_fee_msat = 0                 # optional explicit base leg
# success_fee_msat = 0              # if set, must equal sats * 1000
# currency = "sat"                  # "sat" (default) = satoshis via Lightning
#                                   # "usd"           = US cents via Stripe
#                                   # currency="usd" requires a Stripe payment backend

# Optional I/O schemas (JSON Schema, free shape)
[input_schema]
# example: type = "object", required = ["text"], ...

[output_schema]
# example: type = "object", required = ["translated"], ...

# Optional provider-private local verification fixture. It is never copied to
# public service records, signed offers, or the artifact feed.
# [verification]
# input_json = '{"text":"hello"}'
# expected_output_json = '{"translated":"hola"}'
```

### Required fields

- `schema_version` — `"froglet-service/v4"` for new authoring. V3 and v2 are
  compatibility inputs.
- `service_id` — non-empty, lowercase + digits + hyphens, ≤63 chars
- `runtime` — one of `python` | `wasm` | `container`; `builtin` is reserved
- `package_kind` — one of `inline_source` | `inline_module` | `oci_image`;
  `builtin` is reserved
- `[hosting] default` — one of `local` | `relay` | `tor` | `self` | `managed` in v4
- `[settlement] method` — one of `"none"` | `"lightning"` | `"stripe"`

### Conditional requirements

| Runtime | Package kind | Required additional |
|---|---|---|
| `python` | `inline_source` | `entrypoint` (relative path to source file) |
| `wasm` | `inline_module` | `entrypoint` (relative path to `.wat` source or `.wasm` artifact) |
| `wasm` | `oci_image` | `oci.reference` AND `oci.digest` |
| `python` | `oci_image` | `oci.reference` AND `oci.digest` |
| `container` | `oci_image` | `oci.reference` AND `oci.digest` |
| `builtin` | `builtin` | **rejected at publish time** — builtins are reserved |

### Locked Python bundles

Every new `python + inline_source` publication is a canonical locked bundle,
including services with no third-party dependencies. If `[python].lock` is
omitted, the publish engine detects the local CPython implementation, exact
version, and ABI and creates a dependency-free lock automatically. This is the
fewest-step path for handlers that use only the standard library.

For third-party packages, set `[python].lock` to a JSON file relative to the
entrypoint's directory. The lock may reference only local, hash-pinned,
pure-Python wheels in an `artifacts/` directory beside the lock:

```text
handler.py
python-lock.json
artifacts/
└── example_pkg-1.2.3-py3-none-any.whl
```

```json
{
  "schema_version": "froglet.python-lock.v1",
  "source_sha256": "<sha256-of-handler.py>",
  "runtime": {
    "implementation": "cpython",
    "version": "3.12.4",
    "abi": "cpython-312"
  },
  "artifacts": [
    {
      "name": "example-pkg",
      "version": "1.2.3",
      "filename": "example_pkg-1.2.3-py3-none-any.whl",
      "sha256": "<sha256-of-exact-wheel-bytes>"
    }
  ]
}
```

Artifact entries must be sorted and unique by `name`. Publication never runs
pip, a dependency resolver, or a network download. It rejects native wheels,
unsafe ZIP paths, symlinks, metadata/tag mismatches, digest mismatches, and
oversized archives. The package binding is SHA-256 of the exact canonical
bundle envelope. Before invocation, the provider independently attests that
its CPython implementation/version/ABI exactly matches the lock, materializes
the verified wheel files into an invocation-private temporary directory, and
executes with a cleared environment. The canonical bundle remains
provider-private; public service-addressed requests carry only its digest.

The one narrow exception to the builtin reservation is the v4 native
read-only data lane. JSON and SQLite carry their own schema. CSV requires an
ordered type declaration and at least one index; Froglet never guesses CSV
types from sample rows:

```toml
schema_version = "froglet-service/v4"
service_id = "people"
runtime = "builtin"
package_kind = "builtin"
contract_version = "froglet.builtin.data_query.csv.v1"
verification = { input = { op = "describe" } }

[data]
path = "people.csv"
format = "csv"                    # json | csv | sqlite
collection = "people"             # CSV only

[[data.columns]]
name = "id"
type = "integer"                  # string | integer | number | boolean
indexed = true                     # at least one CSV column

[[data.columns]]
name = "name"
type = "string"
nullable = false

[hosting]
default = "relay"

[settlement]
method = "none"
```

The first CSV record must exactly match the declared column names and order.
An empty cell becomes `null` only for a nullable column. Equality selection on
CSV must include an indexed column. Import is bounded, builds a private
content-addressed SQLite cache once, and later invocation accepts only
`describe` or structured `select`; paths and raw SQL are never caller input.

For `wasm + inline_module`, a `.wat` entrypoint is compiled by the exact WAT
compiler embedded in the released `froglet-node`; a clean host does not need
Rust, Node.js, Python, or a separate Wasm toolchain. `.wasm` input is
snapshotted directly. The runtime interface is always
`froglet.wasm.run_json.v1`, `entrypoint_kind = "module"`, `entrypoint = "run"`.

### Hosting-backend-specific fields

| Backend | Required | Notes |
|---|---|---|
| `local` | — | Private dev only; not registered with marketplace |
| `relay` | — | Default public path. The Froglet Node must report the exact configured identity-derived HTTPS endpoint before approval; the manifest contains no relay URL or credential. |
| `tor` | — | The engine uses the onion URL advertised by a running daemon configured with Tor or dual network mode. |
| `self` | `hosting.self.url` | Public HTTPS or approved onion transport. Marketplace registration validates `/v1/feed` and signed descriptor/offer consistency. |
| `managed` | `hosting.managed.target`, `hosting.managed.profile` | Provider-neutral v4 selector. The current publish engine validates this shape but does not yet implement the deployment adapter. |
| `fly` | `hosting.fly.app`, `hosting.fly.region` | V3 compatibility only; accepted with a deprecation warning. V4 rejects it. |

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
| `limits.*` | provider runtime defaults and maxima |
| `price.sats` | `0` |
| `price.currency` | `"sat"` — satoshis, Lightning rail. Use `"usd"` for US cents on the Stripe rail (requires Stripe payment backend). |
| `marketplace.url` | inherited from `froglet.toml`, else `"https://marketplace.froglet.dev"` |

---

## Resolution rules

V4 service manifests declare runtime, hosting, and settlement explicitly;
`[project.defaults]` does not cascade those fields into v4. The project
marketplace URL is inherited when the service does not override it. A project
hosting default remains readable only for legacy v2 compatibility, where the
service schema did not require `[hosting]`.

`project_id` in the service manifest must match `project.name` in the project
manifest when both are present. Mismatch is a validation error.

---

## Validation rules

The parser's `validate()` method enforces:

- **Schema version**: `"froglet/v1"` for project and `"froglet-service/v4"`
  for new services. `froglet-service/v3` remains accepted; v3 Fly hosting
  emits a deprecation warning. `froglet-service/v2` remains accepted with a
  warning per missing publication section.
- **Identifier shape**: `name`, `project_id`, `service_id`, `offer_id` are
  lowercase ASCII + digits + interior hyphens, 1-63 chars, no leading/
  trailing hyphen.
- **Runtime + package_kind combos**: per the table above. Invalid combos
  rejected with a message naming the combo and the allowed alternatives.
- **Hosting behavior**: `local` remains private and the publish engine skips
  marketplace registration even when the project defines a marketplace URL.
- **Settlement allowlist**: `[settlement] method` must be one of `"none"`,
  `"lightning"`, or `"stripe"`; any other value is rejected. `"stripe"`
  additionally requires `price.currency = "usd"`; `"lightning"` requires
  `price.currency = "sat"` or absent.
- **Limits sanity**: each supplied byte/runtime limit must be `> 0`.
  `fuel_limit` may be `0` to mean "no independent fuel ceiling within
  max_runtime_ms". The provider rejects requests above its available maxima.
- **Mount safety**: authoring mount handles and kinds are validated;
  `read_only` defaults to `true`, and provider-owned `binding` material is
  rejected in a publication manifest.
- **Verification privacy**: a fixture requires exactly one input
  representation and at most one expected-output representation. It remains
  provider-private.
- **Entrypoint reachability**: relative paths in `entrypoint` are resolved
  against the service manifest's directory; the path must exist when the
  publish engine is invoked.
- **No unknown fields**: extra top-level keys are rejected to catch typos
  early. Nested unknown keys inside `[input_schema]` / `[output_schema]` are
  preserved as-is.

---

## v2/v3 → v4 migration

A v2 manifest is interpreted with compatibility defaults:

- `[hosting] default = "local"` (private)
- `[settlement] method = "none"` (free)
- `[limits]` from engine defaults
- `[marketplace] url` from project default or built-in default

The parser emits one warning per missing section. For ordinary v3 manifests,
change `schema_version` to `froglet-service/v4`. If a v3 manifest selects
`fly`, replace it with `managed` and move the provider-specific choice behind
non-empty `hosting.managed.target` and `hosting.managed.profile` values. The
managed adapter is still pending, so do not treat successful manifest parsing
as deployment proof.

New object-store authoring uses mount kind `object_store` and capability
`mount.object_store.<read|write>.<handle>`. The historical kind `s3` remains a
read compatibility alias: parsing it into a `PublicationIntent` normalizes
both the kind and any `mount.s3.*` authoring capability before a new revision
is signed. Already-signed revisions retain their original exact spelling.

---

## Reuse contract with `ProviderManagedOfferDefinition`

Manifest fields map losslessly into the provider-managed definition in
`src/api/types.rs::ProviderManagedOfferDefinition`. This definition is local
application state, not itself a signed offer; the provider converts its Kernel
subset into the existing signed offer payload:

| Manifest path | Provider definition field |
|---|---|
| `project_id` | `project_id` |
| `service_id` | `service_id` |
| `offer_id` | `offer_id` (defaults to `service_id`) |
| `runtime` | `runtime` |
| `package_kind` | `package_kind` |
| `entrypoint` | `entrypoint` |
| `entrypoint_kind` | `entrypoint_kind` |
| `contract_version` | `contract_version` |
| `mode` | `mode` |
| `mounts` | `mounts` (provider resolves bindings) |
| `capabilities` | `capabilities` |
| `publication_state` | `publication_state` |
| `limits.max_input_bytes` | `max_input_bytes` |
| `limits.max_runtime_ms` | `max_runtime_ms` |
| `limits.max_memory_bytes` | `max_memory_bytes` |
| `limits.max_output_bytes` | `max_output_bytes` |
| `limits.fuel_limit` | `fuel_limit` |
| `price.sats` | `price_sats` |
| `price.base_fee_msat` | `base_fee_msat` |
| `price.success_fee_msat` | `success_fee_msat` |
| `price.currency` | `price_currency` |
| `settlement.method` | `settlement_method` |
| `summary` | `summary` |
| `starter` | `starter` |
| `source_kind` | `source_kind` |
| `input_schema` | `input_schema` |
| `output_schema` | `output_schema` |
| `verification` | `verification` (provider-private; never public offer metadata) |

The 1:1 mapping is deliberate. Translation logic in the engine = bugs.

---

## Stripe service example

A Stripe-priced service uses `settlement.method = "stripe"` with
`price.currency = "usd"`. The integer in `price.sats` is US cents
(e.g. `500` = $5.00).

```toml
schema_version = "froglet-service/v4"
service_id     = "my-stripe-service"
runtime        = "python"
package_kind   = "inline_source"
entrypoint     = "handler.py"
verification  = { input = {} }

[hosting]
default = "relay"

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
