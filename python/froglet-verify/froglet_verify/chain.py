"""Pairwise and full-chain validation of Froglet signed-artifact chains.

Mirrors ``froglet-protocol/src/protocol/chain.rs``: the ``ISSUE_*`` string
codes (a stable public contract — reproduced verbatim, including their
ordering within each check function), the five pairwise ``validate_*``
functions, and their exact check order (envelope issues, then semantic
issues, then cross-artifact link issues — matching
``reports_deterministic_envelope_issues_before_link_issues`` in
``chain.rs``'s own test suite).

Each pairwise validator returns a report dict::

    {"path": "<snake_case ChainPath name>", "valid": bool, "issues": [
        {"code": str, "artifact_type": str, "message": str}, ...
    ]}

:func:`validate_full_chain` mirrors Rust's method-aware composition: Lightning
escrow requires an invoice bundle, other methods reject one, and a Lightning
receipt is cross-bound to the supplied bundle and deal in addition to the
ordinary pairwise paths.
"""

from __future__ import annotations

from typing import Any, Callable

from .envelope import sha256_hex, verify_signed_artifact
from .semantics import (
    validate_deal_artifact,
    validate_descriptor_artifact,
    validate_invoice_bundle_artifact,
    validate_offer_artifact,
    validate_quote_artifact,
    validate_receipt_artifact,
)

__all__ = [
    "ISSUE_ARTIFACT_TYPE_MISMATCH",
    "ISSUE_ARTIFACT_ENVELOPE_INVALID",
    "ISSUE_ARTIFACT_SEMANTIC_INVALID",
    "ISSUE_ARTIFACT_EXPIRED",
    "ISSUE_PROVIDER_MISMATCH",
    "ISSUE_REQUESTER_MISMATCH",
    "ISSUE_DESCRIPTOR_HASH_MISMATCH",
    "ISSUE_OFFER_HASH_MISMATCH",
    "ISSUE_QUOTE_HASH_MISMATCH",
    "ISSUE_DEAL_HASH_MISMATCH",
    "ISSUE_WORKLOAD_KIND_MISMATCH",
    "ISSUE_WORKLOAD_HASH_MISMATCH",
    "ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH",
    "ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER",
    "ISSUE_SETTLEMENT_METHOD_MISMATCH",
    "ISSUE_SETTLEMENT_TERMS_MISMATCH",
    "ISSUE_EXECUTION_LIMITS_EXCEED_OFFER",
    "ISSUE_DEADLINE_ORDER_INVALID",
    "ISSUE_DEADLINE_EXCEEDS_QUOTE",
    "ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING",
    "ISSUE_INVOICE_AMOUNT_MISMATCH",
    "ISSUE_INVOICE_DESTINATION_MISMATCH",
    "ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH",
    "ISSUE_INVOICE_MIN_CLTV_MISMATCH",
    "ISSUE_INVOICE_HASH_MISMATCH",
    "ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL",
    "ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING",
    "ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH",
    "ISSUE_INVOICE_PAYMENT_HASH_MISMATCH",
    "ISSUE_INVOICE_STATE_MISMATCH",
    "PATH_DESCRIPTOR_OFFER",
    "PATH_OFFER_QUOTE",
    "PATH_QUOTE_DEAL",
    "PATH_QUOTE_INVOICE_BUNDLE_DEAL",
    "PATH_QUOTE_DEAL_RECEIPT",
    "validate_descriptor_offer",
    "validate_offer_quote",
    "validate_quote_deal",
    "validate_quote_invoice_bundle_deal",
    "validate_quote_deal_receipt",
    "validate_full_chain",
]

