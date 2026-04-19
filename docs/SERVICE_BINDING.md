# Froglet v1 Service Binding Specification

Status: normative service binding specification (current authoritative contract material — temporary, may later be removed or folded elsewhere)

This document is normative for the interoperable service binding layer only.

It defines:

- the relationship between `service_id`, `offer_id`, `offer_kind`, and `resource_kind`
- the three product shapes: named services, data services, and direct compute
- how each product shape reduces to the kernel deal flow
- the discovery record structure (required and optional fields)
- how `invoke_service` resolves a service manifest into a workload spec and deal parameters

The following are intentionally outside this specification:

- local project layout, file layout, and build pipelines
- host-specific API shapes (OpenClaw/NemoClaw/MCP tool schemas)
- project authoring workflows (create, scaffold, publish)
- deployment topology (Docker Compose, Kubernetes, cloud-native)
- **identity attestations** (DNS and OAuth/OIDC bindings of a Froglet key to
  a real-world identifier) — these are an optional marketplace-layer
  projection, never mandatory and never kernel-gating; see
  [IDENTITY_ATTESTATION.md](IDENTITY_ATTESTATION.md) for the spec

For the kernel contract (signed envelope, artifact types, settlement methods, state machines), see [KERNEL.md](KERNEL.md).

## 1. Scope

The service binding layer sits between the kernel and the node/tool surface.
It defines enough for two concerns:

1. **Service discovery** — a requester can find available services and understand what they offer.
2. **Service invocation** — a requester can invoke a discovered service, and the binding layer compiles that invocation into a normal kernel deal flow.

The kernel defines signed artifacts, settlement, and state machines.
The service binding layer defines how product-level concepts (named services, data queries, open-ended compute) map into those kernel primitives.

All three product shapes reduce to the same kernel deal flow: Offer → Quote → Deal → (InvoiceBundle for paid deals) → Receipt.

## 2. Identifier Relationships

### 2.1 Core Identifiers

**`service_id`** — A stable, human-friendly identifier for a published service.
A `service_id` uniquely identifies a callable service within a single provider.
It is the primary lookup key for discovery and invocation.
A `service_id` MUST be unique per provider but MAY collide across providers.

**`offer_id`** — The provider-chosen identifier placed in the kernel `Offer.payload.offer_id` field.
An `offer_id` identifies the underlying kernel offer that backs a service.
Multiple services MAY share the same `offer_id` (e.g., a generic compute offer backing several named services), but in practice most named services have a 1:1 mapping.

**`offer_kind`** — The workload kind identifier placed in the kernel `Offer.payload.offer_kind` field.
It declares the execution contract family for the offer.
Standard v1 values include:

- `compute.execution.v1` — generic execution (named services, inline Python, OCI-backed containers)
- `compute.wasm.v1` — inline Wasm module execution
- `compute.wasm.oci.v1` — OCI-referenced Wasm execution
- `events.query` — builtin data service (event log queries)
- `confidential.service.v1` — confidential TEE service execution
- `compute.wasm.attested.v1` — attested Wasm in confidential enclave

**`resource_kind`** — A coarse classification of the resource a service provides.
It is a convenience field on the discovery record so that bot hosts do not need to infer the resource category from runtime fields alone.
Standard v1 values:

| `resource_kind` | Meaning |
|---|---|
| `"service"` | Named service or generic compute (default) |
| `"data"` | Data service (e.g., event log queries) |
| `"compute"` | Direct compute (raw Wasm submission) |
| `"confidential"` | Confidential/TEE execution |

### 2.2 Derivation Rules

`resource_kind` is derived from the service's runtime and offer_kind:

- `runtime = "builtin"` and `offer_kind = "events.query"` → `resource_kind = "data"`
- `runtime` in `{"tee.service", "tee.wasm", "tee.python"}` → `resource_kind = "confidential"`
- All other combinations → `resource_kind = "service"` (the default)

When a requester submits a direct compute workload (raw Wasm or inline source without a service manifest), the workload-level `resource_kind` is `"compute"`.
This distinction exists at the workload layer, not the service record layer.

## 3. Product Shapes

Froglet supports three product shapes. All three reduce to the same kernel deal flow.

