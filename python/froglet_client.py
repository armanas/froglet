import asyncio
import hashlib
import json
import os
import time
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
def _require_ecdsa() -> tuple[Any, Any]:
    try:
        from ecdsa import curves, ellipticcurve
    except ImportError as exc:  # pragma: no cover - import guard
        raise RuntimeError(
            "froglet_client requester helpers require the 'ecdsa' package"
        ) from exc
    return curves, ellipticcurve


def _int_from_bytes(value: bytes) -> int:
    return int.from_bytes(value, "big")


def _int_to_bytes(value: int) -> bytes:
    return value.to_bytes(32, "big")


def _xor_bytes(left: bytes, right: bytes) -> bytes:
    return bytes(a ^ b for a, b in zip(left, right, strict=True))


def _tagged_hash(tag: str, data: bytes) -> bytes:
    tag_hash = hashlib.sha256(tag.encode("utf-8")).digest()
    return hashlib.sha256(tag_hash + tag_hash + data).digest()


def _has_even_y(point: Any) -> bool:
    return int(point.y()) % 2 == 0


def _canonical_json_bytes(value: object) -> bytes:
    return json.dumps(
        value,
        separators=(",", ":"),
        sort_keys=True,
        ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")


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
    curves, _ = _require_ecdsa()
    seed_bytes = _seed_bytes(seed)
    scalar = int.from_bytes(seed_bytes, "big")
    if not 1 <= scalar < _SECP256K1_ORDER:
        raise ValueError("requester seed is not a valid secp256k1 secret key")
    point = scalar * curves.SECP256k1.generator
    return int(point.x()).to_bytes(32, "big").hex()


def _schnorr_sign_message(secret_key: bytes | str, message: bytes) -> str:
    curves, _ = _require_ecdsa()
    secret_bytes = _seed_bytes(secret_key)
    secret = _int_from_bytes(secret_bytes)
    if not 1 <= secret < _SECP256K1_ORDER:
        raise ValueError("requester seed is not a valid secp256k1 secret key")

    generator = curves.SECP256k1.generator
    point = secret * generator
    public_key_bytes = _int_to_bytes(int(point.x()))
    message_digest = hashlib.sha256(message).digest()
    secret_scalar = secret if _has_even_y(point) else _SECP256K1_ORDER - secret

    aux = bytes(32)
    nonce_seed = _xor_bytes(
        _int_to_bytes(secret_scalar),
        _tagged_hash("BIP0340/aux", aux),
    )
    nonce = _int_from_bytes(
        _tagged_hash("BIP0340/nonce", nonce_seed + public_key_bytes + message_digest)
    ) % _SECP256K1_ORDER
    if nonce == 0:
        raise ValueError("derived invalid Schnorr nonce")

    nonce_point = nonce * generator
    signing_nonce = nonce if _has_even_y(nonce_point) else _SECP256K1_ORDER - nonce
    nonce_x = _int_to_bytes(int(nonce_point.x()))
    challenge = _int_from_bytes(
        _tagged_hash("BIP0340/challenge", nonce_x + public_key_bytes + message_digest)
    ) % _SECP256K1_ORDER
    signature = nonce_x + _int_to_bytes(
        (signing_nonce + challenge * secret_scalar) % _SECP256K1_ORDER
    )
    return signature.hex()


def _canonical_artifact_signing_bytes(artifact: dict[str, Any]) -> bytes:
    return _canonical_json_bytes(
        [
            artifact["schema_version"],
            artifact["artifact_type"],
            artifact["signer"],
            artifact["created_at"],
            artifact["payload_hash"],
            artifact["payload"],
        ]
    )


def _sign_artifact(
    artifact_type: str,
    payload: dict[str, Any],
    *,
    secret_key: bytes | str,
    created_at: int | None = None,
) -> dict[str, Any]:
    issued_at = created_at if created_at is not None else int(time.time())
    signer = requester_id_from_seed(secret_key)
    artifact = {
        "artifact_type": artifact_type,
        "schema_version": "froglet/v1",
        "signer": signer,
        "created_at": issued_at,
        "payload_hash": hashlib.sha256(_canonical_json_bytes(payload)).hexdigest(),
        "payload": payload,
    }
    signing_bytes = _canonical_artifact_signing_bytes(artifact)
    artifact["hash"] = hashlib.sha256(signing_bytes).hexdigest()
    artifact["signature"] = _schnorr_sign_message(secret_key, signing_bytes)
    return artifact


def sign_deal_artifact_from_quote(
    quote: dict[str, Any],
    requester_seed: bytes | str,
    *,
    success_payment_hash: str,
    created_at: int | None = None,
) -> dict[str, Any]:
    issued_at = created_at if created_at is not None else int(time.time())
    runtime_ms = int(quote["payload"]["execution_limits"]["max_runtime_ms"])
    execution_window_secs = max(1, (runtime_ms + 999) // 1000)
    settlement_terms = quote["payload"]["settlement_terms"]
    total_msat = int(settlement_terms["base_fee_msat"]) + int(
        settlement_terms["success_fee_msat"]
    )
    if (
        settlement_terms["method"] == "lightning.base_fee_plus_success_fee.v1"
        and total_msat > 0
    ):
        quote_expires_at = int(quote["payload"]["expires_at"])
        hold_window_secs = int(settlement_terms["max_success_hold_expiry_secs"])
        admission_window_secs = max(
            int(settlement_terms["max_base_invoice_expiry_secs"]),
            hold_window_secs,
        )
        latest_admission_deadline = quote_expires_at - execution_window_secs - hold_window_secs
        admission_deadline = min(
            latest_admission_deadline,
            issued_at + admission_window_secs,
        )
        if admission_deadline < issued_at:
            raise ValueError(
                "quote no longer has enough time for the Lightning execution and acceptance windows"
            )
        completion_deadline = admission_deadline + execution_window_secs
        acceptance_deadline = completion_deadline + hold_window_secs
    else:
        admission_deadline = int(quote["payload"]["expires_at"])
        completion_deadline = admission_deadline + execution_window_secs
        acceptance_deadline = completion_deadline
    payload = {
        "requester_id": requester_id_from_seed(requester_seed),
        "provider_id": quote["payload"]["provider_id"],
        "quote_hash": quote["hash"],
        "workload_hash": quote["payload"]["workload_hash"],
        "success_payment_hash": success_payment_hash,
        "admission_deadline": admission_deadline,
        "completion_deadline": completion_deadline,
        "acceptance_deadline": acceptance_deadline,
    }
    return _sign_artifact(
        "deal",
        payload,
        secret_key=requester_seed,
        created_at=issued_at,
    )


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
    def __init__(
        self,
        runtime_base_url: str,
        token: str,
        *,
        provider_base_url: str | None = None,
    ) -> None:
        super().__init__(runtime_base_url)
        self.token = token
        self._provider_base_url = (
            provider_base_url.rstrip("/") if provider_base_url is not None else None
        )
        self._provider: ProviderClient | None = None

    @classmethod
    def from_token_file(
        cls,
        runtime_base_url: str,
        token_path: str | Path,
        *,
        provider_base_url: str | None = None,
    ) -> "RuntimeClient":
        token = Path(token_path).read_text(encoding="utf-8").strip()
        return cls(
            runtime_base_url,
            token,
            provider_base_url=provider_base_url,
        )

    async def close(self) -> None:
        if self._provider is not None:
            await self._provider.close()
            self._provider = None
        await super().close()

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.token}"}

    def _provider_client(self) -> ProviderClient:
        if self._provider is None:
            if self._provider_base_url is None:
                raise RuntimeError(
                    "provider_base_url is required for quote/deal and verification helpers"
                )
            self._provider = ProviderClient(self._provider_base_url)
        return self._provider

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
        payload = await self._prepare_buy_payload(request)
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

    async def _prepare_buy_payload(self, request: dict[str, Any]) -> dict[str, Any]:
        payload = dict(request)
        requester_seed = payload.pop("requester_seed_hex", None)
        if requester_seed is None:
            requester_seed = payload.pop("requester_seed", None)
        requester_id = payload.pop("requester_id", None)
        success_payment_hash = payload.pop("success_payment_hash", None)
        if "quote" in payload and "deal" in payload:
            return payload

        offer_id = payload.pop("offer_id", None)
        if offer_id is None:
            raise ValueError("buy_service requires offer_id when quote/deal are not provided")

        if requester_seed is None:
            raise ValueError(
                "buy_service requires requester_seed_hex (local-only) when quote/deal are not provided"
            )

        if success_payment_hash is None:
            raise ValueError(
                "buy_service requires success_payment_hash when quote/deal are not provided"
            )

        derived_requester_id = requester_id_from_seed(requester_seed)
        if requester_id is not None and requester_id != derived_requester_id:
            raise ValueError("requester_id does not match requester_seed_hex")
        requester_id = requester_id or derived_requester_id

        max_price_sats = payload.pop("max_price_sats", None)
        runtime_fields = {}
        for key in ("idempotency_key", "payment"):
            if key in payload:
                runtime_fields[key] = payload.pop(key)

        quote = await self._provider_client().create_quote(
            offer_id,
            payload,
            requester_id=requester_id,
            max_price_sats=max_price_sats,
        )
        deal = sign_deal_artifact_from_quote(
            quote,
            requester_seed,
            success_payment_hash=success_payment_hash,
        )

        return {
            **payload,
            **runtime_fields,
            "quote": quote,
            "deal": deal,
        }

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
        return await self._provider_client().wait_for_deal(
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
        return await self._provider_client().accept_result(
            deal_id,
            success_preimage,
            expected_result_hash=resolved_result_hash,
        )

    async def verify_receipt(self, receipt: dict[str, Any]) -> dict[str, Any]:
        return await self._provider_client().verify_receipt(receipt)


class MarketplaceClient(_JsonApiClient):
    async def search_nodes(
        self, *, limit: int = 20, include_inactive: bool = False
    ) -> list[dict[str, Any]]:
        query = (
            f"/v1/marketplace/search?limit={limit}"
            f"&include_inactive={'true' if include_inactive else 'false'}"
        )
        response = await self._request_json("GET", query)
        return response["nodes"]

    async def get_node(self, node_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/marketplace/nodes/{node_id}")
