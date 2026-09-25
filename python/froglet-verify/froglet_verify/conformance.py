"""Conformance fixture runner: ``python -m froglet_verify.conformance <dir-or-file>``.

Iterates conformance fixture JSON files (frozen fixtures like
``conformance/kernel_v1.json`` and ``conformance/x402_v1.json``) and checks
this package's output against what they record. Fixture *sections* are
handled independently and optionally, so a fixture missing a section (or a
future fixture with a different or partial shape) is tolerated -- as an
informational note when the section is simply absent (nothing to check,
nothing wrong), or as a warning when a section is present but doesn't match
any shape this runner knows how to interpret (worth a human's attention,
but not by itself a conformance failure):

- ``artifacts``: a mapping of name -> {"artifact", "payload_hash",
  "canonical_signing_bytes_hex", "artifact_hash"}. Handled generically over
  *whatever* keys are present (not a hardcoded list of names), so it also
  covers fixtures that add more named vectors in the same shape (e.g.
  x402_v1.json's five, vs. kernel_v1.json's ten). For each vector: the
  recomputed payload_hash must match the recorded one, the recomputed JCS
  signing bytes must match ``canonical_signing_bytes_hex`` byte-for-byte,
  the recomputed artifact hash must match ``artifact_hash``, and the
  envelope must verify.
- ``artifact_verification_cases``: each ``{"name", "artifact_type",
  "artifact", "expected_valid"}`` is checked against the *combined*
  envelope + per-kind-semantic outcome (mirroring the strong-by-default
  ``verify_typed_document`` in ``kernel.rs``, generalized here to all six
  kinds via this package's own :mod:`froglet_verify.semantics` -- Rust's
  ``verify_typed_document`` itself only covers descriptor/offer/receipt
  today). This one function serves both fixtures' reference tests exactly:
  ``kernel_v1.json``'s cases are all envelope-level tampering (so combining
  in the semantic check changes nothing there -- verified empirically
  against all 14 cases), while ``x402_v1.json``'s reject cases are
  semantic-only violations of a validly-signed receipt (so the semantic
  check is load-bearing there, matching
  ``x402_artifact_verification_cases_match_expectations`` in
  ``tests/x402_conformance_vectors.rs``, which calls
  ``verify_typed_document`` rather than the bare ``verify_artifact`` that
  ``kernel_conformance_vectors.rs``'s equivalent test uses).
- ``invoice_bundle_validation_cases``: each ``{"name", "bundle",
  "expected_requester_id", "expected_valid", "expected_issue_codes"}`` is
  checked against a private reimplementation of
  ``settlement::validate_lightning_invoice_bundle`` (see the module
  docstring on ``_validate_lightning_invoice_bundle_case`` below for why
  that logic lives here rather than in the public ``chain``/``semantics``
  API). Absent entirely for single-leg methods like x402.eip3009.v1, which
  have no invoice_bundle transport artifact -- that absence is a note, not
  a warning.

Exits 0 if every check in every recognized section of every fixture passed,
1 if any check failed, 2 on a usage error (bad arguments, unreadable path).
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, NamedTuple

from . import envelope, semantics

__all__ = ["run_fixture_file", "main"]

# Mirrors kernel.rs's `verify_typed_document` dispatch, generalized to all
# six kinds this package has a semantic validator for (Rust's own
# `verify_typed_document` currently only covers descriptor/offer/receipt;
# quote/deal/invoice_bundle fall through to `VerifyError::UnknownKind`
# there). Confirmed empirically to change none of kernel_v1.json's 14
# existing `artifact_verification_cases` outcomes (its cases are all
# envelope-level tampering) while fixing x402_v1.json's semantic-only
# reject cases.
_SEMANTIC_VALIDATORS = {
    "descriptor": semantics.validate_descriptor_artifact,
    "offer": semantics.validate_offer_artifact,
    "quote": semantics.validate_quote_artifact,
    "deal": semantics.validate_deal_artifact,
    "invoice_bundle": semantics.validate_invoice_bundle_artifact,
    "receipt": semantics.validate_receipt_artifact,
}


class _Tally:
    """Accumulates check outcomes for one fixture file.

    ``notes`` are informational: a fixture section is simply absent, which
    is expected and fine (e.g. no ``invoice_bundle_validation_cases`` in a
    single-leg-method fixture). ``warnings`` mean a section *is* present but
    doesn't match a shape this runner recognizes -- worth a human's
    attention, since it might indicate a real gap in this runner's
    coverage, but not itself counted as a failed check.
    """

    def __init__(self) -> None:
        self.passed = 0
        self.failed = 0
        self.failures: list[str] = []
        self.notes: list[str] = []
        self.warnings: list[str] = []

    def check(self, description: str, ok: bool) -> None:
        if ok:
            self.passed += 1
        else:
            self.failed += 1
            self.failures.append(description)

    def note(self, message: str) -> None:
        self.notes.append(message)

    def warn(self, message: str) -> None:
        self.warnings.append(message)

    @property
    def total(self) -> int:
        return self.passed + self.failed


# ─── artifact-vector section ("artifacts": {name: {artifact, payload_hash,
# canonical_signing_bytes_hex, artifact_hash}}) ──────────────────────────────


def _is_artifact_vector(value: Any) -> bool:
    return (
        isinstance(value, dict)
        and isinstance(value.get("artifact"), dict)
        and isinstance(value.get("payload_hash"), str)
        and isinstance(value.get("canonical_signing_bytes_hex"), str)
        and isinstance(value.get("artifact_hash"), str)
    )


def _check_artifact_vectors(fixture: dict[str, Any], tally: _Tally) -> None:
    artifacts = fixture.get("artifacts")
    if artifacts is None:
        tally.note("no 'artifacts' section present; skipping artifact-vector checks")
        return
    if not isinstance(artifacts, dict):
        tally.warn(
            f"'artifacts' section has unexpected shape ({type(artifacts).__name__}); skipping"
        )
        return

    for name, vector in artifacts.items():
        if not _is_artifact_vector(vector):
            tally.warn(
                f"artifacts.{name} does not match the known artifact-vector shape; skipping"
            )
            continue
        artifact = vector["artifact"]

        try:
            recomputed_payload_hash = envelope.payload_hash(artifact["payload"])
        except (KeyError, TypeError, ValueError) as error:
            tally.check(
                f"artifacts.{name}: payload_hash recompute raised {error!r}", False
            )
            continue
        tally.check(
            f"artifacts.{name}: payload_hash matches recorded value",
            recomputed_payload_hash == vector["payload_hash"],
        )

        try:
            signing_bytes = envelope.canonical_signing_bytes(
                artifact["schema_version"],
                artifact["artifact_type"],
                artifact["signer"],
                artifact["created_at"],
                artifact["payload_hash"],
                artifact["payload"],
            )
        except (KeyError, TypeError, ValueError) as error:
            tally.check(
                f"artifacts.{name}: canonical_signing_bytes recompute raised {error!r}",
                False,
            )
            continue
        tally.check(
            f"artifacts.{name}: canonical_signing_bytes_hex matches byte-for-byte",
            signing_bytes.hex() == vector["canonical_signing_bytes_hex"],
        )

        try:
            recomputed_artifact_hash = envelope.artifact_hash(artifact)
        except (KeyError, TypeError, ValueError) as error:
            tally.check(
                f"artifacts.{name}: artifact_hash recompute raised {error!r}", False
            )
        else:
            tally.check(
                f"artifacts.{name}: artifact_hash matches recorded value",
                recomputed_artifact_hash == vector["artifact_hash"],
            )

        envelope_ok, reason = envelope.verify_signed_artifact(artifact)
        tally.check(
            (
                f"artifacts.{name}: envelope verifies ({reason})"
                if not envelope_ok
                else f"artifacts.{name}: envelope verifies"
            ),
            envelope_ok,
        )


# ─── artifact_verification_cases section ────────────────────────────────────


def _verify_typed(doc: dict[str, Any], artifact_type: Any) -> tuple[bool, str | None]:
    """Combined envelope + per-kind-semantic outcome for one artifact,
    generalizing kernel.rs's strong-by-default ``verify_typed_document`` to
    all six kinds this package has a semantic validator for (see the module
    docstring for why this, rather than bare envelope verification, is the
    correct interpretation of ``artifact_verification_cases``)."""
    envelope_ok, envelope_reason = envelope.verify_signed_artifact(doc)
    if not envelope_ok:
        return False, envelope_reason
    validator = (
        _SEMANTIC_VALIDATORS.get(artifact_type)
        if isinstance(artifact_type, str)
        else None
    )
    if validator is None:
        return True, None
    semantic_reason = validator(doc)
    if semantic_reason is not None:
        return False, semantic_reason
    return True, None


def _check_artifact_verification_cases(fixture: dict[str, Any], tally: _Tally) -> None:
    cases = fixture.get("artifact_verification_cases")
    if cases is None:
        tally.note("no 'artifact_verification_cases' section present; skipping")
        return
    if not isinstance(cases, list):
        tally.warn(
            f"'artifact_verification_cases' has unexpected shape ({type(cases).__name__}); skipping"
        )
        return

    for case in cases:
        name = case.get("name", "<unnamed>")
        try:
            ok, reason = _verify_typed(case["artifact"], case.get("artifact_type"))
        except (KeyError, TypeError, ValueError) as error:
            tally.check(f"artifact_verification_cases[{name}]: raised {error!r}", False)
            continue
        expected = case["expected_valid"]
        description = (
            f"artifact_verification_cases[{name}]: expected_valid={expected!r}"
        )
        if ok != expected:
            description += f" (got {ok!r}: {reason})"
        tally.check(description, ok == expected)


# ─── invoice_bundle_validation_cases section ────────────────────────────────
#
# `invoice_bundle_validation_cases` in kernel_v1.json is generated by, and
# checked against, `settlement::validate_lightning_invoice_bundle` in
# `src/settlement/lightning.rs` -- a *closed-source marketplace-layer*
# function (see `tests/kernel_conformance_vectors.rs`,
# `kernel_conformance_invoice_bundle_validation_cases_match_expectations`),
# not anything in the open-source `froglet-protocol` crate this package's
# `chain`/`semantics` modules otherwise mirror. Reproducing it is still
# necessary for byte-for-byte conformance against this fixture, so the
# reimplementation lives here, private to the conformance runner, rather
# than in the public API -- general invoice_bundle chain validation for
# real usage should go through `chain.validate_quote_invoice_bundle_deal`
# instead, which mirrors the protocol-layer `chain.rs` semantics.


class _DecodedMockInvoice(NamedTuple):
    prefix: str
    amount_msat: int
    payment_hash: str
    expires_at: int


_MOCK_BOLT11_PREFIX = "lnmock-"


def _parse_mock_bolt11(invoice: str) -> _DecodedMockInvoice | None:
    if not invoice.startswith(_MOCK_BOLT11_PREFIX):
        return None
    parts = invoice[len(_MOCK_BOLT11_PREFIX) :].split("-", 3)
    if len(parts) != 4:
        return None
    prefix, amount_str, payment_hash, expires_str = parts
    try:
        return _DecodedMockInvoice(
            prefix, int(amount_str), payment_hash, int(expires_str)
        )
    except ValueError:
        return None


def _decode_lightning_invoice(invoice: str) -> _DecodedMockInvoice:
    """Decode a Froglet mock ``lnmock-...`` invoice. Real BOLT11 (bech32 +
    tagged fields) is intentionally not implemented here -- see the parity
    notes in the project report; every invoice in ``kernel_v1.json`` uses
    the mock format, so this suffices for conformance against this fixture.
    """
    decoded = _parse_mock_bolt11(invoice)
    if decoded is None:
        raise ValueError(
            "real BOLT11 invoice decoding is not implemented by this pure-stdlib "
            "reference verifier (only the 'lnmock-' fixture format is supported)"
        )
    return decoded


def _push(issues: list[dict[str, str]], code: str, message: str) -> None:
    issues.append({"code": code, "message": message})


def _validate_lightning_invoice_bundle_case(
    bundle: dict[str, Any],
    quote: dict[str, Any],
    deal: dict[str, Any],
    expected_requester_id: str | None,
) -> dict[str, Any]:
    """Mirrors ``settlement::validate_lightning_invoice_bundle`` (see the
    module-level note above)."""
    issues: list[dict[str, str]] = []

    if not envelope.verify_signed_artifact(bundle)[0]:
        _push(issues, "invalid_bundle_signature", "invoice bundle signature is invalid")
    if not envelope.verify_signed_artifact(quote)[0]:
        _push(issues, "invalid_quote_signature", "quote signature is invalid")
    if not envelope.verify_signed_artifact(deal)[0]:
        _push(issues, "invalid_deal_signature", "deal signature is invalid")

    if bundle["artifact_type"] != "invoice_bundle":
        _push(issues, "bundle_kind_mismatch", "expected bundle kind 'invoice_bundle'")

    quote_payload = quote["payload"]
    bundle_payload = bundle["payload"]
    deal_payload = deal["payload"]
    settlement_terms = quote_payload["settlement_terms"]

    if (
        quote_payload["provider_id"] != quote["signer"]
        or bundle_payload["provider_id"] != quote_payload["provider_id"]
        or bundle["signer"] != quote["signer"]
    ):
        _push(
            issues,
            "provider_identity_mismatch",
            "invoice bundle provider identity does not match the quoted provider",
        )
    if deal_payload["provider_id"] != quote_payload["provider_id"]:
        _push(
            issues,
            "deal_provider_mismatch",
            "deal provider does not match the quoted provider",
        )
    if bundle_payload["quote_hash"] != quote["hash"]:
        _push(
            issues,
            "quote_hash_mismatch",
            "invoice bundle quote_hash does not match the quote artifact hash",
        )
    if bundle_payload["deal_hash"] != deal["hash"]:
        _push(
            issues,
            "deal_hash_mismatch",
            "invoice bundle deal_hash does not match the deal artifact hash",
        )
    if (
        expected_requester_id is not None
        and bundle_payload["requester_id"] != expected_requester_id
    ):
        _push(
            issues,
            "requester_id_mismatch",
            "invoice bundle requester_id does not match the expected requester",
        )
    if settlement_terms["method"] != "lightning.base_fee_plus_success_fee.v1":
        _push(
            issues,
            "quote_payment_method_mismatch",
            "quote does not advertise lightning settlement",
        )
    if (
        deal["signer"] != deal_payload["requester_id"]
        or deal_payload["requester_id"] != quote_payload["requester_id"]
        or bundle_payload["requester_id"] != quote_payload["requester_id"]
    ):
        _push(
            issues,
            "requester_identity_mismatch",
            "deal or bundle requester does not match the quoted requester",
        )
    if (
        deal_payload["quote_hash"] != quote["hash"]
        or deal_payload["workload_hash"] != quote_payload["workload_hash"]
    ):
        _push(
            issues,
            "deal_quote_binding_mismatch",
            "deal artifact does not match the quoted workload commitment",
        )
    if (
        bundle_payload["destination_identity"]
        != settlement_terms["destination_identity"]
    ):
        _push(
            issues,
            "destination_identity_mismatch",
            "invoice bundle destination identity does not match the quoted settlement destination",
        )
    if bundle_payload["base_fee"]["amount_msat"] != settlement_terms["base_fee_msat"]:
        _push(
            issues,
            "base_fee_mismatch",
            "invoice bundle base fee does not match the quote settlement terms",
        )
    if (
        bundle_payload["success_fee"]["amount_msat"]
        != settlement_terms["success_fee_msat"]
    ):
        _push(
            issues,
            "success_fee_mismatch",
            "invoice bundle success fee does not match the quote settlement terms",
        )
    if (
        bundle_payload["min_final_cltv_expiry"]
        != settlement_terms["min_final_cltv_expiry"]
    ):
        _push(
            issues,
            "min_final_cltv_mismatch",
            "invoice bundle CLTV requirement does not match the quote settlement terms",
        )
    if bundle_payload["expires_at"] > quote_payload["expires_at"]:
        _push(
            issues,
            "bundle_expiry_exceeds_quote",
            "invoice bundle expires after the quote deadline",
        )
    if bundle_payload["expires_at"] > deal_payload["admission_deadline"]:
        _push(
            issues,
            "bundle_expiry_exceeds_admission_deadline",
            "invoice bundle expires after the deal admission_deadline",
        )
    if (
        deal_payload["success_payment_hash"]
        != bundle_payload["success_fee"]["payment_hash"]
    ):
        _push(
            issues,
            "success_payment_hash_mismatch",
            "invoice bundle success payment hash does not match the deal commitment",
        )

    for expected_prefix, leg, max_expiry_secs, leg_name in (
        (
            "base",
            bundle_payload["base_fee"],
            settlement_terms["max_base_invoice_expiry_secs"],
            "base_fee",
        ),
        (
            "hold",
            bundle_payload["success_fee"],
            settlement_terms["max_success_hold_expiry_secs"],
            "success_fee",
        ),
    ):
        expected_invoice_hash = envelope.sha256_hex(
            leg["invoice_bolt11"].encode("utf-8")
        )
        if leg["invoice_hash"] != expected_invoice_hash:
            _push(
                issues,
                "invoice_hash_mismatch",
                f"{leg_name} invoice_hash does not match the encoded invoice",
            )

        try:
            decoded = _decode_lightning_invoice(leg["invoice_bolt11"])
        except ValueError as error:
            _push(issues, "invalid_invoice_encoding", f"{leg_name}: {error}")
            continue

        if decoded.prefix != expected_prefix:
            _push(
                issues,
                "invoice_prefix_mismatch",
                f"{leg_name} invoice prefix does not match the expected leg type",
            )
        if decoded.amount_msat != leg["amount_msat"]:
            _push(
                issues,
                "invoice_amount_mismatch",
                f"{leg_name} invoice amount does not match the signed bundle",
            )
        if decoded.payment_hash != leg["payment_hash"]:
            _push(
                issues,
                "invoice_payment_hash_mismatch",
                f"{leg_name} invoice payment hash does not match the signed bundle",
            )
        if decoded.expires_at > bundle["created_at"] + max_expiry_secs:
            _push(
                issues,
                "invoice_expiry_exceeds_terms",
                f"{leg_name} invoice expiry exceeds the quoted settlement constraints",
            )
        if decoded.expires_at > quote_payload["expires_at"]:
            _push(
                issues,
                "invoice_expiry_exceeds_quote",
                f"{leg_name} invoice expiry exceeds the quote deadline",
            )
        if decoded.expires_at > deal_payload["admission_deadline"]:
            _push(
                issues,
                "invoice_expiry_exceeds_admission_deadline",
                f"{leg_name} invoice expiry exceeds the deal admission_deadline",
            )

    return {
        "valid": len(issues) == 0,
        "bundle_hash": bundle["hash"],
        "quote_hash": quote["hash"],
        "deal_hash": deal["hash"],
        "expected_requester_id": expected_requester_id,
        "issues": issues,
    }


def _check_invoice_bundle_validation_cases(
    fixture: dict[str, Any], tally: _Tally
) -> None:
    cases = fixture.get("invoice_bundle_validation_cases")
    if cases is None:
        # Absent entirely for single-leg settlement methods (e.g.
        # x402.eip3009.v1), which have no invoice_bundle transport artifact
        # at all -- that is expected, not a gap in this runner.
        tally.note("no 'invoice_bundle_validation_cases' section present; skipping")
        return
    if not isinstance(cases, list):
        tally.warn(
            f"'invoice_bundle_validation_cases' has unexpected shape ({type(cases).__name__}); skipping"
        )
        return

    artifacts = fixture.get("artifacts")
    if (
        not isinstance(artifacts, dict)
        or "quote" not in artifacts
        or "deal" not in artifacts
    ):
        tally.warn(
            "'invoice_bundle_validation_cases' present but fixture has no artifacts.quote/"
            "artifacts.deal to validate bundles against; skipping"
        )
        return
    quote = artifacts["quote"]["artifact"]
    deal = artifacts["deal"]["artifact"]

    for case in cases:
        name = case.get("name", "<unnamed>")
        try:
            report = _validate_lightning_invoice_bundle_case(
                case["bundle"], quote, deal, case.get("expected_requester_id")
            )
        except (KeyError, TypeError, ValueError) as error:
            tally.check(
                f"invoice_bundle_validation_cases[{name}]: raised {error!r}", False
            )
            continue
        tally.check(
            f"invoice_bundle_validation_cases[{name}]: valid={case['expected_valid']!r}",
            report["valid"] == case["expected_valid"],
        )
        actual_codes = {issue["code"] for issue in report["issues"]}
        expected_codes = set(case.get("expected_issue_codes", []))
        tally.check(
            f"invoice_bundle_validation_cases[{name}]: issue codes match "
            f"(expected={sorted(expected_codes)}, actual={sorted(actual_codes)})",
            actual_codes == expected_codes,
        )


# ─── driver ──────────────────────────────────────────────────────────────────


def run_fixture_file(path: Path) -> _Tally:
    """Run every recognized section's checks against the fixture at ``path``.

    Returns a :class:`_Tally` carrying pass/fail counts plus any informational
    notes and warnings accumulated along the way (see :class:`_Tally` for the
    distinction between the two).
    """
    tally = _Tally()

    try:
        fixture = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        tally.check(f"{path}: failed to read/parse as JSON ({error})", False)
        return tally

    if not isinstance(fixture, dict):
        tally.note(f"{path}: top-level JSON is not an object; nothing to check")
        return tally

    _check_artifact_vectors(fixture, tally)
    _check_artifact_verification_cases(fixture, tally)
    _check_invoice_bundle_validation_cases(fixture, tally)

    return tally


def _iter_fixture_paths(target: Path) -> list[Path]:
    if target.is_dir():
        return sorted(target.glob("*.json"))
    return [target]


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if len(argv) != 1:
        print(
            "usage: python -m froglet_verify.conformance <dir-or-file>", file=sys.stderr
        )
        return 2

    target = Path(argv[0])
    if not target.exists():
        print(f"error: no such file or directory: {target}", file=sys.stderr)
        return 2

    paths = _iter_fixture_paths(target)
    if not paths:
        print(f"warning: no *.json fixture files found under {target}", file=sys.stderr)

    grand_total_passed = 0
    grand_total_failed = 0

    for path in paths:
        tally = run_fixture_file(path)
        grand_total_passed += tally.passed
        grand_total_failed += tally.failed
        print(f"{path}: {tally.passed}/{tally.total} checks passed")
        for note in tally.notes:
            print(f"  note: {note}")
        for warning in tally.warnings:
            print(f"  warning: {warning}")
        for failure in tally.failures:
            print(f"  FAIL: {failure}")

    print()
    print(f"TOTAL: {grand_total_passed} passed, {grand_total_failed} failed")
    # Computed directly from the aggregate failure count (not a separately
    # tracked boolean flag threaded through the loop above) so there is
    # exactly one source of truth for whether this process must exit
    # nonzero -- this return value is wired into scripts/strict_checks.sh as
    # a CI gate, so a silent false-0 here would be worse than not having a
    # conformance runner at all.
    assert grand_total_failed >= 0  # nothing above can make this negative
    return 0 if grand_total_failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
