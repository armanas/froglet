# Agent publishing qualification and first-user trial kit

**Current status, 4 October 2026:** immutable `v0.4.7-beta.1` distribution and a
separate deterministic replay of its downloaded Mac binary are verified. See
[the released artifact and exact scope](RELEASE.md#versioning). A separate native-MCP
Mac-to-Fly qualification passed six bounded free cases, provider restart
persistence and offline task recovery; the isolated host was then removed.
Available hosted/production compute, load, fresh agent/OS qualification and the
independent owner/recipient usefulness trials remain open; publication does not make the original failed agent attempt
pass. The earlier observations below retain their dates and scope.

**Historical checkpoint, 1 October 2026:** work has resumed on free native MCP
selected-data sharing and bounded Wasm execution, with optional A2A between
configured counterparties. The [two-node examples](AGENT_INTEROPERABILITY_PLAN.md#terminology-workflow-qualification---1-october-2026)
have recorded local qualification. The final candidate also passed actual Codex
CLI and extracted-bundle qualification on the current Mac. Clean-machine agent
qualification, published release, public compute HTTPS/capacity, real-data-owner
value, and five-participant
qualification remain separate gates. Payments and GPU expansion remain deferred.

The public documentation website was restored on 1 October at Worker version
`7dd4a560-b18d-478f-a7a1-bf1b6cd73eb2`; seven checked routes returned HTTP 200 and
matched built bytes. Local evidence is
`_tmp/improvement-2026-10-01/site-http-qualification.json` (ignored, not a release
artifact). The hosted compute trial remains unavailable, GCP authentication
blocks new public compute deployment, and the source candidate remains
unpublished. Earlier deployment and agent results below retain their dates and
scope. The trial kit at the end is prepared but has no participants or
observations; no invitations were sent by this documentation pass.

The updated GO, Linux and standalone-demo descriptions were subsequently
published at Worker version `857f8336-ed78-48cb-8f08-959f0ff8bff6`. Both affected
routes and the homepage matched final build bytes; the new scenario rows were
inspected in the live browser. This website update does not close a compute,
installer, agent-host or participant gate.

The final local binary is pinned at SHA-256
`584e3887e9b12d9f433bef2a70487a6ebee65976cd2c146b603bd2b16d76b166`.
Codex CLI `0.154.0` completed four actual native MCP calls: status, table retrieval,
bounded computation and read-only recovery. The extracted demo also ran outside
the checkout; its setup helper needs Python 3.10+. This qualifies this Mac/account,
with unchanged user settings and checked result/Receipt evidence, rather than a
clean OS, public installer or first-time-human journey. Exact local evidence and
remaining release limits are recorded in
[Release](RELEASE.md#october-candidate-local-qualification).

The final pinned terminology/profile example retains six Deals, four oracle
cases and the same 2,000 ms budget. A public GO catalog adaptation was locally
published and retrieved in 100- and 40-row pages with two verified receipts.
These are source-backed exchange demonstrations. Scientific interpretation,
independent data-owner usefulness and voluntary reuse remain unmeasured; the
[optional profile](RESEARCH_PROFILE.md) checks explicit declarations only.

Local independent-requester capacity measurements completed tiny workflows
without errors through concurrency eight. At 16 and 32, six selected-data jobs
returned independently verified `capacity_exhausted` chains. One shared
requester still encounters about one identical requester/workload/terms Quote
per second; collisions are refused before Deal admission. Final provider and
requester counts match (248 independent, 83 shared), with zero pending
submissions. These observations do not establish public capacity, performance
superiority or reduced developer time, and they do not populate the human trial
ledger below.

Continued Linux arm64 runtime qualification passed in an isolated Docker
container from the same recorded production inputs: 625 library tests and nine
native MCP integration tests, with 13 library tests ignored. The terminology
scenario and its six signed chains also passed independent offline review.
This supplies Linux development/runtime evidence, not a clean machine, Linux
agent host, Python OS isolation or a participant observation. The installed
Claude Code CLI is logged out, so that actual host gate remains unrun.

## Historical native agent publishing evidence — 2026-09-26

At that checkpoint, the public publishing entry path was improved and deployed.
Three fresh Codex scenarios passed preparation through exact public consent on
this Mac. That did not qualify full publication/recipient execution, Claude Code,
or every LLM.

## Failure and changes

The reported `prepare-service` port conflict was reproduced with the old
`~/.local/bin/froglet-node`: it interpreted the command as a daemon start.
The running immutable v0.4.5 installation supports preparation. Stopping its
healthy daemon would not repair binary selection. The reported JSON export also
needed normalization into named row collections, and the proposed `query` /
nested `gt` operation is not supported by the built-in query handler. A custom
Python query simulation was not evidence of Froglet execution.

The public guide now directs agents to the existing native MCP tool, otherwise
the exact configured binary and environment. A new-project fallback discovers
the native user service through launchd/systemd records. It preserves identity
and data; it does not silently replace the old PATH binary or reconnect an agent.
The guide specifies mixed-JSON normalization, the actual query contract, real
local verification, exact daemon-generated consent, and failure recovery.

`/publish/agent.md` and `/install/agent.md` are generated from the human guides,
with absolute links and Markdown content types. The root `llms.txt` now routes
by user intent instead of embedding a contradictory hosted-trial prerequisite.
Both primary publishing prompts use the new entrypoint.

## Observed acceptance results

All runs used a fresh Codex CLI session with no earlier conversation or source
checkout. The synthetic catalog combined object metadata and row arrays, nested
stock values, leading-zero identifiers, and an explicitly excluded notes field.
The prompt requested a nested comparison without telling the agent how to
implement it. Installation configuration was supplied only in the first two
current-guide scenarios; the third discovered the installed service itself.

| Scenario | Result | Time |
| --- | --- | --- |
| Previous HTML guide, CLI configuration available (baseline after earlier corrections) | Passed | 178.2 s |
| New plain-text guide, native MCP attached | Passed | 152.0 s |
| New plain-text guide, CLI record but no MCP attachment | Passed | 184.5 s |
| New plain-text guide, no project installation record or MCP attachment | Passed | 226.8 s |

The validator independently checked the unchanged original, every selected
value, notes absence from the served snapshot, manifest data path, free price,
actual native-handler execution, and matching consent from the configured node.
All four snapshots had SHA-256
`2725be968258cb139691386603d2fdcff0d23cb8086d8bf2a79cb615d6d8065a`.
Provider identity and publication state remained unchanged. No public service
was created and no user service was restarted. The MCP trace records actual
`status`, `prepare_service`, and `marketplace_publish` calls; the latter returned
an approval plan. The first mixed-shape inspection failed and the agent recovered
by preserving the source and normalizing a copy.

Transcripts, exact prompts, raw summaries, and independent rechecks are retained
locally in `_tmp/native-agent-acceptance-20260926/` (ignored, not release assets).
This sample is too small to estimate a population-wide success rate.

## Deployment and checks

Final website deployment: Cloudflare Worker `froglet-docs`, version
`84ef3549-5260-4973-9eb8-2326f3411ee4`. Native binaries remain immutable v0.4.5.

- 154 website tests passed; production build passed.
- 29 Python acceptance-validator tests passed, including rejection of leaked
  notes, changed identifiers, and a manifest pointing at the original data.
- Live Markdown guides, `llms.txt`, and task manifest returned HTTP 200 with the
  expected content types and matched built assets byte for byte.
- Rendered homepage and publishing page displayed the updated prompts. The
  publishing page's Copy button reported success.
- `git diff --check` passed. Existing uncommitted work was retained; no commits.

## Open evidence and access gaps

- Claude Code is not signed in on this Mac (`claude auth status` reported
  `loggedIn: false`). Its independent test is still required. The app/model that
  produced the user's original report has not yet been identified.
- These tests stop at public consent. Actual approved publication, separate
  recipient execution, clean-machine installation, Linux, and human trials are
  separate gates. The user's HLA catalog was not published by these tests.
- Cloudflare returns HTTP 403 / error 1010 for Python's default User-Agent on
  the public docs. An honest `FrogletAcceptance/1.0` User-Agent returned 200;
  curl also worked. The guide documents this retry, but the underlying edge
  restriction remains. The current Wrangler OAuth credential cannot access
  zone settings/rulesets (HTTP 403); dashboard sign-in/settings access is needed
  to prepare a narrow public-read exception. No security rule was changed.
- A normalized source is what change detection tracks; edits to the original
  export require regenerating it. The guide states that limit explicitly.

## Exact files changed for this pass

- `docs-site/src/content/docs/learn/share-services.mdx`
- `docs-site/src/content/docs/learn/quickstart.mdx`
- `docs-site/src/pages/[journey]/agent.md.ts`
- `docs-site/src/pages/publish.astro`
- `docs-site/src/pages/index.astro`
- `docs-site/src/data/agent-metadata.ts`
- `docs-site/public/_headers`
- `docs-site/public/llms.txt`
- `docs-site/public/agent-tasks.json`
- `docs-site/src/scripts/__tests__/agent-guides.test.ts`
- `docs-site/src/scripts/__tests__/agent-experience.test.ts`
- `docs-site/src/scripts/__tests__/hosted-trial-copy.test.ts`
- `docs-site/README.md`
- `docs/llms/try.froglet.dev.txt`
- `docs/SHAREABLE_SERVICE_LINK.md` (deployed-status correction)
- `tests/llm_acceptance/run_native_publish.py`
- `tests/llm_acceptance/test_native_publish.py`
- `tests/llm_acceptance/README.md`
- `docs/AGENT_PUBLISH_ACCEPTANCE.md` (this evidence record)

## Local publication and invocation — 2026-09-27

A fresh Codex CLI session with native MCP attached completed preparation, local
publication and one free local invocation in 105.2 seconds. It used an isolated
loopback-only provider, synthetic JSON, invitation mode, zero payment budget,
and finite cumulative allowances. The agent made actual `prepare_service`,
`marketplace_publish` and `invoke_service` calls. Independent checks confirmed
that the source was unchanged, the snapshot and result contained exactly `id`
and `name`, leading-zero IDs survived, and private notes were excluded.
An idempotent replay verified the receipt and did not create another deal:
`4d6b775f2fb4d5d0be5932fa72db5e9b38d0d56ef9dbdd9f74dca53809865de8`.
Usage was one quote, one deal and 10,000 ms reserved. The fixture process was
stopped afterward. Evidence is retained in the ignored
`_tmp/protected-listings/agent-summary.json` and the private temporary directory
named there. This is local development-binary evidence, not clean-machine,
public HTTPS recipient, or first-time-human qualification. Claude Code still
reported `loggedIn: false`; no sign-in or participant recruitment was attempted.

## First-user trial kit — 1 October 2026

Status: **prepared, not run**. A real catalog, five first-time publishers, their
independent recipients, and usable test hosts have not been supplied for this
trial. The empty records below are a plan, not participant or value evidence.
Use pseudonymous IDs and keep consent, source data, credentials, raw transcripts
and private paths outside Git. Publish only an owner-approved, sanitized summary.

### Select one useful job

Ask a real data owner to name a recurring task that another person or their
agent needs to perform against a small JSON, explicitly typed CSV, or SQLite
selection. Record the recipient's question, selected fields, expected output
and success check before either workflow starts. The owner must authorize the
selected data and metadata being exposed. Record what must be excluded and the
permitted recipient/access mode. Do not turn a synthetic example into a claimed
customer catalog.

Use the same task, input, selection and independently checked answer in both
workflows. If an additional disposable Wasm audit is useful, specify its policy
and input explicitly, preserve the exact program digest, and use the same
program/policy in the baseline. A one-target exact-string check measures that
policy; a domain expert remains responsible for scientific interpretation.
Programs needing host imports, private filesystem access or arbitrary Python
are outside this native Wasm action. Record an unsupported requirement as a
blocker rather than silently changing the participant's task.

### Prerequisites and session procedure

1. Record the independently trusted candidate package/binary digest, source
   provenance, supported OS, agent version and model, and publication/recipient
   endpoints. A dirty source build needs its source/build fingerprint as well
   as a Git revision. An older immutable release cannot stand in for missing
   native actions. Record unavailable release or host access as `blocked`.
2. Enroll five first-time publishers only when they and their recipients are
   available. Each recipient uses a separate machine and requester identity.
   Confirm permission for observation and local evidence retention. Supply only
   the normal published/candidate guide, catalog path, recipient's task and
   approved sharing scope; record exactly what was supplied.
3. Start a timestamped transcript and timer at the first publishing prompt.
   Keep installation, approval, retry, waiting and network-failure time inside
   the trial elapsed time. Present this same prompt to every publisher:

   > Make this catalog usable by another agent. Share only the fields the
   > recipient needs, inspect a local result, approve publication, and give the
   > link to the recipient's agent. Then explain what is public and what happens
   > when your computer sleeps.

4. The facilitator observes. Record every explanation, command, configuration
   repair or missing artifact supplied after the start as an intervention.
   Rescue a stalled session only after recording its unassisted failure; retain
   the assisted outcome separately. Do not count the assisted retry as a pass.
5. Stop the timer only after the actual recipient agent has invoked the service,
   returned the expected result, and produced verified result/Receipt evidence.
   Retain the actual agent tool trace. A configured tool or shell-only MCP probe
   does not satisfy this end point. Use a separate offline verifier on the
   exported chain and match the received output to its result hash.
6. Check selected snapshot and returned data against the agreed scope, including
   excluded fields and identifiers. Check that preparation did not change the
   original source. Record the publisher's own privacy and sleep explanations
   before coaching: metadata and selected content may be public; delivered
   results cannot be recalled; the host must remain awake, online and running.
7. Unpublish the test service and revoke trial access when no longer needed;
   record the observed cleanup state. Keep a failed or incomplete session in the
   denominator. Record a replacement session with a new ID and the reason.

Score the documented usability gate only after five valid sessions: at least
four complete in at most 900 seconds, with zero developer interventions,
independent recipient execution, verified evidence, correct output/selection,
and both explanations correct. Missing observations are `not_verified`, never
assumed passes. Retain failures and blockers, then repeat after repairs with
fresh first-time users. Platform qualification remains a separate matrix in
[Release](RELEASE.md#required-platform-and-external-evidence).

### Compare with the owner's existing workflow

Before testing Froglet, record how the owner and recipient currently solve the
same task: selected CSV/file export, their existing API, or another actual method.
Do not build a deliberately weak alternative. Record installed/prepared state,
access controls, transmitted fields and independently checked answer. Predeclare
which workflow runs first for each pair and balance order when possible; retain
the order because prior exposure can improve a later attempt.

Measure actual start/end timestamps, failures, human interventions, setup effort
and recurring operation effort for each method. Record developer preparation
time when observed; leave it unknown otherwise. Separate first setup from
routine use, and retain both: an already configured baseline and a first Froglet
installation have different starting conditions. Record any invoiced or measured
cost with its source; leave unmeasured costs null. No operating-cost claim follows
from a faster arithmetic/audit program alone.

Ask the owner and recipient which workflow they would choose for the next real
task and why. Record their explanation verbatim in private evidence. When they
later reuse the service voluntarily, record the actual task/date/receipt and
whether prompting or developer assistance occurred. A planned follow-up,
intention to reuse, or a valid signature is not observed demand. A five-person
trial can expose blockers and useful cases; it cannot estimate population-wide
adoption or establish scientific correctness.

### Empty session capture record

Copy this JSON once per session into a private evidence directory. Replace nulls
only with observed values and a trace/artifact reference. `not_run`, `blocked`,
`incomplete`, `failed` and `completed` are session states; `completed` describes
the procedure, while the separate pass fields record the assessed gates. Record
timestamps in UTC ISO 8601. Compute elapsed seconds from observed timestamps;
do not infer them from a facilitator's recollection. The schema below is an
evaluation record, not a signed Kernel artifact.

```json
{
  "schema": "froglet.first_user_session.v1",
  "session_id": null,
  "participant_id": null,
  "recipient_id": null,
  "status": "not_run",
  "observation_consent_record": null,
  "candidate": {
    "release_or_candidate": null,
    "source_revision": null,
    "source_build_fingerprint": null,
    "binary_sha256": null,
    "trusted_provenance_record": null
  },
  "publisher_host": {"os": null, "arch": null, "agent": null, "agent_version": null, "model": null},
  "recipient_host": {"os": null, "arch": null, "agent": null, "agent_version": null, "model": null},
  "job": {
    "source_format": null,
    "owner_approved_scope_record": null,
    "recipient_question": null,
    "selected_fields": null,
    "excluded_fields": null,
    "input_evidence": null,
    "independent_expected_result_evidence": null,
    "access_mode": null,
    "baseline_method": null,
    "workflow_order": null,
    "supplied_materials_record": null
  },
  "baseline": {
    "starting_installation_state": null,
    "started_at_utc": null,
    "finished_at_utc": null,
    "elapsed_seconds": null,
    "observed_setup_seconds": null,
    "observed_routine_seconds": null,
    "interventions": null,
    "result_evidence": null,
    "result_matches_expected": null,
    "selection_matches_approval": null,
    "measured_cost_and_source": null
  },
  "froglet": {
    "starting_installation_state": null,
    "started_at_utc": null,
    "finished_at_utc": null,
    "elapsed_seconds": null,
    "observed_setup_seconds": null,
    "observed_routine_seconds": null,
    "interventions": null,
    "actual_agent_tool_trace": null,
    "exact_installation_and_publication_approval_record": null,
    "provider_id": null,
    "requester_id": null,
    "offer_hash": null,
    "deal_id": null,
    "result_hash": null,
    "receipt_hash": null,
    "sanitized_chain_evidence": null,
    "offline_verifier_command_and_output": null,
    "result_matches_expected": null,
    "selection_matches_approval": null,
    "original_source_unchanged": null,
    "privacy_explanation_evidence": null,
    "privacy_explanation_correct": null,
    "sleep_explanation_evidence": null,
    "sleep_explanation_correct": null,
    "cleanup_evidence": null,
    "measured_cost_and_source": null
  },
  "assessment": {
    "usability_pass": null,
    "blockers": null,
    "owner_next_task_choice_and_reason_evidence": null,
    "recipient_next_task_choice_and_reason_evidence": null,
    "voluntary_reuse_observation": null,
    "unknowns": null
  }
}
```

### Empty five-session ledger

Assign people only when enrolled. These slots contain no participant identities
or results. Link each private session record and retain the measured outcome;
write `not_verified` for an unavailable check. A session's elapsed time and gate
decision must be independently recomputable from its evidence.

| Slot | Participant / recipient IDs | Record | State | Seconds | Interventions | Independent result + Receipt | Privacy + sleep | Usability pass |
|---|---|---|---|---|---|---|---|---|
| 1 | — | — | Not run | — | — | Not verified | Not verified | Not verified |
| 2 | — | — | Not run | — | — | Not verified | Not verified | Not verified |
| 3 | — | — | Not run | — | — | Not verified | Not verified | Not verified |
| 4 | — | — | Not run | — | — | Not verified | Not verified | Not verified |
| 5 | — | — | Not run | — | — | Not verified | Not verified | Not verified |

The eventual report should give observed usability passes out of five, all
blockers and interventions, baseline/Froglet results with their starting states,
and any observed repeat use. Keep protocol correctness, public operation,
usability and value conclusions separate. Record unmeasured effects as unknown;
do not fill them with projections or testimonials.
