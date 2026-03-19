import asyncio
import base64
import hashlib
import json
import os
import random
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


DEFAULT_HTTP_TIMEOUT = aiohttp.ClientTimeout(
    total=30.0,
    connect=5.0,
    sock_connect=5.0,
    sock_read=30.0,
)
_MAX_ERROR_BODY_CHARS = 2048
_WAIT_BACKOFF_CAP_SECS = 2.0
_FROGLET_SCHEMA_V1 = "froglet/v1"
_CONFIDENTIAL_PROFILE_ARTIFACT = "confidential_profile"
_CONFIDENTIAL_SESSION_ARTIFACT = "confidential_session"
_CONFIDENTIAL_SERVICE_KIND = "confidential.service.v1"
_CONFIDENTIAL_ATTESTED_WASM_KIND = "compute.wasm.attested.v1"
_CONFIDENTIAL_ENVELOPE_TYPE = "encrypted_envelope"
_CONFIDENTIAL_ENCRYPTION_ALGORITHM = "secp256k1_ecdh_aes_256_gcm_v1"
_CONFIDENTIAL_EXECUTION_MODE = "tee"

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


def _require_confidential_crypto() -> tuple[Any, Any]:
    try:
        from cryptography.hazmat.primitives.asymmetric import ec
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    except ImportError as exc:  # pragma: no cover - import guard
        raise RuntimeError(
            "confidential Froglet helpers require the 'cryptography' package"
        ) from exc
    return ec, AESGCM


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


def _canonical_json_hash(value: object) -> str:
    return hashlib.sha256(_canonical_json_bytes(value)).hexdigest()


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


def _validate_public_key_hex(public_key_hex: str) -> str:
    ec, _ = _require_confidential_crypto()
    normalized = public_key_hex.strip().lower()
    if len(normalized) not in (66, 130):
        raise ValueError("public key hex must be 33-byte or 65-byte secp256k1 SEC1 encoding")
    try:
        encoded = bytes.fromhex(normalized)
    except ValueError as exc:
        raise ValueError("public key hex must be valid lowercase hex") from exc
    ec.EllipticCurvePublicKey.from_encoded_point(ec.SECP256K1(), encoded)
    return normalized


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


def generate_confidential_keypair() -> dict[str, str]:
    ec, _ = _require_confidential_crypto()
    from cryptography.hazmat.primitives import serialization

    private_key = ec.generate_private_key(ec.SECP256K1())
    private_value = private_key.private_numbers().private_value
    public_bytes = private_key.public_key().public_bytes(
        encoding=serialization.Encoding.X962,
        format=serialization.PublicFormat.CompressedPoint,
    )
    return {
        "private_key_hex": private_value.to_bytes(32, "big").hex(),
        "public_key_hex": public_bytes.hex(),
    }


def _derive_confidential_key(
    private_key_hex: str,
    peer_public_key_hex: str,
    confidential_session_hash: str,
    direction: str,
) -> bytes:
    ec, _ = _require_confidential_crypto()
    private_bytes = _seed_bytes(private_key_hex)
    peer_public_key = ec.EllipticCurvePublicKey.from_encoded_point(
        ec.SECP256K1(), bytes.fromhex(_validate_public_key_hex(peer_public_key_hex))
    )
    private_key = ec.derive_private_key(int.from_bytes(private_bytes, "big"), ec.SECP256K1())
    shared_secret = private_key.exchange(ec.ECDH(), peer_public_key)
    return hashlib.sha256(
        b"froglet.confidential.v1"
        + shared_secret
        + confidential_session_hash.encode("utf-8")
        + direction.encode("utf-8")
    ).digest()