# ISSUE_* codes: a stable public contract (chain.rs lines 12-38). Do not rename.
ISSUE_ARTIFACT_TYPE_MISMATCH = "artifact_type_mismatch"
ISSUE_ARTIFACT_ENVELOPE_INVALID = "artifact_envelope_invalid"
ISSUE_ARTIFACT_SEMANTIC_INVALID = "artifact_semantic_invalid"
ISSUE_ARTIFACT_EXPIRED = "artifact_expired"
ISSUE_PROVIDER_MISMATCH = "provider_mismatch"
ISSUE_REQUESTER_MISMATCH = "requester_mismatch"
ISSUE_DESCRIPTOR_HASH_MISMATCH = "descriptor_hash_mismatch"
ISSUE_OFFER_HASH_MISMATCH = "offer_hash_mismatch"
ISSUE_QUOTE_HASH_MISMATCH = "quote_hash_mismatch"
ISSUE_DEAL_HASH_MISMATCH = "deal_hash_mismatch"
ISSUE_WORKLOAD_KIND_MISMATCH = "workload_kind_mismatch"
ISSUE_WORKLOAD_HASH_MISMATCH = "workload_hash_mismatch"
ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH = "confidential_session_hash_mismatch"
ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER = "quote_expiry_exceeds_offer"
ISSUE_SETTLEMENT_METHOD_MISMATCH = "settlement_method_mismatch"
ISSUE_SETTLEMENT_TERMS_MISMATCH = "settlement_terms_mismatch"
ISSUE_EXECUTION_LIMITS_EXCEED_OFFER = "execution_limits_exceed_offer"
ISSUE_DEADLINE_ORDER_INVALID = "deadline_order_invalid"
ISSUE_DEADLINE_EXCEEDS_QUOTE = "deadline_exceeds_quote"
ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING = "invoice_bundle_for_non_lightning_method"
ISSUE_INVOICE_AMOUNT_MISMATCH = "invoice_amount_mismatch"
ISSUE_INVOICE_DESTINATION_MISMATCH = "invoice_destination_mismatch"
ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH = "invoice_success_payment_hash_mismatch"
ISSUE_INVOICE_MIN_CLTV_MISMATCH = "invoice_min_cltv_mismatch"
ISSUE_INVOICE_HASH_MISMATCH = "invoice_hash_mismatch"
ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL = "invoice_expiry_exceeds_deal"
ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING = (
    "invoice_bundle_required_for_lightning_method"
)
ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH = "receipt_bundle_hash_mismatch"
ISSUE_INVOICE_PAYMENT_HASH_MISMATCH = "invoice_payment_hash_mismatch"
ISSUE_INVOICE_STATE_MISMATCH = "invoice_state_mismatch"

# ChainPath serde `rename_all = "snake_case"` names.
PATH_DESCRIPTOR_OFFER = "descriptor_offer"
PATH_OFFER_QUOTE = "offer_quote"
PATH_QUOTE_DEAL = "quote_deal"
PATH_QUOTE_INVOICE_BUNDLE_DEAL = "quote_invoice_bundle_deal"
PATH_QUOTE_DEAL_RECEIPT = "quote_deal_receipt"

_SETTLEMENT_METHOD_NONE = "none"
_SETTLEMENT_METHOD_LIGHTNING_ESCROW = "lightning.base_fee_plus_success_fee.v1"
# Mirrors chain.rs's `is_known_paid_settlement_method`.
_KNOWN_PAID_SETTLEMENT_METHODS = frozenset(
    {
        "lightning.base_fee_plus_success_fee.v1",
        "stripe_mpp.v1",
        "lightning.prepaid.v1",
        "x402.eip3009.v1",
    }
)

_ARTIFACT_TYPE_DESCRIPTOR = "descriptor"
_ARTIFACT_TYPE_OFFER = "offer"
_ARTIFACT_TYPE_QUOTE = "quote"
_ARTIFACT_TYPE_DEAL = "deal"
_ARTIFACT_TYPE_RECEIPT = "receipt"
_TRANSPORT_TYPE_INVOICE_BUNDLE = "invoice_bundle"


class _Report:
    """Mutable builder for a chain validation report; converted to a plain
    dict via :meth:`to_dict`. Kept private -- callers only ever see the dict
    form, matching the other modules' plain-dict API."""

    __slots__ = ("path", "valid", "issues")

    def __init__(self, path: str) -> None:
        self.path = path
        self.valid = True
        self.issues: list[dict[str, str]] = []

    def push_issue(self, code: str, artifact_type: str, message: str) -> None:
        self.valid = False
        self.issues.append(
            {"code": code, "artifact_type": artifact_type, "message": message}
        )

    def to_dict(self) -> dict[str, Any]:
        return {"path": self.path, "valid": self.valid, "issues": self.issues}


def _verify_common_artifact(
    report: _Report, artifact: dict[str, Any], expected_artifact_type: str
) -> None:
    actual_type = artifact.get("artifact_type") if isinstance(artifact, dict) else None
    if actual_type != expected_artifact_type:
        report.push_issue(
            ISSUE_ARTIFACT_TYPE_MISMATCH,
            expected_artifact_type,
            f"expected artifact_type {expected_artifact_type}, got {actual_type}",
        )
    envelope_ok, _reason = verify_signed_artifact(artifact)
    if not envelope_ok:
        report.push_issue(
            ISSUE_ARTIFACT_ENVELOPE_INVALID,
            expected_artifact_type,
            "artifact envelope hash, payload hash, schema version, or signature is invalid",
        )


