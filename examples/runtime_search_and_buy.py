#!/usr/bin/env python3

import argparse
import asyncio
import hashlib
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
PYTHON_ROOT = REPO_ROOT / "python"
if str(PYTHON_ROOT) not in sys.path:
    sys.path.insert(0, str(PYTHON_ROOT))

from froglet_client import RuntimeClient  # noqa: E402

VALID_WASM_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432"
)


def build_request(provider_id: str) -> dict[str, object]:
    module_bytes = bytes.fromhex(VALID_WASM_HEX)
    return {
        "provider": {"provider_id": provider_id},
        "offer_id": "execute.wasm",
        "kind": "wasm",
        "submission": {
            "schema_version": "froglet/v1",
            "submission_type": "wasm_submission",
            "workload": {
                "schema_version": "froglet/v1",
                "workload_kind": "compute.wasm.v1",
                "abi_version": "froglet.wasm.run_json.v1",
                "module_format": "application/wasm",
                "module_hash": hashlib.sha256(module_bytes).hexdigest(),
                "input_format": "application/json+jcs",
                "input_hash": hashlib.sha256(b"null").hexdigest(),
                "requested_capabilities": [],
            },
            "module_bytes_hex": VALID_WASM_HEX,
            "input": None,
        },
        "idempotency_key": "example-runtime-buy",
    }


async def main() -> None:
    parser = argparse.ArgumentParser(description="Create a deal through the local Froglet runtime")
    parser.add_argument("--runtime-url", default="http://127.0.0.1:8081")
    parser.add_argument("--token-path", default="./data/runtime/auth.token")
    args = parser.parse_args()

    runtime = RuntimeClient.from_token_file(args.runtime_url, args.token_path)
    async with runtime:
        providers = await runtime.search(limit=5)
        if not providers:
            raise RuntimeError("no providers returned by runtime search")
        provider_id = providers[0]["descriptor"]["node_id"]
        provider = await runtime.get_provider(provider_id)
        handle = await runtime.buy_service(build_request(provider_id), include_payment_intent=True)

    print(
        json.dumps(
            {
                "provider_id": provider_id,
                "offer_ids": [offer["payload"]["offer_id"] for offer in provider["offers"]],
                "deal_id": handle.deal["deal_id"],
                "deal_status": handle.deal["status"],
                "payment_intent_path": handle.payment_intent_path,
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    asyncio.run(main())
