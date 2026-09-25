"""Per-kind semantic validators for Froglet signed artifacts.

Each ``validate_*_artifact`` function takes a full signed-artifact document
(the same shape :mod:`froglet_verify.envelope` operates on: a dict with
``signer``, ``payload``, etc.) and returns ``None`` if the artifact's
kind-specific invariants hold, or an error-message ``str`` otherwise. These
run *after* envelope verification (:func:`froglet_verify.envelope.
verify_signed_artifact`) — a valid signature only proves the signer produced
these exact bytes, not that the claimed identities/values inside the payload
are self-consistent.

Sources, mirrored exactly (error strings copied verbatim, check order
preserved). All six ``validate_*_artifact`` functions, plus their shared
helpers (``validate_quote_settlement_terms``, ``validate_invoice_leg``,
``is_lower_hex_len``), are now public functions co-located in
``froglet-protocol/src/protocol/kernel.rs`` — quote/deal/invoice_bundle's
validators moved there from private helpers of the same name in
``froglet-protocol/src/protocol/chain.rs`` (``chain.rs`` now imports them
rather than defining its own copies); this module has always grouped all
six together the same way, so that refactor changed nothing here beyond
where the docstrings point.

Settlement methods covered by ``validate_quote_settlement_terms`` and the
``method``-dispatch branch of ``validate_receipt_artifact``: ``"none"``,
``"lightning.base_fee_plus_success_fee.v1"``, ``"stripe_mpp.v1"``,
``"lightning.prepaid.v1"``, and ``"x402.eip3009.v1"`` (the EVM stablecoin
rail settled via an EIP-3009 ``TransferWithAuthorization``). Like
``lightning.prepaid.v1``, x402 is single-leg and prepaid (no hold invoices,
empty-canceled ``success_fee``), but unlike every other method its
``destination_identity`` is a 20-byte lowercase-hex EVM address (not a
33-byte compressed secp256k1 key) and a settled receipt *requires*
``bundle_hash`` (committing to the payer-signed EIP-712 authorization plus
the on-chain settlement reference) rather than forbidding it.

This module operates on plain ``dict``/``list``/``str``/``int``/``bool``/
``None`` values (as produced by ``json.load``), not on typed objects. Fields
that are ``Option<T>`` (or have ``#[serde(default)]``) on the Rust struct are
read with ``.get(...)``, which naturally yields the same default (``None``,
``[]``, ``""``, etc.) whether the JSON key is absent or explicitly ``null`` —
matching serde's deserialization behavior for those fields. Fields that are
*required* (no default, not `Option`) are read via direct indexing (``[...]``);
a malformed document missing one raises ``KeyError``/``TypeError`` rather
than being silently treated as valid. In the Rust source such a document
would already have failed to deserialize into the typed struct before any of
these checks could run at all — callers accepting untrusted input (the CLI,
the conformance runner) catch and report that as a distinct "malformed"
outcome rather than treating it as a clean semantic accept/reject.
"""

from __future__ import annotations

from typing import Any

from .envelope import sha256_hex
from .jcs import canonicalize
from .schnorr import bip340_verify

__all__ = [
    "validate_descriptor_artifact",
    "validate_offer_artifact",
    "validate_quote_artifact",
    "validate_deal_artifact",
    "validate_invoice_bundle_artifact",
    "validate_receipt_artifact",
]


def is_lower_hex_len(value: Any, length: int) -> bool:
    """Exactly ``length`` ASCII hex digits, lowercase only (no uppercase
    hex digits accepted). Mirrors ``chain::is_lower_hex_len``, which is
    intentionally stricter than a case-insensitive hex check."""
    return (
        isinstance(value, str)
        and len(value) == length
        and all(("0" <= c <= "9") or ("a" <= c <= "f") for c in value)
    )


def _decode_hex32(value: Any) -> bytes | None:
    """Case-insensitive hex decode requiring exactly 32 bytes (64 hex
    chars). Mirrors ``decode_hash32`` in ``kernel.rs``, which is built on
    Rust's ``hex::decode`` — case-insensitive but, unlike Python's
    ``bytes.fromhex``, intolerant of embedded whitespace. A manual character
    check (rather than a bare ``try: bytes.fromhex`` ) is used here so this
    function rejects whitespace the same way the Rust ``hex`` crate does."""
    if not isinstance(value, str) or len(value) != 64:
        return None
    if not all(
        ("0" <= c <= "9") or ("a" <= c <= "f") or ("A" <= c <= "F") for c in value
    ):
        return None
    return bytes.fromhex(value)