def _validate_descriptor_semantics(
    report: _Report, descriptor: dict[str, Any], now: int | None
) -> None:
    # Each `_validate_*_semantics` helper wraps its *entire* body (the
    # semantic check plus the expiry check) in one try/except: Rust's
    # `validate_*_semantics` never needs this, because a document missing a
    # required field would already have failed to deserialize into the typed
    # `SignedArtifact<T>` before either check could run. Operating on plain
    # dicts loses that upstream guarantee, so a single catch-all here keeps
    # one malformed document from raising out of `validate_full_chain`/the
    # CLI mid-report; call sites can still choose to let a bad *envelope*
    # (checked separately by `_verify_common_artifact`) surface as its own
    # issue rather than a crash.
    try:
        message = validate_descriptor_artifact(descriptor)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _ARTIFACT_TYPE_DESCRIPTOR, message
            )

        expires_at = descriptor["payload"].get("expires_at")
        if now is not None and expires_at is not None and expires_at < now:
            report.push_issue(
                ISSUE_ARTIFACT_EXPIRED,
                _ARTIFACT_TYPE_DESCRIPTOR,
                "descriptor expires_at is earlier than now",
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_DESCRIPTOR,
            f"malformed descriptor payload: {error!r}",
        )


def _validate_offer_settlement(report: _Report, offer: dict[str, Any]) -> None:
    price_schedule = offer["payload"]["price_schedule"]
    settlement_method = offer["payload"]["settlement_method"]
    fees_are_zero = (
        price_schedule["base_fee_msat"] == 0 and price_schedule["success_fee_msat"] == 0
    )
    if fees_are_zero and settlement_method != _SETTLEMENT_METHOD_NONE:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_OFFER,
            "zero-fee offers must use settlement_method none",
        )
    elif not fees_are_zero and settlement_method not in _KNOWN_PAID_SETTLEMENT_METHODS:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_OFFER,
            "paid offers must use a known paid settlement method",
        )


def _validate_offer_semantics(
    report: _Report, offer: dict[str, Any], now: int | None
) -> None:
    try:
        message = validate_offer_artifact(offer)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _ARTIFACT_TYPE_OFFER, message
            )

        _validate_offer_settlement(report, offer)

        expires_at = offer["payload"].get("expires_at")
        if now is not None and expires_at is not None and expires_at < now:
            report.push_issue(
                ISSUE_ARTIFACT_EXPIRED,
                _ARTIFACT_TYPE_OFFER,
                "offer expires_at is earlier than now",
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_OFFER,
            f"malformed offer payload: {error!r}",
        )


def _validate_quote_semantics(
    report: _Report, quote: dict[str, Any], now: int | None
) -> None:
    try:
        message = validate_quote_artifact(quote)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _ARTIFACT_TYPE_QUOTE, message
            )

        if now is not None and quote["payload"]["expires_at"] < now:
            report.push_issue(
                ISSUE_ARTIFACT_EXPIRED,
                _ARTIFACT_TYPE_QUOTE,
                "quote expires_at is earlier than now",
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_QUOTE,
            f"malformed quote payload: {error!r}",
        )


def _validate_deal_semantics(report: _Report, deal: dict[str, Any]) -> None:
    try:
        message = validate_deal_artifact(deal)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _ARTIFACT_TYPE_DEAL, message
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_DEAL,
            f"malformed deal payload: {error!r}",
        )


def _validate_invoice_bundle_semantics(
    report: _Report, invoice_bundle: dict[str, Any], now: int | None
) -> None:
    try:
        message = validate_invoice_bundle_artifact(invoice_bundle)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _TRANSPORT_TYPE_INVOICE_BUNDLE, message
            )

        if now is not None and invoice_bundle["payload"]["expires_at"] < now:
            report.push_issue(
                ISSUE_ARTIFACT_EXPIRED,
                _TRANSPORT_TYPE_INVOICE_BUNDLE,
                "invoice_bundle expires_at is earlier than now",
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            f"malformed invoice_bundle payload: {error!r}",
        )


