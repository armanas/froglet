#!/usr/bin/env python3

import asyncio
import hashlib
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[3]
PYTHON_ROOT = REPO_ROOT / "python"
if str(PYTHON_ROOT) not in sys.path:
    sys.path.insert(0, str(PYTHON_ROOT))

from froglet_client import (  # noqa: E402
    ProviderClient,
    RuntimeClient,
    generate_requester_seed,
    runtime_requester_fields,
)

DEFAULT_WAIT_STATUSES = ["result_ready", "succeeded", "failed", "rejected"]


class BridgeError(RuntimeError):
    pass


def require_string(value: Any, field_name: str) -> str:
    if not isinstance(value, str) or len(value.strip()) == 0:
        raise BridgeError(f"{field_name} must be a non-empty string")
    return value.strip()


def optional_string(value: Any) -> str | None:
    if not isinstance(value, str) or len(value.strip()) == 0:
        return None
    return value.strip()


def require_object(value: Any, field_name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise BridgeError(f"{field_name} must be an object")
    return dict(value)


def require_runtime_url(payload: dict[str, Any]) -> str:
    return require_string(payload.get("runtime_url"), "runtime_url")


def require_provider_url(payload: dict[str, Any]) -> str:
    return require_string(payload.get("provider_url"), "provider_url")


def require_runtime_auth_token_path(payload: dict[str, Any]) -> Path:
    return Path(require_string(payload.get("runtime_auth_token_path"), "runtime_auth_token_path"))


def wait_statuses(payload: dict[str, Any]) -> list[str]:
    value = payload.get("wait_statuses")
    if not isinstance(value, list) or len(value) == 0:
        return DEFAULT_WAIT_STATUSES
    normalized = []
    for item in value:
        if isinstance(item, str) and len(item.strip()) > 0:
            normalized.append(item.strip())
    return normalized or DEFAULT_WAIT_STATUSES


def state_dir_for_token(token_path: Path) -> Path:
    return token_path.resolve().parent / "openclaw-froglet"


def state_path_for_deal(token_path: Path, deal_id: str) -> Path:
    return state_dir_for_token(token_path) / f"{deal_id}.json"


def load_state(token_path: Path | None, deal_id: str) -> tuple[dict[str, Any] | None, Path | None]:
    if token_path is None:
        return None, None
    state_path = state_path_for_deal(token_path, deal_id)
    if not state_path.exists():
        return None, state_path
    try:
        return json.loads(state_path.read_text(encoding="utf-8")), state_path
    except json.JSONDecodeError as exc:
        raise BridgeError(f"Failed to parse local OpenClaw Froglet state {state_path}: {exc}") from exc


def persist_state(token_path: Path, deal_id: str, payload: dict[str, Any]) -> Path:
    state_dir = state_dir_for_token(token_path)
    state_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    state_path = state_path_for_deal(token_path, deal_id)

    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        dir=state_dir,
        prefix=f"{deal_id}.",
        suffix=".tmp",
        delete=False,
    ) as handle:
        temp_path = Path(handle.name)
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")

    try:
        os.chmod(temp_path, 0o600)
    except PermissionError:
        pass
    temp_path.replace(state_path)
    try:
        os.chmod(state_path, 0o600)
    except PermissionError:
        pass
    return state_path


def sha256_hex_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


async def handle_buy(payload: dict[str, Any]) -> dict[str, Any]:
    runtime_url = require_runtime_url(payload)
    provider_url = require_provider_url(payload)
    token_path = require_runtime_auth_token_path(payload)
    request = require_object(payload.get("request"), "request")

    managed_preimage_hex = None
    if "quote" not in request or "deal" not in request:
        has_requester_seed = any(
            key in request and optional_string(request.get(key)) is not None
            for key in ("requester_seed_hex", "requester_seed")
        )
        has_success_payment_hash = optional_string(request.get("success_payment_hash")) is not None
        if not has_requester_seed and not has_success_payment_hash:
            requester_seed = generate_requester_seed()
            success_preimage = os.urandom(32)
            request.update(runtime_requester_fields(requester_seed, success_preimage))
            managed_preimage_hex = success_preimage.hex()

    runtime = RuntimeClient.from_token_file(
        runtime_url,
        token_path,
        provider_base_url=provider_url,
    )
    async with runtime:
        handle = await runtime.buy_service(
            request,
            wait_for_receipt=payload.get("wait_for_receipt") is True,
            wait_timeout_secs=payload.get("wait_timeout_secs"),
            include_payment_intent=payload.get("include_payment_intent") is not False,
        )

    stored_state_path = None
    if managed_preimage_hex is not None and handle.terminal is False:
        stored_state_path = persist_state(
            token_path,
            str(handle.deal["deal_id"]),
            {
                "created_at": int(time.time()),
                "deal_id": handle.deal["deal_id"],
                "provider_url": provider_url,
                "runtime_url": runtime_url,
                "success_preimage_hex": managed_preimage_hex,
            },
        )

    return {
        "runtime_url": runtime_url,
        "provider_url": provider_url,
        "runtime_auth_token_path": str(token_path),
        "quote": handle.quote,
        "deal": handle.deal,
        "terminal": handle.terminal,
        "payment_intent_path": handle.payment_intent_path,
        "payment_intent": handle.payment_intent,
        "stored_preimage": managed_preimage_hex is not None,
        "stored_state_path": str(stored_state_path) if stored_state_path is not None else None,
    }


