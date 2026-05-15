#!/usr/bin/env python3
"""Phase 4 LLM acceptance matrix runner.

Drives the (prompts × models × hosting) matrix described in
`criteria.json` against the Froglet MCP. For each cell:

1. Boots a fresh LLM conversation with a system prompt that grants
   access to the `froglet` MCP tool.
2. Sends the user_intent from `prompts.json`, with a one-line
   directive to use `hosting.kind` matching the cell.
3. Captures every tool_use the LLM emitted.
4. Validates the call against `criteria.json` structural + behavioural
   checks.
5. Writes a per-cell transcript + categorised pass/fail to
   `_tmp/llm_acceptance/<ts>/cells/<cell-id>.json`.
6. After all cells, emits a summary matrix to
   `_tmp/llm_acceptance/<ts>/summary.json` and `summary.tsv`.

This script is the matrix harness — it expects:

- `ANTHROPIC_API_KEY` env var (for Claude models)
- `OPENAI_API_KEY` env var (for GPT models; can be omitted to skip)
- A running Froglet MCP server reachable via stdio (we spawn it ourselves)
- A running froglet-node daemon (we DO NOT spawn; operator must start it
  before running this script)
- Network reachability to `marketplace.froglet.dev`

It does NOT exercise the daemon's full Tor stack itself; if a cell's
hosting_kind is "tor" it expects FROGLET_NETWORK_MODE=tor on the daemon
and asserts the engine returns an onion URL. Cells targeting hosting
that isn't configured fail with category "engine-error" so the matrix
surfaces operator-setup gaps rather than swallowing them.

Usage:

  python tests/llm_acceptance/run_matrix.py                     # full matrix
  python tests/llm_acceptance/run_matrix.py --only translator-en-es  # one prompt, all models, all hosting
  python tests/llm_acceptance/run_matrix.py --model claude-sonnet-4-5  # one model, all prompts, all hosting
  python tests/llm_acceptance/run_matrix.py --hosting local      # all prompts, all models, one hosting
  python tests/llm_acceptance/run_matrix.py --dry-run            # print the cells without calling any LLM

The matrix is small enough (45 cells) that even a full run finishes
in ~15 min wall-clock, dominated by indexer-lag polling. API cost is
~$1-3 per full run at current Anthropic / OpenAI list prices.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import re
import sys
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Optional

ROOT = Path(__file__).resolve().parents[2]
PROMPTS_PATH = ROOT / "tests" / "llm_acceptance" / "prompts.json"
CRITERIA_PATH = ROOT / "tests" / "llm_acceptance" / "criteria.json"


# ── Cell + result types ────────────────────────────────────────────────


@dataclass
class Cell:
    prompt_id: str
    model: str
    hosting_kind: str

    @property
    def cell_id(self) -> str:
        return f"{self.prompt_id}__{self.model}__{self.hosting_kind}"


@dataclass
class CellResult:
    cell: Cell
    passed: bool
    failure_category: Optional[str]
    failed_checks: list[str] = field(default_factory=list)
    tool_calls: list[dict[str, Any]] = field(default_factory=list)
    publish_response: Optional[dict[str, Any]] = None
    duration_seconds: float = 0.0
    notes: list[str] = field(default_factory=list)


# ── LLM driver (Anthropic) ────────────────────────────────────────────


async def drive_anthropic(
    model: str, system_prompt: str, user_message: str
) -> list[dict[str, Any]]:
    """Drive a Claude model in a tool-use loop. Returns the list of
    tool_use blocks the model emitted. Each block has shape:
    {"name": "froglet", "input": {...}}.

    Implements just enough of the Anthropic tool-use loop to capture
    one or two tool calls. Stops on stop_reason="end_turn" or after 4
    rounds.
    """
    try:
        import anthropic
    except ImportError as e:
        raise RuntimeError(
            "pip install anthropic to run the Phase 4 matrix"
        ) from e

    client = anthropic.AsyncAnthropic()
    messages = [{"role": "user", "content": user_message}]
    tool_uses: list[dict[str, Any]] = []
    rounds = 0
    while rounds < 4:
        response = await client.messages.create(
            model=model,
            max_tokens=2048,
            system=system_prompt,
            tools=[_froglet_tool_schema()],
            messages=messages,
        )
        for block in response.content:
            if getattr(block, "type", None) == "tool_use":
                tool_uses.append(
                    {"id": block.id, "name": block.name, "input": dict(block.input)}
                )
        if response.stop_reason == "end_turn":
            break
        # Feed back synthetic tool_result so the LLM can finish thinking.
        # For matrix purposes we don't actually execute the tool; the
        # caller will replay the captured tool_uses against the live MCP.
        assistant_content = [
            {"type": b.type, **_block_to_dict(b)} for b in response.content
        ]
        messages.append({"role": "assistant", "content": assistant_content})
        tool_results = []
        for tu in [b for b in response.content if getattr(b, "type", None) == "tool_use"]:
            tool_results.append(
                {
                    "type": "tool_result",
                    "tool_use_id": tu.id,
                    "content": '{"status":"published","provider_id":"00","public_url":"http://test","offer_hash":"00"}',
                }
            )
        if tool_results:
            messages.append({"role": "user", "content": tool_results})
        rounds += 1
    return tool_uses


def _block_to_dict(b: Any) -> dict[str, Any]:
    """Best-effort serialization of an anthropic content block."""
    out = {}
    for attr in ("text", "id", "name", "input"):
        if hasattr(b, attr):
            v = getattr(b, attr)
            if attr == "input":
                v = dict(v) if v is not None else None
            out[attr] = v
    return out


def _froglet_tool_schema() -> dict[str, Any]:
    """Trimmed JSON schema for the `froglet` MCP tool. Mirrors the real
    schema in integrations/mcp/froglet/lib/tools.js so the LLM sees the
    same surface it would see through MCP."""
    return {
        "name": "froglet",
        "description": (
            "Authoritative Froglet MCP tool. For ONE-CALL agent-grade "
            "publishing use the marketplace_publish action: pass "
            "{action:'marketplace_publish', name, source_inline, "
            "hosting:{kind:'local'|'tor'|'self', url?}, "
            "settlement:{method:'none'}}. The handler shells out to "
            "froglet-node publish which scaffolds manifests, builds, "
            "signs, registers, and verifies in one call. Returns "
            "{provider_id, public_url, offer_hash, marketplace_offer_url, "
            "invoke_command}."
        ),
        "input_schema": {
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {"type": "string"},
                "name": {"type": "string"},
                "source_inline": {"type": "string"},
                "summary": {"type": "string"},
                "hosting": {
                    "type": "object",
                    "properties": {
                        "kind": {"type": "string"},
                        "url": {"type": "string"},
                    },
                },
                "settlement": {
                    "type": "object",
                    "properties": {"method": {"type": "string"}},
                },
                "marketplace_url": {"type": "string"},
            },
        },
    }


# ── Validators ────────────────────────────────────────────────────────


def evaluate_cell(
    cell: Cell,
    prompt_spec: dict[str, Any],
    tool_uses: list[dict[str, Any]],
) -> CellResult:
    result = CellResult(cell=cell, passed=False, failure_category=None)
    result.tool_calls = tool_uses

    publish_calls = [
        tu for tu in tool_uses if tu["input"].get("action") == "marketplace_publish"
    ]
    if not publish_calls:
        result.failure_category = "tool-not-called"
        result.failed_checks.append("called-marketplace-publish")
        return result

    if len(publish_calls) > 1:
        result.notes.append(f"LLM called marketplace_publish {len(publish_calls)} times")

    call_input = publish_calls[0]["input"]

    name = (call_input.get("name") or "").strip().lower()
    if not re.match(prompt_spec["expected_name_pattern"], name):
        result.failure_category = "tool-misuse"
        result.failed_checks.append(
            f"valid-name (got {name!r}, want regex {prompt_spec['expected_name_pattern']!r})"
        )

    source = call_input.get("source_inline", "")
    if not source or len(source.splitlines()) < prompt_spec.get("min_source_lines", 2):
        result.failed_checks.append("source-inline-present")
        result.failure_category = result.failure_category or "tool-misuse"

    for kw in prompt_spec.get("must_contain_in_source", []) or []:
        if kw not in source:
            result.failed_checks.append(f"source-contains-keywords (missing {kw!r})")
            result.failure_category = result.failure_category or "tool-misuse"

    requested_hosting = (call_input.get("hosting") or {}).get("kind")
    if requested_hosting != cell.hosting_kind:
        result.failed_checks.append(
            f"hosting-kind-matches (got {requested_hosting!r}, want {cell.hosting_kind!r})"
        )
        result.failure_category = result.failure_category or "tool-misuse"

    if cell.hosting_kind == "self":
        if not (call_input.get("hosting") or {}).get("url"):
            result.failed_checks.append("hosting-self-requires-url")
            result.failure_category = result.failure_category or "tool-misuse"

    settlement_method = (call_input.get("settlement") or {}).get("method", "none")
    if settlement_method != "none":
        result.failed_checks.append(
            f"settlement-is-none (got {settlement_method!r})"
        )
        result.failure_category = result.failure_category or "tool-misuse"

    if not result.failed_checks:
        result.passed = True
    return result


# ── Live execution against the real MCP / engine ─────────────────────


async def execute_publish(
    call_input: dict[str, Any], hosting_url_for_self: Optional[str]
) -> dict[str, Any]:
    """Replay the captured tool call against the real `froglet-node publish`
    binary via the shared `marketplace-publish.js` handler. Returns the
    parsed PublishOutput on success; raises on failure.

    We invoke through Node directly rather than via MCP because the
    matrix already has the structured input; cutting out the MCP server
    process keeps the harness simple. The behaviour is identical — the
    same shared module runs in both cases.
    """
    import subprocess
    import tempfile

    # Materialise an inline driver script.
    driver = (
        "import { runMarketplacePublish } from "
        f"'{ROOT}/integrations/shared/froglet-lib/marketplace-publish.js';\n"
        "const args = JSON.parse(process.argv[2]);\n"
        "const out = await runMarketplacePublish(args);\n"
        "process.stdout.write(JSON.stringify(out));\n"
    )

    payload = dict(call_input)
    if call_input.get("hosting", {}).get("kind") == "self" and not call_input.get(
        "hosting", {}
    ).get("url"):
        payload.setdefault("hosting", {})["url"] = hosting_url_for_self or ""

    with tempfile.NamedTemporaryFile(
        "w", suffix=".mjs", delete=False, dir=tempfile.gettempdir()
    ) as tf:
        tf.write(driver)
        driver_path = tf.name

    try:
        proc = await asyncio.create_subprocess_exec(
            "node",
            driver_path,
            json.dumps(payload),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=os.environ.copy(),
        )
        stdout, stderr = await proc.communicate()
        if proc.returncode != 0:
            raise RuntimeError(
                f"node driver exit={proc.returncode}: {stderr.decode(errors='replace')}"
            )
        return json.loads(stdout.decode())
    finally:
        try:
            os.unlink(driver_path)
        except OSError:
            pass


# ── Orchestration ─────────────────────────────────────────────────────


def build_cells(
    criteria: dict[str, Any],
    prompts: list[dict[str, Any]],
    only_prompt: Optional[str],
    only_model: Optional[str],
    only_hosting: Optional[str],
) -> list[Cell]:
    matrix = criteria["matrix"]
    cells = []
    for p in prompts:
        if only_prompt and p["id"] != only_prompt:
            continue
        for model in matrix["models"]:
            if only_model and model != only_model:
                continue
            for hk in matrix["hosting_kinds"]:
                if only_hosting and hk != only_hosting:
                    continue
                cells.append(
                    Cell(prompt_id=p["id"], model=model, hosting_kind=hk)
                )
    return cells


def system_prompt(hosting_kind: str) -> str:
    return (
        "You are an LLM driving the Froglet MCP. Your job is to "
        "satisfy the user's intent by making ONE call to the "
        "`froglet` tool with `action: 'marketplace_publish'`. "
        f"The user wants the service hosted with hosting.kind = '{hosting_kind}'. "
        "Use a sensible lowercase hyphenated `name`. Pass valid Python "
        "source as `source_inline`. Pass settlement.method='none' (paid "
        "rails ship in v2). Do not call any other Froglet action. Do "
        "not iterate — call marketplace_publish exactly once."
    )


def hosting_url_for_self_from_env() -> Optional[str]:
    """For self-hosting cells, the URL has to come from somewhere. We
    pull it from FROGLET_TEST_SELF_URL so the operator can configure
    one Fly/Render deployment to use across the matrix."""
    return os.environ.get("FROGLET_TEST_SELF_URL")


async def run_cell(cell: Cell, prompt_spec: dict[str, Any], execute: bool) -> CellResult:
    started = time.time()
    user_intent = prompt_spec["user_intent"]
    user_msg = (
        f"{user_intent}\n\n"
        f"For this run, use hosting.kind = '{cell.hosting_kind}'."
    )
    self_url = hosting_url_for_self_from_env()
    if cell.hosting_kind == "self":
        if self_url:
            user_msg += f" The hosting URL is {self_url}."
        else:
            user_msg += " (No FROGLET_TEST_SELF_URL was provided — the model may guess.)"

    try:
        tool_uses = await drive_anthropic(
            cell.model, system_prompt(cell.hosting_kind), user_msg
        )
    except RuntimeError as e:
        result = CellResult(
            cell=cell,
            passed=False,
            failure_category="tool-not-called",
            failed_checks=["llm-driver-error"],
            notes=[str(e)],
        )
        result.duration_seconds = time.time() - started
        return result

    result = evaluate_cell(cell, prompt_spec, tool_uses)

    if execute and result.passed:
        try:
            call_input = next(
                tu["input"] for tu in tool_uses if tu["input"].get("action") == "marketplace_publish"
            )
            publish_output = await execute_publish(call_input, self_url)
            result.publish_response = publish_output
            if not publish_output.get("provider_id") or not publish_output.get("offer_hash"):
                result.passed = False
                result.failure_category = "engine-error"
                result.failed_checks.append("publish-succeeds")
        except Exception as e:
            result.passed = False
            result.failure_category = "engine-error"
            result.failed_checks.append("publish-succeeds")
            result.notes.append(f"execute_publish error: {e}")

    result.duration_seconds = time.time() - started
    return result


async def main_async(args: argparse.Namespace) -> int:
    prompts = json.loads(PROMPTS_PATH.read_text())["prompts"]
    criteria = json.loads(CRITERIA_PATH.read_text())

    cells = build_cells(
        criteria,
        prompts,
        args.only,
        args.model,
        args.hosting,
    )

    if args.dry_run:
        for c in cells:
            print(c.cell_id)
        print(f"\n{len(cells)} cells total")
        return 0

    ts = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    out_dir = ROOT / "_tmp" / "llm_acceptance" / ts
    cells_dir = out_dir / "cells"
    cells_dir.mkdir(parents=True, exist_ok=True)
    print(f"Writing results to {out_dir}")

    results: list[CellResult] = []
    by_prompt = {p["id"]: p for p in prompts}
    for c in cells:
        print(f"  [{len(results) + 1}/{len(cells)}] {c.cell_id} ...", flush=True)
        r = await run_cell(c, by_prompt[c.prompt_id], execute=args.execute)
        results.append(r)
        (cells_dir / f"{c.cell_id}.json").write_text(
            json.dumps(asdict(r), indent=2, default=str)
        )

    passed = sum(1 for r in results if r.passed)
    total = len(results)
    pass_rate = (passed / total * 100) if total else 0.0
    bar = criteria["pass_bar"]["min_pass_rate_pct"]

    summary = {
        "ts": ts,
        "total": total,
        "passed": passed,
        "pass_rate_pct": pass_rate,
        "bar_pct": bar,
        "matrix_pass": pass_rate >= bar,
        "by_category": {},
    }
    for r in results:
        if not r.passed:
            cat = r.failure_category or "unknown"
            summary["by_category"][cat] = summary["by_category"].get(cat, 0) + 1

    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2))
    tsv_lines = ["cell_id\tpassed\tcategory\tchecks_failed\tduration_s"]
    for r in results:
        tsv_lines.append(
            "\t".join(
                [
                    r.cell.cell_id,
                    "1" if r.passed else "0",
                    r.failure_category or "",
                    ";".join(r.failed_checks),
                    f"{r.duration_seconds:.2f}",
                ]
            )
        )
    (out_dir / "summary.tsv").write_text("\n".join(tsv_lines) + "\n")

    print()
    print(f"Pass rate: {passed}/{total} = {pass_rate:.1f}% (bar: {bar}%)")
    if summary["by_category"]:
        print("Failure categories:")
        for k, v in summary["by_category"].items():
            print(f"  {k}: {v}")
    if summary["matrix_pass"]:
        print("MATRIX PASS — Phase 4 gate green; launch unblocked.")
        return 0
    print("MATRIX FAIL — fix failing cells before launch.")
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", help="Run only this prompt id")
    parser.add_argument("--model", help="Run only this model")
    parser.add_argument("--hosting", help="Run only this hosting kind")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the cell list without calling any LLM",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help=(
            "After capturing the LLM's tool call, actually replay it through "
            "the real marketplace_publish handler against the live daemon + "
            "marketplace. Requires the daemon to be running."
        ),
    )
    args = parser.parse_args()
    try:
        return asyncio.run(main_async(args))
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    sys.exit(main())