def _validate_receipt_semantics(report: _Report, receipt: dict[str, Any]) -> None:
    # Receipts have no `expires_at`, so there is no analogous expiry check
    # here (chain.rs's `validate_receipt_semantics` is the semantic check
    # only -- see `ChainValidationReport::valid` usage in `chain.rs`).
    try:
        message = validate_receipt_artifact(receipt)
        if message is not None:
            report.push_issue(
                ISSUE_ARTIFACT_SEMANTIC_INVALID, _ARTIFACT_TYPE_RECEIPT, message
            )
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            _ARTIFACT_TYPE_RECEIPT,
            f"malformed receipt payload: {error!r}",
        )


def _limits_within(limits: dict[str, Any], maxima: dict[str, Any]) -> bool:
    return bool(
        limits["max_input_bytes"] <= maxima["max_input_bytes"]
        and limits["max_runtime_ms"] <= maxima["max_runtime_ms"]
        and limits["max_memory_bytes"] <= maxima["max_memory_bytes"]
        and limits["max_output_bytes"] <= maxima["max_output_bytes"]
        and limits["fuel_limit"] <= maxima["fuel_limit"]
    )


def _validate_offer_quote_links(
    report: _Report, offer: dict[str, Any], quote: dict[str, Any]
) -> None:
    offer_payload = offer["payload"]
    quote_payload = quote["payload"]

    if quote_payload["provider_id"] != offer_payload["provider_id"]:
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote provider_id must match offer provider_id",
        )
    if quote_payload["descriptor_hash"] != offer_payload["descriptor_hash"]:
        report.push_issue(
            ISSUE_DESCRIPTOR_HASH_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote descriptor_hash must match offer descriptor_hash",
        )
    if quote_payload["offer_hash"] != offer["hash"]:
        report.push_issue(
            ISSUE_OFFER_HASH_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote offer_hash must match offer hash",
        )

    offer_expires_at = offer_payload.get("expires_at")
    if offer_expires_at is not None and quote_payload["expires_at"] > offer_expires_at:
        report.push_issue(
            ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER,
            _ARTIFACT_TYPE_QUOTE,
            "quote expires_at must not exceed offer expires_at",
        )
    if quote_payload["workload_kind"] != offer_payload["offer_kind"]:
        report.push_issue(
            ISSUE_WORKLOAD_KIND_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote workload_kind must match offer offer_kind",
        )
    if (
        quote_payload["settlement_terms"]["method"]
        != offer_payload["settlement_method"]
    ):
        report.push_issue(
            ISSUE_SETTLEMENT_METHOD_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote settlement_terms.method must match offer settlement_method",
        )
    if (
        quote_payload["settlement_terms"]["base_fee_msat"]
        != offer_payload["price_schedule"]["base_fee_msat"]
        or quote_payload["settlement_terms"]["success_fee_msat"]
        != offer_payload["price_schedule"]["success_fee_msat"]
    ):
        report.push_issue(
            ISSUE_SETTLEMENT_TERMS_MISMATCH,
            _ARTIFACT_TYPE_QUOTE,
            "quote settlement fee amounts must match offer price_schedule",
        )
    if not _limits_within(
        quote_payload["execution_limits"], offer_payload["execution_profile"]
    ):
        report.push_issue(
            ISSUE_EXECUTION_LIMITS_EXCEED_OFFER,
            _ARTIFACT_TYPE_QUOTE,
            "quote execution_limits must not exceed offer execution_profile maxima",
        )


def _validate_quote_deal_links(
    report: _Report, quote: dict[str, Any], deal: dict[str, Any]
) -> None:
    quote_payload = quote["payload"]
    deal_payload = deal["payload"]

    if deal_payload["provider_id"] != quote_payload["provider_id"]:
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            _ARTIFACT_TYPE_DEAL,
            "deal provider_id must match quote provider_id",
        )
    if deal_payload["requester_id"] != quote_payload["requester_id"]:
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            _ARTIFACT_TYPE_DEAL,
            "deal requester_id must match quote requester_id",
        )
    if deal_payload["quote_hash"] != quote["hash"]:
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            _ARTIFACT_TYPE_DEAL,
            "deal quote_hash must match quote hash",
        )
    if deal_payload["workload_hash"] != quote_payload["workload_hash"]:
        report.push_issue(
            ISSUE_WORKLOAD_HASH_MISMATCH,
            _ARTIFACT_TYPE_DEAL,
            "deal workload_hash must match quote workload_hash",
        )
    if deal_payload.get("confidential_session_hash") != quote_payload.get(
        "confidential_session_hash"
    ):
        report.push_issue(
            ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH,
            _ARTIFACT_TYPE_DEAL,
            "deal confidential_session_hash must match quote confidential_session_hash",
        )
    if deal_payload["admission_deadline"] > quote_payload["expires_at"]:
        report.push_issue(
            ISSUE_DEADLINE_EXCEEDS_QUOTE,
            _ARTIFACT_TYPE_DEAL,
            "deal admission_deadline must not exceed quote expires_at",
        )
    if (
        deal_payload["completion_deadline"] <= deal_payload["admission_deadline"]
        or deal_payload["acceptance_deadline"] < deal_payload["completion_deadline"]
    ):
        report.push_issue(
            ISSUE_DEADLINE_ORDER_INVALID,
            _ARTIFACT_TYPE_DEAL,
            "deal deadlines must satisfy admission < completion <= acceptance",
        )