async def handle_wait_deal(payload: dict[str, Any]) -> dict[str, Any]:
    deal_id = require_string(payload.get("deal_id"), "deal_id")
    token_path_value = optional_string(payload.get("runtime_auth_token_path"))
    token_path = Path(token_path_value) if token_path_value is not None else None
    state, _state_path = load_state(token_path, deal_id)
    provider_url = optional_string(payload.get("provider_url")) or (
        state.get("provider_url") if isinstance(state, dict) else None
    )
    if provider_url is None:
        raise BridgeError("provider_url is required when no stored local state exists for this deal")

    async with ProviderClient(provider_url) as provider:
        deal = await provider.wait_for_deal(
            deal_id,
            statuses=set(wait_statuses(payload)),
            timeout_secs=float(payload.get("timeout_secs", 15)),
            poll_interval_secs=float(payload.get("poll_interval_secs", 0.2)),
        )

    return {
        "deal": deal,
        "deal_id": deal_id,
        "provider_url": provider_url,
        "wait_statuses": wait_statuses(payload),
    }


def resolve_runtime_context_for_deal(payload: dict[str, Any]) -> tuple[str, str | None, Path, dict[str, Any] | None, Path | None]:
    deal_id = require_string(payload.get("deal_id"), "deal_id")
    token_path = require_runtime_auth_token_path(payload)
    state, state_path = load_state(token_path, deal_id)
    runtime_url = optional_string(payload.get("runtime_url")) or (
        state.get("runtime_url") if isinstance(state, dict) else None
    )
    if runtime_url is None:
        raise BridgeError("runtime_url is required when no stored local state exists for this deal")
    provider_url = optional_string(payload.get("provider_url")) or (
        state.get("provider_url") if isinstance(state, dict) else None
    )
    return runtime_url, provider_url, token_path, state, state_path


async def handle_payment_intent(payload: dict[str, Any]) -> dict[str, Any]:
    deal_id = require_string(payload.get("deal_id"), "deal_id")
    runtime_url, provider_url, token_path, _state, _state_path = resolve_runtime_context_for_deal(
        payload
    )

    runtime = RuntimeClient.from_token_file(
        runtime_url,
        token_path,
        provider_base_url=provider_url,
    )
    async with runtime:
        payment_intent = await runtime.payment_intent(deal_id)

    return {
        "deal_id": deal_id,
        "runtime_url": runtime_url,
        "provider_url": provider_url,
        "payment_intent": payment_intent,
    }


async def handle_accept_result(payload: dict[str, Any]) -> dict[str, Any]:
    deal_id = require_string(payload.get("deal_id"), "deal_id")
    runtime_url, provider_url, token_path, state, state_path = resolve_runtime_context_for_deal(payload)
    if state is None or state_path is None:
        raise BridgeError(
            "Local OpenClaw Froglet state was not found for this deal. accept_result only works for deals initiated by this helper."
        )

    success_preimage_hex = optional_string(state.get("success_preimage_hex"))
    if success_preimage_hex is None:
        raise BridgeError(
            f"Local OpenClaw Froglet state {state_path} does not contain a success preimage"
        )
    if provider_url is None:
        raise BridgeError(
            f"Local OpenClaw Froglet state {state_path} does not contain a provider_url and no provider_url override was supplied"
        )

    runtime = RuntimeClient.from_token_file(
        runtime_url,
        token_path,
        provider_base_url=provider_url,
    )
    async with runtime:
        terminal = await runtime.accept_result(
            deal_id,
            success_preimage_hex,
            expected_result_hash=optional_string(payload.get("expected_result_hash")),
        )

    return {
        "deal_id": deal_id,
        "runtime_url": runtime_url,
        "provider_url": provider_url,
        "stored_state_path": str(state_path),
        "terminal": terminal,
    }


async def handle_publish_services(payload: dict[str, Any]) -> dict[str, Any]:
    runtime_url = require_runtime_url(payload)
    provider_url = optional_string(payload.get("provider_url"))
    token_path = require_runtime_auth_token_path(payload)

    runtime = RuntimeClient.from_token_file(
        runtime_url,
        token_path,
        provider_base_url=provider_url,
    )
    async with runtime:
        published = await runtime.publish_services()

    return {
        "runtime_url": runtime_url,
        "provider_url": provider_url,
        "descriptor": published["descriptor"],
        "offers": published["offers"],
    }


async def dispatch(payload: dict[str, Any]) -> dict[str, Any]:
    action = require_string(payload.get("action"), "action")
    if action == "buy":
        return await handle_buy(payload)
    if action == "wait_deal":
        return await handle_wait_deal(payload)
    if action == "payment_intent":
        return await handle_payment_intent(payload)
    if action == "accept_result":
        return await handle_accept_result(payload)
    if action == "publish_services":
        return await handle_publish_services(payload)
    raise BridgeError(f"Unsupported action {action}")


def read_payload() -> dict[str, Any]:
    try:
        payload = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        raise BridgeError(f"Bridge input must be valid JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise BridgeError("Bridge input must be a JSON object")
    return payload


def main() -> int:
    try:
        payload = read_payload()
        result = asyncio.run(dispatch(payload))
    except BridgeError as exc:
        print(str(exc), file=sys.stderr)
        return 1
    except Exception as exc:  # pragma: no cover - unexpected bridge failures
        print(f"Unexpected bridge failure: {exc}", file=sys.stderr)
        return 1

    json.dump(result, sys.stdout, sort_keys=True)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