def validate_descriptor_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors ``validate_descriptor_artifact`` in ``kernel.rs``."""
    payload = doc["payload"]
    if doc["signer"] != payload["provider_id"]:
        return "descriptor signer does not match provider_id"
    if payload["protocol_version"] != "froglet/v1":
        return "descriptor protocol_version must be froglet/v1"
    for identity in payload.get("linked_identities", []):
        if identity["identity_kind"] != "nostr":
            continue
        if not is_lower_hex_len(identity["identity"], 64):
            return "descriptor linked Nostr identity must be a 32-byte lowercase hex key"
        if identity["signature_algorithm"] != "secp256k1_schnorr_bip340":
            return "descriptor linked Nostr identity signature_algorithm is invalid"
        scopes = identity["scope"]
        if not scopes or any(
            not isinstance(scope, str) or not scope.startswith("publication.")
            for scope in scopes
        ):
            return (
                "descriptor linked Nostr identity scope must contain only "
                "publication scopes"
            )
        expires_at = identity.get("expires_at")
        if expires_at is not None and expires_at <= identity["created_at"]:
            return (
                "descriptor linked Nostr identity expires_at must be later than "
                "created_at"
            )
        if not is_lower_hex_len(identity["linked_signature"], 128):
            return (
                "descriptor linked Nostr identity signature must be 64-byte "
                "lowercase hex"
            )
        scope_hash = sha256_hex(canonicalize(scopes))
        expiry = "-" if expires_at is None else str(expires_at)
        challenge = (
            "froglet:identity_link:v1\n"
            f"{payload['provider_id']}\n"
            f"{identity['identity_kind']}\n"
            f"{identity['identity']}\n"
            f"{scope_hash}\n"
            f"{identity['created_at']}\n"
            f"{expiry}"
        ).encode("utf-8")
        if not bip340_verify(
            identity["identity"], identity["linked_signature"], challenge
        ):
            return "descriptor linked Nostr identity signature is invalid"
    return None


def validate_offer_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors ``validate_offer_artifact`` in ``kernel.rs``."""
    payload = doc["payload"]
    if doc["signer"] != payload["provider_id"]:
        return "offer signer does not match provider_id"
    if payload["offer_id"].strip() == "":
        return "offer offer_id must be non-empty"
    if payload["descriptor_hash"].strip() == "":
        return "offer descriptor_hash must be non-empty"
    price = payload["price_schedule"]
    is_free = price["base_fee_msat"] == 0 and price["success_fee_msat"] == 0
    if is_free:
        if payload["settlement_method"] != "none":
            return "free offer settlement_method must be none"
        return None
    if payload["settlement_method"] not in (
        "lightning.base_fee_plus_success_fee.v1",
        "stripe_mpp.v1",
        "lightning.prepaid.v1",
        "x402.eip3009.v1",
    ):
        return "paid offer settlement_method is unsupported"
    if (
        payload["settlement_method"] != "lightning.base_fee_plus_success_fee.v1"
        and price["success_fee_msat"] != 0
    ):
        return "single-leg paid offer success_fee_msat must be zero"
    return None


def _validate_quote_settlement_terms(terms: dict[str, Any]) -> str | None:
    """Mirrors ``validate_quote_settlement_terms`` in ``kernel.rs`` (moved
    there from a private ``chain.rs`` helper of the same name; the check
    order and error strings are unchanged by that move)."""
    method = terms["method"]
    if method == "none":
        if terms["destination_identity"] != "":
            return "free quote destination_identity must be empty"
        if terms["base_fee_msat"] != 0 or terms["success_fee_msat"] != 0:
            return "free quote fee amounts must be zero"
    elif method == "lightning.base_fee_plus_success_fee.v1":
        if not is_lower_hex_len(terms["destination_identity"], 66):
            return (
                "lightning quote destination_identity must be compressed "
                "secp256k1 lowercase hex"
            )
    elif method == "stripe_mpp.v1":
        if terms["destination_identity"] != "":
            return "non-escrow quote destination_identity must be empty"
        if terms["success_fee_msat"] != 0:
            return "non-escrow quote success_fee_msat must be zero"
    elif method == "lightning.prepaid.v1":
        if not is_lower_hex_len(terms["destination_identity"], 66):
            return (
                "lightning prepaid quote destination_identity must be "
                "compressed secp256k1 lowercase hex"
            )
        if terms["success_fee_msat"] != 0:
            return "non-escrow quote success_fee_msat must be zero"
    elif method == "x402.eip3009.v1":
        if not is_lower_hex_len(terms["destination_identity"], 40):
            return "x402 quote destination_identity must be a 20-byte lowercase hex EVM address"
        if terms["success_fee_msat"] != 0:
            return "x402 quote success_fee_msat must be zero"
    else:
        return "quote settlement_terms.method is invalid"
    return None


