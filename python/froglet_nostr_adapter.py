import argparse
import asyncio
import hashlib
import json
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Awaitable, Callable, Iterable, TypeVar

import aiohttp
from ecdsa import curves, ellipticcurve

try:
    from .froglet_client import DEFAULT_HTTP_TIMEOUT, RuntimeClient
except ImportError:
    from froglet_client import DEFAULT_HTTP_TIMEOUT, RuntimeClient

AUTH_EVENT_KIND = 22242
SECP256K1 = curves.SECP256k1
FIELD_PRIME = SECP256K1.curve.p()
GROUP_ORDER = SECP256K1.order
GENERATOR = SECP256K1.generator
_DEFAULT_RELAY_CONCURRENCY = 8
_RelayItem = TypeVar("_RelayItem")
_RelayResult = TypeVar("_RelayResult")


class NostrRelayError(RuntimeError):
    pass


class NostrRelayConfigError(NostrRelayError):
    pass


@dataclass(frozen=True)
class RelayPublishResult:
    relay_url: str
    event_id: str
    accepted: bool
    message: str


@dataclass(frozen=True)
class RelayPolicy:
    relay_url: str
    read: bool = True
    write: bool = True

    def __post_init__(self) -> None:
        relay_url = self.relay_url.strip()
        if not relay_url:
            raise ValueError("relay_url must not be empty")
        if not self.read and not self.write:
            raise ValueError("relay policy must allow read, write, or both")
        object.__setattr__(self, "relay_url", relay_url)

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RelayPolicy":
        relay_url = payload.get("url", payload.get("relay_url"))
        if not isinstance(relay_url, str):
            raise ValueError("relay entry must include a string url")
        read = payload.get("read", True)
        write = payload.get("write", True)
        if not isinstance(read, bool) or not isinstance(write, bool):
            raise ValueError("relay entry read/write flags must be booleans")
        return cls(relay_url=relay_url, read=read, write=write)


@dataclass(frozen=True)
class RetryPolicy:
    max_attempts: int = 3
    initial_backoff_secs: float = 0.25
    max_backoff_secs: float = 2.0

    def __post_init__(self) -> None:
        if self.max_attempts < 1:
            raise ValueError("max_attempts must be at least 1")
        if self.initial_backoff_secs < 0.0 or self.max_backoff_secs < 0.0:
            raise ValueError("backoff values must be non-negative")
        if self.max_backoff_secs < self.initial_backoff_secs:
            raise ValueError("max_backoff_secs must be >= initial_backoff_secs")

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> "RetryPolicy":
        return cls(
            max_attempts=int(payload.get("max_attempts", 3)),
            initial_backoff_secs=float(payload.get("initial_backoff_secs", 0.25)),
            max_backoff_secs=float(payload.get("max_backoff_secs", 2.0)),
        )


@dataclass(frozen=True)
class RelayListConfig:
    relay_policies: tuple[RelayPolicy, ...]
    retry_policy: RetryPolicy

    @classmethod
    def from_json_file(cls, path: str | Path) -> "RelayListConfig":
        payload = json.loads(Path(path).read_text(encoding="utf-8"))
        relay_entries = payload.get("relays")
        if not isinstance(relay_entries, list) or not relay_entries:
            raise ValueError("relay config must contain a non-empty relays array")
        relay_policies = tuple(RelayPolicy.from_dict(entry) for entry in relay_entries)
        retry_payload = payload.get("retry", {})
        if retry_payload is None:
            retry_payload = {}
        if not isinstance(retry_payload, dict):
            raise ValueError("relay config retry section must be an object")
        return cls(relay_policies=relay_policies, retry_policy=RetryPolicy.from_dict(retry_payload))


