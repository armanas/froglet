# Explicit research declarations and preflight

The optional `froglet.research-profile/v1` profile makes a publisher's field
types, units, identifier namespaces, identifier versions, provenance and mapping
assumptions inspectable. It is an experimental higher-layer service annotation,
not a new Kernel artifact or an ontology alignment standard.

The implemented authoring path is **native** `froglet-node prepare-service` /
native MCP `prepare_service`. The JavaScript MCP/OpenClaw publishers have not been
extended with a `research_profile` authoring argument. Existing service readers
can read the annotation inside `output_schema` after publication.

## What is checked

Native preparation accepts optional `research_profile` beside `selection` and
`example_input`. It checks every selected row, not only the example: exact
collection and field sets, JSON scalar types, nullability, and declared
identifier prefixes. A profile cannot name an excluded field. Preparation of
CSV or SQLite sources produces a selected JSON snapshot; direct profiled
publication currently requires that JSON snapshot representation. File and
Wasm preparations reject this option.

Profiled snapshots require nonempty collections so their declared fields can
be checked against actual rows. A later query may legitimately return no rows.
Published profiles must use object fields and omit absent optional declarations;
null placeholders and positional struct arrays are rejected at publication.

The generated manifest carries the profile as the
`x-froglet-research-profile` annotation in its existing `output_schema_json`.
The provider checks it again against actual snapshot bytes. All other schema
keywords must exactly match the generated data-query schema. Publication then
includes that schema in the existing signed Publication Revision.

Native `inspect_service` verifies the signed revision/Offer/Descriptor binding
and returns `output_schema`, without executing the service. Consumers can then
use `examples/research_profile.py` to compare their own explicit requirements.
It reports each mismatch with its path, expected value, advertised value and
plain-language reason. Namespaces, versions, types, units and policies compare
exactly. The helper does not perform signature verification itself; its
`--inspection` producer-status check is not an independent cryptographic proof.

The preflight is explicit consumer logic. Other clients can ignore the
annotation, and native `run_compute` does not automatically require a research
profile. The ontology demo invokes preflight before submitting its analyses and
checks actual retrieved row structure before using the rows.

## What is not established

A signature attributes a declaration to a provider. It does not establish that
the publisher used the correct scientific namespace, release or units.
Provenance URLs, version claims, license declarations and citations are not
automatically resolved or adjudicated. The same version string is not proof of
the same ontology contents; use independently pinned source digests when that
matters. The existing snapshot and execution hashes still bind actual bytes.

There is no unit conversion, normalization, synonym lookup, ontology reasoning,
scientific validity check or cross-version compatibility inference. `unit:
"none"` explicitly means no physical unit in that field. A unit mismatch requires
an explicit conversion or a decision to decline. An identical metadata profile
can still contain a scientifically incorrect mapping.

## Try the synthetic profile

`python3 examples/a2a_compute_demo.py --scenario ontology` now publishes the
profile, checks that the actual signed revision retained it, and runs Alice's
explicit preflight before submitting the existing exact-string checks. The
synthetic data and deterministic results remain the same. Namespace, version,
unit, provenance-version and mapping-policy refusal tests run with:

```sh
python3 -W error -m unittest examples.test_research_profile -v
```

For a verified native inspection saved as `inspection.json`, provide the
requester's own nonempty requirements, not requirements inferred from the
publisher's profile:

```sh
python3 examples/research_profile.py --inspection inspection.json --requirements alice-requirements.json
```

Exit 0 means declared compatibility, 1 means an explained mismatch, and 2 means
invalid declarations or requirements. These outcomes do not authorize payment
or assert scientific truth.

## Optional public GO term catalog

The [GO Consortium generic subset](https://geneontology.org/docs/go-subset-guide/)
is a non-sensitive existing public term catalog. The optional example pins its
[2026-06-19 TSV](https://release.geneontology.org/2026-06-19/ontology/subsets/goslim_generic.tsv)
to SHA-256 `a415d86095d2c11105cf855a4605ec180c5e219c113856766843e51a4e576b99`.
The checked file is 9,703 bytes and contains 140 distinct term identifiers.
The parser derives IDs from URI suffixes and selects `id`, `name`, `term_uri`.
It does not traverse GO relations, slim gene annotations or perform enrichment.

Download the attributed public source, then generate an inspectable local plan
in a new absolute directory:

```sh
curl --fail --max-time 20 --max-filesize 2000000 https://release.geneontology.org/2026-06-19/ontology/subsets/goslim_generic.tsv -o /tmp/goslim_generic.tsv
python3 examples/go_subset_catalog.py --source-tsv /tmp/goslim_generic.tsv --destination /tmp/froglet-go-catalog
froglet-node prepare-service --request /tmp/froglet-go-catalog/prepare-request.json --json
python3 examples/research_profile.py --profile /tmp/froglet-go-catalog/research-profile.json --requirements /tmp/froglet-go-catalog/alice-requirements.json
```

The example writes attribution, release URI, input digest, profile and explicit
consumer requirements. Preparation is local; publication requires the existing
separate publication action and public hosting qualification. Use pagination to
retrieve all 140 rows: each select is bounded to 100 rows.

GO Consortium data is licensed under
[CC BY 4.0 and its citation policy](https://geneontology.org/docs/go-citation-policy/).
The generated attribution names the Consortium, release, adaptation, license
and warranty disclaimer, with the source URI and requested reference papers.
This sample supplies reproducible public data. It is not evidence of participant
adoption, independent scientific evaluation or demand for Froglet.
