# Public self-service beta

The public entry point is https://froglet.dev/services/. Visitors need no
installation, account or invitation. They can compile an editable AssemblyScript
program to Wasm in their browser, supply JSON input, query five synthetic
terminology rows, inspect a signed execution failure and export the signed chain.

## Execution boundary

The website Worker forwards only allowlisted free native-provider operations to
`froglet-public-beta-20261006.fly.dev`. `provider_proxy.py` repeats the narrow
operation/runtime/resource checks before forwarding to the node on loopback.
Neither layer forwards caller cookies or authorization headers. Operator, legacy
runtime, arbitrary URL, paid and Python/container operations are not public demo
operations. Host capabilities are refused except for the
[optional named tools](#optional-named-tools-disabled-by-default), which stay
disabled unless configured. Native Froglet remains responsible for artifact
verification, job execution and atomic execution allowances.

`entrypoint.py` checks the executable digest, starts the native node, publishes
only the selected synthetic catalog on first boot, and supervises the ingress.
The node and ingress run as UID/GID 10001 after volume initialization. The native
provider control token and identity remain on the Fly volume. There is no shared
browser signing credential: each new request is signed with a browser-generated
identity. Only its public signed request is saved for recovery.

## Optional named tools (disabled by default)

Three fixed publications, `marketplace-provider`, `marketplace-search` and
`marketplace-receipts`, can run as free public tools. Each may make exactly one
approved HTTP operation to the website's bounded marketplace read adapters on
`froglet.dev`. They are enabled only when every layer is configured:

- Fly: `FROGLET_PUBLIC_DEMO_PROFILE_PATH` and `FROGLET_PUBLIC_DEMO_PROFILE_SHA256`
  name a protected profile file inside `/state` and its digest. Leaving both
  unset or empty disables the tools; setting only one refuses startup.
- Fly: `FROGLET_WASM_POLICY_PATH` must be `/state/approved-http-policy.toml`,
  pinned by `FROGLET_PUBLIC_DEMO_HTTP_POLICY_SHA256`. `entrypoint.py` refuses a
  policy that differs from the reviewed scope: one call per execution, host
  `froglet.dev` only, no private networks or redirects, 2 s, 2 KiB request and
  128 KiB response.
- Fly: an existing provider identity is required, so the tools cannot be
  enabled on a first boot.
- Worker: `FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_ENABLED=true` and
  `FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON` with all three profiles; an
  incomplete set enables none.

These pins are kept in three places (Worker profile JSON, Fly profile file and
policy file) and are synchronized by hand; nothing cross-checks them at startup.
Drift in an approved offer makes `/v1/provider/offers` and `/v1/feed` refuse
with `503` for every caller, including the original demo.

## Pinned runtime and deployment

The wrapper uses the unchanged official `v0.4.7-beta.1` Linux x86_64 executable
from source `d70cb50017c7c526d1f7ad4faf278da07e8248a4`:

- Archive SHA-256: `254e4b50c3f4b883a7b39c4edccb5366d3b9f7ab552ac1ef7435849ea5bce02c`.
- Executable SHA-256: `5afdbaf98f7aa2eb10cff3c29dbeca25d099e421a46a4cff40439aeaf3a25b77`.

Verify the immutable release through [the release procedure](../../docs/RELEASE.md)
before assembling a build context. Place the verified executable next to copies
of `Dockerfile`, `provider_proxy.py`, `entrypoint.py` and `catalog-source.json` in
an ignored temporary directory. The Docker build checks the executable hash;
never add the binary to Git or replace it with an unverified local build.
The wrapper is a derivative deployment image, not an original release image.

The current isolated Fly app has one shared CPU, 512 MiB RAM and one encrypted
1 GiB volume in Amsterdam. Auto-stop and auto-start are disabled in `fly.toml`.
An intentional stop therefore requires an explicit operator start. Keep the
existing volume, native database, HTTP ledger and identity when deploying.
Do not modify other apps or the separate GCP reference service.

## Finite allowances and recovery

Each program has at most 2 seconds, 8 MiB memory, 50 million fuel, 128 KiB input
and output, and 256 KiB module bytes. Native cumulative limits are 1,000 deals,
3,000 quotes and 2,000,000 milliseconds of reserved execution. The persistent
HTTP ledger allows at most 50,000 admissions and 16 GiB of conservatively reserved
traffic; it reserves a full response ceiling before forwarding, so bytes can
exhaust before the admission count. Busy/exhausted calls are refused. These are
shared beta ceilings, not per-visitor promises or sustained-load qualification.

The Worker additionally uses Cloudflare's best-effort per-IP edge rate limiter;
it does not replace the durable provider ledgers. Owner usage is read only on
loopback and reduced to public counters at `/demo/status`. The UI displays the
observed native deal allowance, not a guarantee of all other capacity.

The browser saves the exact signed request before submission. Recovery of a known
accepted job uses GET; an uncertain submission reuses the exact body and retry
key. Discarding a browser reference does not cancel remote work or replenish an
allowance. Missing existing state fails closed. Never recreate a database or
restore an older usage ledger to make capacity available again.

## Marketplace discovery

The ingress exposes `/v1/node/capabilities`, a selected `/v1/feed`, and exact
`/v1/artifacts/{hash}` reads for the active synthetic catalog's signed offer and
its bound descriptor, plus the approved named-tool offers and descriptors when
those tools are configured. The Worker forwards only exact 64-hex artifact
reads. It preserves the native ledger cursors and documents;
visitor quotes, deals and receipts never enter this discovery feed. Unknown
hashes and malformed, repeated or out-of-range feed queries are refused.
Discovery responses have a 32 KiB ceiling and reserve that ceiling in the same
persistent traffic ledger; prior reservations remain spent. Other demo requests
retain their existing response reservation.

`POST /v1/publications/{revision}/canary` accepts only a current selected
revision (the synthetic catalog or a configured named tool), its exact offer and
immutable public verification input with a challenge. The ingress checks only
the challenge's form, not its freshness, so a reused challenge is accepted and
spends allowance again. The unchanged native node signs the response and
charges its finite execution allowance. This is an execution check, not a
health badge.

Submit the signed revision and its bound canary input through the marketplace's
exact registration path. Direct HTTPS candidates require operator review.
Registration submission alone does not establish activation or health: confirm
the exact indexed offer and a healthy, unexpired lease, then observe the lease
renewing after another successful indexer poll. Marketplace reachability checks
and the public portal's execution checks are separate evidence.

`operator_backup.py` makes SQLite online backups and isolated restored copies on
the same private volume. It verifies integrity, schemas and row digests, never
restores production and never exports credentials or databases. Individual DB
backups are consistent; this is not an atomic cross-database snapshot or a
cross-machine identity recovery procedure. Coordinate maintenance before any
actual production restoration and preserve current cumulative usage.

## Verification

Local boundary tests (no cloud capacity consumed):

```sh
python3 -W error -m unittest python.tests.test_public_demo_proxy python.tests.test_public_demo_entrypoint -v
npm test --prefix docs-site -- src/scripts/__tests__/public-demo-proxy.test.ts
```

Real local runtime tests, using an explicitly verified executable:

```sh
FROGLET_NODE_BIN=/absolute/path/to/verified/froglet-node \
FROGLET_PUBLIC_DEMO_REAL_NODE=1 npm test --prefix docs-site -- \
  src/scripts/__tests__/live-service-native.test.ts
```

Cloud canaries require explicit operator opt-in and spend the shared allowance:

```sh
FROGLET_PUBLIC_DEMO_REMOTE_NODE=1 npm test --prefix docs-site -- \
  src/scripts/__tests__/live-service-cloud.test.ts
```

On 6 October 2026 operator HTTPS canaries exercised default/edited arithmetic,
selected data, signed budget failure and exact accepted-result recovery. Actual
browser requests and standalone verification also ran. A controlled same-volume
restart preserved the provider identity, exact completed job, native usage and
remaining allowance. VM-only backup restoration checks passed. This evidence does
not establish power-loss recovery, independent human onboarding, a fresh LLM host,
paid settlement, GPU/general containers, sustained load or an uptime guarantee.

Website publication must use the existing guarded `npm run deploy:site` command.
The cache-library applicability exception expires on 17 October 2026; clean raw
audit output does not bypass it or remediate the disputed library behavior.