def encrypt_confidential_payload(
    confidential_session_hash: str,
    sender_private_key_hex: str,
    recipient_public_key_hex: str,
    payload: Any,
    *,
    payload_format: str = "application/json+jcs",
    direction: str = "request",
) -> dict[str, Any]:
    _, AESGCM = _require_confidential_crypto()
    key = _derive_confidential_key(
        sender_private_key_hex,
        recipient_public_key_hex,
        confidential_session_hash,
        direction,
    )
    nonce = os.urandom(12)
    aad = _canonical_json_bytes(
        [
            _FROGLET_SCHEMA_V1,
            _CONFIDENTIAL_ENVELOPE_TYPE,
            _CONFIDENTIAL_ENCRYPTION_ALGORITHM,
            confidential_session_hash,
            direction,
            payload_format,
        ]
    )
    ciphertext = AESGCM(key).encrypt(nonce, _canonical_json_bytes(payload), aad)
    return {
        "schema_version": _FROGLET_SCHEMA_V1,
        "envelope_type": _CONFIDENTIAL_ENVELOPE_TYPE,
        "algorithm": _CONFIDENTIAL_ENCRYPTION_ALGORITHM,
        "confidential_session_hash": confidential_session_hash,
        "direction": direction,
        "payload_format": payload_format,
        "nonce_b64": base64.b64encode(nonce).decode("ascii"),
        "ciphertext_b64": base64.b64encode(ciphertext).decode("ascii"),
    }


def decrypt_confidential_envelope(
    confidential_session_hash: str,
    recipient_private_key_hex: str,
    sender_public_key_hex: str,
    envelope: dict[str, Any],
    *,
    expected_direction: str = "result",
) -> Any:
    _, AESGCM = _require_confidential_crypto()
    if envelope.get("schema_version") != _FROGLET_SCHEMA_V1:
        raise ValueError("confidential envelope has unsupported schema_version")
    if envelope.get("envelope_type") != _CONFIDENTIAL_ENVELOPE_TYPE:
        raise ValueError("confidential envelope has unsupported envelope_type")
    if envelope.get("algorithm") != _CONFIDENTIAL_ENCRYPTION_ALGORITHM:
        raise ValueError("confidential envelope has unsupported algorithm")
    if envelope.get("confidential_session_hash") != confidential_session_hash:
        raise ValueError("confidential envelope hash does not match expected session")
    if envelope.get("direction") != expected_direction:
        raise ValueError("confidential envelope direction does not match expected direction")
    key = _derive_confidential_key(
        recipient_private_key_hex,
        sender_public_key_hex,
        confidential_session_hash,
        expected_direction,
    )
    nonce = base64.b64decode(envelope["nonce_b64"])
    ciphertext = base64.b64decode(envelope["ciphertext_b64"])
    aad = _canonical_json_bytes(
        [
            _FROGLET_SCHEMA_V1,
            _CONFIDENTIAL_ENVELOPE_TYPE,
            _CONFIDENTIAL_ENCRYPTION_ALGORITHM,
            confidential_session_hash,
            expected_direction,
            envelope.get("payload_format", "application/json+jcs"),
        ]
    )
    plaintext = AESGCM(key).decrypt(nonce, ciphertext, aad)
    return json.loads(plaintext.decode("utf-8"))


