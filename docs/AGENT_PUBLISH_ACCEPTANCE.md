# Native agent publishing evidence — 2026-09-26

Status: the public publishing entry path is improved and deployed. Three fresh
Codex scenarios passed preparation through exact public consent on this Mac.
This is **not** full publication/recipient qualification, Claude Code
qualification, or a guarantee about every LLM.

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
