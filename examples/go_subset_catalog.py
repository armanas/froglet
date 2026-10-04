#!/usr/bin/env python3
"""Turn a hash-pinned public GO subset into a local selected-data preparation.

No network call, publication, upstream credential or patient data is involved.
The exact downloaded release bytes are checked before parsing. This is a term
catalog and identifier-format example, not a gene annotation or ontology mapper.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import re

if __package__:
    from .research_profile import VERSION, preflight, validate_profile, validate_rows
else:
    from research_profile import VERSION, preflight, validate_profile, validate_rows

SOURCE_URL = "https://release.geneontology.org/2026-06-19/ontology/subsets/goslim_generic.tsv"
SOURCE_SHA256 = "a415d86095d2c11105cf855a4605ec180c5e219c113856766843e51a4e576b99"
RELEASE = "2026-06-19"
ATTRIBUTION = (
    "Gene Ontology Consortium, generic GO subset, 2026-06-19 release. "
    "Copyright Gene Ontology Consortium. CC BY 4.0, supplied AS-IS without warranty. "
    "https://geneontology.org/docs/go-citation-policy/ ; "
    "https://creativecommons.org/licenses/by/4.0/ ; " + SOURCE_URL + " . "
    "Adaptation: selected id, name and term_uri fields; GO IDs derived from term URI suffixes. "
    "References: https://doi.org/10.1038/75556 ; https://doi.org/10.1093/nar/gkaf1292 ."
)


def catalog_profile():
    identifier = {"namespace": "http://purl.obolibrary.org/obo/go", "version": RELEASE}
    fields = {
        "id": {"type": "string", "nullable": False, "unit": "none", "identifier": dict(identifier, prefix="GO:")},
        "name": {"type": "string", "nullable": False, "unit": "none"},
        "term_uri": {"type": "string", "nullable": False, "unit": "none", "identifier": dict(identifier, prefix="http://purl.obolibrary.org/obo/GO_")},
    }
    return {"schema_version": VERSION, "collections": {"go_terms": {"fields": fields}},
            "provenance": {"source": SOURCE_URL, "version": RELEASE, "citation": ATTRIBUTION, "license": "CC-BY-4.0"},
            "mapping_assumptions": {"collection": "go_terms", "source_field": "id", "target_field": "term_uri",
                "comparison": "exact_case_sensitive", "cardinality": "one_target_per_source", "duplicate_policy": "ignore_identical"}}


def parse_catalog(content):
    if hashlib.sha256(content).hexdigest() != SOURCE_SHA256:
        raise ValueError("GO release digest differs; review and pin a new release explicitly")
    table = list(csv.reader(io.StringIO(content.decode("utf-8")), delimiter="\t", quoting=csv.QUOTE_NONE))
    if not table or table[0] != ["?x", "?label"]:
        raise ValueError("Unexpected GO TSV header")
    rows = []
    for index, row in enumerate(table[1:], start=2):
        if len(row) != 2:
            raise ValueError(f"Unexpected GO TSV columns on line {index}")
        match = re.fullmatch(r"<(http://purl\.obolibrary\.org/obo/GO_(\d{7}))>", row[0])
        if not match:
            raise ValueError(f"Unexpected GO term URI on line {index}")
        name = json.loads(row[1])
        if not isinstance(name, str) or not name:
            raise ValueError(f"Unexpected GO term name on line {index}")
        rows.append({"id": "GO:" + match[2], "name": name, "term_uri": match[1]})
    if len(rows) != 140 or len({row["id"] for row in rows}) != 140:
        raise ValueError("Pinned GO subset must contain exactly 140 distinct term IDs")
    validate_rows(catalog_profile(), "go_terms", rows)
    return {"go_terms": rows}


def create_preparation(content, destination):
    """Write an inspectable local plan; the operator separately calls prepare_service."""
    if not destination.is_absolute():
        raise ValueError("Destination must be an explicit absolute new directory")
    data = parse_catalog(content)
    profile = catalog_profile()
    validate_profile(profile)
    requirements = {"schema_version": VERSION, "collections": profile["collections"],
                    "provenance": {"source": SOURCE_URL, "version": RELEASE},
                    "mapping_assumptions": profile["mapping_assumptions"]}
    destination.mkdir(mode=0o700)
    source = destination / "go-terms.json"
    request = {"source": str(source), "destination": str(destination / "service"),
               "service_id": "go-generic-slim-2026-06-19",
               "summary": "GO Consortium generic subset 2026-06-19: 140 term IDs, names and term URIs. CC BY 4.0. Identifier representation lookup only; no gene annotation, enrichment or scientific mapping inference.",
               "selection": {"go_terms": ["id", "name", "term_uri"]},
               "example_input": {"op": "select", "collection": "go_terms", "columns": ["id", "name", "term_uri"], "limit": 100},
               "research_profile": profile}
    evidence = {"source_url": SOURCE_URL, "release": RELEASE, "source_sha256": SOURCE_SHA256,
                "source_bytes": len(content), "rows": len(data["go_terms"]), "attribution": ATTRIBUTION,
                "preflight": preflight(profile, requirements), "publicly_published": False,
                "scope": "A local adaptation of an existing public GO term catalog. No participant trial, "
                         "scientific mapping evaluation, publication or independent research-demand evidence."}
    for name, value in (("go-terms.json", data), ("research-profile.json", profile),
                        ("alice-requirements.json", requirements), ("prepare-request.json", request),
                        ("source-evidence.json", evidence)):
        path = destination / name
        with path.open("x", encoding="utf-8") as output:
            json.dump(value, output, indent=2)
            output.write("\n")
        path.chmod(0o600)
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-tsv", type=Path, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        with args.source_tsv.open("rb") as source:
            content = source.read(2_000_001)
        if len(content) > 2_000_000:
            raise ValueError("GO TSV exceeds the 2 MB example bound")
        print(json.dumps(create_preparation(content, args.destination), indent=2))
    except (ValueError, OSError) as error:
        parser.exit(2, str(error) + "\n")


if __name__ == "__main__":
    main()
