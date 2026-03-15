#!/usr/bin/env python3

import argparse
import asyncio
import json
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from froglet_client import ProviderClient, RuntimeClient  # noqa: E402


async def main() -> None:
    parser = argparse.ArgumentParser(
        description="Issue a curated list and verify local publication intents"
    )
    parser.add_argument("--runtime-url", default="http://127.0.0.1:8081")
    parser.add_argument(
        "--provider-url",
        default="http://127.0.0.1:8080",
        help="Public provider API base URL used for descriptor and verification flows.",
    )
    parser.add_argument("--token-path", default="./data/runtime/auth.token")
    parser.add_argument("--list-id", default="example-curated-list")
    args = parser.parse_args()

    runtime = RuntimeClient.from_token_file(
        args.runtime_url,
        args.token_path,
        provider_base_url=args.provider_url,
    )
    async with ProviderClient(args.provider_url) as provider:
        async with runtime:
            snapshot = await runtime.provider_start()
            published = await runtime.publish_services()
            descriptor = published["descriptor"]
            curated_list = await runtime.issue_curated_list(
                list_id=args.list_id,
                expires_at=int(time.time()) + 3600,
                entries=[
                    {
                        "provider_id": descriptor["payload"]["provider_id"],
                        "descriptor_hash": descriptor["hash"],
                        "tags": ["bootstrap", "local"],
                        "note": "example curated provider entry",
                    }
                ],
            )
            publications = await runtime.nostr_provider_publications()

        curated_verification = await provider.verify_curated_list(curated_list)
        nostr_verification = await provider.verify_nostr_event(
            publications["descriptor_summary"]
        )

    print(
        json.dumps(
            {
                "provider_id": snapshot["descriptor"]["payload"]["provider_id"],
                "offer_count": len(published["offers"]),
                "curated_list_id": curated_list["payload"]["list_id"],
                "curated_list_valid": curated_verification["valid"],
                "descriptor_summary_kind": publications["descriptor_summary"]["kind"],
                "descriptor_summary_valid": nostr_verification["valid"],
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    asyncio.run(main())