class NostrAuthSigner:
    def __init__(self, secret_key: bytes) -> None:
        if len(secret_key) != 32:
            raise ValueError("Nostr auth secret key must be exactly 32 bytes")
        self._secret_key = secret_key
        self.pubkey_hex = _schnorr_pubkey_hex(secret_key)

    @classmethod
    def from_seed_file(cls, path: str | Path) -> "NostrAuthSigner":
        seed_hex = Path(path).read_text(encoding="utf-8").strip()
        try:
            secret_key = bytes.fromhex(seed_hex)
        except ValueError as exc:
            raise ValueError(f"invalid hex in Nostr auth seed {path}") from exc
        return cls(secret_key)

    def build_auth_event(self, relay_url: str, challenge: str) -> dict[str, Any]:
        created_at = int(time.time())
        tags = [["relay", relay_url], ["challenge", challenge]]
        content = ""
        event_id_bytes = _event_id_preimage(
            self.pubkey_hex,
            created_at,
            AUTH_EVENT_KIND,
            tags,
            content,
        )
        event_id = hashlib.sha256(event_id_bytes).hexdigest()
        return {
            "id": event_id,
            "pubkey": self.pubkey_hex,
            "created_at": created_at,
            "kind": AUTH_EVENT_KIND,
            "tags": tags,
            "content": content,
            "sig": _bip340_sign(self._secret_key, bytes.fromhex(event_id)),
        }


class NostrRelayClient:
    def __init__(
        self,
        relay_url: str,
        *,
        session: aiohttp.ClientSession | None = None,
        auth_signer: NostrAuthSigner | None = None,
    ) -> None:
        self.relay_url = relay_url
        self.auth_signer = auth_signer
        self._session = session
        self._owns_session = session is None

    async def __aenter__(self) -> "NostrRelayClient":
        await self._ensure_session()
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        await self.close()

    async def close(self) -> None:
        if self._owns_session and self._session is not None:
            await self._session.close()
            self._session = None

    async def publish_event(
        self,
        event: dict[str, Any],
        *,
        timeout_secs: float = 5.0,
    ) -> RelayPublishResult:
        session = await self._ensure_session()
        auth_event_ids: set[str] = set()
        async with session.ws_connect(self.relay_url, heartbeat=timeout_secs) as ws:
            await ws.send_json(["EVENT", event])
            while True:
                message = await ws.receive(timeout=timeout_secs)
                payload = _decode_ws_payload(message)
                if await self._maybe_handle_auth(
                    ws,
                    payload,
                    auth_event_ids=auth_event_ids,
                    expected_pubkey=event.get("pubkey"),
                ):
                    continue
                if (
                    isinstance(payload, list)
                    and len(payload) >= 4
                    and payload[0] == "OK"
                    and payload[1] in auth_event_ids
                ):
                    if not bool(payload[2]):
                        raise NostrRelayConfigError(
                            f"relay {self.relay_url} rejected auth event: {payload[3]}"
                        )
                    auth_event_ids.remove(payload[1])
                    continue
                if (
                    isinstance(payload, list)
                    and len(payload) >= 4
                    and payload[0] == "OK"
                    and payload[1] == event["id"]
                ):
                    return RelayPublishResult(
                        relay_url=self.relay_url,
                        event_id=event["id"],
                        accepted=bool(payload[2]),
                        message=str(payload[3]),
                    )
                if isinstance(payload, list) and payload and payload[0] == "NOTICE":
                    raise NostrRelayError(f"relay notice from {self.relay_url}: {payload[1]}")

    async def query_events(
        self,
        filters: list[dict[str, Any]],
        *,
        timeout_secs: float = 5.0,
    ) -> list[dict[str, Any]]:
        session = await self._ensure_session()
        subscription_id = uuid.uuid4().hex
        auth_event_ids: set[str] = set()
        events: list[dict[str, Any]] = []

        async with session.ws_connect(self.relay_url, heartbeat=timeout_secs) as ws:
            await ws.send_json(["REQ", subscription_id, *filters])
            while True:
                message = await ws.receive(timeout=timeout_secs)
                payload = _decode_ws_payload(message)
                if await self._maybe_handle_auth(ws, payload, auth_event_ids=auth_event_ids):
                    continue
                if (
                    isinstance(payload, list)
                    and len(payload) >= 4
                    and payload[0] == "OK"
                    and payload[1] in auth_event_ids
                ):
                    if not bool(payload[2]):
                        raise NostrRelayConfigError(
                            f"relay {self.relay_url} rejected auth event: {payload[3]}"
                        )
                    auth_event_ids.remove(payload[1])
                    continue
                if not isinstance(payload, list) or len(payload) < 2:
                    continue
                if payload[0] == "EVENT" and payload[1] == subscription_id and len(payload) >= 3:
                    events.append(payload[2])
                elif payload[0] == "EOSE" and payload[1] == subscription_id:
                    await ws.send_json(["CLOSE", subscription_id])
                    return events
                elif payload[0] == "NOTICE":
                    raise NostrRelayError(f"relay notice from {self.relay_url}: {payload[1]}")

    async def _maybe_handle_auth(
        self,
        ws: aiohttp.ClientWebSocketResponse,
        payload: Any,
        *,
        auth_event_ids: set[str],
        expected_pubkey: str | None = None,
    ) -> bool:
        if not isinstance(payload, list) or len(payload) < 2 or payload[0] != "AUTH":
            return False
        if self.auth_signer is None:
            raise NostrRelayConfigError(
                f"relay {self.relay_url} requested AUTH but no auth seed was configured"
            )
        auth_event = self.auth_signer.build_auth_event(self.relay_url, str(payload[1]))
        if expected_pubkey is not None and auth_event["pubkey"] != expected_pubkey:
            raise NostrRelayConfigError(
                f"relay {self.relay_url} requested AUTH for pubkey {expected_pubkey}, "
                f"but auth seed resolves to {auth_event['pubkey']}"
            )
        auth_event_ids.add(auth_event["id"])
        await ws.send_json(["AUTH", auth_event])
        return True

    async def _ensure_session(self) -> aiohttp.ClientSession:
        if self._session is None or self._session.closed:
            self._session = aiohttp.ClientSession(timeout=DEFAULT_HTTP_TIMEOUT)
        return self._session


