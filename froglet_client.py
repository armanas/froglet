import asyncio
import hashlib
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

import aiohttp


class FrogletClientError(RuntimeError):
    def __init__(self, status: int, payload: Any):
        self.status = status
        self.payload = payload
        super().__init__(f"froglet request failed with status {status}: {payload}")


_SECP256K1_ORDER = int(
    "fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141",
    16,
)


def _require_ecdsa() -> Any:
    try:
        from ecdsa import curves
    except ImportError as exc:  # pragma: no cover - import guard
        raise RuntimeError(
            "froglet_client requester helpers require the 'ecdsa' package"
        ) from exc
    return curves


def _seed_bytes(seed: bytes | str) -> bytes:
    if isinstance(seed, bytes):
        if len(seed) != 32:
            raise ValueError("requester seed must be exactly 32 bytes")
        return seed
    normalized = seed.strip().lower()
    if len(normalized) != 64:
        raise ValueError("requester seed hex must be exactly 64 hex characters")
    try:
        seed_bytes = bytes.fromhex(normalized)
    except ValueError as exc:
        raise ValueError("requester seed hex must be valid lowercase hex") from exc
    if len(seed_bytes) != 32:
        raise ValueError("requester seed hex must decode to exactly 32 bytes")
    return seed_bytes


def _preimage_bytes(preimage: bytes | str) -> bytes:
    if isinstance(preimage, bytes):
        if len(preimage) != 32:
            raise ValueError("success preimage must be exactly 32 bytes")
        return preimage
    normalized = preimage.strip().lower()
    if len(normalized) != 64:
        raise ValueError("success preimage hex must be exactly 64 hex characters")
    try:
        preimage_bytes = bytes.fromhex(normalized)
    except ValueError as exc:
        raise ValueError("success preimage hex must be valid lowercase hex") from exc
    if len(preimage_bytes) != 32:
        raise ValueError("success preimage hex must decode to exactly 32 bytes")
    return preimage_bytes


def generate_requester_seed() -> bytes:
    while True:
        candidate = os.urandom(32)
        scalar = int.from_bytes(candidate, "big")
        if 1 <= scalar < _SECP256K1_ORDER:
            return candidate


def requester_id_from_seed(seed: bytes | str) -> str:
    curves = _require_ecdsa()
    seed_bytes = _seed_bytes(seed)
    scalar = int.from_bytes(seed_bytes, "big")
    if not 1 <= scalar < _SECP256K1_ORDER:
        raise ValueError("requester seed is not a valid secp256k1 secret key")
    point = scalar * curves.SECP256k1.generator
    return int(point.x()).to_bytes(32, "big").hex()


def runtime_requester_fields(
    seed: bytes | str,
    success_preimage: bytes | str,
) -> dict[str, str]:
    seed_bytes = _seed_bytes(seed)
    preimage_bytes = _preimage_bytes(success_preimage)
    return {
        "requester_id": requester_id_from_seed(seed_bytes),
        "requester_seed_hex": seed_bytes.hex(),
        "success_payment_hash": hashlib.sha256(preimage_bytes).hexdigest(),
    }


@dataclass
class DealHandle:
    quote: dict[str, Any]
    deal: dict[str, Any]
    terminal: bool
    payment_intent_path: str | None = None
    payment_intent: dict[str, Any] | None = None


class _JsonApiClient:
    def __init__(self, base_url: str) -> None:
        self.base_url = base_url.rstrip("/")
        self._session: aiohttp.ClientSession | None = None

    async def __aenter__(self) -> "_JsonApiClient":
        await self._ensure_session()
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        await self.close()

    async def close(self) -> None:
        if self._session is not None:
            await self._session.close()
            self._session = None

    async def _ensure_session(self) -> aiohttp.ClientSession:
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession()
        return self._session

    def _headers(self) -> dict[str, str]:
        return {}

    async def _request_json(
        self,
        method: str,
        path: str,
        *,
        json_body: dict[str, Any] | None = None,
        expected_statuses: Iterable[int] = (200,),
    ) -> Any:
        session = await self._ensure_session()
        async with session.request(
            method,
            f"{self.base_url}{path}",
            headers=self._headers(),
            json=json_body,
        ) as resp:
            payload = await resp.json()
        if resp.status not in set(expected_statuses):
            raise FrogletClientError(resp.status, payload)
        return payload


