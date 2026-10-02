#!/usr/bin/env python3
"""Check explicit research declarations before computation; no scientific inference.

This higher-layer annotation is transported in the existing signed output_schema.
The helper does not verify signatures: use native inspect_service first when
checking a remote publication. Exact metadata agreement is not semantic truth.
"""
from __future__ import annotations

import argparse
import json
import math
from pathlib import Path

ANNOTATION = "x-froglet-research-profile"
VERSION = "froglet.research-profile/v1"


def _object(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= set(value) or set(value) - set(required) - set(optional):
        raise ValueError("Unexpected or missing research declaration fields")


def _text(value):
    if not isinstance(value, str) or not value.strip() or len(value.encode()) > 1024 or any(
            ord(character) < 32 or 127 <= ord(character) <= 159 for character in value):
        raise ValueError("Research declaration text must contain 1–1024 non-control bytes")


def validate_profile(profile):
    _object(profile, ("schema_version", "collections", "provenance"), ("mapping_assumptions",))
    if profile["schema_version"] != VERSION:
        raise ValueError("Unsupported research profile version")
    if len(json.dumps(profile, ensure_ascii=False, separators=(",", ":")).encode()) > 32768:
        raise ValueError("Research profile exceeds 32 KiB")
    collections = profile["collections"]
    if not isinstance(collections, dict) or not 1 <= len(collections) <= 32:
        raise ValueError("Research profile needs 1–32 collections")
    for name, collection in collections.items():
        _text(name)
        _object(collection, ("fields",))
        if not isinstance(collection["fields"], dict) or not 1 <= len(collection["fields"]) <= 256:
            raise ValueError("Research collection needs 1–256 fields")
        for name, field in collection["fields"].items():
            _text(name)
            _object(field, ("type", "nullable", "unit"), ("identifier",))
            if field["type"] not in ("string", "integer", "number", "boolean") or type(field["nullable"]) is not bool:
                raise ValueError("Unsupported research field type or nullability")
            _text(field["unit"])
            if "identifier" in field:
                if field["type"] != "string":
                    raise ValueError("Identifiers require string fields")
                _object(field["identifier"], ("namespace", "version", "prefix"))
                for value in field["identifier"].values():
                    _text(value)
    _object(profile["provenance"], ("source", "version", "citation", "license"))
    for value in profile["provenance"].values():
        _text(value)
    if "mapping_assumptions" in profile:
        mapping = profile["mapping_assumptions"]
        _object(mapping, ("collection", "source_field", "target_field", "comparison", "cardinality", "duplicate_policy"))
        for value in mapping.values():
            _text(value)
        if mapping["source_field"] == mapping["target_field"]:
            raise ValueError("Mapping source and target must differ")
        fields = collections.get(mapping["collection"], {}).get("fields", {})
        for name in (mapping["source_field"], mapping["target_field"]):
            if fields.get(name, {}).get("type") != "string" or "identifier" not in fields.get(name, {}):
                raise ValueError("Mapping fields require declared string identifiers")
    return profile


def validate_rows(profile, collection, rows):
    """Validate the actual retrieved rows, without conversions or normalization."""
    validate_profile(profile)
    if collection not in profile["collections"] or not isinstance(rows, list):
        raise ValueError("Unknown research collection or invalid rows")
    fields = profile["collections"][collection]["fields"]
    for index, row in enumerate(rows):
        if not isinstance(row, dict) or set(row) != set(fields):
            raise ValueError(f"Row {index}: fields differ from selected research profile")
        for name, value in row.items():
            field = fields[name]
            if value is None and field["nullable"]:
                continue
            if field["type"] == "string":
                valid = isinstance(value, str)
            elif field["type"] == "integer":
                valid = type(value) is int
            elif field["type"] == "number":
                valid = type(value) is int or (type(value) is float and math.isfinite(value))
            else:
                valid = type(value) is bool
            if not valid:
                raise ValueError(f"Row {index}, field {name}: declared {field['type']} required")
            if "identifier" in field and not value.startswith(field["identifier"]["prefix"]):
                raise ValueError(f"Row {index}, field {name}: identifier prefix differs")
    return rows


def preflight(profile, requirements):
    """Compare a consumer's explicit requirements with publisher declarations.

    Requirements are a nonempty subset of the profile's object keys. Lists,
    scalar values, units and version strings compare exactly; no compatibility
    ranges, unit conversion, synonym resolution or scientific judgment occurs.
    """
    validate_profile(profile)
    if not isinstance(requirements, dict) or not requirements or requirements.get("schema_version") != VERSION:
        raise ValueError("Explicit nonempty requirements with profile schema_version are required")
    if set(requirements) - {"schema_version", "collections", "provenance", "mapping_assumptions"}:
        raise ValueError("Unknown research requirement")
    # A requirement cannot be only a version assertion and still imply useful
    # compatibility. The requester must state a field or provenance/policy need.
    if not any(requirements.get(key) for key in ("collections", "provenance", "mapping_assumptions")):
        raise ValueError("Specify research fields, provenance or mapping assumptions to check")
    def criteria(value, allowed):
        if not isinstance(value, dict) or not value or set(value) - set(allowed):
            raise ValueError("Research requirements need nonempty objects with recognized keys")

    if "collections" in requirements:
        collections = requirements["collections"]
        criteria(collections, collections.keys() if isinstance(collections, dict) else ())
        for name, collection in collections.items():
            _text(name)
            _object(collection, ("fields",))
            fields = collection["fields"]
            criteria(fields, fields.keys() if isinstance(fields, dict) else ())
            for name, field in fields.items():
                _text(name)
                criteria(field, ("type", "nullable", "unit", "identifier"))
                if "type" in field and field["type"] not in ("string", "integer", "number", "boolean"):
                    raise ValueError("Unsupported required research field type")
                if "nullable" in field and type(field["nullable"]) is not bool:
                    raise ValueError("Required field nullability must be boolean")
                if "unit" in field:
                    _text(field["unit"])
                if "identifier" in field:
                    criteria(field["identifier"], ("namespace", "version", "prefix"))
                    for value in field["identifier"].values():
                        _text(value)
    for key, allowed in (
        ("provenance", ("source", "version", "citation", "license")),
        ("mapping_assumptions", ("collection", "source_field", "target_field", "comparison", "cardinality", "duplicate_policy")),
    ):
        if key in requirements:
            criteria(requirements[key], allowed)
            for value in requirements[key].values():
                _text(value)
    mismatches = []
    missing = object()

    def compare(advertised, expected, path):
        if advertised is missing:
            mismatches.append({"path": path, "expected": expected, "advertised": None,
                "reason": "Required declaration is absent"})
        elif isinstance(expected, dict):
            if not expected:
                raise ValueError("Empty requirement objects are not meaningful")
            if not isinstance(advertised, dict):
                mismatches.append({"path": path, "expected": expected, "advertised": advertised,
                    "reason": "Required declaration is absent or has a different shape"})
            else:
                for key, value in expected.items():
                    compare(advertised.get(key, missing), value, path + "." + key)
        elif type(advertised) is not type(expected) or advertised != expected:
            mismatches.append({"path": path, "expected": expected, "advertised": advertised,
                "reason": "Exact declaration differs; choose an explicit conversion, update expectations or decline"})

    compare(profile, requirements, "research_profile")
    return {"compatible": not mismatches, "status": "compatible" if not mismatches else "incompatible",
            "mismatches": mismatches, "scientific_truth_verified": False,
            "scope": "Exact declared metadata agreement only; no signature verification, scientific validity, "
                     "unit conversion or semantic equivalence is established by this helper."}


def demo_profile():
    fields = {}
    for name, kind in (("source", "sample"), ("target", "specimen")):
        fields[name] = {"type": "string", "nullable": False, "unit": "none", "identifier": {
            "namespace": "urn:froglet:demo:" + kind, "version": "1", "prefix": "DEMO:" + kind + "."}}
    return {"schema_version": VERSION, "collections": {"terminology": {"fields": fields}},
            "provenance": {"source": "urn:froglet:synthetic:terminology", "version": "1",
                "citation": "Synthetic Froglet demonstration labels; not a scientific ontology.", "license": "Apache-2.0"},
            "mapping_assumptions": {"collection": "terminology", "source_field": "source", "target_field": "target",
                "comparison": "exact_case_sensitive", "cardinality": "one_target_per_source", "duplicate_policy": "ignore_identical"}}


def demo_requirements():
    # Alice's independently stated policy must not automatically change when
    # Bob edits his profile. Citation/license remain displayed declarations.
    return {"schema_version": VERSION, "collections": {"terminology": {"fields": {
        "source": {"type": "string", "nullable": False, "unit": "none", "identifier": {
            "namespace": "urn:froglet:demo:sample", "version": "1", "prefix": "DEMO:sample."}},
        "target": {"type": "string", "nullable": False, "unit": "none", "identifier": {
            "namespace": "urn:froglet:demo:specimen", "version": "1", "prefix": "DEMO:specimen."}}}}},
        "provenance": {"source": "urn:froglet:synthetic:terminology", "version": "1"},
        "mapping_assumptions": {"collection": "terminology", "source_field": "source", "target_field": "target",
            "comparison": "exact_case_sensitive", "cardinality": "one_target_per_source", "duplicate_policy": "ignore_identical"}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, help="Explicit publisher profile JSON")
    parser.add_argument("--inspection", type=Path, help="Native inspect_service structuredContent JSON; signatures must have been checked by that producer")
    parser.add_argument("--requirements", type=Path, required=True, help="Alice's explicit metadata requirements JSON")
    args = parser.parse_args()
    if bool(args.profile) == bool(args.inspection):
        parser.error("Use exactly one of --profile or --inspection")
    try:
        if args.inspection:
            inspection = json.loads(args.inspection.read_text())
            # This is a producer status check, not independent signature verification.
            if not isinstance(inspection, dict):
                raise ValueError("Inspection must be a native inspect_service result object")
            verification = inspection.get("verification")
            if not isinstance(verification, dict) or verification.get("status") != "verified":
                raise ValueError("Use native inspect_service to verify the publication first")
            schema = inspection.get("output_schema")
            if not isinstance(schema, dict):
                raise ValueError("Inspection must contain a research-annotated output_schema object")
            profile = schema.get(ANNOTATION)
        else:
            profile = json.loads(args.profile.read_text())
        result = preflight(profile, json.loads(args.requirements.read_text()))
    except (ValueError, OSError) as error:
        print(json.dumps({"status": "invalid", "compatible": False, "error": str(error)}))
        return 2
    print(json.dumps(result, indent=2))
    return 0 if result["compatible"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