def verify_confidential_session_bundle(
    profile: dict[str, Any],
    session: dict[str, Any],
    attestation: dict[str, Any],
    *,
    now: int | None = None,
) -> None:
    current_time = int(time.time()) if now is None else now
    profile_payload = profile["payload"]
    session_payload = session["payload"]
    if session.get("artifact_type") != _CONFIDENTIAL_SESSION_ARTIFACT:
        raise ValueError("session artifact_type must be confidential_session")
    if profile.get("artifact_type") != _CONFIDENTIAL_PROFILE_ARTIFACT:
        raise ValueError("profile artifact_type must be confidential_profile")
    if session_payload["allowed_workload_kind"] != profile_payload["allowed_workload_kind"]:
        raise ValueError("confidential session workload kind does not match profile")
    if session_payload["execution_mode"] != _CONFIDENTIAL_EXECUTION_MODE:
        raise ValueError("confidential session execution_mode must be tee")
    if session_payload["attestation_platform"] != profile_payload["attestation_platform"]:
        raise ValueError("confidential session attestation_platform does not match profile")
    if session_payload["measurement"] != profile_payload["measurement"]:
        raise ValueError("confidential session measurement does not match profile")
    if session_payload["key_release_policy_hash"] != profile_payload["key_release_policy_hash"]:
        raise ValueError("confidential session key_release_policy_hash does not match profile")
    if attestation["platform"] != profile_payload["attestation_platform"]:
        raise ValueError("attestation platform does not match profile")
    if attestation["measurement"] != profile_payload["measurement"]:
        raise ValueError("attestation measurement does not match profile")
    if attestation["session_public_key"] != session_payload["session_public_key"]:
        raise ValueError("attestation session_public_key does not match session")
    if attestation["key_release_policy_hash"] != profile_payload["key_release_policy_hash"]:
        raise ValueError("attestation key_release_policy_hash does not match profile")
    if current_time > int(session_payload["expires_at"]) or current_time > int(attestation["expires_at"]):
        raise ValueError("confidential session attestation is expired")
    if _canonical_json_hash(attestation) != session_payload["attestation_evidence_hash"]:
        raise ValueError("attestation hash does not match confidential session")


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
    if isinstance(quote.get("payload", {}).get("confidential_session_hash"), str):
        payload["confidential_session_hash"] = quote["payload"]["confidential_session_hash"]
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
            self._session = aiohttp.ClientSession(timeout=DEFAULT_HTTP_TIMEOUT)
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
        expected = set(expected_statuses)
        session = await self._ensure_session()
        async with session.request(
            method,
            f"{self.base_url}{path}",
            headers=self._headers(),
            json=json_body,
        ) as resp:
            raw_body = await resp.text()
        payload = _try_parse_json(raw_body)
        if resp.status not in expected:
            if payload is None:
                payload = {
                    "error": "non_json_error_response",
                    "body": _truncate_error_body(raw_body),
                }
            raise FrogletClientError(resp.status, payload)
        if payload is None:
            if not raw_body.strip():
                return None
            raise FrogletClientError(
                resp.status,
                {
                    "error": "invalid_json_response",
                    "body": _truncate_error_body(raw_body),
                },
            )
        return payload