class ProviderClient(_JsonApiClient):
    async def descriptor(self) -> dict[str, Any]:
        return await self._request_json("GET", "/v1/descriptor")

    async def offers(self) -> list[dict[str, Any]]:
        response = await self._request_json("GET", "/v1/offers")
        return response["offers"]

    async def create_quote(
        self,
        offer_id: str,
        request: dict[str, Any],
        *,
        requester_id: str,
        max_price_sats: int | None = None,
    ) -> dict[str, Any]:
        payload = {
            "offer_id": offer_id,
            "requester_id": requester_id,
            **request,
        }
        if max_price_sats is not None:
            payload["max_price_sats"] = max_price_sats
        return await self._request_json(
            "POST",
            "/v1/quotes",
            json_body=payload,
            expected_statuses=(201,),
        )

    async def create_deal(
        self,
        quote: dict[str, Any],
        deal: dict[str, Any],
        request: dict[str, Any],
        *,
        idempotency_key: str | None = None,
        payment: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {"quote": quote, "deal": deal, **request}
        if idempotency_key is not None:
            payload["idempotency_key"] = idempotency_key
        if payment is not None:
            payload["payment"] = payment
        return await self._request_json(
            "POST",
            "/v1/deals",
            json_body=payload,
            expected_statuses=(200, 202),
        )

    async def get_deal(self, deal_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/deals/{deal_id}")

    async def get_invoice_bundle(self, deal_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/deals/{deal_id}/invoice-bundle")

    async def verify_invoice_bundle(
        self,
        bundle: dict[str, Any],
        quote: dict[str, Any],
        deal: dict[str, Any],
        *,
        requester_id: str | None = None,
    ) -> dict[str, Any]:
        payload = {"bundle": bundle, "quote": quote, "deal": deal}
        if requester_id is not None:
            payload["requester_id"] = requester_id
        return await self._request_json(
            "POST",
            "/v1/invoice-bundles/verify",
            json_body=payload,
        )

    async def verify_curated_list(self, curated_list: dict[str, Any]) -> dict[str, Any]:
        return await self._request_json(
            "POST",
            "/v1/curated-lists/verify",
            json_body={"curated_list": curated_list},
        )

    async def verify_nostr_event(self, event: dict[str, Any]) -> dict[str, Any]:
        return await self._request_json(
            "POST",
            "/v1/nostr/events/verify",
            json_body={"event": event},
        )

    async def verify_receipt(self, receipt: dict[str, Any]) -> dict[str, Any]:
        return await self._request_json(
            "POST",
            "/v1/receipts/verify",
            json_body={"receipt": receipt},
        )

    async def accept_result(
        self,
        deal_id: str,
        success_preimage: str,
        *,
        expected_result_hash: str | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {"success_preimage": success_preimage}
        if expected_result_hash is not None:
            payload["expected_result_hash"] = expected_result_hash
        return await self._request_json(
            "POST",
            f"/v1/deals/{deal_id}/release-preimage",
            json_body=payload,
        )

    async def wait_for_deal(
        self,
        deal_id: str,
        *,
        statuses: set[str] | frozenset[str] | None = None,
        timeout_secs: float = 15.0,
        poll_interval_secs: float = 0.2,
    ) -> dict[str, Any]:
        accepted_statuses = statuses or {"succeeded", "failed", "rejected"}
        deadline = asyncio.get_running_loop().time() + timeout_secs
        while asyncio.get_running_loop().time() < deadline:
            deal = await self.get_deal(deal_id)
            if deal["status"] in accepted_statuses:
                return deal
            await asyncio.sleep(poll_interval_secs)
        raise TimeoutError(
            f"timed out waiting for deal {deal_id} to reach {sorted(accepted_statuses)}"
        )


class RuntimeClient(_JsonApiClient):
    def __init__(self, base_url: str, token: str) -> None:
        super().__init__(base_url)
        self.token = token
        self.provider = ProviderClient(base_url)

    @classmethod
    def from_token_file(cls, base_url: str, token_path: str | Path) -> "RuntimeClient":
        token = Path(token_path).read_text(encoding="utf-8").strip()
        return cls(base_url, token)

    async def close(self) -> None:
        await self.provider.close()
        await super().close()

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.token}"}

    async def provider_start(self) -> dict[str, Any]:
        return await self._request_json("POST", "/v1/runtime/provider/start")

    async def wallet_balance(self) -> dict[str, Any]:
        return await self._request_json("GET", "/v1/runtime/wallet/balance")

    async def publish_services(self) -> dict[str, Any]:
        return await self._request_json("POST", "/v1/runtime/services/publish")

    async def buy_service(
        self,
        request: dict[str, Any],
        *,
        wait_for_receipt: bool = False,
        wait_timeout_secs: int | None = None,
        include_payment_intent: bool = False,
    ) -> DealHandle:
        payload = dict(request)
        payload["wait_for_receipt"] = wait_for_receipt
        if wait_timeout_secs is not None:
            payload["wait_timeout_secs"] = wait_timeout_secs
        response = await self._request_json(
            "POST",
            "/v1/runtime/services/buy",
            json_body=payload,
        )
        return DealHandle(
            quote=response["quote"],
            deal=response["deal"],
            terminal=response["terminal"],
            payment_intent_path=response.get("payment_intent_path"),
            payment_intent=response.get("payment_intent") if include_payment_intent else None,
        )

    async def issue_curated_list(
        self,
        *,
        expires_at: int,
        entries: list[dict[str, Any]],
        list_id: str | None = None,
    ) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "expires_at": expires_at,
            "entries": entries,
        }
        if list_id is not None:
            payload["list_id"] = list_id
        response = await self._request_json(
            "POST",
            "/v1/runtime/discovery/curated-lists/issue",
            json_body=payload,
            expected_statuses=(201,),
        )
        return response["curated_list"]

    async def nostr_provider_publications(self) -> dict[str, Any]:
        return await self._request_json(
            "GET", "/v1/runtime/nostr/publications/provider"
        )

    async def nostr_receipt_publication(self, deal_id: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/runtime/nostr/publications/deals/{deal_id}/receipt"
        )

    async def payment_intent(self, deal_id: str) -> dict[str, Any]:
        response = await self._request_json(
            "GET", f"/v1/runtime/deals/{deal_id}/payment-intent"
        )
        return response["payment_intent"]

    async def archive_subject(self, subject_kind: str, subject_id: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/runtime/archive/{subject_kind}/{subject_id}"
        )

    async def set_mock_lightning_state(
        self,
        session_id: str,
        *,
        base_state: str,
        success_state: str,
    ) -> dict[str, Any]:
        return await self._request_json(
            "POST",
            f"/v1/runtime/lightning/invoice-bundles/{session_id}/state",
            json_body={
                "base_state": base_state,
                "success_state": success_state,
            },
        )

    async def wait_for_deal(
        self,
        deal_id: str,
        *,
        statuses: set[str] | frozenset[str] | None = None,
        timeout_secs: float = 15.0,
        poll_interval_secs: float = 0.2,
    ) -> dict[str, Any]:
        return await self.provider.wait_for_deal(
            deal_id,
            statuses=statuses,
            timeout_secs=timeout_secs,
            poll_interval_secs=poll_interval_secs,
        )

    async def accept_result(
        self,
        deal_id: str,
        success_preimage: str,
        *,
        expected_result_hash: str | None = None,
    ) -> dict[str, Any]:
        resolved_result_hash = expected_result_hash
        if resolved_result_hash is None:
            intent = await self.payment_intent(deal_id)
            release_action = intent.get("release_action")
            if release_action is not None:
                resolved_result_hash = release_action.get("expected_result_hash")
        return await self.provider.accept_result(
            deal_id,
            success_preimage,
            expected_result_hash=resolved_result_hash,
        )

    async def verify_receipt(self, receipt: dict[str, Any]) -> dict[str, Any]:
        return await self.provider.verify_receipt(receipt)


class MarketplaceClient(_JsonApiClient):
    async def search_nodes(
        self, *, limit: int = 20, active_only: bool = True
    ) -> list[dict[str, Any]]:
        query = f"/v1/marketplace/search?limit={limit}&active_only={'true' if active_only else 'false'}"
        response = await self._request_json("GET", query)
        return response["nodes"]

    async def get_node(self, node_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/marketplace/nodes/{node_id}")
