# Phase 4 — LLM acceptance matrix

The launch gate for "agent-grade publish actually works." The matrix is
**45 cells** (5 prompts × 3 LLM models × 3 hosting backends) and the
pass bar is **≥40 of 45 (≥90%)**. If the matrix passes, Phase 6 (public
launch) is unblocked. If it fails, the failure category tells us what
to fix.

## What's tested

Each cell drives a real LLM with a one-sentence user intent (e.g.
*"Publish a Froglet service that translates English to Spanish"*) and
checks whether the LLM produces a single, well-formed
`marketplace_publish` call. Cells that pass the structural check then
optionally execute the call against the real daemon + marketplace.

Files:

- `prompts.json` — 5 user intents + per-prompt validators (regex on
  service name, must-contain keywords in source, min source lines).
- `criteria.json` — matrix dimensions, structural + behavioural checks,
  failure category taxonomy, pass bar.
- `run_matrix.py` — Python orchestrator. Drives Anthropic + (optional)
  OpenAI APIs, captures tool calls, validates, writes per-cell
  transcripts + a summary TSV.

## Prerequisites

### Required for any matrix run

```bash
pip install anthropic
export ANTHROPIC_API_KEY=...   # for Claude models
```

### Optional

```bash
export OPENAI_API_KEY=...                       # for GPT-4 cells
export FROGLET_TEST_SELF_URL=https://...        # for hosting=self cells
```

### Required for `--execute` mode

The flag replays each LLM-captured tool call against the live publish
engine, so you also need:

- A running `froglet-node` daemon at `FROGLET_DAEMON_URL`
  (default `http://127.0.0.1:8080`)
- Provider-control token reachable via `FROGLET_PROVIDER_CONTROL_TOKEN`
  or `FROGLET_PROVIDER_CONTROL_TOKEN_PATH`
- For Tor cells: daemon started with `FROGLET_NETWORK_MODE=tor` or `dual`
- Network reachability to `marketplace.froglet.dev`

Without `--execute` you get only the structural pass/fail signal — useful
for an LLM-only smoke before spending time on infrastructure.

## How to run

```bash
# Dry run: just enumerate cells
python tests/llm_acceptance/run_matrix.py --dry-run

# Smoke: one prompt, one model, structural only
python tests/llm_acceptance/run_matrix.py \
  --only translator-en-es --model claude-sonnet-4-5

# Full structural pass (LLM-only, no daemon needed)
python tests/llm_acceptance/run_matrix.py

# Full execution pass (the real launch gate)
python tests/llm_acceptance/run_matrix.py --execute
```

Results land in `_tmp/llm_acceptance/<UTC-timestamp>/`:

```
_tmp/llm_acceptance/20260516T0930Z/
├── cells/
│   ├── translator-en-es__claude-sonnet-4-5__local.json
│   ├── translator-en-es__claude-sonnet-4-5__tor.json
│   └── ... (one per cell)
├── summary.json
└── summary.tsv
```

`summary.json` is the gate output:

```json
{
  "total": 45,
  "passed": 42,
  "pass_rate_pct": 93.3,
  "bar_pct": 90,
  "matrix_pass": true,
  "by_category": {
    "tool-misuse": 2,
    "engine-error": 1
  }
}
```

## Cost + time

- ~$1-3 of API spend per full matrix run at current Anthropic + OpenAI
  list prices (~6 tool-use rounds per cell × 45 cells)
- ~15 min wall-clock without `--execute`; ~30-45 min with `--execute`
  (dominated by indexer-lag polling)

## What happens when a cell fails

Each cell's JSON transcript captures:

- The exact LLM model + prompt + system prompt
- Every tool_use block the LLM emitted
- Which structural checks failed
- The categorised failure reason
- The publish_response (when `--execute` was used)
- Any free-form notes (e.g. "LLM called marketplace_publish 2 times")

Fixing flow:

1. Read `summary.json` → identify failure categories
2. Open the failing cell JSON → see what the LLM actually emitted
3. Decide whether the fix is in the LLM prompt, the tool schema, the
   engine validation, or something else
4. Re-run just that cell (`--only X --model Y --hosting Z`) to verify

## Failure categories

| Category | Means | Likely fix |
|---|---|---|
| `tool-not-called` | LLM never called `marketplace_publish` (called wrong tool or nothing) | Tool description in `tools.js` isn't authoritative enough |
| `tool-misuse` | Called `marketplace_publish` with wrong/missing args | Schema description; rethink which fields are required |
| `engine-error` | Valid args but pipeline failed | Real bug in the engine; check `_tmp/llm_acceptance/.../cells/*.json` notes |
| `marketplace-lag` | Engine warned `IndexerLag` (not a fail) | Operator: poll longer, or check indexer health |
| `llm-loop` | LLM iterated rather than one-shotting | Tighten system prompt; check tool description for "exactly once" |

## Cleanup after a matrix run

Each `--execute` cell publishes a real offer to the marketplace. After
final assessment, run:

```bash
# TODO (Phase 4.6): scripts/llm_acceptance_cleanup.sh reads
# _tmp/llm_acceptance/<ts>/cells/*.json, extracts each provider_id,
# and calls the marketplace admin API to suspend the test offer.
```

For now, the provider_ids are in the per-cell JSON; clean up manually
via the marketplace admin endpoints.

## Why this is the gate

Phase 4 verifies the user's original framing — *"it must be easy for
LLM to run froglets as well as publish on marketplace"* — is
**empirically true**, not just structurally true. The plumbing in
Phase 0–3 makes one-MCP-call publish possible; Phase 4 measures
whether real LLMs in real conditions actually nail it ≥90% of the
time.

If they don't, the launch claim ("publish in one prompt") would be
overreach and HN would catch it within hours. If they do, the launch
claim survives contact with the audience.