### 3.1 Named Services

A named service is a provider-published callable resource with a stable `service_id`, a human-readable summary, and binding information that allows the operator to compile an invocation into a kernel workload.

Examples: a Python handler that returns "pong", an OCI-backed container that processes images, a Wasm module that computes hashes.

**Kernel reduction:**

1. The requester discovers the service via `discover_services` or `get_service`.
2. The operator resolves the service manifest (runtime, package_kind, entrypoint_kind, entrypoint, inline_source or OCI reference).
3. The operator builds a `WorkloadSpec` from the manifest and the requester's input.
4. The operator submits a deal request to the runtime using the service's `offer_id`.
5. The kernel deal flow proceeds: Quote → Deal → (InvoiceBundle if paid) → execution → Receipt.

The requester does not need to know the underlying execution details. The service manifest provides enough binding information for the operator to construct the correct workload.

### 3.2 Data Services

A data service is a provider-published resource backed by a builtin runtime. The canonical v1 data service is `events.query`, which queries the provider's event log.

**Kernel reduction:**

1. The requester discovers the data service (it appears in the service list with `resource_kind: "data"`).
2. The operator builds a `WorkloadSpec::EventsQuery` (or equivalent builtin execution) from the input parameters.
3. The kernel deal flow proceeds identically to named services.

Data services use the same signed artifact chain. The `resource_kind: "data"` classification is a discovery convenience, not a kernel distinction.

### 3.3 Direct Compute

Direct compute is open-ended execution where the requester supplies the execution material (Wasm module, inline source, or OCI reference) rather than invoking a pre-published service.

**Kernel reduction:**

1. The requester submits a compute request via `run_compute` with execution material and a target provider.
2. The operator builds a `WorkloadSpec` from the supplied material.
3. The operator submits a deal request using the provider's generic compute `offer_id`.
4. The kernel deal flow proceeds identically.

Direct compute does not require a `service_id` or service manifest. The requester provides the execution material directly.

### 3.4 Kernel Equivalence

All three product shapes produce the same kernel artifacts:

- The `Offer` is the same signed artifact regardless of product shape.
- The `Quote` commits to a `workload_hash` and `workload_kind` regardless of how the workload was constructed.
- The `Deal`, `InvoiceBundle`, and `Receipt` are identical across product shapes.

The product shape distinction exists only at the service binding layer. The kernel does not distinguish between a deal originating from a named service invocation, a data query, or a direct compute submission.

## 4. Discovery Record Structure

A discovery record is a `ProviderServiceRecord` returned by the provider's service listing endpoint. It contains enough information for a requester to understand what a service does and for the operator to compile an invocation into a kernel workload.

### 4.1 Required Fields

Every discovery record MUST contain:

| Field | Type | Description |
|---|---|---|
| `service_id` | string | Stable human-friendly service identifier |
| `offer_id` | string | Kernel offer identifier backing this service |
| `summary` | string | Human-readable description of the service |
| `mode` | string | Execution mode: `"sync"` or `"async"` |
| `price_sats` | integer | Price in satoshis (0 for free services) |
| `publication_state` | string | Publication state: `"active"` or `"hidden"` |
| `provider_id` | string | Froglet application identity of the provider |

### 4.2 Conditionally Present Fields

These fields are present when non-empty and provide classification and execution binding:

| Field | Type | Description |
|---|---|---|
| `offer_kind` | string | Workload kind identifier (e.g., `"compute.execution.v1"`) |
| `resource_kind` | string | Coarse resource classification (defaults to `"service"`) |
| `runtime` | string | Execution runtime family (e.g., `"python"`, `"wasm"`, `"container"`, `"builtin"`) |
| `package_kind` | string | Package format (e.g., `"inline_source"`, `"inline_module"`, `"oci_image"`, `"builtin"`) |
| `entrypoint_kind` | string | Entrypoint type (e.g., `"handler"`, `"script"`) |
| `entrypoint` | string | Entrypoint value (e.g., `"handler"`, `"main.py"`) |
| `contract_version` | string | Execution contract version string |
| `mounts` | array | Execution mount specifications |

### 4.3 Optional Fields

These fields MAY be present and provide additional metadata or binding material:

| Field | Type | Description |
|---|---|---|
| `project_id` | string or null | Local project identifier (if service is project-backed) |
| `module_hash` | string or null | Hash of the compiled execution module |
| `input_schema` | object or null | JSON Schema for the service's expected input |
| `output_schema` | object or null | JSON Schema for the service's expected output |
| `module_bytes_hex` | string or null | Hex-encoded module bytes (binding material, not for interop) |
| `inline_source` | string or null | Inline source code (binding material, not for interop) |
| `oci_reference` | string or null | OCI image reference (binding material) |
| `oci_digest` | string or null | OCI image digest (binding material) |

### 4.4 Binding Material vs. Interop Fields

Fields such as `module_bytes_hex`, `inline_source`, `oci_reference`, and `oci_digest` are **binding material**: they are used by the operator to compile an invocation into a kernel workload. They are not part of the interoperable discovery contract — a requester does not need to interpret these fields directly. The operator handles the translation.

Fields such as `service_id`, `offer_id`, `summary`, `resource_kind`, `price_sats`, `input_schema`, and `output_schema` are **interop fields**: a requester or bot host uses them to decide whether and how to invoke a service.

## 5. Service Invocation Resolution

### 5.1 `invoke_service` Flow

When a requester invokes a service by `service_id`, the operator resolves the invocation through the following steps:

1. **Fetch the service record.** The operator retrieves the `ProviderServiceRecord` for the given `service_id`. If no `provider_id` or `provider_url` is specified, the operator first checks local services, then searches discovered providers. If the `service_id` matches multiple providers, the operator MUST reject the request and require the requester to disambiguate.

2. **Parse the execution profile.** The operator parses `runtime`, `package_kind`, and `entrypoint_kind` from the service record. If `entrypoint_kind` is empty, the operator uses the default for the runtime. If `entrypoint` is empty, the operator uses the default for the runtime and entrypoint kind.

3. **Build the workload spec.** Based on the `(runtime, package_kind)` combination, the operator constructs the appropriate `WorkloadSpec`:

   | Runtime | Package Kind | Workload Construction |
   |---|---|---|
   | `wasm` | `inline_module` | Inline Wasm submission from `module_bytes_hex` |
   | `wasm` | `oci_image` | OCI Wasm submission from `oci_reference` and `oci_digest` |
   | `python` | `inline_source` | Python inline handler or script from `inline_source` |
   | `python` or `container` | `oci_image` | Container OCI execution from `oci_reference` and `oci_digest` |
   | `builtin` | `builtin` | Builtin execution (e.g., `events.query`) |

4. **Finalize the workload.** The operator sets the `contract_version` and `mounts` from the service record, validates the workload, and produces the final `WorkloadSpec`.

5. **Submit the deal.** The operator submits a deal request to the runtime with the service's `offer_id` and the constructed `WorkloadSpec`. The kernel deal flow takes over from here.

6. **Poll for completion.** For synchronous services (`mode: "sync"`), the operator polls the deal status until it reaches a terminal state or a timeout expires. For asynchronous services, the operator returns the deal immediately and the requester polls separately.

### 5.2 `run_compute` Flow

Direct compute follows a similar path but skips the service record lookup:

1. The requester provides execution material directly (Wasm module, inline source, or OCI reference) along with a target `provider_id` or `provider_url`.
2. The operator builds a `WorkloadSpec` from the supplied material.
3. The operator selects the direct-compute offer that matches the workload kind:
   `execute.compute` for `compute.wasm.v1`, and `execute.compute.generic` for `compute.execution.v1`.
4. The kernel deal flow proceeds identically.

### 5.3 Resolution Invariants

- The `offer_id` used in the deal request MUST come from the resolved service record (for `invoke_service`) or the workload-compatible direct-compute offer (for `run_compute`).
- The `WorkloadSpec` MUST be valid for the offer's `offer_kind`. The provider will reject workloads that do not match the offer's execution profile.
- The `workload_hash` in the resulting `Quote` and `Deal` is computed from the canonical serialization of the `WorkloadSpec`. It is stable for identical inputs.
- Service invocation and direct compute produce identical kernel artifacts. The kernel cannot distinguish the origin of a workload.