class ProviderClient(_JsonApiClient):
    async def descriptor(self) -> dict[str, Any]:
        return await self._request_json("GET", "/v1/provider/descriptor")

    async def offers(self) -> list[dict[str, Any]]:
        response = await self._request_json("GET", "/v1/provider/offers")
        return response["offers"]

    async def confidential_profile(self, artifact_hash: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/provider/confidential/profiles/{artifact_hash}"
        )

    async def open_confidential_session(
        self,
        confidential_profile_hash: str,
        *,
        requester_id: str,
        allowed_workload_kind: str,
        requester_public_key: str,
    ) -> dict[str, Any]:
        return await self._request_json(
            "POST",
            "/v1/provider/confidential/sessions",
            json_body={
                "requester_id": requester_id,
                "confidential_profile_hash": confidential_profile_hash,
                "allowed_workload_kind": allowed_workload_kind,
                "requester_public_key": requester_public_key,
            },
            expected_statuses=(201,),
        )

    async def confidential_session(self, session_id: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/provider/confidential/sessions/{session_id}"
        )

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
            "/v1/provider/quotes",
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
            "/v1/provider/deals",
            json_body=payload,
            expected_statuses=(200, 202),
        )

    async def get_deal(self, deal_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/provider/deals/{deal_id}")

    async def get_invoice_bundle(self, deal_id: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/provider/deals/{deal_id}/invoice-bundle"
        )

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
            f"/v1/provider/deals/{deal_id}/accept",
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
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout_secs
        base_delay = max(0.01, poll_interval_secs)
        max_delay = max(base_delay, min(_WAIT_BACKOFF_CAP_SECS, timeout_secs))
        attempt = 0
        while loop.time() < deadline:
            deal = await self.get_deal(deal_id)
            if deal["status"] in accepted_statuses:
                return deal
            remaining = deadline - loop.time()
            if remaining <= 0:
                break
            sleep_secs = min(remaining, _next_wait_delay(base_delay, max_delay, attempt))
            attempt += 1
            await asyncio.sleep(sleep_secs)
        raise TimeoutError(
            f"timed out waiting for deal {deal_id} to reach {sorted(accepted_statuses)}"
        )


class RuntimeClient(_JsonApiClient):
    def __init__(self, runtime_base_url: str, token: str) -> None:
        super().__init__(runtime_base_url)
        self.token = token

    @classmethod
    def from_token_file(
        cls,
        runtime_base_url: str,
        token_path: str | Path,
    ) -> "RuntimeClient":
        token = Path(token_path).read_text(encoding="utf-8").strip()
        return cls(runtime_base_url, token)

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.token}"}

    async def wallet_balance(self) -> dict[str, Any]:
        return await self._request_json("GET", "/v1/runtime/wallet/balance")

    async def search(
        self, *, limit: int = 20, include_inactive: bool = False
    ) -> list[dict[str, Any]]:
        response = await self._request_json(
            "POST",
            "/v1/runtime/search",
            json_body={
                "limit": limit,
                "include_inactive": include_inactive,
            },
        )
        return response["nodes"]

    async def get_provider(self, provider_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/runtime/providers/{provider_id}")

    async def buy_service(
        self,
        request: dict[str, Any],
        *,
        include_payment_intent: bool = False,
    ) -> DealHandle:
        response = await self._request_json(
            "POST",
            "/v1/runtime/deals",
            json_body=request,
        )
        deal = response["deal"]
        status = str(deal.get("status", ""))
        return DealHandle(
            quote=response["quote"],
            deal=deal,
            terminal=status in {"succeeded", "failed", "rejected"},
            payment_intent_path=response.get("payment_intent_path"),
            payment_intent=response.get("payment_intent") if include_payment_intent else None,
        )

    async def get_deal(self, deal_id: str) -> dict[str, Any]:
        response = await self._request_json("GET", f"/v1/runtime/deals/{deal_id}")
        return response["deal"]

    async def payment_intent(self, deal_id: str) -> dict[str, Any]:
        response = await self._request_json(
            "GET", f"/v1/runtime/deals/{deal_id}/payment-intent"
        )
        return response["payment_intent"]

    async def archive_subject(self, subject_kind: str, subject_id: str) -> dict[str, Any]:
        return await self._request_json(
            "GET", f"/v1/runtime/archive/{subject_kind}/{subject_id}"
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
        loop = asyncio.get_running_loop()
        deadline = loop.time() + timeout_secs
        base_delay = max(0.01, poll_interval_secs)
        max_delay = max(base_delay, min(_WAIT_BACKOFF_CAP_SECS, timeout_secs))
        attempt = 0
        while loop.time() < deadline:
            deal = await self.get_deal(deal_id)
            if deal["status"] in accepted_statuses:
                return deal
            remaining = deadline - loop.time()
            if remaining <= 0:
                break
            sleep_secs = min(remaining, _next_wait_delay(base_delay, max_delay, attempt))
            attempt += 1
            await asyncio.sleep(sleep_secs)
        raise TimeoutError(
            f"timed out waiting for deal {deal_id} to reach {sorted(accepted_statuses)}"
        )

    async def accept_result(
        self,
        deal_id: str,
        *,
        expected_result_hash: str | None = None,
    ) -> dict[str, Any]:
        response = await self._request_json(
            "POST",
            f"/v1/runtime/deals/{deal_id}/accept",
            json_body={"expected_result_hash": expected_result_hash},
        )
        return response["deal"]


class DiscoveryClient(_JsonApiClient):
    async def search_nodes(
        self, *, limit: int = 20, include_inactive: bool = False
    ) -> list[dict[str, Any]]:
        response = await self._request_json(
            "POST",
            "/v1/discovery/search",
            json_body={
                "limit": limit,
                "include_inactive": include_inactive,
            },
        )
        return response["nodes"]

    async def get_node(self, node_id: str) -> dict[str, Any]:
        return await self._request_json("GET", f"/v1/discovery/providers/{node_id}")


def _try_parse_json(raw_body: str) -> Any | None:
    if not raw_body.strip():
        return None
    try:
        return json.loads(raw_body)
    except json.JSONDecodeError:
        return None


def _truncate_error_body(raw_body: str) -> str:
    if len(raw_body) <= _MAX_ERROR_BODY_CHARS:
        return raw_body
    return f"{raw_body[:_MAX_ERROR_BODY_CHARS]}...(truncated)"


def _next_wait_delay(base_delay: float, max_delay: float, attempt: int) -> float:
    exponential = min(max_delay, base_delay * (2**attempt))
    return random.uniform(exponential * 0.5, exponential)