class FrogletNostrRelayAdapter:
    def __init__(
        self,
        runtime_base_url: str,
        token: str,
        relay_policies: Iterable[RelayPolicy],
        *,
        provider_base_url: str | None = None,
        retry_policy: RetryPolicy | None = None,
        auth_signer: NostrAuthSigner | None = None,
    ) -> None:
        self.runtime = RuntimeClient(
            runtime_base_url,
            token,
            provider_base_url=provider_base_url,
        )
        self.relay_policies = list(relay_policies)
        if not self.relay_policies:
            raise ValueError("at least one relay policy is required")
        self.relay_urls = [policy.relay_url for policy in self.relay_policies]
        self.retry_policy = retry_policy or RetryPolicy()
        self.auth_signer = auth_signer
        self._relay_concurrency_limit = _DEFAULT_RELAY_CONCURRENCY

    @classmethod
    def from_token_file(
        cls,
        runtime_base_url: str,
        token_path: str | Path,
        relay_urls: Iterable[str],
        *,
        provider_base_url: str | None = None,
        retry_policy: RetryPolicy | None = None,
        auth_seed_path: str | Path | None = None,
    ) -> "FrogletNostrRelayAdapter":
        token = Path(token_path).read_text(encoding="utf-8").strip()
        auth_signer = (
            NostrAuthSigner.from_seed_file(auth_seed_path)
            if auth_seed_path is not None
            else None
        )
        relay_policies = [RelayPolicy(relay_url=relay_url) for relay_url in relay_urls]
        return cls(
            runtime_base_url,
            token,
            relay_policies,
            provider_base_url=provider_base_url,
            retry_policy=retry_policy,
            auth_signer=auth_signer,
        )

    @classmethod
    def from_config_file(
        cls,
        runtime_base_url: str,
        token_path: str | Path,
        config_path: str | Path,
        *,
        provider_base_url: str | None = None,
        retry_policy: RetryPolicy | None = None,
        auth_seed_path: str | Path | None = None,
    ) -> "FrogletNostrRelayAdapter":
        token = Path(token_path).read_text(encoding="utf-8").strip()
        config = RelayListConfig.from_json_file(config_path)
        auth_signer = (
            NostrAuthSigner.from_seed_file(auth_seed_path)
            if auth_seed_path is not None
            else None
        )
        return cls(
            runtime_base_url,
            token,
            config.relay_policies,
            provider_base_url=provider_base_url,
            retry_policy=retry_policy or config.retry_policy,
            auth_signer=auth_signer,
        )

    async def __aenter__(self) -> "FrogletNostrRelayAdapter":
        await self.runtime.__aenter__()
        return self

    async def __aexit__(self, exc_type, exc, tb) -> None:
        await self.runtime.close()

    async def collect_provider_events(self) -> list[dict[str, Any]]:
        response = await self.runtime.nostr_provider_publications()
        return [response["descriptor_summary"], *response["offer_summaries"]]

    async def collect_receipt_event(self, deal_id: str) -> dict[str, Any]:
        response = await self.runtime.nostr_receipt_publication(deal_id)
        return response["receipt_summary"]

    async def publish_provider_events(self) -> list[RelayPublishResult]:
        return await self.publish_events(await self.collect_provider_events())

    async def publish_receipt_event(self, deal_id: str) -> list[RelayPublishResult]:
        return await self.publish_events([await self.collect_receipt_event(deal_id)])

    async def publish_events(
        self,
        events: Iterable[dict[str, Any]],
    ) -> list[RelayPublishResult]:
        event_list = list(events)
        write_relays = [policy for policy in self.relay_policies if policy.write]
        if not write_relays:
            raise NostrRelayConfigError("no write-enabled relays are configured")

        results: list[RelayPublishResult] = []
        async with aiohttp.ClientSession(timeout=DEFAULT_HTTP_TIMEOUT) as session:

            async def publish_for_relay(
                relay_policy: RelayPolicy,
            ) -> list[RelayPublishResult]:
                relay = NostrRelayClient(
                    relay_policy.relay_url,
                    session=session,
                    auth_signer=self.auth_signer,
                )
                relay_results: list[RelayPublishResult] = []
                for event in event_list:
                    relay_results.append(
                        await self._publish_with_retry(relay, relay_policy, event)
                    )
                return relay_results

            relay_results = await self._gather_relays_bounded(
                write_relays, publish_for_relay
            )
            for item in relay_results:
                results.extend(item)
        return results

    async def query_events(
        self,
        filters: list[dict[str, Any]],
    ) -> list[dict[str, Any]]:
        read_relays = [policy for policy in self.relay_policies if policy.read]
        if not read_relays:
            raise NostrRelayConfigError("no read-enabled relays are configured")

        deduped: dict[str, dict[str, Any]] = {}
        errors: list[str] = []
        async with aiohttp.ClientSession(timeout=DEFAULT_HTTP_TIMEOUT) as session:

            async def query_relay(
                relay_policy: RelayPolicy,
            ) -> tuple[list[dict[str, Any]], str | None]:
                relay = NostrRelayClient(
                    relay_policy.relay_url,
                    session=session,
                    auth_signer=self.auth_signer,
                )
                return await self._query_with_retry(relay, relay_policy, filters)

            relay_results = await self._gather_relays_bounded(read_relays, query_relay)
            for events, error in relay_results:
                if error is not None:
                    errors.append(error)
                    continue
                for event in events:
                    event_id = event.get("id")
                    if isinstance(event_id, str):
                        deduped[event_id] = event
        if deduped or not errors:
            return list(deduped.values())
        raise NostrRelayError("; ".join(errors))

    async def _publish_with_retry(
        self,
        relay: NostrRelayClient,
        relay_policy: RelayPolicy,
        event: dict[str, Any],
    ) -> RelayPublishResult:
        delay = self.retry_policy.initial_backoff_secs
        last_error: str | None = None
        for attempt in range(1, self.retry_policy.max_attempts + 1):
            try:
                return await relay.publish_event(event)
            except NostrRelayConfigError as exc:
                return RelayPublishResult(
                    relay_url=relay_policy.relay_url,
                    event_id=str(event.get("id", "")),
                    accepted=False,
                    message=str(exc),
                )
            except (aiohttp.ClientError, asyncio.TimeoutError, NostrRelayError) as exc:
                last_error = str(exc)
                if attempt == self.retry_policy.max_attempts:
                    break
                await asyncio.sleep(delay)
                delay = min(delay * 2, self.retry_policy.max_backoff_secs)
        return RelayPublishResult(
            relay_url=relay_policy.relay_url,
            event_id=str(event.get("id", "")),
            accepted=False,
            message=(
                f"publish failed after {self.retry_policy.max_attempts} attempt(s): "
                f"{last_error or 'unknown relay error'}"
            ),
        )

    async def _query_with_retry(
        self,
        relay: NostrRelayClient,
        relay_policy: RelayPolicy,
        filters: list[dict[str, Any]],
    ) -> tuple[list[dict[str, Any]], str | None]:
        delay = self.retry_policy.initial_backoff_secs
        last_error: str | None = None
        for attempt in range(1, self.retry_policy.max_attempts + 1):
            try:
                return await relay.query_events(filters), None
            except NostrRelayConfigError as exc:
                return [], str(exc)
            except (aiohttp.ClientError, asyncio.TimeoutError, NostrRelayError) as exc:
                last_error = str(exc)
                if attempt == self.retry_policy.max_attempts:
                    break
                await asyncio.sleep(delay)
                delay = min(delay * 2, self.retry_policy.max_backoff_secs)
        return (
            [],
            f"query against {relay_policy.relay_url} failed after "
            f"{self.retry_policy.max_attempts} attempt(s): {last_error or 'unknown relay error'}",
        )

    async def _gather_relays_bounded(
        self,
        relay_items: list[_RelayItem],
        runner: Callable[[_RelayItem], Awaitable[_RelayResult]],
    ) -> list[_RelayResult]:
        if not relay_items:
            return []
        limit = max(1, min(self._relay_concurrency_limit, len(relay_items)))
        semaphore = asyncio.Semaphore(limit)

        async def run_item(item: _RelayItem) -> _RelayResult:
            async with semaphore:
                return await runner(item)

        return list(await asyncio.gather(*(run_item(item) for item in relay_items)))


