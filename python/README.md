# Python

Python-facing artifacts for Froglet live here.

Contents:

- [froglet_client.py](froglet_client.py): async client helpers for provider,
  runtime, and marketplace flows
- [froglet_nostr_adapter.py](froglet_nostr_adapter.py): external Nostr relay
  bridge and publication/query helper
- `tests/`: Python integration and helper tests

Run the Python test suite from the repo root:

```bash
python3 -m unittest discover -s python/tests -t . -v
```