def validate_quote_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors the private ``validate_quote_artifact`` in ``chain.rs``."""
    payload = doc["payload"]
    if doc["signer"] != payload["provider_id"]:
        return "quote signer does not match provider_id"
    if payload["requester_id"].strip() == "":
        return "quote requester_id must be non-empty"
    if payload["descriptor_hash"].strip() == "":
        return "quote descriptor_hash must be non-empty"
    if payload["offer_hash"].strip() == "":
        return "quote offer_hash must be non-empty"
    if payload["workload_kind"].strip() == "":
        return "quote workload_kind must be non-empty"
    if payload["workload_hash"].strip() == "":
        return "quote workload_hash must be non-empty"
    return _validate_quote_settlement_terms(payload["settlement_terms"])


def validate_deal_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors the private ``validate_deal_artifact`` in ``chain.rs``."""
    payload = doc["payload"]
    if doc["signer"] != payload["requester_id"]:
        return "deal signer does not match requester_id"
    if payload["provider_id"].strip() == "":
        return "deal provider_id must be non-empty"
    if payload["quote_hash"].strip() == "":
        return "deal quote_hash must be non-empty"
    if payload["workload_hash"].strip() == "":
        return "deal workload_hash must be non-empty"
    if not is_lower_hex_len(payload["success_payment_hash"], 64):
        return "deal success_payment_hash must be lowercase 32-byte hex"
    if payload["completion_deadline"] <= payload["admission_deadline"]:
        return "deal completion_deadline must be greater than admission_deadline"
    if payload["acceptance_deadline"] < payload["completion_deadline"]:
        return (
            "deal acceptance_deadline must be greater than or equal to "
            "completion_deadline"
        )
    return None


def _validate_invoice_leg(name: str, leg: dict[str, Any]) -> str | None:
    """Mirrors the private ``validate_invoice_leg`` in ``chain.rs``."""
    if leg["invoice_bolt11"].strip() == "":
        return f"invoice_bundle {name}.invoice_bolt11 must be non-empty"
    if not is_lower_hex_len(leg["invoice_hash"], 64):
        return f"invoice_bundle {name}.invoice_hash must be lowercase 32-byte hex"
    if not is_lower_hex_len(leg["payment_hash"], 64):
        return f"invoice_bundle {name}.payment_hash must be lowercase 32-byte hex"
    expected_invoice_hash = sha256_hex(leg["invoice_bolt11"].encode("utf-8"))
    if leg["invoice_hash"] != expected_invoice_hash:
        return f"invoice_bundle {name}.invoice_hash must equal SHA256(invoice_bolt11)"
    return None