def verify_event(event: dict[str, Any]) -> bool:
    try:
        event_id = event["id"]
        pubkey = event["pubkey"]
        created_at = int(event["created_at"])
        kind = int(event["kind"])
        tags = event["tags"]
        content = event["content"]
        signature = event["sig"]
    except (KeyError, TypeError, ValueError):
        return False
    if not isinstance(event_id, str) or not isinstance(pubkey, str) or not isinstance(signature, str):
        return False
    if not isinstance(tags, list) or not isinstance(content, str):
        return False
    expected_id = hashlib.sha256(
        _event_id_preimage(pubkey, created_at, kind, tags, content)
    ).hexdigest()
    if expected_id != event_id:
        return False
    try:
        event_id_bytes = bytes.fromhex(event_id)
    except ValueError:
        return False
    return _bip340_verify(pubkey, signature, event_id_bytes)


def _decode_ws_payload(message: aiohttp.WSMessage) -> Any:
    if message.type == aiohttp.WSMsgType.TEXT:
        return json.loads(message.data)
    if message.type == aiohttp.WSMsgType.ERROR:
        raise NostrRelayError(f"relay websocket error: {message.data}")
    if message.type in (aiohttp.WSMsgType.CLOSE, aiohttp.WSMsgType.CLOSED):
        raise NostrRelayError("relay websocket closed before completing request")
    raise NostrRelayError(f"unexpected websocket message type: {message.type}")


