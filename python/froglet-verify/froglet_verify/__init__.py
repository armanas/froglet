"""froglet_verify: a pure-Python, zero-runtime-dependency offline verifier
for Froglet protocol signed-artifact chains.

Verifies, entirely offline (no network, no filesystem access beyond reading
the input given to it):

- **Envelope crypto** (:mod:`froglet_verify.envelope`): JCS payload hashing,
  the canonical signing-bytes construction, and BIP-340 Schnorr signature
  verification (:mod:`froglet_verify.schnorr`) over secp256k1, implemented
  with plain-integer arithmetic and no third-party dependencies.
- **Per-kind semantics** (:mod:`froglet_verify.semantics`): the invariants
  each artifact kind (descriptor, offer, quote, deal, invoice_bundle,
  receipt) must satisfy beyond having a valid signature.
- **Pairwise and full-chain validation** (:mod:`froglet_verify.chain`): the
  cross-artifact linkage checks (matching provider/requester identities,
  hash chaining, deadline ordering, settlement-term consistency) that bind
  a descriptor -> offer -> quote -> deal (-> invoice_bundle) (-> receipt)
  sequence into one coherent, verifiable deal.

This is one of three reference conformance runners for the Froglet kernel
(alongside the Rust implementation in ``froglet-protocol`` and a JS/wasm
runner); see :mod:`froglet_verify.conformance` for the fixture-driven
conformance runner, and the package's ``tests/`` for exact-parity checks
against the Rust source these modules mirror.
"""

from __future__ import annotations

from .chain import (
    ISSUE_ARTIFACT_ENVELOPE_INVALID,
    ISSUE_ARTIFACT_EXPIRED,
    ISSUE_ARTIFACT_SEMANTIC_INVALID,
    ISSUE_ARTIFACT_TYPE_MISMATCH,
    ISSUE_CONFIDENTIAL_SESSION_HASH_MISMATCH,
    ISSUE_DEADLINE_EXCEEDS_QUOTE,
    ISSUE_DEADLINE_ORDER_INVALID,
    ISSUE_DEAL_HASH_MISMATCH,
    ISSUE_DESCRIPTOR_HASH_MISMATCH,
    ISSUE_EXECUTION_LIMITS_EXCEED_OFFER,
    ISSUE_INVOICE_AMOUNT_MISMATCH,
    ISSUE_INVOICE_BUNDLE_FOR_NON_LIGHTNING,
    ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING,
    ISSUE_INVOICE_DESTINATION_MISMATCH,
    ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL,
    ISSUE_INVOICE_HASH_MISMATCH,
    ISSUE_INVOICE_MIN_CLTV_MISMATCH,
    ISSUE_INVOICE_PAYMENT_HASH_MISMATCH,
    ISSUE_INVOICE_STATE_MISMATCH,
    ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH,
    ISSUE_OFFER_HASH_MISMATCH,
    ISSUE_PROVIDER_MISMATCH,
    ISSUE_QUOTE_EXPIRY_EXCEEDS_OFFER,
    ISSUE_QUOTE_HASH_MISMATCH,
    ISSUE_REQUESTER_MISMATCH,
    ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH,
    ISSUE_SETTLEMENT_METHOD_MISMATCH,
    ISSUE_SETTLEMENT_TERMS_MISMATCH,
    ISSUE_WORKLOAD_HASH_MISMATCH,
    ISSUE_WORKLOAD_KIND_MISMATCH,
    PATH_DESCRIPTOR_OFFER,
    PATH_OFFER_QUOTE,
    PATH_QUOTE_DEAL,
    PATH_QUOTE_DEAL_RECEIPT,
    PATH_QUOTE_INVOICE_BUNDLE_DEAL,
    validate_descriptor_offer,
    validate_full_chain,
    validate_offer_quote,
    validate_quote_deal,
    validate_quote_deal_receipt,
    validate_quote_invoice_bundle_deal,
)
from .envelope import (
    FROGLET_SCHEMA_V1,
    artifact_hash,
    canonical_signing_bytes,
    payload_hash,
    sha256_hex,
    verify_signed_artifact,
)
from .jcs import canonicalize
from .schnorr import bip340_verify
from .semantics import (
    validate_deal_artifact,
    validate_descriptor_artifact,
    validate_invoice_bundle_artifact,
    validate_offer_artifact,
    validate_quote_artifact,
    validate_receipt_artifact,
)

__version__ = "0.1.0"

__all__ = [
    "__version__",
    # jcs
    "canonicalize",
    # schnorr
    "bip340_verify",
    # envelope
    "FROGLET_SCHEMA_V1",
    "sha256_hex",
    "payload_hash",
    "canonical_signing_bytes",
    "artifact_hash",
    "verify_signed_artifact",
    # semantics
    "validate_descriptor_artifact",
    "validate_offer_artifact",
    "validate_quote_artifact",
    "validate_deal_artifact",
    "validate_invoice_bundle_artifact",
    "validate_receipt_artifact",
    # chain
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
    "ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING",
    "ISSUE_INVOICE_AMOUNT_MISMATCH",
    "ISSUE_INVOICE_DESTINATION_MISMATCH",
    "ISSUE_INVOICE_SUCCESS_PAYMENT_HASH_MISMATCH",
    "ISSUE_INVOICE_MIN_CLTV_MISMATCH",
    "ISSUE_INVOICE_HASH_MISMATCH",
    "ISSUE_INVOICE_PAYMENT_HASH_MISMATCH",
    "ISSUE_INVOICE_STATE_MISMATCH",
    "ISSUE_INVOICE_EXPIRY_EXCEEDS_DEAL",
    "ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH",
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