def _validate_invoice_leg_hash_link(report: _Report, leg: dict[str, Any]) -> None:
    if leg["invoice_hash"] != sha256_hex(leg["invoice_bolt11"].encode("utf-8")):
        report.push_issue(
            ISSUE_INVOICE_HASH_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice leg invoice_hash must equal SHA256(invoice_bolt11)",
        )


def _validate_invoice_bundle_links(
    report: _Report,
    quote: dict[str, Any],
    invoice_bundle: dict[str, Any],
    deal: dict[str, Any],
) -> None:
    quote_payload = quote["payload"]
    bundle_payload = invoice_bundle["payload"]
    deal_payload = deal["payload"]
    settlement_terms = quote_payload["settlement_terms"]

    if settlement_terms["method"] != _SETTLEMENT_METHOD_LIGHTNING_ESCROW:
        report.push_issue(
            ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle is only valid for lightning.base_fee_plus_success_fee.v1 quotes",
        )
    if (
        bundle_payload["provider_id"] != quote_payload["provider_id"]
        or bundle_payload["provider_id"] != deal_payload["provider_id"]
    ):
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle provider_id must match quote and deal provider_id",
        )
    if (
        bundle_payload["requester_id"] != quote_payload["requester_id"]
        or bundle_payload["requester_id"] != deal_payload["requester_id"]
    ):
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle requester_id must match quote and deal requester_id",
        )
    if bundle_payload["quote_hash"] != quote["hash"]:
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle quote_hash must match quote hash",
        )
    if bundle_payload["deal_hash"] != deal["hash"]:
        report.push_issue(
            ISSUE_DEAL_HASH_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle deal_hash must match deal hash",
        )
    if (
        bundle_payload["destination_identity"]
        != settlement_terms["destination_identity"]
    ):
        report.push_issue(
            ISSUE_INVOICE_DESTINATION_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle destination_identity must match quote settlement terms",
        )
    if (
        bundle_payload["base_fee"]["amount_msat"] != settlement_terms["base_fee_msat"]
        or bundle_payload["success_fee"]["amount_msat"]
        != settlement_terms["success_fee_msat"]
    ):
        report.push_issue(
            ISSUE_INVOICE_AMOUNT_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle fee amounts must match quote settlement terms",
        )
    if (
        bundle_payload["success_fee"]["payment_hash"]
        != deal_payload["success_payment_hash"]
    ):
        report.push_issue(
            ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle success_fee.payment_hash must match deal success_payment_hash",
        )
    if (
        bundle_payload["min_final_cltv_expiry"]
        != settlement_terms["min_final_cltv_expiry"]
    ):
        report.push_issue(
            ISSUE_INVOICE_MIN_CLTV_MISMATCH,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle min_final_cltv_expiry must match quote settlement terms",
        )
    if bundle_payload["expires_at"] > quote_payload["expires_at"]:
        report.push_issue(
            ISSUE_DEADLINE_EXCEEDS_QUOTE,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle expires_at must not exceed quote expires_at",
        )
    if bundle_payload["expires_at"] > deal_payload["admission_deadline"]:
        report.push_issue(
            ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "invoice_bundle expires_at must not exceed deal admission_deadline",
        )
    _validate_invoice_leg_hash_link(report, bundle_payload["base_fee"])
    _validate_invoice_leg_hash_link(report, bundle_payload["success_fee"])


