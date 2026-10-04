# Agent interoperability: MCP and A2A

Status: selected scope implemented. Immutable `v0.4.7-beta.1` distribution and
a separate deterministic localhost replay of its downloaded Mac binary were
verified on 4 October 2026; [the release record](RELEASE.md#versioning) names its
exact source and binary. A separate released-Mac native-MCP-to-Fly proof passed
six bounded free cases, restart persistence and offline task recovery; its
isolated test host was removed. This does not newly qualify remote A2A,
production/load, fresh agent hosts, clean OS setup or paid operation.
The local qualification on 1 October and terminal-read
regression qualification on 3 October below remain historical evidence. The fresh generated-program sessions remain only partially qualified;
their exact boundary is recorded below.
The optional A2A profile is limited to configured counterparties and the required
Froglet extension. Production payment rails and full A2A TCK certification are
not qualified by these checks.

The [shared functionality matrix source](../docs-site/src/data/support-matrix.ts)
feeds both the agent guide and developer page. The 3 October website publication
checked six public pages against the prepared build byte for byte, including
the guide and developer page; that evidence is retained at
`_tmp/launch-prep-2026-10-02/site-publication-2026-10-03-http.json`.
The subsequent terminal-recovery row change remains local until republished.
The matrix separates source
implementation, local candidate evidence, experimental/disabled paths, plans
and public deployment qualification.

## Terminal task-read correction — 3 October 2026

Authenticated native `get_task` now reads a saved terminal operation without
contacting Bob. The runtime revalidates the canonical Quote/Deal/Receipt chain,
provider identity, result hash and status bound to the Receipt before returning
the record. This path submits no work and performs no payment or ledger update.
Unsigned or unresolved records still need provider synchronization; invalid
cached signed evidence fails closed. Existing payment mutation paths are
unchanged. Historical requester identity remains bound to its signed Deal after
a genuine local custody rotation.

The current-source local Rust checks passed: `cli_mcp_compute` 12/12,
`cli_invoke` 9/9 and `runtime_routes` 22/22, with warnings denied. They exercised
success and execution-limit failure after requester restart/provider shutdown,
selected-data recovery, signed rejected/canceled fixtures, key rotation,
authentication, unknown tasks, tampering, a provider-signed foreign requester
Receipt and missing-Receipt/pending failures. Logs are retained at
`_tmp/launch-prep-2026-10-02/runtime/terminal-cache-suites.log`.
The public deterministic replay now requires every saved Alice terminal task,
including selected data, to return its original verified evidence offline.
A fresh optimized native candidate then passed independent replay qualification:
seven signed chains, four frozen arithmetic cases and all six saved Alice
terminal task reads with Bob stopped and Alice restarted. Its binary SHA-256 is
`1e6621c461d7f368328467ad15f7e1cc6280997bf438238ed62974c0a96a8c8f`;
the independent report is retained at
`_tmp/launch-prep-2026-10-02/runtime/terminal-cache-2026-10-03/independent-verification-final.json`
(SHA-256 `1ed79f754c94f4358c742b0af7bfb2c7a56de6d8953aa77906fc5099f50966f6`).
The original failed agent qualification and historical HTTP 502 evidence were
rechecked unchanged. Package/release gates and publication of the changed
website copy remain separate checks; the older binary/replay below cannot
qualify these changed runtime inputs.

## Fresh generated-program checkpoint — 1 October 2026

Separate actual Bob and Alice Codex sessions selected and published synthetic
assay rows, then generated/compiled a new bounded batch-summary Wasm program,
executed it remotely and recovered the result. Independent checks matched the
original plus three unseen inputs and verified the selection, publication
signature and snapshot binding. Complete execution chains were not exported:
the recorder attempted a requester-scoped fetch for another requester and failed;
cleanup then removed the private node database. Preserve this as a failed
qualification attempt, not a complete signed-chain pass.

Persistent Codex configuration also changed. Temporary project-trust registration
is an inference, not a proven complete cause. Automatic approval review refused
the repeated Codex-session action; the corrected guarded repeat still requires
explicit approval. No repeat or workaround is claimed here.

A separate deterministic replay of the retained program passed independent
offline verification of seven new execution chains: six successful Deals,
including the original and three unseen oracle cases, plus one deliberate
execution-limit failure. Selection/privacy, signed publication, workload/result
bindings and restart recovery were checked. Two earlier recorder replay runs
failed and are retained. This replay starts no Codex session and does not need
user-agent configuration; it cannot repair the missing evidence or configuration
condition in the earlier actual-agent sessions.

After Bob stopped and Alice's requester restarted, exact `run_compute` and
`invoke_service` retries recovered cached terminal results/receipts without new
Deals. At that checkpoint, native `get_task` instead attempted provider refresh
and returned HTTP 502. This historical failure is preserved; it does not describe
the corrected 3 October current-source read path. The independent report is retained
at `_tmp/local-finalization-2026-10-01/replay/independent-verification.json`
(SHA-256 `5df50cd4cb6b7a21b9791cc19d7fbe7c0048b7595a2b36337e462b57fd072f33`).

## Decision

Make MCP the normal agent-host interface to a local Froglet Node. Add A2A as an
optional provider interface for compatible remote agents. Keep the Kernel,
Artifact Chain, execution environment and settlement drivers in Froglet.

Prioritize native MCP support for requester-supplied bounded Wasm computation.
Then implement one A2A task flow over the same deal and execution machinery.
Do not require both interfaces for every interaction.

The alternative is MCP-only for the first release and conference demonstration.
That still supports remote computation through the existing Froglet HTTP flow;
it postpones the interoperability proof with an independent A2A client.

## Why this scope

MCP lets an agent host inspect, prepare, publish and invoke Froglet resources.
A2A lets independently implemented agents discover a provider and exchange
tasks. Neither specification supplies Froglet's execution environment.
Integrating them reduces the need for a custom agent interface; it does not
establish that Froglet is uniquely necessary or invent distributed computation.

Before this change, the JavaScript MCP server exposed `run_compute`, `get_task`
and `wait_task`, while the native bridge used by the normal installation exposed
service invocation but not `run_compute`. Native invocation already defaulted
to free-only, preserved idempotency and reported receipt verification. The
JavaScript compute path needed matching per-call price and retry controls.

| Choice | Benefit | Cost / limit | Decision |
| --- | --- | --- | --- |
| Native MCP computation | One agent tool reaches the existing requester, remote executor and verifier | Host still needs a Node, credentials and spending policy | Primary supported path |
| Optional A2A adapter | Independent A2A SDKs can discover and track Froglet jobs | Required custom extension, configured counterparties, another version/auth/state mapping to maintain | Include, disabled by default |
| Replace the Kernel with A2A/payment messages | Would reduce Froglet-specific wire objects | Would discard existing signed workload and settlement bindings without a demonstrated replacement | Do not do this |

This is an interoperability and usability decision. It does not prove novel
distributed computation or a unique need for Froglet. The useful engineering
claim is narrower: requester-supplied programs can use the existing bounded
executor and transaction rules through familiar agent interfaces.

Sources: [native MCP](../src/cli/mcp.rs),
[JavaScript compute client](../integrations/shared/froglet-lib/froglet-client.js),
[runtime](RUNTIME.md), [service binding](SERVICE_BINDING.md).

## Responsibilities

| Surface | Responsibility | Boundary |
| --- | --- | --- |
| MCP | Private agent-host access to the operator/requester Node | Publication, credentials and spending authority stay local |
| A2A | Optional public provider discovery and task interface | Exposes advertised computation/services, never arbitrary operator actions |
| Froglet runtime | Quote verification, requester signing, deal admission, execution, recovery and result acceptance | One source of economic and execution state |
| Settlement driver | Selected method's authorization and settlement | No second payment state machine in the interface adapters |

MCP can reach a remote provider through the local requester runtime. It is not
limited to local computation. An A2A request does not itself authorize spending,
publication or signing for the requester.

## First implementation

### MCP

1. Add native `run_compute` for bounded inline Wasm and explicit input. Reuse
   the requester runtime and existing workload verification; do not create a
   second signing or execution path.
2. Expose task status/recovery for unfinished requests and carry an explicit
   idempotency key. Reusing a key with changed work must fail. A caller timeout
   must return a recoverable reference rather than silently starting new work.
3. Default per-call price to zero. Paid calls require an explicit ceiling and
   the existing configured cumulative budget and settlement authority.
4. Bring the JavaScript `run_compute` and `invoke_service` request controls into
   agreement with those boundaries. Return structured status, result hash and
   evidence/verification details without claiming checks that were not run.
5. Retain existing publication consent and lifecycle behavior.

### A2A

Implement a small HTTP+JSON/polling interface inside the Froglet Node. Use a
pinned A2A version and verify that binding with an independently implemented
client. Do not advertise streaming, push notifications or conversational
planning in this first version.

The Agent Card advertises only enabled, admitted skills and a versioned Froglet
deal extension. The extension is required for executing a signed Froglet deal;
an ordinary A2A client must not be described as automatically able to purchase.
Discovery can remain readable without activating the execution extension.

The flow is:

1. A compatible requester obtains the provider's advertised capability and
   signed Descriptor/Offer references.
2. It requests a Quote for an exact bounded workload.
3. Its local runtime checks provider identity, workload, price, limits and
   spending authority, then signs the accepted Deal.
4. A2A carries the unchanged Quote, Deal and workload to existing admission.
5. Task polling projects the existing execution/settlement state. Result
   artifacts carry result/evidence references and the terminal Receipt when it
   exists. A ready result is not represented as terminal settlement.

The requester-side transport adapter is part of this scope: a provider-only
Agent Card and HTTP wrapper are insufficient for the MCP-to-A2A demonstration.
Transport selection belongs in application configuration, outside signed Kernel
payloads. Existing Froglet HTTP invocation remains available. The node loads an
explicit private configuration file through `FROGLET_A2A_CONFIG_PATH`:

- `clients` maps separate A2A bearer credentials to requester signing keys and
  allowed Offer hashes;
- `providers` maps exact provider URLs to the requester-side A2A credential.

The file must be operator-owned and private. Missing configuration leaves A2A
disabled. Credentials are never the node's MCP/runtime administrative token and
must not appear in public metadata or status output. Exact provider URL matching
does not replace the existing requester egress/DNS checks.

On Unix, the credential file must be a regular file owned by the Node user,
with mode `0600`; symbolic links and files larger than 64 KiB are refused.
Configuration supports at most 128 clients and 128 provider origins, and each
client has 1–128 exact lowercase Offer hashes. Generate a distinct random bearer
token for each client (32–256 ASCII letters, digits, `-` or `_`). Obtain active
signed Offer hashes from the existing provider catalog before enabling A2A.
Restart with the selected allowlist; changing an Offer requires an explicit
configuration update.

Configuration shape (placeholders must be replaced; this is not an executable
credential example):

```json
{
  "clients": [{
    "requester_id": "<Alice's lowercase public key>",
    "token": "<separate random A2A credential>",
    "offer_hashes": ["<approved signed Offer hash>"]
  }],
  "providers": [{
    "provider_url": "https://bob.example",
    "token": "<credential Bob assigned to Alice>",
    "allow_loopback": false
  }]
}
```

Bob needs the `clients` entry; Alice's requester Node needs the corresponding
`providers` entry. A dual-role Node may configure both. Setting
`FROGLET_A2A_CONFIG_PATH` affects the Node process, not the MCP tool arguments.

The [reviewed counterparty setup helper](A2A_COUNTERPARTY_SETUP.md) now prepares
these private files without hand-editing credential JSON. An exact review-plan
fingerprint gates creation of a new configuration directory, preserving unrelated
counterparties; activation still requires an explicit Node restart. Existing
provider-wide admission invitations remain separate from the exact A2A scope,
and invitation forwarding continues to require HTTPS.

The initial release is for operator-configured counterparties. Each provider
credential binds a requester public key and an exact Offer allowlist. It does
not provide anonymous federation, general spending delegation or a universal
agent purchasing flow. A provider with no configured clients exposes no A2A
execution interface.

Requester provider entries may set `allow_loopback: true` for an explicitly
configured literal `127.0.0.1` or `[::1]` origin in local qualification. This
exception is bound to the private operator configuration and its credential;
request data cannot activate it. Hostnames, private-network ranges and redirects
do not inherit this exception. HTTPS providers retain the existing egress and
DNS validation.

Use a deterministic task reference tied to the signed Deal and resolve it from
durable node state. Avoid a separate sidecar-only task/deal mapping: current
provider admission checks Quote expiry before exact-Deal replay lookup, so a
lost sidecar mapping cannot safely assume replay will recover an old deal ID.

The first wire profile targets A2A 1.0 HTTP+JSON under `/a2a/v1`. It declares
`https://froglet.dev/a2a/bounded-deal/v1` as its required extension identifier.
This is a Froglet extension, not a claim of registration as an A2A standard.
Requests activate the extension and use the independently configured bearer
credential. Agent Card discovery is at `/.well-known/agent-card.json`.

A user Message has one JSON data Part containing `schema: froglet.a2a.v1`, a
typed `operation`, and `payload`, which is an exact JSON string. The operations
are `catalog`, `quote`, `submit`, `accept`, and `invoice_bundle`. Signed artifacts
and workload values remain inside that string: the official SDK's protobuf
Struct representation converts numeric data to floating point and must not be
allowed to rewrite signed values or narrow unsigned integer ranges.

Catalog and Quote return Messages and do not create execution Tasks. Signed
Deal submission creates a Task resolved by the Deal hash. Subsequent reads use
the task API; acceptance/invoice operations carry the task reference. Task
artifacts use the same exact-JSON envelope for the projected Deal record.
Request history must not retain or expose acceptance preimages or credentials.
Initial submission omits `taskId`; a supplied task reference must already exist,
as required by A2A. A deterministic Deal hash identifies the resulting Task,
but does not grant permission to read it.

A terminal Task does not accept a new conversational continuation. Requester
acceptance retries first read and validate the existing Task; a succeeded
terminal result must match the caller's normalized `expected_result_hash`.
Lightning InvoiceBundle reads use the exact JSON extension projection at
`task.metadata.froglet.invoiceBundle`, including after terminal settlement.
The requester verifies that bundle against the signed Quote and Deal before
returning it through the existing runtime API. No payment preimage is included.

State projection follows economic evidence:

| Froglet state/evidence | A2A projection |
| --- | --- |
| Accepted Deal | Submitted |
| Payment pending | Input required, with payment detail |
| Executing | Working |
| Lightning result ready, awaiting acceptance | Input required |
| Stripe result ready or settlement pending | Working |
| Valid matching succeeded Receipt | Completed |
| Valid matching rejected/failed/canceled Receipt | Corresponding terminal state |
| Unadmitted local failure without Receipt | Failed, explicitly without economic terminal evidence |

The signed Receipt controls terminal economic meaning. Local `failed` alone
cannot decide whether the Receipt says canceled. Result availability alone
cannot establish completion. Noncancelable Tasks return the defined A2A error.

### Required boundaries

- Preserve canonical Froglet artifact bytes, signatures and validation. No
  Kernel schema, hashing or state-transition migration is proposed.
- Keep requester and provider signing authority separate. An adapter must not
  manufacture requester acceptance using the provider's key.
- Authenticate task reads and scope them to the authorized requester; a task
  identifier alone must not grant access to private results or evidence.
- Never forward a public A2A `action` to the private MCP operator dispatcher or
  expose provider/runtime administrative tokens.
- Persist submission identity before any external side effect and recover
  existing work after response loss/restart. Do not promise universal
  exactly-once execution.
- A durable local `submission_pending` record is not evidence of provider
  admission. If the provider never exposes the corresponding Task after an
  ambiguous delivery or payment-token mint, the first implementation retains
  that unresolved intent and its conservative spend hold. It does not
  automatically mint or submit again. Recovery of an accepted Task is tested
  separately from eventual delivery of an unaccepted request.
- There is no current public provider Deal-cancel operation. Return the
  standard noncancelable/unsupported outcome where appropriate. Do not mark an
  A2A task canceled while Froglet work or payment continues.
- Preserve provider admission, invitation, capacity and resource controls.
- Move or reuse operation-level admission checks: the existing middleware
  classifies provider paths, so adding a new A2A URL must not bypass pause,
  private/invitation or storage controls. Recovery/status reads must remain
  distinct from new work and retain a separate recovery rate-limit budget.
- This A2A profile uses direct ingress. The existing relay allowlist is not
  widened; Agent Card discovery refuses a relay-only advertised origin. Relay
  A2A support is outside this change and must not expose operator routes when
  added later.
- Select one existing settlement driver. The kernel recognizes x402, but the
  current paid Deal runtime does not admit it; the separate x402 endpoint is
  not interchangeable with the signed Deal flow. Do not add A2A-x402/AP2
  translation or a new payment rail in this first change.

## Verification and release gates

1. Establish relevant test baselines before edits; preserve unrelated working
   tree changes.
2. Test native MCP schema/dispatch plus the real stdio flow. Run requester-
   supplied Wasm through two actual Nodes and verify the result/evidence.
3. Test an independent A2A client against Agent Card discovery, version and
   extension negotiation, submission, polling and errors. Do not infer
   compatibility from two clients sharing the same custom implementation.
4. Exercise changed workload, wrong requester, invalid signatures, expired
   Quote, missing extension, unauthorized task reads and price/budget refusal.
   Rejected operations must not execute or initiate payment.
5. Exercise duplicate submissions, lost responses and restart across admission,
   execution and settlement boundaries. Verify durable recovery and the cases
   that must remain unresolved rather than rerun uncertain work.
6. Demonstrate a bounded successful computation and a deliberately nonterminating
   workload stopped by the executor's fuel/deadline boundary.
7. Use the existing selected rail's test mode for payment-state integration.
   Label simulated settlement explicitly; production payment qualification is
   a separate gate and is not inferred from mocks.
8. Run affected Rust, shared integration and MCP checks, then the repository's
   required validation matrix. Report unrelated baseline failures separately.

## Presentation after implementation

Update the animated talk only after the implemented flow passes its relevant
checks and the live demonstration is inspected.

The architecture visual should distinguish MCP agent-host access, optional A2A
peer access, the Froglet execution/deal runtime and the selected payment driver.
Use Alice supplying a short-lived program and Bob providing bounded execution.
Show successful work, enforced termination and recovery evidence.

Describe Froglet as bounded remote execution with explicit transaction rules.
Explain which interfaces are implemented, which are optional, which payment
mode is demonstrated and which guarantees are not established. Avoid claiming
novelty from A2A integration, signatures or payments alone, and avoid presenting
the runtime as a complete cluster scheduler or distributed-training platform.

## Primary protocol references

- [A2A specification](https://a2a-protocol.org/latest/specification/)
- [A2A extensions](https://a2a-protocol.org/latest/topics/extensions/)
- [MCP introduction](https://modelcontextprotocol.io/docs/getting-started/intro)

## Implementation file inventory

This inventory identifies this task's changes; other pre-existing working-tree
changes are not part of this interoperability work.

| Area | Files |
| --- | --- |
| Native MCP computation and recovery | `src/cli/mcp.rs`, `src/cli/invoke.rs`, `tests/cli_mcp_compute.rs` |
| Optional A2A configuration, provider and requester transport | `src/a2a_config.rs`, `src/api/a2a.rs`, `src/api/remote_client.rs`, `src/api/mod.rs`, `src/config.rs`, `src/lib.rs` |
| A2A boundary and recovery regressions | `src/api/a2a_tests.rs`, `src/api/a2a_recovery_tests.rs`, `src/api/remote_client_tests.rs` |
| JavaScript MCP/OpenClaw control and evidence parity | `integrations/shared/froglet-lib/froglet-client.js`, `tool-dispatch.js`, `summarize.js`; `integrations/mcp/froglet/lib/tools.js`, `test/execution-controls.test.mjs`; `integrations/openclaw/froglet/lib/froglet-tool.js` |
| Additive `NodeConfig.a2a` construction updates | `src/state.rs`, `src/identity.rs`, `src/identity_custody.rs`, `src/settlement/stripe.rs`, `src/settlement/x402.rs`; `tests/runtime_routes.rs`, `builtin_service_dispatch.rs`, `payments_and_discovery.rs`, `relay_tunnel.rs`, `lnd_rest_settlement.rs`, `full_deal_lifecycle.rs`, `cli_invoke.rs` |
| Repeatable local operator setup and validation | `examples/a2a_compute_demo.py`, `examples/README.md`, `scripts/strict_checks.sh` |
| Operator documentation | This document; `README.md`; `integrations/mcp/froglet/README.md` |
| Local animated talk, intentionally outside tracked source | `_tmp/froglet-animated-2026-09-30/src/app.js`, `src/style.css`, `src/build.py`; generated `output/Froglet-Animated-Talk.html`, `output/START-HERE.txt`, `qa/content.json`, `qa/build-manifest.json` |

Kernel specification and frozen conformance vectors are unchanged. No new
payment driver, general scheduler or arbitrary private-data capability is added.

## Making the implemented profile usable

Implemented source, a running installation and a qualified production deployment
are different deliverables. An older installed release does not acquire these
actions merely because the documentation describes them.

The [local MCP/A2A compute demo](../examples/README.md#local-mcp-and-a2a-compute-demo)
provides the source-checkout path: it builds the Node and the existing Rust Wasm
adder, starts isolated Alice and Bob processes, obtains their actual identities
and Offer scope, writes private A2A configuration, and invokes native MCP. Its
keep-running mode provides host configuration without editing the user's agent
settings. This is the supported free local workflow, not a wallet setup.

The remaining deployment gates are concrete:

1. Package a release containing the tested actions and adapter; verify the
   packaged binary and install path. Use the existing release gate and immutable
   release tooling rather than implying every currently installed binary supports
   the new source.
2. Connect the chosen agent host to the running requester Node and exercise
   `status`, `run_compute` and read-only `get_task` through that host. A stdio
   protocol test does not establish the configuration of a particular IDE.
3. Deploy Bob behind direct HTTPS, configure the actual advertised origin, and
   authorize Alice's signing identity and exact Offer scopes. The configured
   loopback exception is for local qualification only.
4. Select and configure the payment rail, wallet and cumulative spending policy,
   then qualify execution, acceptance, settlement and restart on that deployment.
   Native MCP paid computation currently follows its Lightning wallet path;
   caller-supplied Stripe payment tokens are not arguments to this action.
5. Run the independent client and applicable conformance checks against the
   deployed endpoint. Full TCK certification requires its own evidence; it is
   not inferred from the passing SDK workflow.

Open participation, streaming and relay ingress are additional product scope.
They are not prerequisites for a configured-counterparty polling job. Automatic
resubmission of an ambiguous unadmitted intent is also not an implied fix: it
needs an explicit delivery/payment-recovery design before implementation.

## Qualification results — 1 October 2026

The final `./scripts/strict_checks.sh` run exited successfully:

- Rust: 986 passed, zero failed, eight ignored; compiler warnings denied.
- Formatting, full workspace/all-targets Clippy, dependency-light protocol
  builds, installed wasm32 targets and frozen Kernel vectors passed.
- JavaScript: 199 passed (57 OpenClaw, 113 MCP, 29 shared client), no skips.
- Strict Python runtime subset: 145 passed; standalone verifier: 226 passed;
  conformance reproduction: 90 passed.
- Shell syntax and the configured Gitleaks publication gate passed.

The separate full Python runtime discovery completed with 169 passed and three
optional environment-dependent tests skipped (production Voltage Lightning,
Docker LND regtest and outbound Tor bootstrap). Eight ignored Rust tests are
not claimed as passing. Linux sandbox and live payment qualification remain
separate gates.

Independent official `a2a-sdk==1.0.0` qualification used an explicitly rebuilt
binary and real free localhost Nodes: 27 workflow checks passed, plus two
separately recorded malformed-request probes. Binary/source fingerprints stayed
unchanged during the run. The SDK verified discovery, version/extension errors,
caller isolation, deterministic replay, task timestamps/list pagination,
unsupported operations, successful `{"a":6,"b":7}` computation returning `42`,
fuel exhaustion with signed `execution_limit_exceeded`, independently verified
chains and actual provider restart recovery. The arithmetic fixture supports
two single-digit factors; it is not a scientific workload demonstration.

Native MCP integration tests exercised the real stdio binary, Alice's requester
runtime and Bob's independent listener/executor through configured A2A. Bob's
ordinary provider paths were refused during that test, so transport selection
was observed rather than inferred. Exact retry/recovery retained one provider Deal.

Recovery tests distinguish three different boundaries:

- Free submission: an actually truncated successful provider response, followed
  by requester restart and signed Task recovery.
- Lightning acceptance: an actually truncated acceptance response, followed by
  verified GET recovery and terminal GET-only acceptance/InvoiceBundle reads;
  settlement backend is mocked.
- Stripe accounting: the test deliberately models a persisted paid
  `external_pending` hold, then verifies reconciliation without another
  authorization, capture or execution. This is not an actual paid crash test.

The earlier animated talk was rebuilt from sanitized SDK evidence. Live
browser inspection checked all 36 screens for out-of-stage text, the three
verifier states, moving architecture packets, reduced-motion/blackout pauses,
presenter navigation/timer/reload and isolation from another talk session. It
contains 27 minutes of planned speaking time and eight minutes for questions.
Runtime screens are labeled recorded free local runs; only fixture signature
verification executes inside the presentation. The prior PDF is explicitly
labeled a previous version rather than a current backup.

Local evidence (not release artifacts):

- `/private/tmp/froglet-interop-strict-final-20261001-rerun.log`
- `/private/tmp/froglet-interop-python-full-20261001.log`
- `_tmp/a2a-interop/run-summary.json` and `official-sdk-live.json`
- `_tmp/froglet-animated-2026-09-30/qa/build-manifest.json`

Independent source review reported **No findings** in the tested profile.
Remaining limits are the configured-counterparty/required-extension boundary,
direct ingress without relay A2A, unresolved ambiguous unadmitted intents,
mock-only paid lifecycle tests, and absence of full TCK/production qualification.

The public local runner also passed real-process checks with the Rust adder:
`{"a":6,"b":7}` returned `{"sum":13,"product":42}` with verified evidence.
Its default flow now checks the generated MCP host command and environment,
and is included in strict checks when the Wasm target is installed. Separate
serve-mode qualification used deliberately stale URLs, authentication and proxy
variables; the generated configuration reached the correct Nodes, a new agent
call created its own Deal, and read-only recovery used that returned Deal ID.
Interrupt cleanup removed private state and closed both listeners. A simulated
orchestration failure after startup also reaped child processes and retained
private diagnostic logs. Actual IDE/LLM host installation remains a deployment
step rather than a claim made by these stdio tests.

The operator-path changes then passed a fresh full `./scripts/strict_checks.sh`
run, including the new real-process demo gate: 986 Rust tests passed (eight
ignored), 199 JavaScript tests passed, 145 strict runtime Python tests passed,
226 standalone verifier tests passed, and all 90 conformance fixtures reproduced.
Formatting, Clippy, shell syntax, Wasm builds and the publication scan also
passed. The demo ran with Python warnings treated as errors, and the log contains
no runtime/resource warnings. Evidence:
`/private/tmp/froglet-local-operator-strict-20261001.log`.

Independent serve-mode qualification exercised the generated command and
environment against deliberately hostile inherited configuration, then made a
new agent call, recovered its actual returned Deal ID, and observed both
listeners close after interrupt. Independent source review reported **No
findings** in this final local operator path. These checks establish the local
free workflow; they do not establish installation in a particular agent host
or the deployment gates above.

## Terminology workflow qualification - 1 October 2026

The source-checkout runner now supports
`python3 examples/a2a_compute_demo.py --scenario ontology --serve`. Bob prepares
and publishes a selected synthetic terminology snapshot through native MCP.
Only `source` and `target` are selected; the illustrative private
`curator_note` column is excluded from the prepared snapshot and Alice's result.
Alice retrieves the actual table through native MCP, then supplies a disposable
Wasm checker and explicit JSON input through her separate requester runtime and
configured A2A transport. Bob's publication credentials stay in Bob's context.

The checker uses exact, case-sensitive strings under Alice's chosen one-target
rule. Duplicate identical rows are consistent. Multiple distinct targets are
reported as policy ambiguity, including conflicts elsewhere in the selected
table; missing observed terms are reported separately. It does not normalize
labels, select a scientific interpretation or solve ontology alignment.

Four real compute cases cover an unambiguous control, multiple targets, a
missing term and the combined case. The combined result has two uniquely
mapped observed terms, one source with two targets, and one unmapped term. All
four outputs match both a separate Python calculation and hardcoded expected
results. The full workflow creates six provider Deals: table retrieval, four
audits and a deliberately nonterminating job. Exact retries create no additional
work; changed input is refused; a provider restart preserves the data and
compute result/Receipt hashes. The nonterminating job produces a signed
`execution_limit_exceeded` failure. Its generic error does not establish which
individual execution limit fired.

Independent verification checked all six full signed chains, publication
revision signatures, selected snapshot bytes, exact reconstructed workload
hashes, successful result hashes and the 80,812-byte no-import Wasm module's
digest. A changed Receipt without a new signature was rejected. A separate
process using the generated MCP host configuration also submitted a fresh job;
interrupt cleanup stopped both nodes and removed their temporary private state.
This is a real MCP configuration handoff, not an installed LLM host or
autonomously generated analysis.

The final `./scripts/strict_checks.sh` run exited successfully after adding the
ontology gates: 986 core Rust tests passed (eight ignored), seven checker tests
passed, eight Python oracle tests passed, both actual local demos passed, 199
JavaScript integration tests passed, 145 strict runtime Python tests passed,
226 standalone verifier tests passed, and all 90 conformance checks reproduced.
Formatting, Clippy, Wasm builds and the publication scan passed. The first
attempt could not open local sockets in the execution sandbox. A later run
identified a demo assertion expecting compute admission's error wording for a
data lookup; the lookup actually returned `409 invocation_conflict`. The
assertion was corrected to that actual refusal and retained its no-new-work
check; no runtime guard or Kernel change was needed.

Local evidence, not release artifacts:

- `_tmp/ontology-demo-2026-10-01/report.json`
- `_tmp/ontology-demo-2026-10-01/offline-chain-verification.json`
- `_tmp/ontology-demo-2026-10-01/tampered-chain-refusal.json`
- `_tmp/ontology-demo-2026-10-01/host-agent-call.json`
- `/private/tmp/froglet-ontology-strict-20261001-final.log`
- `/private/tmp/froglet-ontology-independent-review.json`

Independent source/evidence review reported **No findings**. Public HTTPS,
capacity, real payments, actual agent-host installation and release packaging
remain separate deployment gates. This example demonstrates selected-data
exchange and a bounded consistency audit with illustrative data; it does not
establish scientific truth or a measured operating-cost advantage.

The current animated talk now uses that frozen terminology run on screens
20-24. The 36-screen HTML retains the separate live offline verifier on screens
26-28 and has planned pacing of 27 minutes plus eight minutes for questions.
Live Chrome review inspected all 36 rendered screens, checked text bounds and
loaded artwork, and exercised original, changed and restored verifier states.
The current 36-page PDF is a static adaptation of the same content and artwork;
all pages were rendered and reviewed, with text completeness, page bounds and
source links checked. It is not a video or a pixel-identical browser export.

The offline review package contains the self-contained HTML, current PDF,
rehearsal checklist, synthetic selected table and sanitized recorded evidence.
It contains no node credentials or private state, and is not an installer for
the runtime. The HTML and PDF record the actual free local results; only the
embedded conformance verifier executes during that presentation. Physical
projector checks and the speaker's full timed rehearsal remain unverified.
Browser automation rejected direct `file://` navigation. The same self-contained
HTML and verifier were checked over localhost; opening the file directly remains
a manual rehearsal check. True native fullscreen was not observed in the
automation surface, which showed the presentation's documented fallback.
