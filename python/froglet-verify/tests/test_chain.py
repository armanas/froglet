"""Pairwise and full-chain validation tests.

Covers: the canonical paid chain (with and without an invoice_bundle) and
the canonical free chain both validating green; deterministic
envelope-before-link issue ordering; partial-chain hash-mismatch detection
without needing the full path; expiry reporting when ``now`` is supplied;
and that tampering any linking field flips the relevant pairwise check to
invalid with the expected ``ISSUE_*`` code. These mirror the test cases in
``froglet-protocol/src/protocol/chain.rs``'s own ``#[cfg(test)] mod tests``
(``validates_canonical_paid_chain_partials``,
``validates_canonical_free_chain_partials_without_invoice_bundle``,
``reports_deterministic_envelope_issues_before_link_issues``,
``reports_partial_chain_hash_mismatch_without_requiring_full_path``,
``reports_invoice_bundle_link_failures_with_stable_codes``,
``reports_expiry_when_now_is_provided``).
"""

from __future__ import annotations

import copy
import unittest
from typing import Any

from froglet_verify.chain import (
    ISSUE_ARTIFACT_ENVELOPE_INVALID,
    ISSUE_ARTIFACT_EXPIRED,
    ISSUE_ARTIFACT_TYPE_MISMATCH,
    ISSUE_DESCRIPTOR_HASH_MISMATCH,
    ISSUE_INVOICE_AMOUNT_MISMATCH,
    ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING,
    ISSUE_INVOICE_HASH_MISMATCH,
    ISSUE_INVOICE_PAYMENT_HASH_MISMATCH,
    ISSUE_OFFER_HASH_MISMATCH,
    ISSUE_PROVIDER_MISMATCH,
    ISSUE_QUOTE_HASH_MISMATCH,
    ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH,
    ISSUE_SETTLEMENT_METHOD_MISMATCH,
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

from _helpers import artifact, load_fixture, x402_artifact


def _issue_codes(report: dict[str, Any]) -> list[str]:
    return [issue["code"] for issue in report["issues"]]


class CanonicalPaidChainTests(unittest.TestCase):
    """Mirrors chain.rs's `validates_canonical_paid_chain_partials`: every
    pairwise validator over the canonical paid chain reports valid."""

    def setUp(self) -> None:
        self.descriptor = artifact("descriptor")
        self.offer = artifact("offer")
        self.quote = artifact("quote")
        self.deal = artifact("deal")
        self.invoice_bundle = artifact("invoice_bundle")
        self.receipt = artifact("receipt")

    def test_descriptor_offer(self) -> None:
        report = validate_descriptor_offer(self.descriptor, self.offer)
        self.assertTrue(report["valid"], report["issues"])
        self.assertEqual(report["path"], PATH_DESCRIPTOR_OFFER)

    def test_offer_quote(self) -> None:
        report = validate_offer_quote(self.offer, self.quote)
        self.assertTrue(report["valid"], report["issues"])
        self.assertEqual(report["path"], PATH_OFFER_QUOTE)

    def test_quote_deal(self) -> None:
        report = validate_quote_deal(self.quote, self.deal)
        self.assertTrue(report["valid"], report["issues"])
        self.assertEqual(report["path"], PATH_QUOTE_DEAL)

    def test_quote_invoice_bundle_deal(self) -> None:
        report = validate_quote_invoice_bundle_deal(
            self.quote, self.invoice_bundle, self.deal
        )
        self.assertTrue(report["valid"], report["issues"])
        self.assertEqual(report["path"], PATH_QUOTE_INVOICE_BUNDLE_DEAL)

    def test_quote_deal_receipt(self) -> None:
        report = validate_quote_deal_receipt(self.quote, self.deal, self.receipt)
        self.assertTrue(report["valid"], report["issues"])
        self.assertEqual(report["path"], PATH_QUOTE_DEAL_RECEIPT)

    def test_full_chain_with_invoice_bundle(self) -> None:
        result = validate_full_chain(
            self.descriptor,
            self.offer,
            self.quote,
            self.deal,
            invoice_bundle=self.invoice_bundle,
            receipt=self.receipt,
        )
        self.assertTrue(result["valid"], result["reports"])
        paths = [r["path"] for r in result["reports"]]
        self.assertEqual(
            paths,
            [
                PATH_DESCRIPTOR_OFFER,
                PATH_OFFER_QUOTE,
                PATH_QUOTE_INVOICE_BUNDLE_DEAL,
                PATH_QUOTE_DEAL_RECEIPT,
            ],
        )

    def test_full_chain_without_invoice_bundle_is_invalid(self) -> None:
        result = validate_full_chain(
            self.descriptor, self.offer, self.quote, self.deal, receipt=self.receipt
        )
        self.assertFalse(result["valid"])
        all_codes = {code for report in result["reports"] for code in _issue_codes(report)}
        self.assertIn(ISSUE_INVOICE_BUNDLE_REQUIRED_FOR_LIGHTNING, all_codes)
        paths = [r["path"] for r in result["reports"]]
        self.assertEqual(
            paths,
            [
                PATH_DESCRIPTOR_OFFER,
                PATH_OFFER_QUOTE,
                PATH_QUOTE_DEAL,
                PATH_QUOTE_DEAL_RECEIPT,
            ],
        )

    def test_full_chain_binds_receipt_to_invoice_bundle(self) -> None:
        mutations = [
            (
                ("bundle_hash",),
                "aa" * 32,
                ISSUE_RECEIPT_BUNDLE_HASH_MISMATCH,
            ),
            (
                ("base_fee", "amount_msat"),
                self.receipt["payload"]["settlement_refs"]["base_fee"]["amount_msat"]
                + 1,
                ISSUE_INVOICE_AMOUNT_MISMATCH,
            ),
            (
                ("base_fee", "invoice_hash"),
                "bb" * 32,
                ISSUE_INVOICE_HASH_MISMATCH,
            ),
            (
                ("success_fee", "payment_hash"),
                "cc" * 32,
                ISSUE_INVOICE_PAYMENT_HASH_MISMATCH,
            ),
        ]
        for path, value, expected_code in mutations:
            with self.subTest(path=path):
                receipt = copy.deepcopy(self.receipt)
                target = receipt["payload"]["settlement_refs"]
                for key in path[:-1]:
                    target = target[key]
                target[path[-1]] = value
                result = validate_full_chain(
                    self.descriptor,
                    self.offer,
                    self.quote,
                    self.deal,
                    invoice_bundle=self.invoice_bundle,
                    receipt=receipt,
                )
                self.assertFalse(result["valid"])
                all_codes = {
                    code for report in result["reports"] for code in _issue_codes(report)
                }
                self.assertIn(expected_code, all_codes)

    def test_full_chain_without_receipt(self) -> None:
        result = validate_full_chain(
            self.descriptor,
            self.offer,
            self.quote,
            self.deal,
            invoice_bundle=self.invoice_bundle,
        )
        self.assertTrue(result["valid"], result["reports"])
        paths = [r["path"] for r in result["reports"]]
        self.assertEqual(
            paths,
            [PATH_DESCRIPTOR_OFFER, PATH_OFFER_QUOTE, PATH_QUOTE_INVOICE_BUNDLE_DEAL],
        )


class CanonicalFreeChainTests(unittest.TestCase):
    """Mirrors chain.rs's
    `validates_canonical_free_chain_partials_without_invoice_bundle`."""

    def setUp(self) -> None:
        self.descriptor = artifact("descriptor")
        self.offer = artifact("free_offer")
        self.quote = artifact("free_quote")
        self.deal = artifact("free_deal")
        self.receipt = artifact("free_receipt")

    def test_descriptor_offer(self) -> None:
        report = validate_descriptor_offer(self.descriptor, self.offer)
        self.assertTrue(report["valid"], report["issues"])

    def test_offer_quote(self) -> None:
        report = validate_offer_quote(self.offer, self.quote)
        self.assertTrue(report["valid"], report["issues"])

    def test_quote_deal(self) -> None:
        report = validate_quote_deal(self.quote, self.deal)
        self.assertTrue(report["valid"], report["issues"])

    def test_quote_deal_receipt(self) -> None:
        report = validate_quote_deal_receipt(self.quote, self.deal, self.receipt)
        self.assertTrue(report["valid"], report["issues"])

    def test_full_chain(self) -> None:
        result = validate_full_chain(
            self.descriptor, self.offer, self.quote, self.deal, receipt=self.receipt
        )
        self.assertTrue(result["valid"], result["reports"])
        paths = [r["path"] for r in result["reports"]]
        self.assertEqual(
            paths,
            [
                PATH_DESCRIPTOR_OFFER,
                PATH_OFFER_QUOTE,
                PATH_QUOTE_DEAL,
                PATH_QUOTE_DEAL_RECEIPT,
            ],
        )


class X402ChainTests(unittest.TestCase):
    """The x402.eip3009.v1 conformance path from conformance/x402_v1.json:
    single-leg, prepaid, no invoice_bundle transport artifact at all.
    Mirrors ``tests/x402_conformance_vectors.rs``'s
    ``x402_chain_validates_end_to_end_without_an_invoice_bundle``."""

    def setUp(self) -> None:
        self.descriptor = x402_artifact("descriptor")
        self.offer = x402_artifact("offer")
        self.quote = x402_artifact("quote")
        self.deal = x402_artifact("deal")
        self.receipt = x402_artifact("receipt")

    def test_descriptor_offer(self) -> None:
        report = validate_descriptor_offer(self.descriptor, self.offer)
        self.assertTrue(report["valid"], report["issues"])

    def test_offer_quote(self) -> None:
        report = validate_offer_quote(self.offer, self.quote)
        self.assertTrue(report["valid"], report["issues"])

    def test_quote_deal(self) -> None:
        report = validate_quote_deal(self.quote, self.deal)
        self.assertTrue(report["valid"], report["issues"])

    def test_quote_deal_receipt(self) -> None:
        report = validate_quote_deal_receipt(self.quote, self.deal, self.receipt)
        self.assertTrue(report["valid"], report["issues"])

    def test_full_chain_without_invoice_bundle(self) -> None:
        result = validate_full_chain(
            self.descriptor,
            self.offer,
            self.quote,
            self.deal,
            invoice_bundle=None,
            receipt=self.receipt,
        )
        self.assertTrue(result["valid"], result["reports"])
        paths = [r["path"] for r in result["reports"]]
        self.assertEqual(
            paths,
            [
                PATH_DESCRIPTOR_OFFER,
                PATH_OFFER_QUOTE,
                PATH_QUOTE_DEAL,
                PATH_QUOTE_DEAL_RECEIPT,
            ],
        )

    def test_tampered_x402_receipt_breaks_the_chain(self) -> None:
        receipt = copy.deepcopy(self.receipt)
        receipt["payload"]["settlement_refs"]["bundle_hash"] = None
        result = validate_full_chain(
            self.descriptor,
            self.offer,
            self.quote,
            self.deal,
            receipt=receipt,
        )
        self.assertFalse(result["valid"])
        all_codes = {code for r in result["reports"] for code in _issue_codes(r)}
        self.assertIn("artifact_semantic_invalid", all_codes)


class DeterministicIssueOrderingTests(unittest.TestCase):
    """Mirrors chain.rs's
    `reports_deterministic_envelope_issues_before_link_issues`: envelope
    issues (type mismatch, envelope invalid) must appear before link
    issues, in a fixed order."""

    def test_envelope_issues_precede_link_issues(self) -> None:
        offer = artifact("offer")
        quote = artifact("quote")
        quote["artifact_type"] = "deal"  # breaks type match AND envelope hash
        quote["payload"] = dict(quote["payload"])
        quote["payload"]["offer_hash"] = "aa" * 32  # also breaks the offer_hash link

        report = validate_offer_quote(offer, quote)
        self.assertFalse(report["valid"])
        codes = _issue_codes(report)
        self.assertEqual(
            codes[:3],
            [
                ISSUE_ARTIFACT_TYPE_MISMATCH,
                ISSUE_ARTIFACT_ENVELOPE_INVALID,
                ISSUE_OFFER_HASH_MISMATCH,
            ],
        )


class PartialChainHashMismatchTests(unittest.TestCase):
    """Mirrors chain.rs's
    `reports_partial_chain_hash_mismatch_without_requiring_full_path`: a
    pairwise check between artifacts that don't belong to the same chain
    still runs (and reports the hash mismatch) without needing the whole
    chain assembled."""

    def test_quote_against_unrelated_deal_reports_quote_hash_mismatch(self) -> None:
        quote = artifact("quote")
        free_deal = artifact("free_deal")  # belongs to a completely different quote

        report = validate_quote_deal(quote, free_deal)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_QUOTE_HASH_MISMATCH, _issue_codes(report))