def _validate_receipt_links(
    report: _Report,
    quote: dict[str, Any],
    deal: dict[str, Any],
    receipt: dict[str, Any],
) -> None:
    quote_payload = quote["payload"]
    deal_payload = deal["payload"]
    receipt_payload = receipt["payload"]

    if (
        receipt_payload["provider_id"] != quote_payload["provider_id"]
        or receipt_payload["provider_id"] != deal_payload["provider_id"]
    ):
        report.push_issue(
            ISSUE_PROVIDER_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt provider_id must match quote and deal provider_id",
        )
    if (
        receipt_payload["requester_id"] != quote_payload["requester_id"]
        or receipt_payload["requester_id"] != deal_payload["requester_id"]
    ):
        report.push_issue(
            ISSUE_REQUESTER_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt requester_id must match quote and deal requester_id",
        )
    if (
        receipt_payload["quote_hash"] != quote["hash"]
        or receipt_payload["quote_hash"] != deal_payload["quote_hash"]
    ):
        report.push_issue(
            ISSUE_QUOTE_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt quote_hash must match quote hash and deal quote_hash",
        )
    if receipt_payload["deal_hash"] != deal["hash"]:
        report.push_issue(
            ISSUE_DEAL_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt deal_hash must match deal hash",
        )
    if receipt_payload.get("confidential_session_hash") != quote_payload.get(
        "confidential_session_hash"
    ) or receipt_payload.get("confidential_session_hash") != deal_payload.get(
        "confidential_session_hash"
    ):
        report.push_issue(
            ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt confidential_session_hash must match quote and deal confidential_session_hash",
        )
    if (
        receipt_payload["settlement_refs"]["method"]
        != quote_payload["settlement_terms"]["method"]
    ):
        report.push_issue(
            ISSUE_SETTLEMENT_METHOD_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt settlement_refs.method must match quote settlement terms",
        )
    if (
        receipt_payload["settlement_refs"]["base_fee"]["amount_msat"]
        != quote_payload["settlement_terms"]["base_fee_msat"]
        or receipt_payload["settlement_refs"]["success_fee"]["amount_msat"]
        != quote_payload["settlement_terms"]["success_fee_msat"]
    ):
        report.push_issue(
            ISSUE_SETTLEMENT_TERMS_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt settlement fee amounts must match quote settlement terms",
        )
    if (
        receipt_payload["settlement_refs"]["destination_identity"]
        != quote_payload["settlement_terms"]["destination_identity"]
    ):
        report.push_issue(
            ISSUE_INVOICE_DESTINATION_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt destination_identity must match quote settlement terms",
        )
    if not _limits_within(
        receipt_payload["limits_applied"], quote_payload["execution_limits"]
    ):
        report.push_issue(
            ISSUE_EXECUTION_LIMITS_EXCEED_OFFER,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt limits_applied must not exceed quote execution_limits",
        )


def _push_report_issue(
    report: dict[str, Any], code: str, artifact_type: str, message: str
) -> None:
    report["valid"] = False
    report["issues"].append(
        {"code": code, "artifact_type": artifact_type, "message": message}
    )


def _validate_receipt_invoice_bundle_links(
    report: dict[str, Any],
    invoice_bundle: dict[str, Any],
    deal: dict[str, Any],
    receipt: dict[str, Any],
) -> None:
    bundle_payload = invoice_bundle["payload"]
    settlement_refs = receipt["payload"]["settlement_refs"]

    if settlement_refs.get("bundle_hash") != invoice_bundle["hash"]:
        _push_report_issue(
            report,
            ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt bundle_hash must match the invoice_bundle hash",
        )
    if (
        settlement_refs["base_fee"]["amount_msat"]
        != bundle_payload["base_fee"]["amount_msat"]
        or settlement_refs["success_fee"]["amount_msat"]
        != bundle_payload["success_fee"]["amount_msat"]
    ):
        _push_report_issue(
            report,
            ISSUE_INVOICE_AMOUNT_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt settlement amounts must match the invoice_bundle legs",
        )
    if (
        settlement_refs["base_fee"]["invoice_hash"]
        != bundle_payload["base_fee"]["invoice_hash"]
        or settlement_refs["success_fee"]["invoice_hash"]
        != bundle_payload["success_fee"]["invoice_hash"]
    ):
        _push_report_issue(
            report,
            ISSUE_INVOICE_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt invoice hashes must match the invoice_bundle legs",
        )
    if (
        settlement_refs["base_fee"]["payment_hash"]
        != bundle_payload["base_fee"]["payment_hash"]
        or settlement_refs["success_fee"]["payment_hash"]
        != bundle_payload["success_fee"]["payment_hash"]
    ):
        _push_report_issue(
            report,
            ISSUE_INVOICE_PAYMENT_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt payment hashes must match the invoice_bundle legs",
        )
    if (
        settlement_refs["success_fee"]["payment_hash"]
        != deal["payload"]["success_payment_hash"]
    ):
        _push_report_issue(
            report,
            ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "receipt success_fee.payment_hash must match deal success_payment_hash",
        )

    receipt_payload = receipt["payload"]
    base_fee_must_be_settled = (
        receipt_payload["settlement_state"] == "settled"
        or receipt_payload["execution_state"] != "not_started"
    )
    success_fee_must_be_settled = receipt_payload["settlement_state"] == "settled"
    if (
        base_fee_must_be_settled
        and settlement_refs["base_fee"]["state"] != "settled"
    ) or (
        success_fee_must_be_settled
        and settlement_refs["success_fee"]["state"] != "settled"
    ):
        _push_report_issue(
            report,
            ISSUE_INVOICE_STATE_MISMATCH,
            _ARTIFACT_TYPE_RECEIPT,
            "settled or executed lightning receipts require the corresponding invoice legs to be settled",
        )


