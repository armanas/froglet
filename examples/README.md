# Examples

These examples are small bot-facing integrations built on [froglet_client.py](../froglet_client.py).

## 1. Mock-Lightning Buy and Accept

[runtime_mock_lightning_buy_accept.py](runtime_mock_lightning_buy_accept.py) exercises the authenticated runtime happy path:

- buy a priced `execute.wasm` service
- inspect the returned payment intent
- advance mock-Lightning settlement
- wait for `result_ready`
- release the success preimage
- verify the terminal receipt
- export the retained runtime archive

Start Froglet first:

```bash
FROGLET_PRICE_EXEC_WASM=10 \
FROGLET_PAYMENT_BACKEND=lightning \
FROGLET_LIGHTNING_MODE=mock \
cargo run --bin froglet
```

Then run:

```bash
python3 examples/runtime_mock_lightning_buy_accept.py
```

## 2. Curated Discovery Surface

[runtime_curated_discovery.py](runtime_curated_discovery.py) exercises the authenticated discovery/publication helpers:

- fetch the runtime provider snapshot
- publish the current provider surface
- issue a curated list entry
- verify the curated list
- build local Nostr publication intents
- verify the descriptor summary event

Run it against any local Froglet node with a runtime auth token:

```bash
python3 examples/runtime_curated_discovery.py
```