def validate_invoice_bundle_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors the private ``validate_invoice_bundle_artifact`` in
    ``chain.rs``."""
    payload = doc["payload"]
    if doc["signer"] != payload["provider_id"]:
        return "invoice_bundle signer does not match provider_id"
    if payload["requester_id"].strip() == "":
        return "invoice_bundle requester_id must be non-empty"
    if payload["quote_hash"].strip() == "":
        return "invoice_bundle quote_hash must be non-empty"
    if payload["deal_hash"].strip() == "":
        return "invoice_bundle deal_hash must be non-empty"
    if not is_lower_hex_len(payload["destination_identity"], 66):
        return (
            "invoice_bundle destination_identity must be compressed "
            "secp256k1 lowercase hex"
        )

    base_fee = payload["base_fee"]
    success_fee = payload["success_fee"]

    error = _validate_invoice_leg("base_fee", base_fee)
    if error is not None:
        return error
    error = _validate_invoice_leg("success_fee", success_fee)
    if error is not None:
        return error

    if success_fee["state"] != "open":
        return "invoice_bundle success_fee.state must be open at issuance"
    if base_fee["state"] != "open" and not (
        base_fee["amount_msat"] == 0 and base_fee["state"] == "settled"
    ):
        return (
            "invoice_bundle base_fee.state must be open unless zero-valued and settled"
        )
    return None


def _receipt_leg_is_empty_canceled(leg: dict[str, Any]) -> bool:
    """Mirrors ``receipt_leg_is_empty_canceled`` in ``kernel.rs``."""
    return bool(
        leg["amount_msat"] == 0
        and leg["invoice_hash"] == ""
        and leg["payment_hash"] == ""
        and leg["state"] == "canceled"
    )


def validate_receipt_artifact(doc: dict[str, Any]) -> str | None:
    """Mirrors ``validate_receipt_artifact`` in ``kernel.rs``, including all
    four settlement-method branches
    (``lightning.base_fee_plus_success_fee.v1``, ``none``, ``stripe_mpp.v1``,
    ``lightning.prepaid.v1``)."""
    payload = doc["payload"]

    if doc["signer"] != payload["provider_id"]:
        return "receipt signer does not match provider_id"

    started_at = payload.get("started_at")
    if started_at is not None and payload["finished_at"] < started_at:
        return "receipt finished_at is earlier than started_at"

    has_result_hash = payload.get("result_hash") is not None
    has_result_format = payload.get("result_format") is not None
    if payload["execution_state"] == "succeeded":
        if not has_result_hash or not has_result_format:
            return (
                "receipt with execution_state succeeded must include "
                "result_hash and result_format"
            )
    elif has_result_hash or has_result_format:
        return (
            "receipt result_hash and result_format must be absent unless "
            "execution_state is succeeded"
        )

    deal_state = payload["deal_state"]
    execution_state = payload["execution_state"]
    if deal_state == "rejected":
        if execution_state != "not_started":
            return "rejected receipt must have execution_state not_started"
    elif deal_state == "succeeded":
        if execution_state != "succeeded":
            return "successful receipt must have execution_state succeeded"
    elif deal_state == "failed":
        if execution_state != "failed":
            return "failed receipt must have execution_state failed"
    elif deal_state == "canceled":
        if execution_state not in ("not_started", "succeeded"):
            return "canceled receipt must have execution_state not_started or succeeded"
    else:
        return "receipt deal_state is invalid"

    settlement_refs = payload["settlement_refs"]
    method = settlement_refs["method"]
    base_fee = settlement_refs["base_fee"]
    success_fee = settlement_refs["success_fee"]
    settlement_state = payload["settlement_state"]

    if method == "lightning.base_fee_plus_success_fee.v1":
        if not (settlement_refs.get("bundle_hash") or ""):
            return "lightning receipt must include bundle_hash"
        if settlement_refs["destination_identity"] == "":
            return "lightning receipt must include destination_identity"
        if base_fee["state"] in ("open", "accepted") or success_fee["state"] in (
            "open",
            "accepted",
        ):
            return "lightning receipt settlement legs must be terminal"

        if settlement_state == "settled":
            if base_fee["state"] != "settled":
                return (
                    "lightning receipt settlement_state settled requires "
                    "base_fee.state settled"
                )
            if success_fee["state"] != "settled":
                return (
                    "lightning receipt settlement_state settled requires "
                    "success_fee.state settled"
                )
        elif settlement_state == "canceled":
            if success_fee["state"] != "canceled":
                return (
                    "lightning receipt settlement_state canceled requires "
                    "success_fee.state canceled"
                )
        elif settlement_state == "expired":
            if success_fee["state"] != "expired":
                return (
                    "lightning receipt settlement_state expired requires "
                    "success_fee.state expired"
                )
        else:
            return "lightning receipt settlement_state must be settled, canceled, or expired"

        if deal_state == "succeeded" and settlement_state != "settled":
            return "successful lightning receipt must have settlement_state settled"

    elif method == "none":
        if settlement_state != "none":
            return "free receipt settlement_state must be none"
        if settlement_refs.get("bundle_hash") is not None:
            return "free receipt must not include bundle_hash"
        if settlement_refs["destination_identity"] != "":
            return "free receipt destination_identity must be empty"
        if not _receipt_leg_is_empty_canceled(
            base_fee
        ) or not _receipt_leg_is_empty_canceled(success_fee):
            return (
                "free receipt settlement legs must be zero-valued canceled placeholders"
            )

    elif method == "stripe_mpp.v1":
        if settlement_refs.get("bundle_hash") is not None:
            return "stripe_mpp.v1 receipt must not include bundle_hash"
        if settlement_refs["destination_identity"] != "":
            return "stripe_mpp.v1 receipt destination_identity must be empty"
        if not _receipt_leg_is_empty_canceled(success_fee):
            return "stripe_mpp.v1 receipt success_fee must be a zero-valued canceled placeholder"
        if base_fee["state"] in ("open", "accepted"):
            return "stripe_mpp.v1 receipt base_fee.state must be terminal (settled or canceled)"

        if settlement_state == "settled":
            if base_fee["state"] != "settled":
                return (
                    "stripe_mpp.v1 receipt settlement_state settled requires "
                    "base_fee.state settled"
                )
        elif settlement_state == "canceled":
            if base_fee["state"] != "canceled":
                return (
                    "stripe_mpp.v1 receipt settlement_state canceled requires "
                    "base_fee.state canceled"
                )
        else:
            return "stripe_mpp.v1 receipt settlement_state must be settled or canceled"

        if deal_state == "succeeded" and settlement_state != "settled":
            return "successful stripe_mpp.v1 receipt must have settlement_state settled"

    elif method == "lightning.prepaid.v1":
        if settlement_refs.get("bundle_hash") is not None:
            return "lightning.prepaid.v1 receipt must not include bundle_hash"
        if not is_lower_hex_len(settlement_refs["destination_identity"], 66):
            return (
                "lightning.prepaid.v1 receipt destination_identity must be "
                "compressed secp256k1 lowercase hex"
            )
        if not _receipt_leg_is_empty_canceled(success_fee):
            return (
                "lightning.prepaid.v1 receipt success_fee must be a "
                "zero-valued canceled placeholder"
            )
        if base_fee["state"] in ("open", "accepted"):
            return (
                "lightning.prepaid.v1 receipt base_fee.state must be "
                "terminal (settled or canceled)"
            )

        if settlement_state == "settled":
            if base_fee["state"] != "settled":
                return (
                    "lightning.prepaid.v1 receipt settlement_state settled "
                    "requires base_fee.state settled"
                )
            payment_hash = base_fee["payment_hash"]
            preimage = base_fee["invoice_hash"]
            preimage_bytes = _decode_hex32(preimage)
            if preimage_bytes is None:
                return (
                    "lightning.prepaid.v1 settled receipt preimage must be 32-byte hex"
                )
            if _decode_hex32(payment_hash) is None:
                return "lightning.prepaid.v1 settled receipt payment_hash must be 32-byte hex"
            if sha256_hex(preimage_bytes) != payment_hash.lower():
                return (
                    "lightning.prepaid.v1 receipt preimage does not match payment_hash"
                )
        elif settlement_state == "canceled":
            if base_fee["state"] != "canceled":
                return (
                    "lightning.prepaid.v1 receipt settlement_state canceled "
                    "requires base_fee.state canceled"
                )
            if base_fee["invoice_hash"] != "":
                return "lightning.prepaid.v1 canceled receipt must not carry a preimage"
        else:
            return "lightning.prepaid.v1 receipt settlement_state must be settled or canceled"

        if deal_state == "succeeded" and settlement_state != "settled":
            return "successful lightning.prepaid.v1 receipt must have settlement_state settled"

    elif method == "x402.eip3009.v1":
        if not is_lower_hex_len(settlement_refs["destination_identity"], 40):
            return "x402.eip3009.v1 receipt destination_identity must be a 20-byte lowercase hex EVM address"
        if not _receipt_leg_is_empty_canceled(success_fee):
            return "x402.eip3009.v1 receipt success_fee must be a zero-valued canceled placeholder"
        if base_fee["state"] in ("open", "accepted"):
            return "x402.eip3009.v1 receipt base_fee.state must be terminal (settled or canceled)"

        if settlement_state == "settled":
            if base_fee["state"] != "settled":
                return (
                    "x402.eip3009.v1 receipt settlement_state settled requires "
                    "base_fee.state settled"
                )
            bundle_hash = settlement_refs.get("bundle_hash") or ""
            if not is_lower_hex_len(bundle_hash, 64):
                return (
                    "x402.eip3009.v1 settled receipt must include a 32-byte "
                    "lowercase hex bundle_hash"
                )
            if not is_lower_hex_len(base_fee["payment_hash"], 64):
                return (
                    "x402.eip3009.v1 settled receipt payment_hash must carry the "
                    "32-byte hex EIP-3009 authorization nonce"
                )
            if not is_lower_hex_len(base_fee["invoice_hash"], 64):
                return (
                    "x402.eip3009.v1 settled receipt invoice_hash must carry the "
                    "32-byte hex settle transaction hash"
                )
        elif settlement_state == "canceled":
            if base_fee["state"] != "canceled":
                return (
                    "x402.eip3009.v1 receipt settlement_state canceled requires "
                    "base_fee.state canceled"
                )
            if settlement_refs.get("bundle_hash") is not None:
                return "x402.eip3009.v1 canceled receipt must not include bundle_hash"
            if base_fee["invoice_hash"] != "":
                return "x402.eip3009.v1 canceled receipt must not carry a settle transaction hash"
        else:
            return (
                "x402.eip3009.v1 receipt settlement_state must be settled or canceled"
            )

        if deal_state == "succeeded" and settlement_state != "settled":
            return (
                "successful x402.eip3009.v1 receipt must have settlement_state settled"
            )

    else:
        return "receipt settlement_refs.method is invalid"

    return None