def _safe_links(
    report: _Report, artifact_type: str, check: Callable[..., None], *args: Any
) -> None:
    """Run a ``_validate_*_links`` check, converting any exception raised by
    malformed input (missing/wrong-typed fields) into a single reported
    issue instead of letting it propagate. Rust has no equivalent guard
    because these link checks only ever run on already-typed structs."""
    try:
        check(*args)
    except (KeyError, TypeError, AttributeError) as error:
        report.push_issue(
            ISSUE_ARTIFACT_SEMANTIC_INVALID,
            artifact_type,
            f"malformed artifact document: {error!r}",
        )


def validate_descriptor_offer(
    descriptor: dict[str, Any], offer: dict[str, Any], now: int | None = None
) -> dict[str, Any]:
    """Mirrors ``chain::validate_descriptor_offer``."""
    report = _Report(PATH_DESCRIPTOR_OFFER)
    _verify_common_artifact(report, descriptor, _ARTIFACT_TYPE_DESCRIPTOR)
    _verify_common_artifact(report, offer, _ARTIFACT_TYPE_OFFER)
    _validate_descriptor_semantics(report, descriptor, now)
    _validate_offer_semantics(report, offer, now)

    def _links() -> None:
        if offer["payload"]["provider_id"] != descriptor["payload"]["provider_id"]:
            report.push_issue(
                ISSUE_PROVIDER_MISMATCH,
                _ARTIFACT_TYPE_OFFER,
                "offer provider_id must match descriptor provider_id",
            )
        if offer["payload"]["descriptor_hash"] != descriptor["hash"]:
            report.push_issue(
                ISSUE_DESCRIPTOR_HASH_MISMATCH,
                _ARTIFACT_TYPE_OFFER,
                "offer descriptor_hash must match descriptor hash",
            )

    _safe_links(report, _ARTIFACT_TYPE_OFFER, _links)
    return report.to_dict()


def validate_offer_quote(
    offer: dict[str, Any], quote: dict[str, Any], now: int | None = None
) -> dict[str, Any]:
    """Mirrors ``chain::validate_offer_quote``."""
    report = _Report(PATH_OFFER_QUOTE)
    _verify_common_artifact(report, offer, _ARTIFACT_TYPE_OFFER)
    _verify_common_artifact(report, quote, _ARTIFACT_TYPE_QUOTE)
    _validate_offer_semantics(report, offer, now)
    _validate_quote_semantics(report, quote, now)
    _safe_links(
        report, _ARTIFACT_TYPE_QUOTE, _validate_offer_quote_links, report, offer, quote
    )
    return report.to_dict()


def validate_quote_deal(
    quote: dict[str, Any], deal: dict[str, Any], now: int | None = None
) -> dict[str, Any]:
    """Mirrors ``chain::validate_quote_deal``."""
    report = _Report(PATH_QUOTE_DEAL)
    _verify_common_artifact(report, quote, _ARTIFACT_TYPE_QUOTE)
    _verify_common_artifact(report, deal, _ARTIFACT_TYPE_DEAL)
    _validate_quote_semantics(report, quote, now)
    _validate_deal_semantics(report, deal)
    _safe_links(
        report, _ARTIFACT_TYPE_DEAL, _validate_quote_deal_links, report, quote, deal
    )
    return report.to_dict()


