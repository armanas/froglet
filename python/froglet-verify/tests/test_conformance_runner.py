"""Tests for the ``froglet_verify.conformance`` runner's own driver logic:
exit codes, note-vs-warning classification, and the combined
envelope+semantic interpretation of ``artifact_verification_cases``.

Complements ``test_conformance_vectors.py`` (which checks *fixture content*
against this package's output) by checking the *runner itself* -- the CLI
gate wired into ``scripts/strict_checks.sh`` must fail loudly, never
silently, when a fixture's checks don't hold.
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from froglet_verify import conformance

from _helpers import CONFORMANCE_DIR, artifact, load_fixture, x402_verification_case

PACKAGE_ROOT = Path(__file__).resolve().parents[1]


def _write_json(directory: Path, name: str, data: object) -> Path:
    path = directory / name
    path.write_text(json.dumps(data), encoding="utf-8")
    return path


class RealFixturesExitZeroTests(unittest.TestCase):
    """Sanity check: the real, currently-committed fixtures under
    conformance/ must produce a clean (exit 0) run -- this is the "should
    stay green" counterpart to the broken-fixture tests below."""

    def test_conformance_directory_exits_zero(self) -> None:
        exit_code = conformance.main([str(CONFORMANCE_DIR)])
        self.assertEqual(exit_code, 0)

    def test_each_known_fixture_file_has_zero_failures(self) -> None:
        for path in sorted(CONFORMANCE_DIR.glob("*.json")):
            with self.subTest(fixture=path.name):
                tally = conformance.run_fixture_file(path)
                self.assertEqual(tally.failed, 0, msg=f"{path.name}: {tally.failures}")
                self.assertGreater(tally.total, 0, msg=f"{path.name}: ran zero checks")


