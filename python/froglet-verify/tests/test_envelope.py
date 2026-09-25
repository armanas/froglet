"""Envelope verification tests: JCS + BIP-340 wired together against the
frozen conformance fixture, plus tampering tests (any single-field mutation
must flip verification to invalid).

Mirrors ``froglet-protocol/src/protocol/kernel.rs``'s
``randomized_artifact_tampering_breaks_verification`` test in spirit
(schema_version, artifact_type, created_at, payload, payload_hash, hash,
signer, signature tampering must each break verification) and
``kernel_conformance_vectors.rs``'s ``assert_artifact_vector`` (every fixture
artifact's payload_hash/canonical_signing_bytes_hex/artifact_hash must match
exactly, and the envelope must verify).
"""

from __future__ import annotations

import unittest

from froglet_verify.envelope import (
    FROGLET_SCHEMA_V1,
    artifact_hash,
    canonical_signing_bytes,
    payload_hash,
    verify_signed_artifact,
)

from _helpers import artifact, load_fixture


class FixtureArtifactVectorTests(unittest.TestCase):
    """Every artifact vector in kernel_v1.json passes envelope + hash +
    signing-bytes byte-equality checks."""

    def test_every_artifact_vector_matches_exactly(self) -> None:
        fixture = load_fixture()
        for name, vector in fixture["artifacts"].items():
            with self.subTest(artifact=name):
                doc = vector["artifact"]

                self.assertEqual(payload_hash(doc["payload"]), vector["payload_hash"])

                signing_bytes = canonical_signing_bytes(
                    doc["schema_version"],
                    doc["artifact_type"],
                    doc["signer"],
                    doc["created_at"],
                    doc["payload_hash"],
                    doc["payload"],
                )
                self.assertEqual(
                    signing_bytes.hex(), vector["canonical_signing_bytes_hex"]
                )
                self.assertEqual(artifact_hash(doc), vector["artifact_hash"])

                ok, reason = verify_signed_artifact(doc)
                self.assertTrue(
                    ok, msg=f"envelope failed to verify for {name}: {reason}"
                )
                self.assertIsNone(reason)

    def test_schema_version_is_froglet_v1_for_every_vector(self) -> None:
        fixture = load_fixture()
        for name, vector in fixture["artifacts"].items():
            with self.subTest(artifact=name):
                self.assertEqual(
                    vector["artifact"]["schema_version"], FROGLET_SCHEMA_V1
                )


class TamperingBreaksVerificationTests(unittest.TestCase):
    """Mirrors kernel.rs's `randomized_artifact_tampering_breaks_verification`:
    flipping any one envelope field must flip verification to invalid."""

    def setUp(self) -> None:
        self.quote = artifact("quote")
        ok, reason = verify_signed_artifact(self.quote)
        self.assertTrue(ok, msg=f"fixture quote must verify before tampering: {reason}")

    def test_baseline_verifies(self) -> None:
        ok, _reason = verify_signed_artifact(self.quote)
        self.assertTrue(ok)

    def test_tampered_schema_version(self) -> None:
        doc = dict(self.quote)
        doc["schema_version"] = "froglet/v2"
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        self.assertIsNotNone(reason)

    def test_tampered_artifact_type(self) -> None:
        doc = dict(self.quote)
        doc["artifact_type"] = "deal"
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_created_at(self) -> None:
        doc = dict(self.quote)
        doc["created_at"] = doc["created_at"] + 1
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_signer(self) -> None:
        doc = dict(self.quote)
        doc["signer"] = "11" * 32
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_signature(self) -> None:
        doc = dict(self.quote)
        sig = doc["signature"]
        doc["signature"] = ("1" if sig[0] == "0" else "0") + sig[1:]
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_payload_hash(self) -> None:
        doc = dict(self.quote)
        ph = doc["payload_hash"]
        doc["payload_hash"] = ("1" if ph[0] == "0" else "0") + ph[1:]
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_hash(self) -> None:
        doc = dict(self.quote)
        h = doc["hash"]
        doc["hash"] = ("1" if h[0] == "0" else "0") + h[1:]
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_tampered_payload_field(self) -> None:
        doc = dict(self.quote)
        doc["payload"] = dict(doc["payload"])
        doc["payload"]["workload_hash"] = "ff" * 32
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_missing_hash_field_defaults_to_empty_and_fails(self) -> None:
        # Mirrors kernel.rs's `signed_artifact_with_empty_hash_is_rejected`:
        # `hash` defaults to "" when absent (matching the Rust struct's
        # `#[serde(default, skip_serializing_if = "String::is_empty")]`),
        # which then fails to match the recomputed hash.
        doc = dict(self.quote)
        del doc["hash"]
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        self.assertIsNotNone(reason)


class MalformedEnvelopeTests(unittest.TestCase):
    def test_non_dict_input_returns_false_reason(self) -> None:
        ok, reason = verify_signed_artifact([])  # type: ignore[arg-type]
        self.assertFalse(ok)
        self.assertIsNotNone(reason)

    def test_missing_required_field_returns_false_reason(self) -> None:
        doc = artifact("quote")
        del doc["signer"]
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        self.assertIsNotNone(reason)

    def test_non_integer_created_at_returns_false_reason(self) -> None:
        doc = artifact("quote")
        doc["created_at"] = "not-a-number"
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        self.assertIsNotNone(reason)

    def test_bool_created_at_is_rejected_not_treated_as_int(self) -> None:
        # bool is an int subclass in Python; created_at=True must not be
        # silently accepted as created_at=1.
        doc = artifact("quote")
        doc["created_at"] = True
        ok, _reason = verify_signed_artifact(doc)
        self.assertFalse(ok)

    def test_wrong_schema_version_reports_reason(self) -> None:
        doc = artifact("quote")
        doc["schema_version"] = "froglet/v0"
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        assert reason is not None
        self.assertIn("schema_version", reason)

    def test_payload_with_float_fails_closed_rather_than_raising(self) -> None:
        doc = artifact("quote")
        doc["payload"] = dict(doc["payload"])
        doc["payload"]["expires_at"] = 1.5
        ok, reason = verify_signed_artifact(doc)
        self.assertFalse(ok)
        self.assertIsNotNone(reason)


if __name__ == "__main__":
    unittest.main(verbosity=2)