def validate_quote_invoice_bundle_deal(
    quote: dict[str, Any],
    invoice_bundle: dict[str, Any],
    deal: dict[str, Any],
    now: int | None = None,
) -> dict[str, Any]:
    """Mirrors ``chain::validate_quote_invoice_bundle_deal``."""
    report = _Report(PATH_QUOTE_INVOICE_BUNDLE_DEAL)
    _verify_common_artifact(report, quote, _ARTIFACT_TYPE_QUOTE)
    _verify_common_artifact(report, invoice_bundle, _TRANSPORT_TYPE_INVOICE_BUNDLE)
    _verify_common_artifact(report, deal, _ARTIFACT_TYPE_DEAL)
    _validate_quote_semantics(report, quote, now)
    _validate_invoice_bundle_semantics(report, invoice_bundle, now)
    _validate_deal_semantics(report, deal)
    _safe_links(
        report, _ARTIFACT_TYPE_DEAL, _validate_quote_deal_links, report, quote, deal
    )
    _safe_links(
        report,
        _TRANSPORT_TYPE_INVOICE_BUNDLE,
        _validate_invoice_bundle_links,
        report,
        quote,
        invoice_bundle,
        deal,
    )
    return report.to_dict()


def validate_quote_deal_receipt(
    quote: dict[str, Any],
    deal: dict[str, Any],
    receipt: dict[str, Any],
    now: int | None = None,
) -> dict[str, Any]:
    """Mirrors ``chain::validate_quote_deal_receipt``."""
    report = _Report(PATH_QUOTE_DEAL_RECEIPT)
    _verify_common_artifact(report, quote, _ARTIFACT_TYPE_QUOTE)
    _verify_common_artifact(report, deal, _ARTIFACT_TYPE_DEAL)
    _verify_common_artifact(report, receipt, _ARTIFACT_TYPE_RECEIPT)
    _validate_quote_semantics(report, quote, now)
    _validate_deal_semantics(report, deal)
    _validate_receipt_semantics(report, receipt)
    _safe_links(
        report, _ARTIFACT_TYPE_DEAL, _validate_quote_deal_links, report, quote, deal
    )
    _safe_links(
        report,
        _ARTIFACT_TYPE_RECEIPT,
        _validate_receipt_links,
        report,
        quote,
        deal,
        receipt,
    )
    return report.to_dict()


def validate_full_chain(
    descriptor: dict[str, Any],
    offer: dict[str, Any],
    quote: dict[str, Any],
    deal: dict[str, Any],
    invoice_bundle: dict[str, Any] | None = None,
    receipt: dict[str, Any] | None = None,
    now: int | None = None,
) -> dict[str, Any]:
    """Compose the pairwise validators over a complete (or partial) deal.

    Always runs ``descriptor_offer`` and ``offer_quote``. Runs
    ``quote_invoice_bundle_deal`` in place of ``quote_deal`` when
    ``invoice_bundle`` is supplied (the invoice-bundle-aware link check
    covers everything the plain quote/deal link check does, plus the
    bundle's own links). Runs ``quote_deal_receipt`` in addition when
    ``receipt`` is supplied.

    Returns:
        ``{"valid": bool, "reports": [<pairwise report dict>, ...]}`` where
        ``valid`` is the conjunction of every included report's ``valid``.
    """
    reports = [
        validate_descriptor_offer(descriptor, offer, now),
        validate_offer_quote(offer, quote, now),
    ]
    uses_lightning_bundle = (
        quote["payload"]["settlement_terms"]["method"]
        == _SETTLEMENT_METHOD_LIGHTNING_ESCROW
    )
    if invoice_bundle is not None:
        reports.append(
            validate_quote_invoice_bundle_deal(quote, invoice_bundle, deal, now)
        )
    elif uses_lightning_bundle:
        quote_deal = validate_quote_deal(quote, deal, now)
        _push_report_issue(
            quote_deal,
            ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING,
            _TRANSPORT_TYPE_INVOICE_BUNDLE,
            "lightning.base_fee_plus_success_fee.v1 chains require an invoice_bundle",
        )
        reports.append(quote_deal)
    else:
        reports.append(validate_quote_deal(quote, deal, now))
    if receipt is not None:
        receipt_report = validate_quote_deal_receipt(quote, deal, receipt, now)
        if uses_lightning_bundle and invoice_bundle is not None:
            try:
                _validate_receipt_invoice_bundle_links(
                    receipt_report, invoice_bundle, deal, receipt
                )
            except (KeyError, TypeError, AttributeError) as error:
                _push_report_issue(
                    receipt_report,
                    ISSUE_ARTIFACT_SEMANTIC_INVALID,
                    _ARTIFACT_TYPE_RECEIPT,
                    f"malformed artifact document: {error!r}",
                )
        reports.append(receipt_report)

    return {"valid": all(r["valid"] for r in reports), "reports": reports}
