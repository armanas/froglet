"""BIP-340 Schnorr signature verification over secp256k1 (verify-only).

Pure-integer elliptic-curve arithmetic with no third-party dependencies —
this deliberately re-derives the handful of primitives (``lift_x``, point
addition/doubling, scalar multiplication, the tagged hash) needed to verify a
signature; it does not implement signing, key generation, or any
constant-time hardening, since an offline conformance verifier only ever
needs to check signatures it did not produce itself.

Two layers, mirroring the Rust side's dependency (the ``k256`` crate's
``k256::schnorr`` module, `<https://docs.rs/k256/latest/k256/schnorr/>`_,
which Froglet's ``crypto::verify_message`` in ``froglet-protocol/src/crypto.rs``
calls through):

- :func:`verify_raw` is the low-level BIP-340 primitive from the spec: it
  takes the exact 32-byte message used in the challenge hash, with no
  additional hashing. This is what the official BIP-340 test vectors exercise
  directly (their "message" column *is* that 32-byte value) and what k256
  calls ``VerifyingKey::verify_raw`` / the ``PrehashVerifier`` trait.
- :func:`bip340_verify` is the public, arbitrary-length-message entry point
  that mirrors ``crypto::verify_message``: it hex-decodes and length-checks
  the public key and signature (returning ``False`` rather than raising on
  any malformation, exactly like the Rust function), then — critically —
  SHA-256-hashes the message before handing it to :func:`verify_raw`. That
  pre-hash step is not part of the BIP-340 spec itself; it is what k256's
  ``Verifier<Signature>::verify`` does internally
  (``self.verify_digest(Sha256::new_with_prefix(msg), signature)`` in
  ``k256-0.13.4/src/schnorr/verifying.rs``), and it is what
  ``crypto::verify_message`` invokes for Froglet's arbitrary-length JCS
  signing bytes. Skipping it would make every real Froglet signature fail to
  verify.
"""

from __future__ import annotations

import hashlib
from typing import Optional

__all__ = ["bip340_verify", "verify_raw"]

# secp256k1 domain parameters.
_FIELD_PRIME = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F
_GROUP_ORDER = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
_GENERATOR = (
    0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798,
    0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8,
)

# A curve point is an (x, y) tuple of ints mod _FIELD_PRIME, or None for the
# point at infinity.
_Point = Optional[tuple[int, int]]


def _mod_inverse(value: int, modulus: int) -> int:
    # modulus (_FIELD_PRIME) is prime, so Fermat's little theorem applies.
    return pow(value, modulus - 2, modulus)


def _point_add(p1: _Point, p2: _Point) -> _Point:
    if p1 is None:
        return p2
    if p2 is None:
        return p1
    x1, y1 = p1
    x2, y2 = p2
    if x1 == x2:
        if (y1 + y2) % _FIELD_PRIME == 0:
            return None  # P + (-P) = point at infinity
        # Point doubling.
        lam = (3 * x1 * x1) * _mod_inverse((2 * y1) % _FIELD_PRIME, _FIELD_PRIME)
    else:
        lam = (y2 - y1) * _mod_inverse((x2 - x1) % _FIELD_PRIME, _FIELD_PRIME)
    lam %= _FIELD_PRIME
    x3 = (lam * lam - x1 - x2) % _FIELD_PRIME
    y3 = (lam * (x1 - x3) - y1) % _FIELD_PRIME
    return (x3, y3)


def _scalar_mult(scalar: int, point: _Point) -> _Point:
    """Double-and-add scalar multiplication. ``scalar`` must be >= 0."""
    result: _Point = None
    addend = point
    while scalar:
        if scalar & 1:
            result = _point_add(result, addend)
        addend = _point_add(addend, addend)
        scalar >>= 1
    return result


