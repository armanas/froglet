# Examples

## Local Three-Role Stack

```bash
docker compose up --build
```

## Python Runtime Examples

- `runtime_search_and_inspect.py`
- `runtime_search_and_buy.py`

Both examples assume the local Compose stack:

- runtime: `http://127.0.0.1:8081`
- discovery: `http://127.0.0.1:9090`
- token: `./data/runtime/auth.token`