class InvoiceBundleLinkFailureTests(unittest.TestCase):
    """Mirrors chain.rs's
    `reports_invoice_bundle_link_failures_with_stable_codes`. Despite the
    Rust test's name, it actually calls ``validate_quote_deal_receipt`` with
    the *free_receipt* fixture standing in for the receipt argument against
    the paid quote/deal -- exercising `_validate_receipt_links`'s
    settlement-method cross-check, not the invoice_bundle link checks."""

    def test_free_receipt_against_paid_quote_reports_settlement_method_mismatch(
        self,
    ) -> None:
        quote = artifact("quote")
        deal = artifact("deal")
        free_receipt = artifact(
            "free_receipt"
        )  # settlement method "none", quote expects lightning

        report = validate_quote_deal_receipt(quote, deal, free_receipt)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_SETTLEMENT_METHOD_MISMATCH, _issue_codes(report))


class ExpiryReportingTests(unittest.TestCase):
    """Mirrors chain.rs's `reports_expiry_when_now_is_provided`."""

    def test_artifact_expired_when_now_exceeds_quote_expiry(self) -> None:
        offer = artifact("offer")
        quote = artifact("quote")

        report = validate_offer_quote(
            offer, quote, now=quote["payload"]["expires_at"] + 1
        )
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_ARTIFACT_EXPIRED, _issue_codes(report))

    def test_no_expiry_issue_when_now_is_before_expiry(self) -> None:
        offer = artifact("offer")
        quote = artifact("quote")
        report = validate_offer_quote(
            offer, quote, now=quote["payload"]["expires_at"] - 1
        )
        self.assertTrue(report["valid"], report["issues"])

    def test_no_expiry_check_when_now_is_none(self) -> None:
        offer = artifact("offer")
        quote = artifact("quote")
        report = validate_offer_quote(offer, quote, now=None)
        self.assertTrue(report["valid"], report["issues"])