def _event_id_preimage(
    pubkey: str,
    created_at: int,
    kind: int,
    tags: list[list[str]],
    content: str,
) -> bytes:
    return json.dumps(
        [0, pubkey, created_at, kind, tags, content],
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode("utf-8")


def _schnorr_pubkey_hex(secret_key: bytes) -> str:
    point = _int_from_bytes(secret_key) * GENERATOR
    return _int_to_bytes(point.x()).hex()


def _bip340_sign(secret_key: bytes, message: bytes) -> str:
    secret = _int_from_bytes(secret_key)
    if not 1 <= secret < GROUP_ORDER:
        raise ValueError("invalid secp256k1 secret key")

    point = secret * GENERATOR
    secret_scalar = secret if _has_even_y(point) else GROUP_ORDER - secret
    pubkey_bytes = _int_to_bytes(point.x())
    aux = bytes(32)
    t = _xor_bytes(_int_to_bytes(secret_scalar), _tagged_hash("BIP0340/aux", aux))
    nonce = _int_from_bytes(_tagged_hash("BIP0340/nonce", t + pubkey_bytes + message)) % GROUP_ORDER
    if nonce == 0:
        raise ValueError("derived invalid Schnorr nonce")

    nonce_point = nonce * GENERATOR
    signing_nonce = nonce if _has_even_y(nonce_point) else GROUP_ORDER - nonce
    nonce_x = _int_to_bytes(nonce_point.x())
    challenge = _int_from_bytes(
        _tagged_hash("BIP0340/challenge", nonce_x + pubkey_bytes + message)
    ) % GROUP_ORDER
    signature = nonce_x + _int_to_bytes((signing_nonce + challenge * secret_scalar) % GROUP_ORDER)
    return signature.hex()


def _bip340_verify(pubkey_hex: str, signature_hex: str, message: bytes) -> bool:
    try:
        pubkey_bytes = bytes.fromhex(pubkey_hex)
        signature = bytes.fromhex(signature_hex)
    except ValueError:
        return False

    if len(pubkey_bytes) != 32 or len(signature) != 64:
        return False

    point = _lift_x(pubkey_bytes)
    if point is None:
        return False

    r = _int_from_bytes(signature[:32])
    s = _int_from_bytes(signature[32:])
    if r >= FIELD_PRIME or s >= GROUP_ORDER:
        return False

    challenge = _int_from_bytes(
        _tagged_hash("BIP0340/challenge", signature[:32] + pubkey_bytes + message)
    ) % GROUP_ORDER
    candidate = s * GENERATOR + ((GROUP_ORDER - challenge) % GROUP_ORDER) * point
    if candidate == ellipticcurve.INFINITY or not _has_even_y(candidate):
        return False
    return candidate.x() == r


def _lift_x(pubkey_bytes: bytes) -> ellipticcurve.Point | None:
    if len(pubkey_bytes) != 32:
        return None
    x = _int_from_bytes(pubkey_bytes)
    if x >= FIELD_PRIME:
        return None
    alpha = (pow(x, 3, FIELD_PRIME) + 7) % FIELD_PRIME
    beta = pow(alpha, (FIELD_PRIME + 1) // 4, FIELD_PRIME)
    if (beta * beta) % FIELD_PRIME != alpha:
        return None
    y = beta if beta % 2 == 0 else FIELD_PRIME - beta
    return ellipticcurve.Point(SECP256K1.curve, x, y, GROUP_ORDER)


def _has_even_y(point: ellipticcurve.Point) -> bool:
    return point.y() % 2 == 0


def _tagged_hash(tag: str, message: bytes) -> bytes:
    tag_hash = hashlib.sha256(tag.encode("utf-8")).digest()
    return hashlib.sha256(tag_hash + tag_hash + message).digest()


def _xor_bytes(left: bytes, right: bytes) -> bytes:
    return bytes(a ^ b for a, b in zip(left, right, strict=True))


def _int_from_bytes(data: bytes) -> int:
    return int.from_bytes(data, "big")


def _int_to_bytes(value: int) -> bytes:
    return value.to_bytes(32, "big")


def _retry_policy_from_args(
    args: argparse.Namespace,
    *,
    default: RetryPolicy | None = None,
) -> RetryPolicy | None:
    if (
        args.retry_attempts is None
        and args.initial_backoff_secs is None
        and args.max_backoff_secs is None
    ):
        return default
    fallback = default or RetryPolicy()
    return RetryPolicy(
        max_attempts=args.retry_attempts
        if args.retry_attempts is not None
        else fallback.max_attempts,
        initial_backoff_secs=args.initial_backoff_secs
        if args.initial_backoff_secs is not None
        else fallback.initial_backoff_secs,
        max_backoff_secs=args.max_backoff_secs
        if args.max_backoff_secs is not None
        else fallback.max_backoff_secs,
    )


async def _run_cli(args: argparse.Namespace) -> int:
    retry_policy = _retry_policy_from_args(args)
    if args.relay_config is not None:
        config = RelayListConfig.from_json_file(args.relay_config)
        adapter = FrogletNostrRelayAdapter.from_config_file(
            args.runtime_url,
            args.token_file,
            args.relay_config,
            provider_base_url=args.provider_url,
            retry_policy=retry_policy or config.retry_policy,
            auth_seed_path=args.auth_seed_file,
        )
    else:
        adapter = FrogletNostrRelayAdapter.from_token_file(
            args.runtime_url,
            args.token_file,
            args.relay or [],
            provider_base_url=args.provider_url,
            retry_policy=retry_policy,
            auth_seed_path=args.auth_seed_file,
        )

    async with adapter:
        if args.command == "publish-provider":
            results = await adapter.publish_provider_events()
            print(json.dumps([result.__dict__ for result in results], indent=2))
            return 0 if all(result.accepted for result in results) else 1
        if args.command == "publish-receipt":
            results = await adapter.publish_receipt_event(args.deal_id)
            print(json.dumps([result.__dict__ for result in results], indent=2))
            return 0 if all(result.accepted for result in results) else 1
        if args.command == "query":
            filters: dict[str, Any] = {}
            if args.kind:
                filters["kinds"] = args.kind
            if args.author:
                filters["authors"] = args.author
            if args.limit is not None:
                filters["limit"] = args.limit
            events = await adapter.query_events([filters])
            print(json.dumps(events, indent=2))
            return 0
    raise AssertionError("unreachable")


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="External Nostr relay adapter for Froglet publication intents"
    )
    parser.add_argument("--runtime-url", required=True)
    parser.add_argument(
        "--provider-url",
        help="Public provider API base URL. Defaults to the runtime URL when omitted.",
    )
    parser.add_argument("--token-file", required=True)
    relay_group = parser.add_mutually_exclusive_group(required=True)
    relay_group.add_argument(
        "--relay",
        action="append",
        help="Relay websocket URL. Repeat for multiple relays.",
    )
    relay_group.add_argument(
        "--relay-config",
        help="JSON file describing allowed relays, read/write roles, and retry policy.",
    )
    parser.add_argument(
        "--auth-seed-file",
        help="Path to the linked Nostr publication seed used for relay AUTH challenges.",
    )
    parser.add_argument("--retry-attempts", type=int)
    parser.add_argument("--initial-backoff-secs", type=float)
    parser.add_argument("--max-backoff-secs", type=float)

    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("publish-provider")

    publish_receipt = subparsers.add_parser("publish-receipt")
    publish_receipt.add_argument("--deal-id", required=True)

    query = subparsers.add_parser("query")
    query.add_argument("--kind", action="append", type=int, default=[])
    query.add_argument("--author", action="append", default=[])
    query.add_argument("--limit", type=int)
    return parser


def main() -> int:
    parser = _build_parser()
    return asyncio.run(_run_cli(parser.parse_args()))


if __name__ == "__main__":
    raise SystemExit(main())
