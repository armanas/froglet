"""Regression tests for `evaluate_cell` + `validate_publish_response`.

Each test below corresponds to a bug the LLM reviewer caught (or
nearly caught) in the original Phase 4 harness:

- run_matrix.py:218 — multi-call + non-publish-action both passed
- run_matrix.py:416 — --execute didn't verify hosting URL
- run_matrix.py:94  — GPT-4 silently routed to Anthropic
- criteria.json:13 — pass bar was 88.9% vs documented 90%

Run with:

    python3 -m unittest discover tests/llm_acceptance -v
"""

from __future__ import annotations

import asyncio
import json
import unittest
from pathlib import Path

import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tests" / "llm_acceptance"))

# Import the module under test by file path so we don't depend on an
# `__init__.py` in tests/.
import run_matrix  # type: ignore[import-not-found]


PROMPT_SPEC = {
    "id": "translator-en-es",
    "expected_name_pattern": "^(translator|en.?es|translate).*",
    "must_contain_in_source": ["es:"],
    "min_source_lines": 3,
}


def cell(hosting_kind: str = "tor") -> "run_matrix.Cell":
    return run_matrix.Cell(
        prompt_id="translator-en-es",
        model="claude-sonnet-4-5",
        hosting_kind=hosting_kind,
    )


def valid_publish_call() -> dict[str, object]:
    return {
        "id": "tu1",
        "name": "froglet",
        "input": {
            "action": "marketplace_publish",
            "name": "translator-en-es",
            "source_inline": "def handle(p):\n    return {'translated': 'es:' + p['text']}\n# 3 lines\n",
            "hosting": {"kind": "tor"},
            "settlement": {"method": "none"},
        },
    }


class EvaluateCellTests(unittest.TestCase):
    def test_single_valid_call_passes(self):
        r = run_matrix.evaluate_cell(cell(), PROMPT_SPEC, [valid_publish_call()])
        self.assertTrue(r.passed, msg=f"failed checks: {r.failed_checks}")
        self.assertIsNone(r.failure_category)

    # ── Reviewer's P1: "exactly once" not enforced ───────────────────

    def test_two_publish_calls_fails_as_llm_loop(self):
        r = run_matrix.evaluate_cell(
            cell(),
            PROMPT_SPEC,
            [valid_publish_call(), valid_publish_call()],
        )
        self.assertFalse(r.passed, "two publish calls must FAIL")
        self.assertEqual(r.failure_category, "llm-loop")
        self.assertTrue(
            any("no-retry-loop" in c for c in r.failed_checks),
            f"checks were: {r.failed_checks}",
        )

    def test_status_then_publish_fails_as_llm_loop(self):
        status_call = {
            "id": "tu0",
            "name": "froglet",
            "input": {"action": "status"},
        }
        r = run_matrix.evaluate_cell(
            cell(),
            PROMPT_SPEC,
            [status_call, valid_publish_call()],
        )
        self.assertFalse(r.passed, "status-then-publish must FAIL")
        self.assertEqual(r.failure_category, "llm-loop")
        self.assertTrue(
            any("no-other-action" in c for c in r.failed_checks),
            f"checks were: {r.failed_checks}",
        )

    def test_no_publish_call_fails_as_tool_not_called(self):
        r = run_matrix.evaluate_cell(
            cell(),
            PROMPT_SPEC,
            [{"id": "tu0", "name": "froglet", "input": {"action": "status"}}],
        )
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-not-called")

    def test_empty_tool_uses_fails_as_tool_not_called(self):
        r = run_matrix.evaluate_cell(cell(), PROMPT_SPEC, [])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-not-called")

    # ── Structural checks ────────────────────────────────────────────

    def test_bad_name_fails_as_tool_misuse(self):
        call = valid_publish_call()
        call["input"]["name"] = "not-a-translator-at-all"
        r = run_matrix.evaluate_cell(cell(), PROMPT_SPEC, [call])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-misuse")

    def test_missing_keyword_in_source_fails(self):
        call = valid_publish_call()
        call["input"]["source_inline"] = (
            "def handle(p):\n    return p\n# no required keyword\n"
        )
        r = run_matrix.evaluate_cell(cell(), PROMPT_SPEC, [call])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-misuse")

    def test_wrong_hosting_kind_fails(self):
        call = valid_publish_call()
        call["input"]["hosting"]["kind"] = "local"
        r = run_matrix.evaluate_cell(cell("tor"), PROMPT_SPEC, [call])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-misuse")

    def test_self_hosting_without_url_fails(self):
        call = valid_publish_call()
        call["input"]["hosting"] = {"kind": "self"}
        r = run_matrix.evaluate_cell(cell("self"), PROMPT_SPEC, [call])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-misuse")

    def test_paid_settlement_fails(self):
        call = valid_publish_call()
        call["input"]["settlement"] = {"method": "lightning"}
        r = run_matrix.evaluate_cell(cell(), PROMPT_SPEC, [call])
        self.assertFalse(r.passed)
        self.assertEqual(r.failure_category, "tool-misuse")


