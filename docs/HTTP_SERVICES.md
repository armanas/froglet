# Bounded HTTP services and invitations

Froglet can expose one existing HTTPS JSON operation as an ordinary Wasm service.
Examples include a lookup tool, a document classifier or a model inference
endpoint. The provider runs the adapter and owns the upstream account and its
credentials. Consumers supply schema-checked JSON and receive a signed deal
result. The existing publication engine, service binding and receipt verifier
remain in use; this adds no Kernel artifact or payment rail.

## Prepare without calling the upstream

The native `froglet-node mcp` tool supports `prepare_http_service`. The CLI
equivalent is `froglet-node prepare-http-service --request /absolute/request.json
--json`. A request looks like this (replace the destination and endpoint):

```json
{
  "destination": "/absolute/new-service-project",
  "service_id": "bounded-classifier",
  "summary": "Classify a short text using the provider's fixed model",
  "operation": {
    "url": "https://api.example.com/v1/classify",
    "method": "POST",
    "auth_profile": "classifier",
    "input_schema": {
      "type": "object",
      "properties": {"text": {"type": "string", "maxLength": 2000}},
      "required": ["text"],
      "additionalProperties": false
    },
    "output_schema": {
      "type": "object",
      "properties": {"label": {"type": "string"}},
      "required": ["label"],
      "additionalProperties": false
    },
    "fixed_body": {"model": "approved-model", "max_tokens": 32, "stream": false},
    "timeout_ms": 5000,
    "max_request_bytes": 8192,
    "max_response_bytes": 16384
  },
  "example_input": {"text": "An example to classify"}
}
```

Choose the fields and schemas for the actual upstream API. This is a direct JSON
adapter, not a universal model API translator. `fixed_body` fields cannot also
be caller-controlled properties. Fix the model, output-token ceiling and other
upstream cost controls where the API supports them. Preparation validates the
definition and example but does not execute them. It creates a new private
directory with `froglet-service.toml`, `operation.wasm` and `operation.json`, and
returns the exact `operation_hash` for operator approval.

The endpoint, schemas, fixed fields and example are public package material.
Keep secrets out of all of them. Put credentials only in the provider policy.

## Operator setup

Put the returned operation hash in a provider-owned policy file. For an
authenticated operation the file must be a regular file owned by the running
user with Unix mode `0600`; a symlink is refused. A policy example is:

```toml
[http]
operations_only = true
operation_hashes = ["REPLACE_WITH_RETURNED_64_HEX_OPERATION_HASH"]
allowed_hosts = ["api.example.com"]
allow_private_networks = false
max_calls_per_execution = 1
max_timeout_ms = 5000
max_request_body_bytes = 8192
max_response_body_bytes = 16384
max_redirects = 0

[http.auth_profiles.classifier]
scheme = "https"
host = "api.example.com"
port = 443
path_prefix = "/v1/classify"
header_name = "authorization"
header_value = "Bearer REPLACE_PRIVATELY_ON_PROVIDER"
```

Set `FROGLET_WASM_POLICY_PATH` to that file and restart the node. The exact hash
covers the URL, method, schemas, fixed fields, profile name and limits. Editing
the operation requires a new hash approval and publication. `operations_only`
disables general `http.fetch` for the whole provider, including existing Wasm
services. Use a separate provider if existing services need general HTTP access.

While payments are deferred, put explicit finite allowances in the node’s
environment file or service-manager configuration, for example:

```dotenv
FROGLET_PAYMENT_BACKEND=none
FROGLET_PROVIDER_ACCESS_MODE=invite
FROGLET_PROVIDER_MAX_TOTAL_QUOTES=20
FROGLET_PROVIDER_MAX_TOTAL_DEALS=10
FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS=60000
```

These are examples to review, not automatic defaults or currency limits.
The node refuses to start with approved HTTP operations in legacy `open` mode
or without the explicit cumulative allowances required by protected modes.
Counters persist in the node database; restart, pause/resume and invitation rotation do
not reset them. Quotas apply to one provider database, not every machine using
the same upstream credential. Use the upstream account's own hard usage controls
as well. A timeout closes Froglet's request; it cannot guarantee cancellation of
work already started upstream. Publication verification and public canaries can
also incur upstream charges. An unpredictable or unbounded upstream operation
is unsuitable for this adapter.

