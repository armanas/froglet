import json
import unittest
from pathlib import Path

from test_support import (
    canonical_artifact_signing_bytes,
    canonical_json_bytes,
    sha256_hex,
    verify_signed_artifact,
    workload_hash_from_submission,
)


FIXTURE_PATH = Path(__file__).resolve().parents[2] / "conformance" / "kernel_v1.json"


def load_fixture() -> dict:
    return json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))


class KernelConformanceVectorTests(unittest.TestCase):
    def test_artifact_signing_vectors_match_exact_bytes(self) -> None:
        fixture = load_fixture()
        for name, vector in fixture["artifacts"].items():
            artifact = vector["artifact"]
            signing_bytes = canonical_artifact_signing_bytes(artifact)
            self.assertEqual(
                signing_bytes.hex(),
                vector["canonical_signing_bytes_hex"],
                name,
            )
            self.assertEqual(
                sha256_hex(canonical_json_bytes(artifact["payload"])),
                vector["payload_hash"],
                name,
            )
            self.assertEqual(sha256_hex(signing_bytes), vector["artifact_hash"], name)
            self.assertTrue(verify_signed_artifact(artifact), name)

    def test_linked_identity_challenge_vector_is_stable(self) -> None:
        fixture = load_fixture()
        linked = fixture["linked_identity"]
        scope_hash = sha256_hex(canonical_json_bytes(linked["scope"]))
        challenge_utf8 = (
            "froglet:identity_link:v1\n"
            f"{linked['provider_id']}\n"
            f"{linked['identity_kind']}\n"
            f"{linked['identity']}\n"
            f"{scope_hash}\n"
            f"{linked['created_at']}\n"
            f"{linked['expires_at'] if linked['expires_at'] is not None else '-'}"
        )
        self.assertEqual(scope_hash, linked["scope_hash"])
        self.assertEqual(challenge_utf8, linked["challenge_utf8"])
        self.assertEqual(challenge_utf8.encode().hex(), linked["challenge_hex"])

    def test_artifact_verification_cases_match_expectations(self) -> None:
        fixture = load_fixture()
        for case in fixture["artifact_verification_cases"]:
            self.assertEqual(
                verify_signed_artifact(case["artifact"]),
                case["expected_valid"],
                case["name"],
            )

    def test_workload_and_result_hash_vectors_match(self) -> None:
        fixture = load_fixture()
        workload_hash = workload_hash_from_submission(fixture["workload_spec"]["submission"])
        result_hash = sha256_hex(canonical_json_bytes(fixture["result"]))
        quote = fixture["artifacts"]["quote"]["artifact"]
        receipt = fixture["artifacts"]["receipt"]["artifact"]

        self.assertEqual(workload_hash, quote["payload"]["workload_hash"])
        self.assertEqual(result_hash, receipt["payload"]["result_hash"])
        self.assertEqual(
            quote["payload"]["requester_id"], fixture["keys"]["requester_id"]
        )
        self.assertEqual(
            receipt["payload"]["settlement_refs"]["bundle_hash"],
            fixture["artifacts"]["invoice_bundle"]["artifact"]["hash"],
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