class BrokenFixtureExitNonzeroTests(unittest.TestCase):
    """The core regression test: a fixture with a deliberately-wrong
    recorded value must make the runner report a failure AND exit nonzero.
    The broken fixture is built fresh in a temp directory for each test run
    (not committed anywhere, and never written under conformance/) so this
    test is self-contained and has no stale state to go wrong."""

    def test_wrong_payload_hash_fails_and_exits_nonzero_via_main(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            fixture = load_fixture()
            broken = json.loads(json.dumps(fixture))  # deep copy via round-trip
            # Corrupt one recorded payload_hash so it no longer matches a
            # fresh recompute -- this must surface as a FAIL, not silently
            # pass.
            broken["artifacts"]["quote"]["payload_hash"] = "00" * 32
            path = _write_json(tmp_path, "broken_kernel_v1.json", broken)

            exit_code = conformance.main([str(path)])
            self.assertEqual(exit_code, 1)

    def test_wrong_payload_hash_is_recorded_as_a_specific_failure(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            fixture = load_fixture()
            broken = json.loads(json.dumps(fixture))
            broken["artifacts"]["quote"]["payload_hash"] = "00" * 32
            path = _write_json(tmp_path, "broken_kernel_v1.json", broken)

            tally = conformance.run_fixture_file(path)
            self.assertGreater(tally.failed, 0)
            self.assertTrue(
                any(
                    "artifacts.quote: payload_hash matches recorded value" in failure
                    for failure in tally.failures
                ),
                tally.failures,
            )

    def test_wrong_artifact_verification_case_expectation_fails(self) -> None:
        # A minimal, self-contained fixture (not derived from kernel_v1.json)
        # whose single artifact_verification_cases entry has an
        # intentionally-wrong `expected_valid`, to pin down the exact
        # section this test is targeting independent of any other fixture
        # content.
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            receipt = artifact("receipt")
            broken_fixture = {
                "artifact_verification_cases": [
                    {
                        "name": "deliberately_wrong_expectation",
                        "artifact_type": "receipt",
                        "artifact": receipt,
                        # This receipt is genuinely valid; asserting False
                        # here is the deliberate defect under test.
                        "expected_valid": False,
                    }
                ]
            }
            path = _write_json(tmp_path, "broken_cases.json", broken_fixture)

            exit_code = conformance.main([str(path)])
            self.assertEqual(exit_code, 1)

            tally = conformance.run_fixture_file(path)
            self.assertEqual(tally.failed, 1)
            self.assertIn("deliberately_wrong_expectation", tally.failures[0])

    def test_broken_fixture_exits_nonzero_via_actual_subprocess(self) -> None:
        # End-to-end through the real `python -m froglet_verify.conformance`
        # entry point (not just calling main() in-process), so this also
        # catches a regression in the `if __name__ == "__main__"` guard
        # itself or in how the process's actual exit status is set.
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            fixture = load_fixture()
            broken = json.loads(json.dumps(fixture))
            broken["artifacts"]["quote"]["payload_hash"] = "00" * 32
            path = _write_json(tmp_path, "broken_kernel_v1.json", broken)

            result = subprocess.run(
                [sys.executable, "-m", "froglet_verify.conformance", str(path)],
                cwd=PACKAGE_ROOT,
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 1, msg=result.stdout + result.stderr)
            self.assertIn("FAIL", result.stdout)

    def test_directory_with_one_broken_and_one_clean_fixture_exits_nonzero(
        self,
    ) -> None:
        # Mirrors the real invocation shape (`conformance.main(["conformance/"])`
        # scanning a directory of multiple fixture files): one broken file
        # among otherwise-clean ones must still fail the whole run.
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            fixture = load_fixture()

            clean = json.loads(json.dumps(fixture))
            _write_json(tmp_path, "aaa_clean.json", clean)

            broken = json.loads(json.dumps(fixture))
            broken["artifacts"]["quote"]["payload_hash"] = "00" * 32
            _write_json(tmp_path, "zzz_broken.json", broken)

            exit_code = conformance.main([str(tmp_path)])
            self.assertEqual(exit_code, 1)


class NoteVersusWarningTests(unittest.TestCase):
    """A section that is simply *absent* is a note (nothing wrong); a
    section present with an unrecognized shape is a warning (something
    might be wrong). These must not be conflated."""

    def test_absent_invoice_bundle_section_is_a_note_not_a_warning(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            # A single-leg-style fixture: artifacts + verification cases,
            # deliberately no invoice_bundle_validation_cases key at all.
            minimal = {
                "artifacts": {},
                "artifact_verification_cases": [],
            }
            path = _write_json(tmp_path, "single_leg.json", minimal)

            tally = conformance.run_fixture_file(path)
            self.assertEqual(tally.failed, 0)
            self.assertTrue(
                any("invoice_bundle_validation_cases" in note for note in tally.notes)
            )
            self.assertFalse(
                any(
                    "invoice_bundle_validation_cases" in warning
                    for warning in tally.warnings
                )
            )

    def test_malformed_invoice_bundle_section_is_a_warning(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            malformed = {
                "artifacts": {},
                "invoice_bundle_validation_cases": "not-a-list",
            }
            path = _write_json(tmp_path, "malformed.json", malformed)

            tally = conformance.run_fixture_file(path)
            self.assertTrue(
                any(
                    "invoice_bundle_validation_cases" in warning
                    for warning in tally.warnings
                )
            )
            self.assertFalse(
                any("invoice_bundle_validation_cases" in note for note in tally.notes)
            )


class CombinedEnvelopeAndSemanticCasesTests(unittest.TestCase):
    """Focused regression test for the permissiveness defect: a case whose
    artifact is validly *signed* but semantically invalid must be reported
    as invalid.

    This package is verify-only (no signing capability), so it cannot mint
    a fresh "envelope-valid, semantics-invalid" document on the fly; the
    only such real, validly-signed documents on hand are the x402 reject
    cases in ``conformance/x402_v1.json`` (an envelope-invalid document, by
    contrast, is trivial to build by tampering a payload post-signature,
    which is exactly what ``test_wrong_payload_hash_...`` above already
    covers). Rather than depend on that file's ``artifact_verification_cases``
    array staying shaped the way this test expects, this lifts one concrete
    signed artifact out of it into a minimal, self-contained fixture of this
    test's own construction.
    """

    def test_envelope_valid_but_semantically_invalid_case_is_rejected(self) -> None:
        case = x402_verification_case("x402_receipt_missing_bundle_hash")
        self.assertFalse(case["expected_valid"])

        # Confirm the premise directly: signature/hash integrity holds, but
        # the settlement-method semantics do not.
        envelope_ok, _reason = conformance.envelope.verify_signed_artifact(
            case["artifact"]
        )
        self.assertTrue(envelope_ok, "premise of this test requires a valid envelope")

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            case_fixture = {"artifact_verification_cases": [case]}
            path = _write_json(tmp_path, "semantic_only_case.json", case_fixture)
            tally = conformance.run_fixture_file(path)
            self.assertEqual(tally.failed, 0, tally.failures)
            self.assertEqual(tally.passed, 1)

    def test_envelope_valid_and_semantically_valid_case_is_accepted(self) -> None:
        case = x402_verification_case("x402_receipt_valid")
        self.assertTrue(case["expected_valid"])

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            case_fixture = {"artifact_verification_cases": [case]}
            path = _write_json(tmp_path, "semantic_only_case.json", case_fixture)
            tally = conformance.run_fixture_file(path)
            self.assertEqual(tally.failed, 0, tally.failures)
            self.assertEqual(tally.passed, 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
