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

from froglet_client import (  # noqa: E402
    RuntimeClient,
    generate_requester_seed,
    runtime_requester_fields,
)

VALID_WASM_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432"
)


def build_wasm_request(module_hex: str) -> dict[str, object]:
    module_bytes = bytes.fromhex(module_hex)
    return {
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
            "module_bytes_hex": module_hex,
            "input": None,
        },
    }


async def main() -> None:
    parser = argparse.ArgumentParser(
        description="Buy and accept a priced Wasm service through the Froglet runtime"
    )
    parser.add_argument("--runtime-url", default="http://127.0.0.1:8081")
    parser.add_argument(
        "--provider-url",
        default="http://127.0.0.1:8080",
        help="Public provider API base URL used for quote/deal and verification flows.",
    )
    parser.add_argument("--token-path", default="./data/runtime/auth.token")
    parser.add_argument("--offer-id", default="execute.wasm")
    parser.add_argument("--idempotency-key", default="example-runtime-buy-accept")
    args = parser.parse_args()

    requester_seed = generate_requester_seed()
    success_preimage = b"example-success-preimage".rjust(32, b"\0")
    request = build_wasm_request(VALID_WASM_HEX)
    request["offer_id"] = args.offer_id
    request["idempotency_key"] = args.idempotency_key
    request.update(runtime_requester_fields(requester_seed, success_preimage))

    runtime = RuntimeClient.from_token_file(
        args.runtime_url,
        args.token_path,
        provider_base_url=args.provider_url,
    )
    async with runtime:
        snapshot = await runtime.provider_start()
        handle = await runtime.buy_service(request, include_payment_intent=True)
        if handle.terminal:
            raise RuntimeError("expected a non-terminal priced deal handle")
        if handle.payment_intent is None:
            raise RuntimeError("runtime did not return a payment_intent")

        await runtime.set_mock_lightning_state(
            handle.payment_intent["session_id"],
            base_state="settled",
            success_state="accepted",
        )
        result_ready = await runtime.wait_for_deal(
            handle.deal["deal_id"], statuses={"result_ready"}
        )
        terminal = await runtime.accept_result(
            handle.deal["deal_id"],
            success_preimage.hex(),
            expected_result_hash=result_ready["result_hash"],
        )
        verification = await runtime.verify_receipt(terminal["receipt"])
        archive = await runtime.archive_subject("deal", handle.deal["deal_id"])

    print(
        json.dumps(
            {
                "provider_id": snapshot["descriptor"]["payload"]["provider_id"],
                "deal_id": handle.deal["deal_id"],
                "payment_intent_path": handle.payment_intent_path,
                "terminal_status": terminal["status"],
                "receipt_hash": terminal["receipt"]["hash"],
                "receipt_valid": verification["valid"],
                "archive_artifact_count": len(archive["artifact_documents"]),
                "archive_evidence_count": len(archive["execution_evidence"]),
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    asyncio.run(main())