Owner-initiated publication verification is outside deal reservations. It requires
the operator credential; never share that credential with recipients. Cloud
hosting and processing rejected traffic can still cost money while the provider
is running, even when no further execution is admitted.

Publish through `marketplace_publish` with `project_dir` and `host: "local"`, or
run `froglet-node publish --host local --json` inside the generated project.
Publication runs the example once and fails if its output does not match the
schema. `local_verified` means the service is published on this node; it does
not mean there is a marketplace listing or a verified public endpoint.

## Invite a recipient

Use the native MCP actions `invite_create`, `invite_list`, `invite_revoke`,
`safeguards_status`, `safeguards_pause` and `safeguards_resume`. CLI equivalents:

```sh
froglet-node safeguards invite-create --name Alice --expires-at UNIX_SECONDS \
  --max-requests 20 --token-file /absolute/new-alice.token --json
froglet-node safeguards invite-list --json
froglet-node safeguards invite-revoke --invite-id HASH_FROM_LIST --json
```

Expiry must be within 30 days; the request allowance is 2–10000. Only the hash
is stored in the database. The credential is written once to a new mode-0600
file and is omitted from CLI/MCP output. Existing files are never overwritten.
The authenticated HTTP issue endpoint returns the credential once with
`Cache-Control: no-store`. A normal invocation consumes two invitation requests
(quote and deal); failed requests also count. Invitations grant access to new
quotes and deals **across this provider**, not just one service. They never
grant operator access. Use distinct providers for distinct recipient scopes.

Deliver the credential file through a trusted channel. Do not put it in prompts,
share links or QR codes. A recipient with a local Froglet runtime can use native
`invoke_service` with `provider_id`, `provider_url`, `service_id`, `input`, a
stable `idempotency_key`, and `access_token_file`. CLI:

```sh
froglet-node invoke bounded-classifier '{"text":"Classify this"}' \
  --provider-id PROVIDER_ID --provider-url https://your-provider.example \
  --access-token-file /absolute/alice.token --idempotency-key alice-call-1 --json
```

Remote invitation transport requires HTTPS and the existing public-address
validation. The recipient file must be owned by that user, mode 0600 on Unix,
and not a symlink. Revocation takes effect immediately; existing completed deals
remain recoverable. Reuse the same idempotency key and input to reconcile an
uncertain invocation; creating a new key requests new work.

## Current boundaries

- Private/invite services can be invoked directly over an operator-configured
  public HTTPS endpoint. Public marketplace activation currently requires
  anonymous execution canaries; it cannot activate these protected services.
  Keep them local-published and share the provider address/identity privately.
  A bounded `trial` provider is the existing option for a public demonstration.
- Supports fixed GET (empty input) or POST JSON. No caller-selected URLs,
  redirects, retries, streaming, uploads, storage or arbitrary proxying.
- Uses JSON Schema draft 2020-12 with finite definition size/depth. References,
  regex patterns and combinators are rejected. Request ceiling: 512 KiB;
  response ceiling: 128 KiB; timeout ceiling: 30 seconds. Smaller node and
  operation limits prevail. Response headers and upstream error bodies are
  withheld; schema-invalid responses and plainly echoed credentials fail.
- This is not a GPU scheduler or an unbounded agent loop. The current reference
  executor refuses `compute.gpu` capabilities until device attachment and
  accounting are implemented.

## Release qualification

Run `scripts/strict_checks.sh` before release. The release workflow now depends
on the same CI workflow, including Linux integration and installation checks,
before creating or uploading release assets. Local test success is not evidence
that the release workflow or deployed provider has run this build.

Before enabling remote use: deploy the reviewed build, load the private policy
and finite allowances, inspect `safeguards status`, verify anonymous refusal,
perform one invited call from a separate public-network host, verify the receipt,
revoke the invite and check that new work stops while result recovery still
works. Preserve the identity, database and policy during upgrades or rollback;
never reset the database to replenish allowances. Keep admission paused if any
of these checks fails. Production protection is unverified until this drill is
run against the actual deployed endpoint.
