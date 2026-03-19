# Python

Python-facing artifacts for Froglet live here.

Install the required Python dependencies first:

```bash
python3 -m pip install -r python/requirements.txt
```

Contents:

- [froglet_client.py](froglet_client.py): async client helpers for provider,
  runtime, reference discovery, and confidential session/envelope flows
- [froglet_nostr_adapter.py](froglet_nostr_adapter.py): external Nostr relay
  bridge and publication/query helper
- `tests/`: Python integration and helper tests

Run the Python test suite from the repo root:

```bash
python3 -m unittest discover -s python/tests -t . -v
```

Package-safe CLI entrypoints also work from the repo root, for example:

```bash
python3 -m python.froglet_nostr_adapter --help
```