def _lift_x(x: int) -> _Point:
    """BIP-340 ``lift_x``: the point on the curve with x-coordinate ``x`` and
    an even y-coordinate, or ``None`` if ``x`` is out of range or not a valid
    x-coordinate (i.e. ``x**3 + 7`` is not a quadratic residue mod p)."""
    if x >= _FIELD_PRIME:
        return None
    y_squared = (pow(x, 3, _FIELD_PRIME) + 7) % _FIELD_PRIME
    y = pow(y_squared, (_FIELD_PRIME + 1) // 4, _FIELD_PRIME)
    if pow(y, 2, _FIELD_PRIME) != y_squared:
        return None
    if y % 2 != 0:
        y = _FIELD_PRIME - y
    return (x, y)


def _tagged_hash(tag: str, data: bytes) -> bytes:
    tag_hash = hashlib.sha256(tag.encode("ascii")).digest()
    return hashlib.sha256(tag_hash + tag_hash + data).digest()


def verify_raw(pubkey_x: bytes, signature: bytes, message: bytes) -> bool:
    """The core BIP-340 ``Verify`` algorithm, operating on ``message`` as-is
    (no pre-hashing). ``pubkey_x`` must be exactly 32 bytes (the x-only
    public key) and ``signature`` exactly 64 bytes (``r || s``); returns
    ``False`` for any structural or cryptographic failure rather than
    raising, matching the fail-closed contract of the Rust reference.

    This is what the official BIP-340 test vectors call directly — their
    32-byte "message" column is used here verbatim. Froglet's own artifacts
    go through :func:`bip340_verify` instead, which SHA-256-hashes an
    arbitrary-length message down to 32 bytes first.
    """
    if len(pubkey_x) != 32 or len(signature) != 64:
        return False

    point = _lift_x(int.from_bytes(pubkey_x, "big"))
    if point is None:
        return False

    r = int.from_bytes(signature[:32], "big")
    s = int.from_bytes(signature[32:], "big")
    # BIP-340: fail if r >= p or s >= n. k256's `Signature` parsing additionally
    # rejects r == 0 / s == 0 (via FieldElement/NonZeroScalar), which we mirror
    # for exact parity with the Rust dependency actually in use.
    if r >= _FIELD_PRIME or r == 0:
        return False
    if s >= _GROUP_ORDER or s == 0:
        return False

    challenge = (
        int.from_bytes(
            _tagged_hash("BIP0340/challenge", signature[:32] + pubkey_x + message),
            "big",
        )
        % _GROUP_ORDER
    )

    # R = s*G - e*P, computed as s*G + (n - e)*P.
    r_point = _point_add(
        _scalar_mult(s, _GENERATOR),
        _scalar_mult((_GROUP_ORDER - challenge) % _GROUP_ORDER, point),
    )
    if r_point is None:
        return False
    rx, ry = r_point
    if ry % 2 != 0:
        return False
    return rx == r


def bip340_verify(pubkey_x_hex: str, sig_hex: str, message: bytes) -> bool:
    """Verify a BIP-340 Schnorr signature over an arbitrary-length message.

    Mirrors ``crypto::verify_message`` in ``froglet-protocol/src/crypto.rs``
    exactly, including its length/hex validation (64-hex-char pubkey,
    128-hex-char signature; anything else returns ``False`` rather than
    raising) and its use of ``k256``'s ``Verifier<Signature>::verify``, which
    SHA-256-hashes ``message`` before running the core BIP-340 algorithm.

    Args:
        pubkey_x_hex: 64 lowercase-or-uppercase hex characters (32-byte
            x-only secp256k1 public key).
        sig_hex: 128 hex characters (64-byte ``r || s`` signature).
        message: the arbitrary-length signing bytes (for Froglet artifacts,
            the JCS-canonical signing-bytes array).

    Returns:
        ``True`` only if the signature verifies; ``False`` for any
        malformation or cryptographic failure.
    """
    if not isinstance(pubkey_x_hex, str) or len(pubkey_x_hex) != 64:
        return False
    if not isinstance(sig_hex, str) or len(sig_hex) != 128:
        return False
    try:
        pubkey_bytes = bytes.fromhex(pubkey_x_hex)
        sig_bytes = bytes.fromhex(sig_hex)
    except ValueError:
        return False

    digest = hashlib.sha256(message).digest()
    return verify_raw(pubkey_bytes, sig_bytes, digest)
