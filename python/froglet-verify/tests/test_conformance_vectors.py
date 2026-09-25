"""Fixture-driven conformance tests against conformance/kernel_v1.json.

Mirrors the assertions in ``tests/kernel_conformance_vectors.rs``'s
``kernel_conformance_artifact_verification_cases_match_expectations`` and
``kernel_conformance_invoice_bundle_validation_cases_match_expectations``:
every ``artifact_verification_cases`` entry's envelope-only outcome must
match ``expected_valid``, and every ``invoice_bundle_validation_cases``
entry's validity *and issue-code set* must match exactly.

This exercises the same code paths as
``python -m froglet_verify.conformance``, just as ``unittest`` assertions
instead of a CLI run, so ``python3 -m unittest discover`` alone is enough to
catch a regression without needing to separately invoke the conformance
runner.
"""

from __future__ import annotations

import unittest

from froglet_verify.envelope import verify_signed_artifact
from froglet_verify.conformance import _validate_lightning_invoice_bundle_case

from _helpers import load_fixture


class ArtifactVerificationCaseTests(unittest.TestCase):
    def test_every_case_matches_expected_valid(self) -> None:
        fixture = load_fixture()
        cases = fixture["artifact_verification_cases"]
        self.assertGreater(
            len(cases), 0, "fixture must actually contain verification cases"
        )
        for case in cases:
            with self.subTest(name=case["name"]):
                ok, _reason = verify_signed_artifact(case["artifact"])
                self.assertEqual(ok, case["expected_valid"], case["name"])


class InvoiceBundleValidationCaseTests(unittest.TestCase):
    def test_every_case_matches_expected_valid_and_issue_codes(self) -> None:
        fixture = load_fixture()
        cases = fixture["invoice_bundle_validation_cases"]
        self.assertGreater(
            len(cases),
            0,
            "fixture must actually contain invoice_bundle validation cases",
        )

        quote = fixture["artifacts"]["quote"]["artifact"]
        deal = fixture["artifacts"]["deal"]["artifact"]

        for case in cases:
            with self.subTest(name=case["name"]):
                report = _validate_lightning_invoice_bundle_case(
                    case["bundle"], quote, deal, case.get("expected_requester_id")
                )
                self.assertEqual(report["valid"], case["expected_valid"], case["name"])
                actual_codes = {issue["code"] for issue in report["issues"]}
                expected_codes = set(case["expected_issue_codes"])
                self.assertEqual(actual_codes, expected_codes, case["name"])
                self.assertEqual(report["quote_hash"], quote["hash"])
                self.assertEqual(report["deal_hash"], deal["hash"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
