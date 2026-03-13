Kernel conformance vectors for the frozen `froglet/v1` economic core.

Files:
- `kernel_v1.json`: fixed seeds, fixed timestamps, exact signing bytes, artifact hashes, a canonical `Descriptor -> Offer -> Quote -> Deal -> InvoiceBundle -> Receipt` path, and negative verification cases.

These vectors are exercised by:
- [tests/kernel_conformance_vectors.rs](/Users/armanas/Projects/github.com/armanas/froglet/tests/kernel_conformance_vectors.rs)
- [test_conformance_vectors.py](/Users/armanas/Projects/github.com/armanas/froglet/test_conformance_vectors.py)

The fixture is intentionally checked in as data rather than regenerated at test time so that review can focus on the irreversible wire values themselves.
