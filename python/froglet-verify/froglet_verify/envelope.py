"""Signed-artifact envelope verification.

Mirrors the envelope layer of ``froglet-protocol/src/protocol/kernel.rs``:
the ``SignedArtifact<T>`` shape, ``payload_hash``, ``canonical_signing_bytes``,
``artifact_hash``, and ``verify_artifact``. This module is deliberately
type-agnostic — it operates on plain ``dict`` documents (as parsed from JSON)
rather than on kind-specific typed payloads; per-kind semantic invariants
live in :mod:`froglet_verify.semantics`.

A Froglet signed artifact is a JSON object shaped like::

    {
      "artifact_type": "quote",
      "schema_version": "froglet/v1",
      "signer": "<64-hex x-only secp256k1 pubkey>",
      "created_at": 1700000000,
      "payload_hash": "<sha256 hex of JCS(payload)>",
      "hash": "<sha256 hex of JCS([schema_version, artifact_type, signer,
                created_at, payload_hash, payload])>",
      "payload": { ... kind-specific ... },
      "signature": "<128-hex BIP-340 signature over the same JCS bytes>"
    }
"""

from __future__ import annotations

import hashlib
from typing import Any

from .jcs import canonicalize
from .schnorr import bip340_verify

__all__ = [
    "FROGLET_SCHEMA_V1",
    "sha256_hex",
    "payload_hash",
    "canonical_signing_bytes",
    "artifact_hash",
    "verify_signed_artifact",
]

FROGLET_SCHEMA_V1 = "froglet/v1"

_REQUIRED_STRING_FIELDS = (
    "artifact_type",
    "schema_version",
    "signer",
    "payload_hash",
    "signature",
)


def sha256_hex(data: bytes) -> str:
    """SHA-256 of ``data``, as lowercase hex. Mirrors ``crypto::sha256_hex``."""
    return hashlib.sha256(data).hexdigest()


def payload_hash(payload: Any) -> str:
    """``sha256_hex(JCS(payload))``. Mirrors ``protocol::payload_hash``."""
    return sha256_hex(canonicalize(payload))


def canonical_signing_bytes(
    schema_version: str,
    artifact_type: str,
    signer: str,
    created_at: int,
    payload_hash_hex: str,
    payload: Any,
) -> bytes:
    """The exact bytes that are hashed (for ``hash``) and signed (for
    ``signature``): the JCS encoding of the 6-element JSON array
    ``[schema_version, artifact_type, signer, created_at, payload_hash,
    payload]``. Mirrors ``protocol::canonical_signing_bytes``.
    """
    return canonicalize(
        [schema_version, artifact_type, signer, created_at, payload_hash_hex, payload]
    )


def artifact_hash(doc: dict[str, Any]) -> str:
    """Recompute what ``doc["hash"]`` *should* be, from ``doc["payload"]``
    and the envelope fields, ignoring whatever ``doc["payload_hash"]`` and
    ``doc["hash"]`` currently claim.

    Mirrors ``protocol::artifact_hash`` exactly: that Rust function also
    recomputes ``payload_hash`` fresh from the payload rather than trusting
    the stored field. This makes it a pure "what would this hash to"
    function, useful for round-trip/determinism checks against a fixture's
    recorded ``artifact_hash`` independent of whether the stored
    ``payload_hash``/``hash`` fields are themselves correct (that integrity
    check is what :func:`verify_signed_artifact` performs).

    Requires ``doc`` to contain ``schema_version``, ``artifact_type``,
    ``signer``, ``created_at``, and ``payload`` (raises ``KeyError`` if any
    are missing — callers verifying untrusted input should catch that).
    """
    fresh_payload_hash = payload_hash(doc["payload"])
    signing_bytes = canonical_signing_bytes(
        doc["schema_version"],
        doc["artifact_type"],
        doc["signer"],
        doc["created_at"],
        fresh_payload_hash,
        doc["payload"],
    )
    return sha256_hex(signing_bytes)


def verify_signed_artifact(doc: dict[str, Any]) -> tuple[bool, str | None]:
    """Verify a signed artifact's envelope: schema version, payload-hash
    integrity, artifact-hash integrity, and the BIP-340 signature.

    Mirrors ``protocol::verify_artifact`` (which returns a bare ``bool``);
    this returns ``(False, reason)`` on failure instead so callers building a
    human-readable report don't have to re-derive why verification failed.
    The reason strings are this module's own diagnostic text (Rust's
    ``verify_artifact`` has no error messages to mirror here — it is a
    boolean check); they are not part of any cross-implementation contract.

    Any structurally malformed input (missing required field, wrong type,
    non-integer ``created_at``, a payload that fails JCS canonicalization
    such as containing a float) is treated as a verification failure rather
    than raising, matching the fail-closed spirit of the Rust function
    (whose static typing makes such documents unrepresentable in the first
    place — they would fail to deserialize before ``verify_artifact`` is
    ever called).

    Returns:
        ``(True, None)`` if the envelope verifies; otherwise
        ``(False, reason)``.
    """
    if not isinstance(doc, dict):
        return False, f"artifact must be a JSON object, got {type(doc).__name__}"

    for field in _REQUIRED_STRING_FIELDS:
        value = doc.get(field)
        if not isinstance(value, str):
            return False, f"missing or non-string required field: {field}"

    created_at = doc.get("created_at")
    if isinstance(created_at, bool) or not isinstance(created_at, int):
        return False, "missing or non-integer required field: created_at"

    if "payload" not in doc:
        return False, "missing required field: payload"

    if doc["schema_version"] != FROGLET_SCHEMA_V1:
        return (
            False,
            f"schema_version must be {FROGLET_SCHEMA_V1!r}, got {doc['schema_version']!r}",
        )

    try:
        recomputed_payload_hash = payload_hash(doc["payload"])
    except (TypeError, ValueError) as error:
        return False, f"payload_hash computation failed: {error}"

    if recomputed_payload_hash != doc["payload_hash"]:
        return False, "payload_hash does not match recomputed hash of payload"

    try:
        signing_bytes = canonical_signing_bytes(
            doc["schema_version"],
            doc["artifact_type"],
            doc["signer"],
            created_at,
            doc["payload_hash"],
            doc["payload"],
        )
    except (TypeError, ValueError) as error:
        return False, f"canonical signing bytes computation failed: {error}"

    computed_hash = sha256_hex(signing_bytes)
    # `hash` defaults to "" when absent, matching the Rust struct's
    # `#[serde(default, skip_serializing_if = "String::is_empty")]`.
    stored_hash = doc.get("hash", "")
    if not isinstance(stored_hash, str) or stored_hash != computed_hash:
        return False, "hash does not match recomputed hash of signing bytes"

    if not bip340_verify(doc["signer"], doc["signature"], signing_bytes):
        return False, "BIP-340 signature verification failed"

    return True, None