class PublishResponseTests(unittest.TestCase):
    """Reviewer's P2: --execute didn't verify hosting URL."""

    def test_tor_response_with_onion_passes(self):
        failed = run_matrix.validate_publish_response(
            cell("tor"),
            valid_publish_call()["input"],
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "http://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.onion",
            },
        )
        self.assertEqual(failed, [])

    def test_tor_response_with_non_onion_fails(self):
        failed = run_matrix.validate_publish_response(
            cell("tor"),
            valid_publish_call()["input"],
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "https://something-else.example.com",
            },
        )
        self.assertTrue(any("hosting-url-shape:tor" in c for c in failed), failed)

    def test_local_response_with_loopback_passes(self):
        failed = run_matrix.validate_publish_response(
            cell("local"),
            valid_publish_call()["input"],
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "http://127.0.0.1:8080",
            },
        )
        self.assertEqual(failed, [])

    def test_local_response_with_public_url_fails(self):
        failed = run_matrix.validate_publish_response(
            cell("local"),
            valid_publish_call()["input"],
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "https://something-else.example.com",
            },
        )
        self.assertTrue(any("hosting-url-shape:local" in c for c in failed), failed)

    def test_self_response_matching_url_passes(self):
        call_input = valid_publish_call()["input"]
        call_input["hosting"] = {"kind": "self", "url": "https://my-host.fly.dev"}
        failed = run_matrix.validate_publish_response(
            cell("self"),
            call_input,
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "https://my-host.fly.dev",
            },
        )
        self.assertEqual(failed, [])

    def test_self_response_with_mismatched_url_fails(self):
        call_input = valid_publish_call()["input"]
        call_input["hosting"] = {"kind": "self", "url": "https://my-host.fly.dev"}
        failed = run_matrix.validate_publish_response(
            cell("self"),
            call_input,
            {
                "provider_id": "abc",
                "offer_hash": "def",
                "public_url": "https://other-host.fly.dev",
            },
        )
        self.assertTrue(any("hosting-url-shape:self" in c for c in failed), failed)

    def test_empty_provider_id_fails(self):
        failed = run_matrix.validate_publish_response(
            cell("tor"),
            valid_publish_call()["input"],
            {"provider_id": "", "offer_hash": "x", "public_url": "http://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.onion"},
        )
        self.assertIn("publish-succeeds:provider_id-empty", failed)


class DriveModelDispatchTests(unittest.TestCase):
    """Reviewer's P1: GPT-4 was silently routed to Anthropic."""

    def test_gpt_model_raises_not_implemented(self):
        async def run():
            await run_matrix.drive_model("gpt-4-turbo", "sys", "user")

        with self.assertRaises(RuntimeError) as ctx:
            asyncio.run(run())
        self.assertIn("OpenAI driver is not implemented", str(ctx.exception))

    def test_unknown_model_raises(self):
        async def run():
            await run_matrix.drive_model("llama-3", "sys", "user")

        with self.assertRaises(RuntimeError) as ctx:
            asyncio.run(run())
        self.assertIn("no driver", str(ctx.exception))


class PassBarTests(unittest.TestCase):
    """Reviewer's P2: 40/45 was 88.9% but bar claimed 90%."""

    def test_default_bar_count_matches_documented_rate(self):
        criteria = json.loads(
            (ROOT / "tests" / "llm_acceptance" / "criteria.json").read_text()
        )
        total = criteria["pass_bar"]["total_cells"]
        min_count = criteria["pass_bar"]["min_passing"]
        min_rate = criteria["pass_bar"]["min_pass_rate_pct"]
        observed_rate = (min_count / total) * 100
        self.assertGreaterEqual(
            observed_rate,
            min_rate,
            f"min_passing={min_count}/{total}={observed_rate:.1f}% must be >= "
            f"min_pass_rate_pct={min_rate}% — otherwise the count gate is "
            "satisfiable without satisfying the rate gate",
        )


if __name__ == "__main__":
    unittest.main()
