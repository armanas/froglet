"""BIP-340 Schnorr verification tests.

Embeds the official BIP-340 test vectors verbatim, as extracted
byte-for-byte from the vendored ``k256-0.13.4`` crate source
(``k256-0.13.4/src/schnorr.rs``, ``BIP340_SIGN_VECTORS`` /
``BIP340_VERIFY_VECTORS`` / ``bip340_ext_sign_vectors``) -- the exact Rust
dependency ``crypto::verify_message`` calls through in this codebase's
Rust kernel, and the canonical source at
https://github.com/bitcoin/bips/blob/master/bip-0340/test-vectors.csv.

Indices 0-14 are the original 32-byte-message vectors (0-3 are sign vectors,
which also double as "must verify" cases; 4-14 are verify-only vectors, one
passing and ten covering distinct failure modes: point not on curve,
non-even-y R, negated message, negated s, R at infinity two ways, r not a
valid field element two ways, s at the curve order, and a pubkey exceeding
the field size). Indices 15-18 are k256's own extension vectors for
arbitrary-length (0/1/17/100-byte) messages verified via the *raw*
(unhashed) primitive, exercising :func:`froglet_verify.schnorr.verify_raw`
directly with variable-length input the same way the official vectors do
with fixed 32-byte input.
"""

from __future__ import annotations

import unittest

from froglet_verify.schnorr import bip340_verify, verify_raw

from _helpers import artifact

# (pubkey_hex, message_hex, sig_hex, expected_valid) -- verbatim from
# k256-0.13.4/src/schnorr.rs. `message_hex` here is the *pre-hashed* 32-byte
# value the official vectors operate on directly (fed to `verify_raw`, no
# extra SHA-256).
OFFICIAL_VECTORS: list[tuple[str, str, str, bool]] = [
    # --- indices 0-3: sign vectors, also valid verify cases ---
    (
        "F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "E907831F80848D1069A5371B402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0",
        True,
    ),
    (
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "6896BD60EEAE296DB48A229FF71DFE071BDE413E6D43F917DC8DCF8C78DE33418906D11AC976ABCCB20B091292BFF4EA897EFCB639EA871CFA95F6DE339E4B0A",
        True,
    ),
    (
        "DD308AFEC5777E13121FA72B9CC1B7CC0139715309B086C960E18FD969774EB8",
        "7E2D58D8B3BCDF1ABADEC7829054F90DDA9805AAB56C77333024B9D0A508B75C",
        "5831AAEED7B44BB74E5EAB94BA9D4294C49BCF2A60728D8B4C200F50DD313C1BAB745879A5AD954A72C45A91C3A51D3C7ADEA98D82F8481E0E1E03674A6F3FB7",
        True,
    ),
    (
        # "test fails if msg is reduced modulo p or n" per the upstream comment.
        "25D1DFF95105F5253C4022F628A996AD3A0D95FBF21D468A1B33F8C160D8F517",
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
        "7EB0509757E246F19449885651611CB965ECC1A187DD51B64FDA1EDC9637D5EC97582B9CB13DB3933705B32BA982AF5AF25FD78881EBB32771FC5922EFC66EA3",
        True,
    ),
    # --- indices 4-14: verify-only vectors ---
    (
        "D69C3509BB99E412E68B0FE8544E72837DFA30746D8BE2AA65975F29D22DC7B9",
        "4DF3C3F68FCC83B27E9D42C90431A72499F17875C81A599B566C9889B9696703",
        "00000000000000000000003B78CE563F89A0ED9414F5AA28AD0D96D6795F9C6376AFB1548AF603B3EB45C9F8207DEE1060CB71C04E80F593060B07D28308D7F4",
        True,
    ),
    (
        # public key not on curve
        "EEFDEA4CDB677750A420FEE807EACF21EB9898AE79B9768766E4FAA04A2D4A34",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        False,
    ),
    (
        # has_even_y(R) is false
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "FFF97BD5755EEEA420453A14355235D382F6472F8568A18B2F057A14602975563CC27944640AC607CD107AE10923D9EF7A73C643E166BE5EBEAFA34B1AC553E2",
        False,
    ),
    (
        # negated message
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "1FA62E331EDBC21C394792D2AB1100A7B432B013DF3F6FF4F99FCB33E0E1515F28890B3EDB6E7189B630448B515CE4F8622A954CFE545735AAEA5134FCCDB2BD",
        False,
    ),
    (
        # negated s value
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769961764B3AA9B2FFCB6EF947B6887A226E8D7C93E00C5ED0C1834FF0D0C2E6DA6",
        False,
    ),
    (
        # sG - eP is infinite (has_even_y(inf) defined as true, x(inf) as 0 case)
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "0000000000000000000000000000000000000000000000000000000000000000123DDA8328AF9C23A94C1FEECFD123BA4FB73476F0D594DCB65C6425BD186051",
        False,
    ),
    (
        # sG - eP is infinite (has_even_y(inf) defined as true, x(inf) as 1 case)
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "00000000000000000000000000000000000000000000000000000000000000017615FBAF5AE28864013C099742DEADB4DBA87F11AC6754F93780D5A1837CF197",
        False,
    ),
    (
        # sig[0:32] is not an X coordinate on the curve
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "4A298DACAE57395A15D0795DDBFD1DCB564DA82B0F269BC70A74F8220429BA1D69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        False,
    ),
    (
        # sig[0:32] is equal to field size
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F69E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        False,
    ),
    (
        # sig[32:64] is equal to curve order
        "DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E177769FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141",
        False,
    ),
    (
        # public key is not a valid X coordinate because it exceeds the field size
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC30",
        "243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89",
        "6CFF5C3BA86C69EA4B7376F31A9BCB4F74C1976089B2D9963DA2E5543E17776969E89B4C5564D00349106B8497785DD7D1D713A8AE82B32FA79D5F7FC407D39B",
        False,
    ),
]

