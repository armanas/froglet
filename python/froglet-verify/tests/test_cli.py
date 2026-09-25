"""End-to-end CLI smoke tests for ``python -m froglet_verify``.

Runs the actual CLI as a subprocess (not just calling ``main()`` in-process)
to exercise argument parsing, stdin/file input, and process exit codes
exactly as an external caller would experience them.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from _helpers import artifact, load_fixture

PACKAGE_ROOT = Path(__file__).resolve().parents[1]


def _run_cli(
    args: list[str], *, stdin_text: str | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "froglet_verify", *args],
        cwd=PACKAGE_ROOT,
        input=stdin_text,
        capture_output=True,
        text=True,
        timeout=30,
    )


class SingleArtifactCliTests(unittest.TestCase):
    def test_valid_receipt_from_file_exits_zero(self) -> None:
        receipt = artifact("receipt")
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
            json.dump(receipt, handle)
            path = handle.name
        try:
            result = _run_cli([path])
        finally:
            Path(path).unlink()

        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(len(report["artifacts"]), 1)
        self.assertTrue(report["artifacts"][0]["valid"])
        self.assertTrue(report["valid"])
        self.assertIsNone(report["chain"])  # a lone receipt isn't a full chain

    def test_valid_receipt_from_stdin_exits_zero(self) -> None:
        receipt = artifact("receipt")
        result = _run_cli(["-"], stdin_text=json.dumps(receipt))
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertTrue(report["valid"])

    def test_tampered_receipt_exits_one(self) -> None:
        receipt = artifact("receipt")
        receipt["payload"]["result_hash"] = "ff" * 32  # breaks payload_hash integrity
        result = _run_cli(["-"], stdin_text=json.dumps(receipt))
        self.assertEqual(result.returncode, 1, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertFalse(report["valid"])
        self.assertFalse(report["artifacts"][0]["envelope_valid"])

    def test_semantic_and_envelope_failures_are_reported_independently(self) -> None:
        # Tampering the payload without re-signing breaks the envelope (the
        # payload_hash/hash no longer match) *and* trips a semantic check
        # (deal_state is not one of the recognized values). The CLI runs
        # both checks regardless of the other's outcome and reports each
        # with its own reason -- this is more informative for a diagnostic
        # tool than short-circuiting at the first failure.
        receipt = artifact("receipt")
        receipt["payload"]["deal_state"] = "levitating"
        result = _run_cli(["-"], stdin_text=json.dumps(receipt))
        self.assertEqual(result.returncode, 1)
        report = json.loads(result.stdout)
        self.assertFalse(report["valid"])
        entry = report["artifacts"][0]
        self.assertFalse(entry["envelope_valid"])
        self.assertFalse(entry["semantic_valid"])
        self.assertEqual(entry["semantic_reason"], "receipt deal_state is invalid")

    def test_missing_file_exits_two(self) -> None:
        result = _run_cli(["/no/such/file.json"])
        self.assertEqual(result.returncode, 2)
        self.assertIn("error", result.stderr.lower())

    def test_invalid_json_exits_two(self) -> None:
        result = _run_cli(["-"], stdin_text="{not valid json")
        self.assertEqual(result.returncode, 2)

    def test_no_arguments_exits_two(self) -> None:
        result = _run_cli([])
        self.assertEqual(result.returncode, 2)

    def test_array_input_accepted(self) -> None:
        offer = artifact("offer")
        descriptor = artifact("descriptor")
        result = _run_cli(["-"], stdin_text=json.dumps([descriptor, offer]))
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(len(report["artifacts"]), 2)

    def test_artifacts_page_input_accepted(self) -> None:
        page = {"artifacts": [artifact("descriptor"), artifact("offer")]}
        result = _run_cli(["-"], stdin_text=json.dumps(page))
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(len(report["artifacts"]), 2)


class FullChainCliTests(unittest.TestCase):
    def test_full_paid_chain_with_bundle_and_receipt_runs_chain_validation(
        self,
    ) -> None:
        docs = [
            artifact("descriptor"),
            artifact("offer"),
            artifact("quote"),
            artifact("deal"),
            artifact("invoice_bundle"),
            artifact("receipt"),
        ]
        result = _run_cli(["-"], stdin_text=json.dumps(docs))
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertIsNotNone(report["chain"])
        self.assertTrue(report["chain"]["valid"])
        self.assertEqual(len(report["chain"]["reports"]), 4)
        self.assertTrue(report["valid"])

    def test_free_chain_without_bundle(self) -> None:
        docs = [
            artifact("descriptor"),
            artifact("free_offer"),
            artifact("free_quote"),
            artifact("free_deal"),
            artifact("free_receipt"),
        ]
        result = _run_cli(["-"], stdin_text=json.dumps(docs))
        self.assertEqual(result.returncode, 0, msg=result.stdout + result.stderr)
        report = json.loads(result.stdout)
        self.assertTrue(report["chain"]["valid"])

    def test_tampered_link_breaks_chain_but_not_usage(self) -> None:
        deal = artifact("deal")
        deal["payload"]["workload_hash"] = "ff" * 32
        docs = [artifact("descriptor"), artifact("offer"), artifact("quote"), deal]
        result = _run_cli(["-"], stdin_text=json.dumps(docs))
        self.assertEqual(result.returncode, 1)
        report = json.loads(result.stdout)
        self.assertFalse(report["chain"]["valid"])
        all_codes = {
            issue["code"]
            for rep in report["chain"]["reports"]
            for issue in rep["issues"]
        }
        self.assertIn("workload_hash_mismatch", all_codes)

    def test_now_flag_triggers_expiry_issue(self) -> None:
        fixture = load_fixture()
        quote_expires_at = fixture["artifacts"]["quote"]["artifact"]["payload"][
            "expires_at"
        ]
        docs = [
            artifact("descriptor"),
            artifact("offer"),
            artifact("quote"),
            artifact("deal"),
        ]
        result = _run_cli(
            ["--now", str(quote_expires_at + 1), "-"], stdin_text=json.dumps(docs)
        )
        self.assertEqual(result.returncode, 1)
        report = json.loads(result.stdout)
        all_codes = {
            issue["code"]
            for rep in report["chain"]["reports"]
            for issue in rep["issues"]
        }
        self.assertIn("artifact_expired", all_codes)

    def test_duplicate_required_kind_skips_chain_validation(self) -> None:
        docs = [
            artifact("descriptor"),
            artifact("offer"),
            artifact("offer"),
            artifact("quote"),
            artifact("deal"),
        ]
        result = _run_cli(["-"], stdin_text=json.dumps(docs))
        report = json.loads(result.stdout)
        self.assertIsNone(report["chain"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
