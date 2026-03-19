#!/usr/bin/env python3

import argparse
import asyncio
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
PYTHON_ROOT = REPO_ROOT / "python"
if str(PYTHON_ROOT) not in sys.path:
    sys.path.insert(0, str(PYTHON_ROOT))

from froglet_client import RuntimeClient  # noqa: E402


async def main() -> None:
    parser = argparse.ArgumentParser(description="Search Froglet providers through the local runtime")
    parser.add_argument("--runtime-url", default="http://127.0.0.1:8081")
    parser.add_argument("--token-path", default="./data/runtime/auth.token")
    args = parser.parse_args()

    runtime = RuntimeClient.from_token_file(args.runtime_url, args.token_path)
    async with runtime:
        providers = await runtime.search(limit=5)
        if not providers:
            raise RuntimeError("no providers returned by runtime search")
        provider_id = providers[0]["descriptor"]["node_id"]
        details = await runtime.get_provider(provider_id)

    print(
        json.dumps(
            {
                "provider_id": provider_id,
                "descriptor_hash": details["descriptor"]["hash"],
                "offer_ids": [offer["payload"]["offer_id"] for offer in details["offers"]],
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    asyncio.run(main())