# Extension vectors 15-18 (k256's `bip340_ext_sign_vectors`): arbitrary-length
# messages signed with secret key
# 0340034003400340034003400340034003400340034003400340034003400340. The
# corresponding public key's x-coordinate was independently derived via the
# `ecdsa` package (already a dependency of python/tests/test_support.py, used
# only to *derive this one constant offline*, not imported by
# froglet_verify itself) as secret_key * G, cross-checked against the
# `verify_raw` implementation under test passing all four vectors below.
_EXT_PUBKEY = "778caa53b4393ac467774d09497a87224bf9fab6f6e68b23086497324d6fd117"
EXT_VECTORS: list[tuple[bytes, str]] = [
    (
        b"",
        "71535DB165ECD9FBBC046E5FFAEA61186BB6AD436732FCCC25291A55895464CF6069CE26BF03466228F19A3A62DB8A649F2D560FAC652827D1AF0574E427AB63",
    ),
    (
        bytes.fromhex("11"),
        "08A20A0AFEF64124649232E0693C583AB1B9934AE63B4C3511F3AE1134C6A303EA3173BFEA6683BD101FA5AA5DBC1996FE7CACFC5A577D33EC14564CEC2BACBF",
    ),
    (
        bytes.fromhex("0102030405060708090A0B0C0D0E0F1011"),
        "5130F39A4059B43BC7CAC09A19ECE52B5D8699D1A71E3C52DA9AFDB6B50AC370C4A482B77BF960F8681540E25B6771ECE1E5A37FD80E5A51897C5566A97EA5A5",
    ),
    (
        bytes([0x99] * 100),
        "403B12B0D8555A344175EA7EC746566303321E5DBFA8BE6F091635163ECA79A8585ED3E3170807E7C03B720FC54C7B23897FCBA0E9D0B4A06894CFD249F22367",
    ),
]


class OfficialBip340VectorTests(unittest.TestCase):
    def test_official_vectors_via_verify_raw(self) -> None:
        for index, (pubkey_hex, message_hex, sig_hex, expected) in enumerate(
            OFFICIAL_VECTORS
        ):
            with self.subTest(index=index):
                pubkey_bytes = bytes.fromhex(pubkey_hex)
                message_bytes = bytes.fromhex(message_hex)
                sig_bytes = bytes.fromhex(sig_hex)
                self.assertEqual(
                    verify_raw(pubkey_bytes, sig_bytes, message_bytes), expected
                )

    def test_extension_vectors_variable_length_messages(self) -> None:
        pubkey_bytes = bytes.fromhex(_EXT_PUBKEY)
        for index, (message, sig_hex) in enumerate(EXT_VECTORS, start=15):
            with self.subTest(index=index, message_len=len(message)):
                self.assertTrue(
                    verify_raw(pubkey_bytes, bytes.fromhex(sig_hex), message)
                )


