"""Per-kind semantic validator parity tests.

Spot-checks each of the six ``validate_*_artifact`` functions against the
exact error-message text and check order in the Rust source (all six are
now public functions co-located in
``froglet-protocol/src/protocol/kernel.rs``; quote/deal/invoice_bundle's
validators moved there from private helpers of the same name in
``froglet-protocol/src/protocol/chain.rs``), including all five receipt
settlement-method branches: ``lightning.base_fee_plus_success_fee.v1``,
``none``, ``stripe_mpp.v1``, ``lightning.prepaid.v1``, and
``x402.eip3009.v1``.

These tests operate on the six ``validate_*_artifact`` functions directly,
which only ever read ``doc["signer"]`` and ``doc["payload"]`` -- so a
minimal envelope wrapper (:func:`_wrap`) with placeholder crypto fields is
sufficient; no real signature is needed to exercise semantics.
"""

from __future__ import annotations

import copy
import hashlib
import unittest
from typing import Any

from froglet_verify.semantics import (
    validate_deal_artifact,
    validate_descriptor_artifact,
    validate_invoice_bundle_artifact,
    validate_offer_artifact,
    validate_quote_artifact,
    validate_receipt_artifact,
)

from _helpers import artifact, x402_artifact, x402_verification_case


def _wrap(artifact_type: str, signer: str, payload: dict[str, Any]) -> dict[str, Any]:
    """A minimal signed-artifact envelope for semantics-only testing.
    validate_*_artifact never reads schema_version/created_at/payload_hash/
    hash/signature, so placeholders are fine here."""
    return {
        "artifact_type": artifact_type,
        "schema_version": "froglet/v1",
        "signer": signer,
        "created_at": 0,
        "payload_hash": "",
        "hash": "",
        "payload": payload,
        "signature": "",
    }


def _mutate(doc: dict[str, Any], **payload_updates: Any) -> dict[str, Any]:
    """Deep copy ``doc`` and shallow-update its payload with
    ``payload_updates``."""
    new_doc = copy.deepcopy(doc)
    new_doc["payload"].update(payload_updates)
    return new_doc


def _mutate_nested(
    doc: dict[str, Any], path: tuple[str, ...], value: Any
) -> dict[str, Any]:
    """Deep copy ``doc`` and set a nested payload field, e.g.
    ``_mutate_nested(doc, ("settlement_terms", "method"), "none")``."""
    new_doc = copy.deepcopy(doc)
    target = new_doc["payload"]
    for key in path[:-1]:
        target = target[key]
    target[path[-1]] = value
    return new_doc


class DescriptorSemanticsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.descriptor = artifact("descriptor")

    def test_baseline_valid(self) -> None:
        self.assertIsNone(validate_descriptor_artifact(self.descriptor))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.descriptor)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_descriptor_artifact(doc),
            "descriptor signer does not match provider_id",
        )

    def test_empty_protocol_version(self) -> None:
        doc = _mutate(self.descriptor, protocol_version="")
        self.assertEqual(
            validate_descriptor_artifact(doc),
            "descriptor protocol_version must be froglet/v1",
        )

    def test_wrong_protocol_version(self) -> None:
        doc = _mutate(self.descriptor, protocol_version="froglet/v999")
        self.assertEqual(
            validate_descriptor_artifact(doc),
            "descriptor protocol_version must be froglet/v1",
        )

    def test_forged_linked_nostr_signature(self) -> None:
        doc = copy.deepcopy(self.descriptor)
        doc["payload"]["linked_identities"][0]["linked_signature"] = "00" * 64
        self.assertEqual(
            validate_descriptor_artifact(doc),
            "descriptor linked Nostr identity signature is invalid",
        )


class OfferSemanticsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.offer = artifact("offer")

    def test_baseline_valid(self) -> None:
        self.assertIsNone(validate_offer_artifact(self.offer))

    def test_free_offer_baseline_valid(self) -> None:
        self.assertIsNone(validate_offer_artifact(artifact("free_offer")))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.offer)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_offer_artifact(doc), "offer signer does not match provider_id"
        )

    def test_empty_offer_id(self) -> None:
        doc = _mutate(self.offer, offer_id="")
        self.assertEqual(
            validate_offer_artifact(doc), "offer offer_id must be non-empty"
        )

    def test_empty_descriptor_hash(self) -> None:
        doc = _mutate(self.offer, descriptor_hash="")
        self.assertEqual(
            validate_offer_artifact(doc), "offer descriptor_hash must be non-empty"
        )

    def test_unknown_paid_settlement_method(self) -> None:
        doc = _mutate(self.offer, settlement_method="future.rail.v9")
        self.assertEqual(
            validate_offer_artifact(doc),
            "paid offer settlement_method is unsupported",
        )

    def test_free_offer_rejects_paid_settlement_method(self) -> None:
        doc = _mutate(artifact("free_offer"), settlement_method="stripe_mpp.v1")
        self.assertEqual(
            validate_offer_artifact(doc),
            "free offer settlement_method must be none",
        )

    def test_single_leg_method_rejects_success_fee(self) -> None:
        doc = copy.deepcopy(self.offer)
        doc["payload"]["settlement_method"] = "lightning.prepaid.v1"
        self.assertEqual(
            validate_offer_artifact(doc),
            "single-leg paid offer success_fee_msat must be zero",
        )


class QuoteSemanticsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.quote = artifact("quote")  # lightning.base_fee_plus_success_fee.v1
        self.free_quote = artifact("free_quote")  # none

    def test_baseline_valid(self) -> None:
        self.assertIsNone(validate_quote_artifact(self.quote))

    def test_free_quote_baseline_valid(self) -> None:
        self.assertIsNone(validate_quote_artifact(self.free_quote))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.quote)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_quote_artifact(doc), "quote signer does not match provider_id"
        )

    def test_empty_requester_id(self) -> None:
        doc = _mutate(self.quote, requester_id="")
        self.assertEqual(
            validate_quote_artifact(doc), "quote requester_id must be non-empty"
        )

    def test_empty_descriptor_hash(self) -> None:
        doc = _mutate(self.quote, descriptor_hash="")
        self.assertEqual(
            validate_quote_artifact(doc), "quote descriptor_hash must be non-empty"
        )

    def test_empty_offer_hash(self) -> None:
        doc = _mutate(self.quote, offer_hash="")
        self.assertEqual(
            validate_quote_artifact(doc), "quote offer_hash must be non-empty"
        )

    def test_empty_workload_kind(self) -> None:
        doc = _mutate(self.quote, workload_kind="")
        self.assertEqual(
            validate_quote_artifact(doc), "quote workload_kind must be non-empty"
        )

    def test_empty_workload_hash(self) -> None:
        doc = _mutate(self.quote, workload_hash="")
        self.assertEqual(
            validate_quote_artifact(doc), "quote workload_hash must be non-empty"
        )

    def test_free_quote_nonempty_destination_identity(self) -> None:
        doc = _mutate_nested(
            self.free_quote,
            ("settlement_terms", "destination_identity"),
            "02" + "aa" * 32,
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "free quote destination_identity must be empty",
        )

    def test_free_quote_nonzero_fees(self) -> None:
        doc = _mutate_nested(self.free_quote, ("settlement_terms", "base_fee_msat"), 1)
        self.assertEqual(
            validate_quote_artifact(doc), "free quote fee amounts must be zero"
        )

    def test_lightning_quote_bad_destination_identity(self) -> None:
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "destination_identity"), "not-hex"
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "lightning quote destination_identity must be compressed secp256k1 lowercase hex",
        )

    def test_lightning_quote_uppercase_destination_identity_rejected(self) -> None:
        # is_lower_hex_len requires *lowercase* hex specifically.
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "destination_identity"), ("02" + "AA" * 32)
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "lightning quote destination_identity must be compressed secp256k1 lowercase hex",
        )

    def test_stripe_quote_nonempty_destination_identity(self) -> None:
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "method"), "stripe_mpp.v1"
        )
        doc = _mutate_nested(
            doc, ("settlement_terms", "destination_identity"), "02" + "aa" * 32
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "non-escrow quote destination_identity must be empty",
        )

    def test_stripe_quote_nonzero_success_fee(self) -> None:
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "method"), "stripe_mpp.v1"
        )
        doc = _mutate_nested(doc, ("settlement_terms", "destination_identity"), "")
        doc = _mutate_nested(doc, ("settlement_terms", "success_fee_msat"), 1)
        self.assertEqual(
            validate_quote_artifact(doc),
            "non-escrow quote success_fee_msat must be zero",
        )

    def test_prepaid_quote_requires_destination_identity(self) -> None:
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "method"), "lightning.prepaid.v1"
        )
        doc = _mutate_nested(doc, ("settlement_terms", "destination_identity"), "")
        doc = _mutate_nested(doc, ("settlement_terms", "success_fee_msat"), 0)
        self.assertEqual(
            validate_quote_artifact(doc),
            "lightning prepaid quote destination_identity must be compressed secp256k1 lowercase hex",
        )

        doc = _mutate_nested(
            doc, ("settlement_terms", "destination_identity"), "02" + "aa" * 32
        )
        self.assertIsNone(validate_quote_artifact(doc))

    def test_unknown_settlement_method(self) -> None:
        doc = _mutate_nested(
            self.quote, ("settlement_terms", "method"), "carrier_pigeon.v1"
        )
        doc = _mutate_nested(doc, ("settlement_terms", "destination_identity"), "")
        self.assertEqual(
            validate_quote_artifact(doc), "quote settlement_terms.method is invalid"
        )

    def test_x402_quote_baseline_valid(self) -> None:
        self.assertIsNone(validate_quote_artifact(x402_artifact("quote")))

    def test_x402_quote_bad_destination_identity_length(self) -> None:
        doc = _mutate_nested(
            x402_artifact("quote"), ("settlement_terms", "destination_identity"), "aa"
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "x402 quote destination_identity must be a 20-byte lowercase hex EVM address",
        )

    def test_x402_quote_uppercase_destination_identity_rejected(self) -> None:
        doc = _mutate_nested(
            x402_artifact("quote"),
            ("settlement_terms", "destination_identity"),
            "AA" * 20,
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "x402 quote destination_identity must be a 20-byte lowercase hex EVM address",
        )

    def test_x402_quote_0x_prefixed_destination_identity_rejected(self) -> None:
        # A "0x"-prefixed address is 42 chars, not the required 40.
        doc = _mutate_nested(
            x402_artifact("quote"),
            ("settlement_terms", "destination_identity"),
            "0x" + "aa" * 20,
        )
        self.assertEqual(
            validate_quote_artifact(doc),
            "x402 quote destination_identity must be a 20-byte lowercase hex EVM address",
        )

    def test_x402_quote_nonzero_success_fee(self) -> None:
        doc = _mutate_nested(
            x402_artifact("quote"), ("settlement_terms", "success_fee_msat"), 1
        )
        self.assertEqual(
            validate_quote_artifact(doc), "x402 quote success_fee_msat must be zero"
        )


class DealSemanticsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.deal = artifact("deal")

    def test_baseline_valid(self) -> None:
        self.assertIsNone(validate_deal_artifact(self.deal))

    def test_free_deal_baseline_valid(self) -> None:
        self.assertIsNone(validate_deal_artifact(artifact("free_deal")))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.deal)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_deal_artifact(doc), "deal signer does not match requester_id"
        )

    def test_empty_provider_id(self) -> None:
        doc = _mutate(self.deal, provider_id="")
        self.assertEqual(
            validate_deal_artifact(doc), "deal provider_id must be non-empty"
        )

    def test_empty_quote_hash(self) -> None:
        doc = _mutate(self.deal, quote_hash="")
        self.assertEqual(
            validate_deal_artifact(doc), "deal quote_hash must be non-empty"
        )

    def test_empty_workload_hash(self) -> None:
        doc = _mutate(self.deal, workload_hash="")
        self.assertEqual(
            validate_deal_artifact(doc), "deal workload_hash must be non-empty"
        )

    def test_bad_success_payment_hash_wrong_length(self) -> None:
        doc = _mutate(self.deal, success_payment_hash="aa")
        self.assertEqual(
            validate_deal_artifact(doc),
            "deal success_payment_hash must be lowercase 32-byte hex",
        )

    def test_bad_success_payment_hash_uppercase(self) -> None:
        doc = _mutate(self.deal, success_payment_hash="AA" * 32)
        self.assertEqual(
            validate_deal_artifact(doc),
            "deal success_payment_hash must be lowercase 32-byte hex",
        )

    def test_completion_deadline_not_after_admission_deadline(self) -> None:
        doc = copy.deepcopy(self.deal)
        doc["payload"]["completion_deadline"] = doc["payload"]["admission_deadline"]
        self.assertEqual(
            validate_deal_artifact(doc),
            "deal completion_deadline must be greater than admission_deadline",
        )

    def test_acceptance_deadline_before_completion_deadline(self) -> None:
        doc = copy.deepcopy(self.deal)
        doc["payload"]["acceptance_deadline"] = (
            doc["payload"]["completion_deadline"] - 1
        )
        self.assertEqual(
            validate_deal_artifact(doc),
            "deal acceptance_deadline must be greater than or equal to completion_deadline",
        )


class InvoiceBundleSemanticsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.bundle = artifact("invoice_bundle")

    def test_baseline_valid(self) -> None:
        self.assertIsNone(validate_invoice_bundle_artifact(self.bundle))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.bundle)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle signer does not match provider_id",
        )

    def test_empty_requester_id(self) -> None:
        doc = _mutate(self.bundle, requester_id="")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle requester_id must be non-empty",
        )

    def test_empty_quote_hash(self) -> None:
        doc = _mutate(self.bundle, quote_hash="")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle quote_hash must be non-empty",
        )

    def test_empty_deal_hash(self) -> None:
        doc = _mutate(self.bundle, deal_hash="")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle deal_hash must be non-empty",
        )

    def test_bad_destination_identity(self) -> None:
        doc = _mutate(self.bundle, destination_identity="02aa")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle destination_identity must be compressed secp256k1 lowercase hex",
        )

    def test_base_fee_empty_invoice_bolt11(self) -> None:
        doc = _mutate_nested(self.bundle, ("base_fee", "invoice_bolt11"), "")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle base_fee.invoice_bolt11 must be non-empty",
        )

    def test_base_fee_bad_invoice_hash_length(self) -> None:
        doc = _mutate_nested(self.bundle, ("base_fee", "invoice_hash"), "aa")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle base_fee.invoice_hash must be lowercase 32-byte hex",
        )

    def test_base_fee_bad_payment_hash_length(self) -> None:
        doc = _mutate_nested(self.bundle, ("base_fee", "payment_hash"), "aa")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle base_fee.payment_hash must be lowercase 32-byte hex",
        )

    def test_base_fee_invoice_hash_mismatch(self) -> None:
        doc = _mutate_nested(self.bundle, ("base_fee", "invoice_hash"), "ab" * 32)
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle base_fee.invoice_hash must equal SHA256(invoice_bolt11)",
        )

    def test_success_fee_leg_checked_after_base_fee_leg(self) -> None:
        # validate_invoice_leg("base_fee", ...) runs before
        # validate_invoice_leg("success_fee", ...); corrupting only the
        # success_fee leg must surface the success_fee-specific message.
        doc = _mutate_nested(self.bundle, ("success_fee", "invoice_bolt11"), "")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle success_fee.invoice_bolt11 must be non-empty",
        )

    def test_success_fee_state_not_open_at_issuance(self) -> None:
        doc = _mutate_nested(self.bundle, ("success_fee", "state"), "accepted")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle success_fee.state must be open at issuance",
        )

    def test_base_fee_state_not_open_and_not_zero_settled(self) -> None:
        doc = _mutate_nested(self.bundle, ("base_fee", "state"), "accepted")
        self.assertEqual(
            validate_invoice_bundle_artifact(doc),
            "invoice_bundle base_fee.state must be open unless zero-valued and settled",
        )

    def test_base_fee_state_settled_and_zero_valued_is_allowed(self) -> None:
        # The one exception to "must be open": zero-valued AND settled.
        doc = copy.deepcopy(self.bundle)
        invoice_bolt11 = "lnmock-base-0-" + ("11" * 32) + "-1700000304"
        doc["payload"]["base_fee"] = {
            "amount_msat": 0,
            "invoice_bolt11": invoice_bolt11,
            "invoice_hash": hashlib.sha256(invoice_bolt11.encode()).hexdigest(),
            "payment_hash": "11" * 32,
            "state": "settled",
        }
        self.assertIsNone(validate_invoice_bundle_artifact(doc))


