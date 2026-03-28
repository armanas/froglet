# Python

This directory is now reserved for Python-backed runtime tests and transitional
repo-local helpers that still support the core Froglet node.

Install the required Python dependencies first:

```bash
python3 -m pip install -r python/requirements.txt
```

Contents:

- `requirements.txt`: dependencies for Python-backed core tests
- `tests/`: Python-backed runtime, protocol, conformance, and security tests

Run the Python test suite from the repo root:

```bash
python3 -W error -m unittest \
  python.tests.test_protocol \
  python.tests.test_runtime \
  python.tests.test_discovery \
  python.tests.test_jobs \
  python.tests.test_payments \
  python.tests.test_sandbox \
  python.tests.test_security \
  python.tests.test_privacy \
  python.tests.test_hardening \
  python.tests.test_conformance_vectors -v
```
