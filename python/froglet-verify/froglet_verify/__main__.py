"""CLI: ``python -m froglet_verify [--now <unix>] <file|->``.

Accepts a single artifact object, a JSON array of artifacts, or a
``{"artifacts": [...]}`` page (matching the shape used elsewhere in this
package's own fixtures/tests) and:

1. Verifies the envelope (:func:`froglet_verify.envelope.verify_signed_artifact`)
   and, for the six recognized kinds, the per-kind semantics
   (:mod:`froglet_verify.semantics`) of every artifact in the input.
2. If the input contains exactly one each of ``descriptor``/``offer``/
   ``quote``/``deal`` (an ``invoice_bundle`` and/or a ``receipt`` are
   optional, but at most one of each), also runs
   :func:`froglet_verify.chain.validate_full_chain` over them.

Prints a JSON report to stdout. Exit codes: ``0`` if everything checked is
valid, ``1`` if anything is invalid, ``2`` on a usage error (bad arguments,
unreadable input, input that is not an artifact/array/page).
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any

from . import chain, envelope, semantics

__all__ = ["main"]

_SEMANTIC_VALIDATORS = {
    "descriptor": semantics.validate_descriptor_artifact,
    "offer": semantics.validate_offer_artifact,
    "quote": semantics.validate_quote_artifact,
    "deal": semantics.validate_deal_artifact,
    "invoice_bundle": semantics.validate_invoice_bundle_artifact,
    "receipt": semantics.validate_receipt_artifact,
}

_CHAIN_REQUIRED_KINDS = ("descriptor", "offer", "quote", "deal")
_CHAIN_OPTIONAL_SINGLE_KINDS = ("invoice_bundle", "receipt")


class _UsageError(Exception):
    pass


def _load_artifacts(source: str) -> list[Any]:
    if source == "-":
        text = sys.stdin.read()
    else:
        try:
            with open(source, "r", encoding="utf-8") as handle:
                text = handle.read()
        except OSError as error:
            raise _UsageError(f"could not read {source!r}: {error}") from error

    try:
        data = json.loads(text)
    except json.JSONDecodeError as error:
        raise _UsageError(f"input is not valid JSON: {error}") from error

    if isinstance(data, dict):
        maybe_page = data.get("artifacts")
        if isinstance(maybe_page, list):
            return maybe_page
        return [data]
    if isinstance(data, list):
        return data
    raise _UsageError(
        "input JSON must be a single artifact object, an array of artifacts, "
        'or a {"artifacts": [...]} page'
    )


def _verify_one(doc: Any) -> dict[str, Any]:
    if not isinstance(doc, dict):
        return {
            "artifact_type": None,
            "envelope_valid": False,
            "envelope_reason": f"artifact must be a JSON object, got {type(doc).__name__}",
            "semantic_valid": None,
            "semantic_reason": None,
            "valid": False,
        }

    raw_artifact_type = doc.get("artifact_type")
    artifact_type: str | None = (
        raw_artifact_type if isinstance(raw_artifact_type, str) else None
    )

    try:
        envelope_ok, envelope_reason = envelope.verify_signed_artifact(doc)
    except Exception as error:  # fail-closed: never let malformed input crash the CLI
        return {
            "artifact_type": artifact_type,
            "envelope_valid": False,
            "envelope_reason": f"malformed artifact: {error!r}",
            "semantic_valid": None,
            "semantic_reason": None,
            "valid": False,
        }

    semantic_valid: bool | None = None
    semantic_reason: str | None = None
    validator = (
        _SEMANTIC_VALIDATORS.get(artifact_type) if artifact_type is not None else None
    )
    if validator is not None:
        try:
            semantic_reason = validator(doc)
            semantic_valid = semantic_reason is None
        except (KeyError, TypeError, AttributeError) as error:
            semantic_valid = False
            semantic_reason = f"malformed artifact: {error!r}"

    overall_valid = envelope_ok and semantic_valid is not False
    return {
        "artifact_type": artifact_type,
        "envelope_valid": envelope_ok,
        "envelope_reason": envelope_reason,
        "semantic_valid": semantic_valid,
        "semantic_reason": semantic_reason,
        "valid": overall_valid,
    }


def _group_by_kind(artifacts: list[Any]) -> dict[Any, list[Any]]:
    grouped: dict[Any, list[Any]] = {}
    for doc in artifacts:
        kind = doc.get("artifact_type") if isinstance(doc, dict) else None
        grouped.setdefault(kind, []).append(doc)
    return grouped


def _maybe_run_full_chain(
    artifacts: list[Any], now: int | None
) -> dict[str, Any] | None:
    by_kind = _group_by_kind(artifacts)

    if any(len(by_kind.get(kind, [])) != 1 for kind in _CHAIN_REQUIRED_KINDS):
        return None
    if any(len(by_kind.get(kind, [])) > 1 for kind in _CHAIN_OPTIONAL_SINGLE_KINDS):
        return None

    invoice_bundle = (
        by_kind["invoice_bundle"][0] if by_kind.get("invoice_bundle") else None
    )
    receipt = by_kind["receipt"][0] if by_kind.get("receipt") else None

    try:
        return chain.validate_full_chain(
            by_kind["descriptor"][0],
            by_kind["offer"][0],
            by_kind["quote"][0],
            by_kind["deal"][0],
            invoice_bundle=invoice_bundle,
            receipt=receipt,
            now=now,
        )
    except (KeyError, TypeError, AttributeError) as error:
        return {
            "valid": False,
            "reports": [],
            "error": f"chain validation raised {error!r}",
        }


def _build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python -m froglet_verify",
        description="Verify Froglet signed-artifact envelopes, semantics, and chains offline.",
    )
    parser.add_argument(
        "--now",
        type=int,
        default=None,
        metavar="UNIX_TIME",
        help="reference time for expiry checks",
    )
    parser.add_argument(
        "file", help='path to a JSON artifact/array/page, or "-" for stdin'
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = _build_arg_parser()
    args = parser.parse_args(sys.argv[1:] if argv is None else argv)

    try:
        artifacts = _load_artifacts(args.file)
    except _UsageError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    per_artifact = [_verify_one(doc) for doc in artifacts]
    chain_report = _maybe_run_full_chain(artifacts, args.now)

    all_valid = all(entry["valid"] for entry in per_artifact)
    if chain_report is not None:
        all_valid = all_valid and bool(chain_report.get("valid"))

    report = {"artifacts": per_artifact, "chain": chain_report, "valid": all_valid}
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if all_valid else 1


if __name__ == "__main__":
    raise SystemExit(main())