class ReceiptSemanticsTests(unittest.TestCase):
    """Covers all four settlement-method branches of
    `validate_receipt_artifact`."""

    def setUp(self) -> None:
        self.receipt = artifact("receipt")  # lightning.base_fee_plus_success_fee.v1
        self.free_receipt = artifact("free_receipt")  # none

    # --- cross-cutting checks (apply regardless of settlement method) ---

    def test_baseline_lightning_receipt_valid(self) -> None:
        self.assertIsNone(validate_receipt_artifact(self.receipt))

    def test_baseline_free_receipt_valid(self) -> None:
        self.assertIsNone(validate_receipt_artifact(self.free_receipt))

    def test_signer_mismatch(self) -> None:
        doc = copy.deepcopy(self.receipt)
        doc["signer"] = "11" * 32
        self.assertEqual(
            validate_receipt_artifact(doc), "receipt signer does not match provider_id"
        )

    def test_finished_before_started(self) -> None:
        doc = copy.deepcopy(self.receipt)
        doc["payload"]["started_at"] = doc["payload"]["finished_at"] + 1
        self.assertEqual(
            validate_receipt_artifact(doc),
            "receipt finished_at is earlier than started_at",
        )

    def test_succeeded_execution_state_requires_result_hash_and_format(self) -> None:
        doc = _mutate(self.receipt, result_hash=None)
        self.assertEqual(
            validate_receipt_artifact(doc),
            "receipt with execution_state succeeded must include result_hash and result_format",
        )

    def test_non_succeeded_execution_state_forbids_result_hash(self) -> None:
        doc = _mutate(
            self.receipt,
            execution_state="failed",
            deal_state="failed",
            failure_code="execution_failed",
            failure_message="boom",
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "receipt result_hash and result_format must be absent unless execution_state is succeeded",
        )

    def test_rejected_deal_state_requires_not_started_execution(self) -> None:
        doc = _mutate(
            self.receipt,
            deal_state="rejected",
            execution_state="not_started",
            result_hash=None,
            result_format=None,
        )
        self.assertIsNone(validate_receipt_artifact(doc))
        doc2 = _mutate(doc, execution_state="failed")
        self.assertEqual(
            validate_receipt_artifact(doc2),
            "rejected receipt must have execution_state not_started",
        )

    def test_invalid_deal_state(self) -> None:
        doc = _mutate(self.receipt, deal_state="teleported")
        self.assertEqual(
            validate_receipt_artifact(doc), "receipt deal_state is invalid"
        )

    # --- lightning.base_fee_plus_success_fee.v1 ---

    def test_lightning_missing_bundle_hash(self) -> None:
        doc = _mutate_nested(self.receipt, ("settlement_refs", "bundle_hash"), None)
        self.assertEqual(
            validate_receipt_artifact(doc), "lightning receipt must include bundle_hash"
        )

    def test_lightning_missing_destination_identity(self) -> None:
        doc = _mutate_nested(
            self.receipt, ("settlement_refs", "destination_identity"), ""
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning receipt must include destination_identity",
        )

    def test_lightning_legs_must_be_terminal(self) -> None:
        doc = _mutate_nested(
            self.receipt, ("settlement_refs", "base_fee", "state"), "open"
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning receipt settlement legs must be terminal",
        )

    def test_lightning_settled_requires_success_fee_settled(self) -> None:
        # settlement_state on the fixture receipt is already "settled".
        doc = _mutate_nested(
            self.receipt, ("settlement_refs", "success_fee", "state"), "canceled"
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning receipt settlement_state settled requires success_fee.state settled",
        )

    def test_lightning_settled_requires_base_fee_settled(self) -> None:
        doc = _mutate_nested(
            self.receipt, ("settlement_refs", "base_fee", "state"), "canceled"
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning receipt settlement_state settled requires base_fee.state settled",
        )

    def test_lightning_invalid_settlement_state(self) -> None:
        doc = _mutate(self.receipt, settlement_state="disputed")
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning receipt settlement_state must be settled, canceled, or expired",
        )

    def test_lightning_successful_deal_requires_settled(self) -> None:
        doc = copy.deepcopy(self.receipt)
        doc["payload"]["settlement_state"] = "expired"
        doc["payload"]["settlement_refs"]["success_fee"]["state"] = "expired"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "successful lightning receipt must have settlement_state settled",
        )

    # --- none (free) ---

    def test_free_receipt_settlement_state_must_be_none(self) -> None:
        doc = _mutate(self.free_receipt, settlement_state="settled")
        self.assertEqual(
            validate_receipt_artifact(doc), "free receipt settlement_state must be none"
        )

    def test_free_receipt_must_not_include_bundle_hash(self) -> None:
        doc = _mutate_nested(
            self.free_receipt, ("settlement_refs", "bundle_hash"), "aa" * 32
        )
        self.assertEqual(
            validate_receipt_artifact(doc), "free receipt must not include bundle_hash"
        )

    def test_free_receipt_destination_identity_must_be_empty(self) -> None:
        doc = _mutate_nested(
            self.free_receipt,
            ("settlement_refs", "destination_identity"),
            "02" + "aa" * 32,
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "free receipt destination_identity must be empty",
        )

    def test_free_receipt_legs_must_be_zero_valued_canceled(self) -> None:
        doc = _mutate_nested(
            self.free_receipt, ("settlement_refs", "base_fee", "amount_msat"), 1
        )
        self.assertEqual(
            validate_receipt_artifact(doc),
            "free receipt settlement legs must be zero-valued canceled placeholders",
        )

    # --- stripe_mpp.v1 ---

    def _valid_stripe_receipt(self, *, captured: bool) -> dict[str, Any]:
        """Mirrors kernel.rs's test helper `valid_stripe_receipt_payload`."""
        base = artifact("receipt")
        payload = copy.deepcopy(base["payload"])
        if captured:
            payload.update(
                deal_state="succeeded",
                execution_state="succeeded",
                settlement_state="settled",
                result_hash="44" * 32,
                result_format="application/json+jcs",
                failure_code=None,
                failure_message=None,
            )
            base_state = "settled"
        else:
            payload.update(
                deal_state="failed",
                execution_state="failed",
                settlement_state="canceled",
                result_hash=None,
                result_format=None,
                failure_code="execution_failed",
                failure_message="test failure",
            )
            base_state = "canceled"
        payload["settlement_refs"] = {
            "method": "stripe_mpp.v1",
            "bundle_hash": None,
            "destination_identity": "",
            "base_fee": {
                "amount_msat": 30000,
                "invoice_hash": "",
                "payment_hash": "pi_test_stripe_intent_123",
                "state": base_state,
            },
            "success_fee": {
                "amount_msat": 0,
                "invoice_hash": "",
                "payment_hash": "",
                "state": "canceled",
            },
        }
        base["payload"] = payload
        return base

    def test_stripe_success_receipt_passes(self) -> None:
        self.assertIsNone(
            validate_receipt_artifact(self._valid_stripe_receipt(captured=True))
        )

    def test_stripe_failure_receipt_passes(self) -> None:
        self.assertIsNone(
            validate_receipt_artifact(self._valid_stripe_receipt(captured=False))
        )

    def test_stripe_receipt_with_bundle_hash_fails(self) -> None:
        # Explicitly requested spot check: a stripe_mpp.v1 receipt carrying a
        # bundle_hash (a lightning-escrow-only concept) must fail with this
        # exact message.
        doc = self._valid_stripe_receipt(captured=True)
        doc["payload"]["settlement_refs"]["bundle_hash"] = "aa" * 32
        self.assertEqual(
            validate_receipt_artifact(doc),
            "stripe_mpp.v1 receipt must not include bundle_hash",
        )

    def test_stripe_receipt_with_destination_identity_fails(self) -> None:
        doc = self._valid_stripe_receipt(captured=True)
        doc["payload"]["settlement_refs"]["destination_identity"] = "02" + "aa" * 32
        self.assertEqual(
            validate_receipt_artifact(doc),
            "stripe_mpp.v1 receipt destination_identity must be empty",
        )

    def test_stripe_receipt_nonempty_success_fee_fails(self) -> None:
        doc = self._valid_stripe_receipt(captured=True)
        doc["payload"]["settlement_refs"]["success_fee"]["payment_hash"] = "bb" * 32
        self.assertEqual(
            validate_receipt_artifact(doc),
            "stripe_mpp.v1 receipt success_fee must be a zero-valued canceled placeholder",
        )

    def test_stripe_receipt_base_fee_not_terminal_fails(self) -> None:
        doc = self._valid_stripe_receipt(captured=True)
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "accepted"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "stripe_mpp.v1 receipt base_fee.state must be terminal (settled or canceled)",
        )

    def test_stripe_receipt_wrong_settlement_state_fails(self) -> None:
        # A "succeeded" deal must have settlement_state "settled".
        doc = self._valid_stripe_receipt(captured=True)
        doc["payload"]["settlement_state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "canceled"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "successful stripe_mpp.v1 receipt must have settlement_state settled",
        )

    def test_stripe_receipt_invalid_settlement_state(self) -> None:
        doc = self._valid_stripe_receipt(captured=False)
        doc["payload"]["settlement_state"] = "expired"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "stripe_mpp.v1 receipt settlement_state must be settled or canceled",
        )

    # --- lightning.prepaid.v1 ---

    _PREPAID_PREIMAGE = "11" * 32
    _PREPAID_PAYMENT_HASH = hashlib.sha256(bytes.fromhex(_PREPAID_PREIMAGE)).hexdigest()

    def _valid_prepaid_receipt(self, *, settled: bool) -> dict[str, Any]:
        """Mirrors kernel.rs's test helper `valid_prepaid_receipt_payload`."""
        base = artifact("receipt")
        payload = copy.deepcopy(base["payload"])
        if settled:
            payload.update(
                deal_state="succeeded",
                execution_state="succeeded",
                settlement_state="settled",
                result_hash="44" * 32,
                result_format="application/json+jcs",
                failure_code=None,
                failure_message=None,
            )
        else:
            payload.update(
                deal_state="failed",
                execution_state="failed",
                settlement_state="settled",  # buyer already prepaid; no refund
                result_hash=None,
                result_format=None,
                failure_code="execution_failed",
                failure_message="test failure",
            )
        payload["settlement_refs"] = {
            "method": "lightning.prepaid.v1",
            "bundle_hash": None,
            "destination_identity": "02" + "55" * 32,
            "base_fee": {
                "amount_msat": 30000,
                "invoice_hash": self._PREPAID_PREIMAGE,
                "payment_hash": self._PREPAID_PAYMENT_HASH,
                "state": "settled",
            },
            "success_fee": {
                "amount_msat": 0,
                "invoice_hash": "",
                "payment_hash": "",
                "state": "canceled",
            },
        }
        base["payload"] = payload
        return base

    def _canceled_prepaid_receipt(self) -> dict[str, Any]:
        doc = self._valid_prepaid_receipt(settled=False)
        doc["payload"]["settlement_state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["invoice_hash"] = ""
        doc["payload"]["settlement_refs"]["base_fee"]["payment_hash"] = ""
        doc["payload"]["settlement_refs"]["base_fee"]["amount_msat"] = 0
        return doc

    def test_prepaid_success_receipt_passes(self) -> None:
        self.assertIsNone(
            validate_receipt_artifact(self._valid_prepaid_receipt(settled=True))
        )

    def test_prepaid_failed_but_paid_receipt_passes(self) -> None:
        self.assertIsNone(
            validate_receipt_artifact(self._valid_prepaid_receipt(settled=False))
        )

    def test_prepaid_canceled_receipt_passes(self) -> None:
        self.assertIsNone(validate_receipt_artifact(self._canceled_prepaid_receipt()))

    def test_prepaid_mismatched_preimage_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_refs"]["base_fee"]["invoice_hash"] = (
            "22" * 32
        )  # wrong preimage
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning.prepaid.v1 receipt preimage does not match payment_hash",
        )

    def test_prepaid_non_hex_preimage_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_refs"]["base_fee"]["invoice_hash"] = "not-hex"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning.prepaid.v1 settled receipt preimage must be 32-byte hex",
        )

    def test_prepaid_with_bundle_hash_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_refs"]["bundle_hash"] = "aa" * 32
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning.prepaid.v1 receipt must not include bundle_hash",
        )

    def test_prepaid_without_destination_identity_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_refs"]["destination_identity"] = ""
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning.prepaid.v1 receipt destination_identity must be compressed secp256k1 lowercase hex",
        )

    def test_prepaid_nonempty_success_fee_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_refs"]["success_fee"]["payment_hash"] = "bb" * 32
        self.assertEqual(
            validate_receipt_artifact(doc),
            "lightning.prepaid.v1 receipt success_fee must be a zero-valued canceled placeholder",
        )

    def test_prepaid_succeeded_with_canceled_settlement_fails(self) -> None:
        doc = self._valid_prepaid_receipt(settled=True)
        doc["payload"]["settlement_state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["invoice_hash"] = ""
        self.assertEqual(
            validate_receipt_artifact(doc),
            "successful lightning.prepaid.v1 receipt must have settlement_state settled",
        )

    # --- unknown method ---

    def test_unknown_settlement_method_fails(self) -> None:
        doc = _mutate_nested(
            self.free_receipt, ("settlement_refs", "method"), "carrier_pigeon.v1"
        )
        self.assertEqual(
            validate_receipt_artifact(doc), "receipt settlement_refs.method is invalid"
        )


class X402ReceiptSemanticsTests(unittest.TestCase):
    """The x402.eip3009.v1 branch of `validate_receipt_artifact`, exercised
    against the real conformance/x402_v1.json fixture: the one accept case
    plus all five reject cases, each asserted against the *specific* error
    message from the Rust source (not just "some error")."""

    def test_x402_receipt_valid(self) -> None:
        case = x402_verification_case("x402_receipt_valid")
        self.assertTrue(case["expected_valid"])
        self.assertIsNone(validate_receipt_artifact(case["artifact"]))

    def test_x402_receipt_missing_bundle_hash(self) -> None:
        case = x402_verification_case("x402_receipt_missing_bundle_hash")
        self.assertFalse(case["expected_valid"])
        self.assertEqual(
            validate_receipt_artifact(case["artifact"]),
            "x402.eip3009.v1 settled receipt must include a 32-byte lowercase hex bundle_hash",
        )

    def test_x402_receipt_uppercase_nonce(self) -> None:
        case = x402_verification_case("x402_receipt_uppercase_nonce")
        self.assertFalse(case["expected_valid"])
        self.assertEqual(
            validate_receipt_artifact(case["artifact"]),
            "x402.eip3009.v1 settled receipt payment_hash must carry the 32-byte "
            "hex EIP-3009 authorization nonce",
        )

    def test_x402_receipt_succeeded_but_unsettled(self) -> None:
        case = x402_verification_case("x402_receipt_succeeded_but_unsettled")
        self.assertFalse(case["expected_valid"])
        self.assertEqual(
            validate_receipt_artifact(case["artifact"]),
            "successful x402.eip3009.v1 receipt must have settlement_state settled",
        )

    def test_x402_receipt_canceled_with_tx_hash(self) -> None:
        case = x402_verification_case("x402_receipt_canceled_with_tx_hash")
        self.assertFalse(case["expected_valid"])
        self.assertEqual(
            validate_receipt_artifact(case["artifact"]),
            "x402.eip3009.v1 canceled receipt must not carry a settle transaction hash",
        )

    def test_x402_receipt_prefixed_destination(self) -> None:
        case = x402_verification_case("x402_receipt_prefixed_destination")
        self.assertFalse(case["expected_valid"])
        self.assertEqual(
            validate_receipt_artifact(case["artifact"]),
            "x402.eip3009.v1 receipt destination_identity must be a 20-byte "
            "lowercase hex EVM address",
        )

    def test_x402_receipt_success_fee_must_be_empty_canceled(self) -> None:
        doc = x402_artifact("receipt")
        doc["payload"]["settlement_refs"]["success_fee"]["amount_msat"] = 1
        self.assertEqual(
            validate_receipt_artifact(doc),
            "x402.eip3009.v1 receipt success_fee must be a zero-valued canceled placeholder",
        )

    def test_x402_receipt_base_fee_must_be_terminal(self) -> None:
        doc = x402_artifact("receipt")
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "accepted"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "x402.eip3009.v1 receipt base_fee.state must be terminal (settled or canceled)",
        )

    def test_x402_receipt_settled_requires_base_fee_settled(self) -> None:
        doc = x402_artifact("receipt")
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "canceled"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "x402.eip3009.v1 receipt settlement_state settled requires base_fee.state settled",
        )

    def test_x402_receipt_invalid_settlement_state(self) -> None:
        doc = x402_artifact("receipt")
        doc["payload"]["settlement_state"] = "expired"
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "expired"
        self.assertEqual(
            validate_receipt_artifact(doc),
            "x402.eip3009.v1 receipt settlement_state must be settled or canceled",
        )

    def test_x402_canceled_receipt_with_bundle_hash_fails(self) -> None:
        doc = x402_artifact("receipt")
        # deal_state "canceled" permits execution_state "not_started" (per
        # the deal_state/execution_state matrix checked earlier in
        # validate_receipt_artifact, ahead of the settlement-method dispatch
        # this test targets).
        doc["payload"]["deal_state"] = "canceled"
        doc["payload"]["execution_state"] = "not_started"
        doc["payload"]["result_hash"] = None
        doc["payload"]["result_format"] = None
        doc["payload"]["settlement_state"] = "canceled"
        doc["payload"]["settlement_refs"]["base_fee"]["state"] = "canceled"
        # bundle_hash left populated -- a canceled x402 receipt must not
        # carry one at all (unlike the settled case, where it is required).
        self.assertEqual(
            validate_receipt_artifact(doc),
            "x402.eip3009.v1 canceled receipt must not include bundle_hash",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