class Bip340VerifyMalformedInputTests(unittest.TestCase):
    """`bip340_verify` must return False (never raise) on any malformation,
    mirroring `crypto::verify_message`."""

    def _sample_inputs(self) -> tuple[str, bytes, str]:
        # A syntactically well-formed (pubkey_hex, message, sig_hex) triple,
        # not asserted to verify -- these tests only care that malforming
        # the pubkey/signature *encoding* returns False rather than raising.
        pubkey_hex, _message_hex, sig_hex, _expected = OFFICIAL_VECTORS[1]
        return pubkey_hex, b"arbitrary froglet message", sig_hex

    def test_short_pubkey_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex[:-2], sig_hex, message))

    def test_long_pubkey_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex + "00", sig_hex, message))

    def test_short_signature_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex, sig_hex[:-2], message))

    def test_long_signature_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex, sig_hex + "00", message))

    def test_non_hex_pubkey_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify("zz" * 32, sig_hex, message))

    def test_non_hex_signature_returns_false(self) -> None:
        pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex, "yy" * 64, message))

    def test_non_string_pubkey_returns_false(self) -> None:
        _pubkey_hex, message, sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(None, sig_hex, message))  # type: ignore[arg-type]

    def test_non_string_signature_returns_false(self) -> None:
        pubkey_hex, message, _sig_hex = self._sample_inputs()
        self.assertFalse(bip340_verify(pubkey_hex, 12345, message))  # type: ignore[arg-type]


class Bip340VerifyRoundTripTests(unittest.TestCase):
    """`bip340_verify` SHA-256-prehashes its message before running the core
    algorithm (mirroring k256's `Verifier::verify`), so it is *not*
    interchangeable with `verify_raw`. Demonstrated two ways: (1) a real
    Froglet-signed artifact's signature verifies via `bip340_verify` over
    its raw (unhashed) signing bytes, and equivalently via `verify_raw` over
    the SHA-256 digest of those same signing bytes; (2) calling
    `bip340_verify` with an official BIP-340 vector's already-a-digest
    "message" fails, because that message is not itself the SHA-256 of
    anything meaningful to re-hash.
    """

    def test_bip340_verify_equals_verify_raw_over_sha256_digest(self) -> None:
        import hashlib

        from froglet_verify.envelope import canonical_signing_bytes

        quote = artifact("quote")
        signing_bytes = canonical_signing_bytes(
            quote["schema_version"],
            quote["artifact_type"],
            quote["signer"],
            quote["created_at"],
            quote["payload_hash"],
            quote["payload"],
        )
        pubkey_bytes = bytes.fromhex(quote["signer"])
        sig_bytes = bytes.fromhex(quote["signature"])
        digest = hashlib.sha256(signing_bytes).digest()

        self.assertTrue(
            bip340_verify(quote["signer"], quote["signature"], signing_bytes)
        )
        self.assertTrue(verify_raw(pubkey_bytes, sig_bytes, digest))
        # And, showing the prehash is load-bearing: verify_raw over the raw
        # (unhashed) signing bytes must NOT verify, since the real signature
        # was produced over the SHA-256 digest, not the raw bytes.
        self.assertFalse(verify_raw(pubkey_bytes, sig_bytes, signing_bytes))

    def test_bip340_verify_rejects_an_official_vectors_prehashed_message(self) -> None:
        # OFFICIAL_VECTORS' "message" is already the exact 32 bytes used in
        # the challenge hash (per verify_raw); it is essentially never also
        # equal to SHA-256(itself), so running it through bip340_verify's
        # extra pre-hash step must fail even though verify_raw on the same
        # inputs succeeds.
        pubkey_hex, message_hex, sig_hex, expected = OFFICIAL_VECTORS[0]
        assert expected is True
        message_bytes = bytes.fromhex(message_hex)
        self.assertTrue(
            verify_raw(bytes.fromhex(pubkey_hex), bytes.fromhex(sig_hex), message_bytes)
        )
        self.assertFalse(bip340_verify(pubkey_hex, sig_hex, message_bytes))


if __name__ == "__main__":
    unittest.main(verbosity=2)
