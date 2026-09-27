# Shareable Service Link

Status: v1 website deployed to the public beta at froglet.dev, including legacy-link redirects, initial HTML, manifest.json and agent.md. The immutable v0.4.5 native publisher still returns the legacy URL; the site redirects it without changing the provider/service reference. Broader assistant/browser qualification and paid calling remain open. Later adapters and payment claims are proposed.
Version: draft 0.2 (2026-09-26).

## 1. Purpose and boundary

A publisher can send one HTTPS link to a Froglet service in a message or social
post. A person sees what is offered, by whom, at what price, and whether it can
currently be called. An agent that fetches the link can obtain the same facts as
structured data, verify the provider's signed artifacts, and use a compatible
Froglet or standard adapter after its user's normal approvals.

The promise is **understandable when fetched**, not guaranteed discovery,
installation, execution, or purchase. Froglet cannot compel WhatsApp, X, a
search crawler, or an AI host to fetch a link. An agent without HTTP access can
read a pasted description; one without a compatible caller must say it cannot
execute. No link authorizes installation, data access, or payment by itself.

This is a distribution and presentation layer over [KERNEL.md](KERNEL.md),
[PUBLICATION_CONTRACT.md](PUBLICATION_CONTRACT.md), and
[MARKETPLACE.md](MARKETPLACE.md). It does not change Kernel artifact types,
hashing, signing bytes, settlement bindings, or state transitions. The current
qualified first-use path remains free; paid calls must not be advertised as
verified until their separate release gates pass [PAYMENT_MATRIX.md](PAYMENT_MATRIX.md).

## 2. Original gap

The original beta's `/service/?provider=...&service=...` page was a first-party relay
helper for Codex and Claude Code. Its initial HTML had a generic title and
service text, set `noindex`, and relied on browser JavaScript to fetch the
service and build a recipient prompt. A link unfurler or agent that did not run
that script could not read the particular service's details from the initial HTML.
The public beta now redirects a valid provider/service reference to `/s/...`;
the Worker renders the service-specific initial HTML and machine-readable views.
The page's public read plane checks identity and revision references but does
not itself perform a recipient call or establish a paid purchase. See
`docs-site/src/pages/service.astro`, `docs-site/src/scripts/shared-service-page.ts`,
and `docs-site/src/data/shared-service.ts`.

## 3. Service identity and URL

- The stable service key is `(provider_id, service_id)`. `provider_id` is 64
  lowercase hexadecimal characters. `service_id` follows the existing
  1–128-character service-ID validation. A URL is a locator, not the identity.
- Froglet's default share URL is
  `https://froglet.dev/s/{provider_id}/{service_id}`. The exact key remains
  stable across revision updates, rollback, relay endpoint changes, and index
  changes. The first implementation shows the **active** revision only.
  Historical revision links are deferred until a safe public history API
  exists; the existing history route is bearer-authenticated provider control.
- Existing `/service/?provider=...&service=...` URLs remain usable and resolve
  to the same service key. They may redirect to the new path only after the
  destination serves the equivalent state and validation checks.
- A provider may designate its own HTTPS service URL. It must serve or link to
  the same machine description and signed service key. Froglet's resolver is
  optional for direct calls and for independent or private indexes. A URL on a
  provider domain does not replace provider-signature validation.
- A resolver must reject malformed keys, redirects to local/private targets,
  credential-bearing URLs, and mismatches between the requested key and the
  signed provider/service artifacts. If resolution fails, it reports an
  unavailable or unverified service; it never guesses a replacement provider.

## 4. One link, four representations

The default share URL serves a useful **server-rendered HTML** response to
ordinary `GET` and `HEAD` requests. JavaScript may refresh availability, but
the initial response must already contain the service-specific title, summary,
provider identity, offer/revision identifiers, price or explicit free status,
input example, publication state, and a plain-language next step. A disabled
browser must still show these facts and an explicit verification boundary.