class LinkTamperingFlipsValidationTests(unittest.TestCase):
    """Tampering any single cross-artifact link field must flip the
    relevant pairwise check to invalid, with the corresponding stable
    ISSUE_* code."""

    def setUp(self) -> None:
        self.descriptor = artifact("descriptor")
        self.offer = artifact("offer")
        self.quote = artifact("quote")
        self.deal = artifact("deal")
        self.invoice_bundle = artifact("invoice_bundle")
        self.receipt = artifact("receipt")

    def test_offer_provider_id_mismatch_against_descriptor(self) -> None:
        offer = copy.deepcopy(self.offer)
        offer["payload"]["provider_id"] = "11" * 32
        report = validate_descriptor_offer(self.descriptor, offer)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_PROVIDER_MISMATCH, _issue_codes(report))

    def test_offer_descriptor_hash_mismatch(self) -> None:
        offer = copy.deepcopy(self.offer)
        offer["payload"]["descriptor_hash"] = "aa" * 32
        report = validate_descriptor_offer(self.descriptor, offer)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_DESCRIPTOR_HASH_MISMATCH, _issue_codes(report))

    def test_deal_provider_mismatch_against_quote(self) -> None:
        deal = copy.deepcopy(self.deal)
        deal["payload"]["provider_id"] = "11" * 32
        report = validate_quote_deal(self.quote, deal)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_PROVIDER_MISMATCH, _issue_codes(report))

    def test_deal_quote_hash_mismatch(self) -> None:
        deal = copy.deepcopy(self.deal)
        deal["payload"]["quote_hash"] = "aa" * 32
        report = validate_quote_deal(self.quote, deal)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_QUOTE_HASH_MISMATCH, _issue_codes(report))

    def test_receipt_deal_hash_mismatch(self) -> None:
        receipt = copy.deepcopy(self.receipt)
        receipt["payload"]["deal_hash"] = "aa" * 32
        report = validate_quote_deal_receipt(self.quote, self.deal, receipt)
        self.assertFalse(report["valid"])
        self.assertIn("deal_hash_mismatch", _issue_codes(report))

    def test_invoice_bundle_quote_hash_mismatch(self) -> None:
        bundle = copy.deepcopy(self.invoice_bundle)
        bundle["payload"]["quote_hash"] = "aa" * 32
        report = validate_quote_invoice_bundle_deal(self.quote, bundle, self.deal)
        self.assertFalse(report["valid"])
        self.assertIn(ISSUE_QUOTE_HASH_MISMATCH, _issue_codes(report))

    def test_full_chain_becomes_invalid_when_any_link_is_tampered(self) -> None:
        deal = copy.deepcopy(self.deal)
        deal["payload"]["workload_hash"] = "ff" * 32
        result = validate_full_chain(
            self.descriptor,
            self.offer,
            self.quote,
            deal,
            invoice_bundle=self.invoice_bundle,
            receipt=self.receipt,
        )
        self.assertFalse(result["valid"])
        all_codes = {code for r in result["reports"] for code in _issue_codes(r)}
        self.assertIn("workload_hash_mismatch", all_codes)


class ConfidentialFixtureShapeSanityTests(unittest.TestCase):
    """Sanity: the fixture actually round-trips through the full,
    documented conformance path, matching kernel_v1.json's own
    `conformance_path`/`free_service_conformance_path` metadata."""

    def test_conformance_path_metadata_matches_expected_order(self) -> None:
        fixture = load_fixture()
        self.assertEqual(
            fixture["conformance_path"]["artifact_order"],
            ["descriptor", "offer", "quote", "deal", "invoice_bundle", "receipt"],
        )
        self.assertEqual(
            fixture["free_service_conformance_path"]["artifact_order"],
            ["descriptor", "free_offer", "free_quote", "free_deal", "free_receipt"],
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