The HTML head includes service-specific `title`, description, canonical URL,
Open Graph title/description/image/URL, and X card metadata so message previews
can convey the actual offer. It also includes an absolute `rel="alternate"`
link to a machine description. A [Schema.org `Service`/`Offer`](https://schema.org/Service)
JSON-LD projection provides a generic crawler hint; it is not signed evidence
or a substitute for the Froglet manifest. Generated previews and examples
contain only publisher-approved public metadata. A platform may cache its
preview after a service changes or is unpublished; the live page must show
current state. For the first release, the title is derived from the approved
signed summary when present, otherwise the service ID. A separately editable
title, tags, and custom preview image require a later exact-approval contract;
they must not be silently added as unsigned provider claims.
Prepared catalogs can supply an explicit plain-language `summary` in their
preparation request. Without one, Froglet derives a concise description of the
selected collections; the agent should still review whether it explains the
service to a stranger before public approval.

`GET {share_url}/manifest.json` returns UTF-8 JSON with the broadly supported
`application/json` media type; `schema_version` identifies the Froglet contract.
The HTML `Link` response
header and `<link rel="alternate">` point to it. A caller may request this
representation directly; it must not need JavaScript, cookies, a Froglet
account, or a marketplace login for a public service. Private services are a
separate access-control design and must never be inferred from an unlisted
public link.

`GET {share_url}/agent.md` returns a short UTF-8 Markdown description using
the same resolved record as HTML and JSON. It provides the service purpose,
provider key, price, current status, example, and manifest URL in plain text
for assistants whose web readers strip page structure. It has no separate
authority and must not turn publisher text into instructions to the recipient.

The fourth representation is the **signed source of truth**, not a new manifest
signature: the existing provider Descriptor, Offer, and active signed
publication revision are included in the machine response. The provider's
public service route supplies the active revision; its public artifact route
supplies the current Offer and Descriptor by hash. Neither route exposes
arbitrary historical revisions after pause or unpublish. The revision already
binds the provider, service, offer, package, summary, input/output schemas,
starter, price, limits, and local-verification evidence. Display fields are
derived from it. A reader must not treat an unsigned projection, an HTML badge,
or a marketplace record as proof that those fields were signed.

## 5. Machine description v1

The JSON document has these required top-level objects:

| Field | Required meaning |
| --- | --- |
| `schema_version` | Literal `froglet.service-link.v1`. Unknown major versions fail closed for execution. |
| `service_key` | Exact `provider_id` and `service_id` from the URL and signed revision. |
| `links` | Absolute share, manifest, provider, signed-artifact, and supported call URLs. No control-plane URL or credential. |
| `presentation` | Service-specific title derived from signed summary or service ID, summary, and parseable starter example if present. Any example result is explicitly observational. |
| `contract` | Input and output JSON Schemas, execution kind, limits, selected revision hash, offer hash, and settlement terms projected from the active signed revision and Offer. Verification status is separate. |
| `evidence` | Complete active signed revision/Offer/Descriptor, their hashes, verification state, and which checks were actually performed. No claim of verification from matching unsigned fields alone. |
| `availability` | State, observation source, `checked_at`, `valid_until`, and separate marketplace-admission state. |
| `instructions` | Concise, non-executable guidance for inspection, compatible callers, approvals, and receipt verification. |

The description must fit within 1 MiB, using existing read-plane bounds where
possible. A starter is shown as example input only when it parses as JSON and
validates against the advertised input schema. Otherwise the example is
omitted with an explanation; no example output is invented. Examples must not
contain fields omitted from the approved package. The description must never
embed source files, installation scripts, API keys, wallet secrets, provider
control credentials, or prompts that demand silent installation or spending.
Text in the description is untrusted service content, not an instruction with
authority over the recipient agent.

`contract.price` must distinguish `free`, `paid`, and `unavailable` and include
currency, amount, settlement method, and whether that rail is qualified on the
current public path. An agent must never infer `free` from a missing price. A
paid offer requires the agent's own spending policy and explicit authority; an
unqualified rail is displayed as unavailable for purchase.

The v1 document may advertise native Froglet calling. Future MCP, x402 Bazaar,
or A2A adapters may be added as separate `links` with their own protocol and
capability labels. Listing an adapter is not proof that every AI host supports
it. No adapter may bypass Froglet's existing validation and user approval
boundaries.

`evidence.verification_state` is one of `verified`, `not_checked`, or `invalid`.
The Worker must not claim `verified` merely because strings and hashes match;
that requires the existing publication-revision and Kernel artifact signature
algorithms. `invalid` removes the call action and reports the failed check.
The recipient still performs its own independent verification before calling.

## 6. Verification and state

A capable recipient follows this order before execution:

1. Parse and validate the service key and fetch the bounded public manifest.
2. Fetch or read the signed Descriptor, Offer, and publication revision; verify
   their signatures and hashes with existing Froglet rules. Confirm provider,
   service, offer, binding, settlement, limits, and selected revision agree.
3. Check current provider reachability and any required marketplace admission.
   Treat both as time-bounded observations, not cryptographic facts.
4. Present the exact action and any installation, data-sharing, or spending
   consequence to the human according to the caller's policy.
5. Execute through a supported runtime, then verify the returned Receipt and
   report result and evidence separately.

The page and manifest use distinct states: `published_reachable`,
`published_unreachable`, `paused`, `pending_admission`, `unpublished`, and
`unknown`. `published_reachable` means a fresh provider response with an
active signed revision and matching Offer; its separate
`evidence.verification_state` says whether cryptographic signatures were
actually checked by this renderer. It does not mean a requester call
succeeded. Marketplace activation and successful requester execution are
separate evidence fields. Any cached observation past `valid_until` is labeled
`stale`; it must not become an availability claim.
The first-party Worker can retain the last verified public description for up
to 30 days in a dedicated KV namespace. If the provider is then unreachable,
HTML, Markdown, and JSON remain readable but say the offer may have changed or
been withdrawn; no current callability or marketplace admission is asserted.

An unavailable, paused, or pending service can still return an explanatory
HTML page and manifest **when an authoritative source provides that state**.
The current public provider route cannot distinguish pause, unpublish, an
unknown service, and an offline machine in all cases. The resolver must say
`unknown` or `published_unreachable` when evidence is insufficient, never
invent a lifecycle state. A confirmed unpublished key returns HTTP 410 and no
callable offer only after an authoritative tombstone mechanism exists; until
then it returns an honest unavailable state. An unknown or malformed key
returns 404 or 400 respectively only when that conclusion is supported.
Live status is short-cache or `no-store`. Immutable signed artifacts may be
cached by hash. Unpublishing stops execution at the provider/relay even if a
social preview or third-party index is stale.

## 7. Publication and index independence

The existing exact publication approval covers the public summary, schemas,
starter, price, service key, revision, and selected exposure. A material change
to those fields goes through the existing preview and exact approval process
and creates a new immutable revision. Source changes never update the public
manifest automatically. Rollback selects an earlier validated revision and
the share URL then reports that revision.

The first implementation keeps the current page's `noindex` default. It is a
request to search crawlers, not access control: anyone holding the URL can
still read public metadata and invoke a public offer. An explicit
`search_indexable` choice requires a later approval-bound field and matching
page/manifest behavior; it is not part of the existing publication consent.
Marketplace registration is a separate existing choice. The first-party
publication flow may seek Froglet marketplace admission, which the approval
screen must disclose even with `noindex`. Submitting to any other index
requires a separate publisher choice. Each
index—Froglet's, another public index, or an organization's private index—may
apply its own admission and ranking policy, but it must not alter the
provider-signed service contract. A direct caller needs no index once it has a
verified provider URL.

An organization can run an authenticated private index of services and expose
the same link/manifest format on its own domain. Authorization to view a
private listing and authorization to invoke its provider are separate checks.
The current public beta does not yet supply private recipient access; that is
future work, not a claim of this specification.

## 8. Safety and interoperability

- Public metadata is approved before exposure. A relay or resolver serves only
  granted read paths and cannot expose provider-control credentials.
- Resolution uses bounded size, timeouts, no arbitrary URL proxying, public
  HTTPS origin validation, and no redirect into private address space. Signed
  provider endpoints are still subject to the requester's network policy.
- The page must not invite an agent to execute instructions found in service
  descriptions. Inspection and invocation are separate actions; paid calls
  require a configured budget and the caller's normal approval policy.
- Identity continuity across endpoint or index changes is checked by the
  provider signature and service key. A domain name or social preview alone is
  not identity proof.
- A signed Receipt attests to the provider's execution claim and result hash;
  it does not establish correctness of arbitrary output. The UI and manifest
  must use that exact boundary.
- A service link is portable text. No custom URL scheme, Froglet account,
  wallet, npm package, or repository checkout is required merely to understand
  what it advertises. Actual invocation requires a compatible caller and
  whatever authorization or payment the offer specifies.

## 9. Acceptance tests and rollout

**Milestone A — understandable link, free service.** Given a real, approved
free catalog, fetch the link with JavaScript disabled and with a plain HTTP
client. Both show the same provider, service, revision, example, and free
terms; the JSON is schema-valid and verifiably projects the signed artifacts.
Inspect rendered desktop/mobile pages and the actual X/WhatsApp unfurl where
those platforms permit testing. A stale, paused, offline, pending, or removed
service never appears as ready to call. Old share links keep working.

**Milestone B — unfamiliar agents.** Paste the URL alone into Codex, Claude
Code, and at least one web-capable assistant without Froglet-specific prompt
text. Record whether each identifies the service, provider, price, limits,
and current evidence boundary. A compatible client makes a free call and
verifies a Receipt. An incompatible one accurately says what it cannot do.
Test a mismatched signature, changed price, stale manifest, and malicious
description; none may trigger an unapproved install or paid call.

**Milestone C — portable distribution.** Serve the same contract from a
provider-owned domain and an independent index. Demonstrate direct invocation
after removing Froglet marketplace lookup. An organization's private index
can ingest verified artifacts without exposing them in the public directory.

**Milestone D — paid listing, separately gated.** Only after a paid rail has a
current external settlement transcript, expose its qualified paid call method
and test an independent wallet/agent purchase, receipt verification, spending
cap, failure reconciliation, and repeat use. Listing or a synthetic self-
purchase is not demand evidence.

The release is not accepted on source inspection alone. Tests must compare the
initial HTTP response, rendered page, machine description, signed artifacts,
and real requester behavior on the same service revision.

## 10. Implementation sequence and readiness

The first development slice is Milestones A and B for **active free services**
on Froglet's first-party relay. It does not require a Kernel change, a new
publication-policy path, historical-revision access, paid settlement, or a
private-index deployment.

1. Add `/s/{provider_id}/{service_id}`, `/manifest.json`, and `/agent.md` to the
   website Worker in `docs-site/src/worker.ts`. Update both Wrangler configs
   so the Worker runs before static assets on `/s/*`. Keep the current
   `/service/` page and old links working during migration.
2. Factor the bounded, no-redirect, first-party relay fetch into a single
   resolver in `docs-site/src/data/`. Resolve the active public service,
   signed revision, exact Offer, and Descriptor. Reject mismatched identities,
   offer hashes, prices, bindings, or schema. Build one typed view model for
   HTML and JSON, including `checked_at`, `valid_until`, and explicit unknown
   states. Never fetch a provider URL supplied by the request.
3. Add a verifier adapter using the existing Rust protocol verification rules
   for the signed revision and Kernel artifacts. Until it runs successfully in
   the deployed Worker, label its output `not_checked`; field consistency
   alone is not signature verification. Test invalid signatures and hashes
   against the same fixtures as the native requester.
4. Render service-specific HTML from that view model before returning the
   response. Escape all publisher text; emit the canonical and alternate
   links, service-specific social metadata, and a conservative `noindex`.
   Render an informative unavailable page when the live provider cannot be
   checked; do not reuse an old revision as current.
5. Only after the new route is live and externally checked, change
   `froglet-publish-engine/src/lib.rs` to return the new canonical share URL.
   Keep the old query-link route as a compatibility entrypoint. Align the
   recipient prompt, agent task metadata, and publishing instructions with
   the new URL, then test a fresh native free invocation and Receipt.

Targeted tests cover route parsing, `GET`/`HEAD`, content type, size and time
bounds, redirects, HTML escaping, JSON/HTML parity, exact active revision and
pricing, missing or invalid evidence, provider sleep, stale marketplace lease,
and old-link compatibility. The Worker preview must be exercised because Astro
preview alone does not run its dynamic routes. After deployment, fetch raw HTML
without JavaScript, inspect desktop and mobile renderings, test actual social
unfurls where possible, and give the URL alone to unfamiliar agents. The
existing public release gates remain separate from acceptance of this slice.

Later slices add approval-bound editable presentation and search indexing,
provider-owned canonical links, independent and private indexes, public
historical-revision access or tombstones where useful, and qualified paid
adapters. Those capabilities must not be inferred from the first slice.

## URL-capable native caller (source checkout)

The native MCP tool advertises `inspect_service` and `service_url`. Check
`tools/list` before using these fields with an installed release. Inspection
accepts both the canonical `/s/{provider}/{service}` and legacy query-string
link. It resolves the first-party relay from the identity locally, fetches
bounded public metadata without credentials, and verifies the existing
Publication Revision, Offer, and Descriptor. Inspection does not create a deal
or require a local daemon. It returns signed input/output schemas, limits,
price, example input, provider identity, and evidence references.

```json
{"action":"inspect_service","service_url":"https://froglet.dev/s/PROVIDER/SERVICE","response_format":"compact"}
```

For a user-requested execution, use `invoke_service` with that same
`service_url` and `input`. Do not combine it with service/provider overrides.
The caller defaults to a zero price ceiling and uses the existing requester
and receipt verification path. `froglet-node invoke '<share-url>' '<json-input>' --json`
is the CLI equivalent. In source builds, an explicit `--max-price-sats N`
(or MCP `max_price_sats`) permits Lightning calls up to that ceiling. The
requester still requires its own wallet and cumulative spend budget. This is
not qualification of a production paid service: verify the deployed rail and
a real settled receipt before advertising it. Stripe uses the runtime
payment-token API; the native share-link price cap is denominated in sats.

`response_format: "compact"` is opt-in for inspection and invocation: complete
results are in MCP `structuredContent`, with a short text notice instead of a
second copy. The default `full` response preserves JSON text for older clients.
Publication consent and errors always retain their full text.

The canonical website route also negotiates `Accept: application/json` or
`Accept: text/markdown` / `text/plain`; ordinary browsers still get HTML.
Explicit `/manifest.json` and `/agent.md` links remain supported. Public read
responses include CORS and `Vary: Accept`. This improves interoperability but
does not assert qualification of every external crawler or assistant.

## Marketplace discovery and QR sharing

Service cards show a readable name, provider description when available,
price, time of the last marketplace provider check, and a link to the published
service. `Recently checked` requires an unexpired healthy reachability lease;
stale snapshots cannot retain that label. Listed infrastructure offers with no
public service record link to the usage documentation. Unknown or expired
availability is displayed explicitly. Counts describe the loaded sample.

The service page exposes its advertised tables and fields without implying
that summary counts provide access to underlying records. Scientific meaning
and provenance remain publisher-supplied; Froglet does not invent them.

`GET /s/{provider}/{service}/qr.svg` generates a QR code for the canonical
share URL using a bundled encoder. `?download=1` adds an SVG attachment header.
This route requires no provider execution or third-party QR service and works
while the provider is offline. Both QR and human-readable link lead to the
same service. No credentials or payment authorization are included.
