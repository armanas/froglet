import argparse
import asyncio
import contextlib
import io
import json
import socket
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from aiohttp import web

from froglet_client import ProviderClient, RuntimeClient
from froglet_nostr_adapter import (
    FrogletNostrRelayAdapter,
    NostrAuthSigner,
    NostrRelayClient,
    NostrRelayConfigError,
    RelayPublishResult,
    RelayListConfig,
    RelayPolicy,
    RetryPolicy,
    _build_parser,
    _run_cli,
    main,
    verify_event,
)
from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    generate_schnorr_signing_key,
    listening_port,
    schnorr_pubkey_hex,
    sha256_hex,
)


class FakeNostrRelay:
    def __init__(
        self,
        *,
        require_auth: bool = False,
        fail_first_event_attempts: int = 0,
    ) -> None:
        self.events: dict[str, dict] = {}
        self.auth_events: list[dict] = []
        self.require_auth = require_auth
        self.fail_first_event_attempts = fail_first_event_attempts
        self.event_attempts = 0
        self.query_attempts = 0
        self._runner: web.AppRunner | None = None
        self.url: str | None = None

    async def start(self) -> None:
        app = web.Application()
        app.router.add_get("/", self._handle_ws)
        self._runner = web.AppRunner(app)
        await self._runner.setup()
        site = web.TCPSite(self._runner, "127.0.0.1", 0)
        await site.start()
        self.url = f"ws://127.0.0.1:{listening_port(site)}/"

    async def stop(self) -> None:
        if self._runner is not None:
            await self._runner.cleanup()
            self._runner = None

    def store_event(self, event: dict) -> None:
        self.events[event["id"]] = event

    async def _handle_ws(self, request: web.Request) -> web.WebSocketResponse:
        ws = web.WebSocketResponse()
        await ws.prepare(request)
        authenticated = not self.require_auth
        challenge = f"challenge-{uuid.uuid4().hex}"
        pending_event: tuple[dict, int] | None = None
        pending_query: tuple[str, list[dict], int] | None = None
        async for message in ws:
            payload = json.loads(message.data)
            if not isinstance(payload, list) or not payload:
                continue
            if payload[0] == "AUTH" and len(payload) >= 2:
                auth_event = payload[1]
                self.auth_events.append(auth_event)
                if not _auth_event_matches(auth_event, challenge, self.url):
                    await ws.send_json(["OK", auth_event.get("id", ""), False, "invalid auth"])
                    continue
                authenticated = True
                await ws.send_json(["OK", auth_event["id"], True, "auth ok"])
                if pending_event is not None:
                    event, attempt_no = pending_event
                    pending_event = None
                    if not await self._accept_event(ws, event, attempt_no):
                        break
                if pending_query is not None:
                    subscription_id, filters, attempt_no = pending_query
                    pending_query = None
                    await self._serve_query(ws, subscription_id, filters, attempt_no)
                continue
            if payload[0] == "EVENT" and len(payload) >= 2:
                event = payload[1]
                self.event_attempts += 1
                attempt_no = self.event_attempts
                if not authenticated:
                    pending_event = (event, attempt_no)
                    await ws.send_json(["AUTH", challenge])
                    continue
                if not await self._accept_event(ws, event, attempt_no):
                    break
            elif payload[0] == "REQ" and len(payload) >= 2:
                subscription_id = payload[1]
                filters = payload[2:] or [{}]
                self.query_attempts += 1
                attempt_no = self.query_attempts
                if not authenticated:
                    pending_query = (subscription_id, filters, attempt_no)
                    await ws.send_json(["AUTH", challenge])
                    continue
                await self._serve_query(ws, subscription_id, filters, attempt_no)
            elif payload[0] == "CLOSE":
                await ws.close()
                break
        return ws

    async def _accept_event(
        self,
        ws: web.WebSocketResponse,
        event: dict,
        attempt_no: int,
    ) -> bool:
        if attempt_no <= self.fail_first_event_attempts:
            await ws.close()
            return False
        self.events[event["id"]] = event
        await ws.send_json(["OK", event["id"], True, "stored"])
        return True

    async def _serve_query(
        self,
        ws: web.WebSocketResponse,
        subscription_id: str,
        filters: list[dict],
        attempt_no: int,
    ) -> None:
        del attempt_no
        for event in self.events.values():
            if any(_event_matches_filter(event, filter_) for filter_ in filters):
                await ws.send_json(["EVENT", subscription_id, event])
        await ws.send_json(["EOSE", subscription_id])


def _event_matches_filter(event: dict, filter_: dict) -> bool:
    kinds = filter_.get("kinds")
    if kinds is not None and event.get("kind") not in kinds:
        return False
    authors = filter_.get("authors")
    if authors is not None and event.get("pubkey") not in authors:
        return False
    return True


def _auth_event_matches(event: dict, challenge: str, relay_url: str | None) -> bool:
    if relay_url is None or not verify_event(event):
        return False
    if event.get("kind") != 22242 or event.get("content") != "":
        return False
    tags = event.get("tags")
    if not isinstance(tags, list):
        return False
    return ["relay", relay_url] in tags and ["challenge", challenge] in tags


def _runtime_token_path(node) -> str:
    return str(node.data_dir / "runtime" / "auth.token")


def _runtime_requester_fields(secret_key: bytes, success_payment_hash: str) -> dict[str, str]:
    return {
        "requester_id": schnorr_pubkey_hex(secret_key),
        "requester_seed_hex": secret_key.hex(),
        "success_payment_hash": success_payment_hash,
    }


class NostrRelayAdapterTests(FrogletAsyncTestCase):
    async def asyncSetUp(self) -> None:
        await super().asyncSetUp()
        self.relay = FakeNostrRelay()
        await self.relay.start()
        self.addAsyncCleanup(self.relay.stop)

    async def test_external_adapter_publishes_provider_summaries_to_relay(self) -> None:
        node = await self.start_node()

        async with ProviderClient(node.base_url) as provider:
            descriptor = await provider.descriptor()

        adapter = FrogletNostrRelayAdapter.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            [self.relay.url],
            provider_base_url=node.base_url,
        )
        async with adapter:
            publish_results = await adapter.publish_provider_events()
            queried = await adapter.query_events(
                [
                    {
                        "kinds": [30390, 30391],
                        "authors": [descriptor["payload"]["linked_identities"][0]["identity"]],
                    }
                ]
            )

        self.assertEqual(len(publish_results), 3)
        self.assertTrue(all(result.accepted for result in publish_results))
        self.assertGreaterEqual(len(queried), 3)
        self.assertTrue(
            all(
                event["pubkey"] == descriptor["payload"]["linked_identities"][0]["identity"]
                for event in queried
            )
        )

    async def test_external_adapter_publishes_terminal_receipts_to_relay(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        runtime = RuntimeClient.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            provider_base_url=node.base_url,
        )
        success_preimage = "44" * 32

        async with ProviderClient(node.base_url) as provider:
            descriptor = await provider.descriptor()

        async with runtime:
            requester_key = generate_schnorr_signing_key()
            handle = await runtime.buy_service(
                {
                    "offer_id": "execute.wasm",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "relay-adapter-receipt",
                    **_runtime_requester_fields(
                        requester_key,
                        sha256_hex(bytes.fromhex(success_preimage)),
                    ),
                },
                include_payment_intent=True,
            )
            await runtime.set_mock_lightning_state(
                handle.payment_intent["session_id"],
                base_state="settled",
                success_state="accepted",
            )
            result_ready = await runtime.wait_for_deal(
                handle.deal["deal_id"], statuses={"result_ready"}
            )
            await runtime.accept_result(
                handle.deal["deal_id"],
                success_preimage,
                expected_result_hash=result_ready["result_hash"],
            )

        adapter = FrogletNostrRelayAdapter.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            [self.relay.url],
            provider_base_url=node.base_url,
        )
        async with adapter:
            publish_results = await adapter.publish_receipt_event(handle.deal["deal_id"])
            queried = await adapter.query_events(
                [
                    {
                        "kinds": [1390],
                        "authors": [descriptor["payload"]["linked_identities"][0]["identity"]],
                    }
                ]
            )

        self.assertEqual(len(publish_results), 1)
        self.assertTrue(publish_results[0].accepted)
        self.assertEqual(len(queried), 1)
        self.assertEqual(queried[0]["kind"], 1390)
        self.assertEqual(
            queried[0]["pubkey"], descriptor["payload"]["linked_identities"][0]["identity"]
        )

    async def test_external_adapter_retries_transient_publish_failures(self) -> None:
        retry_relay = FakeNostrRelay(fail_first_event_attempts=2)
        await retry_relay.start()
        self.addAsyncCleanup(retry_relay.stop)
        node = await self.start_node()

        adapter = FrogletNostrRelayAdapter.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            [retry_relay.url],
            provider_base_url=node.base_url,
            retry_policy=RetryPolicy(
                max_attempts=3,
                initial_backoff_secs=0.01,
                max_backoff_secs=0.02,
            ),
        )
        async with adapter:
            publish_results = await adapter.publish_provider_events()

        self.assertEqual(len(publish_results), 3)
        self.assertTrue(all(result.accepted for result in publish_results))
        self.assertEqual(retry_relay.event_attempts, len(publish_results) + 2)

    async def test_external_adapter_handles_relay_auth_challenges(self) -> None:
        auth_relay = FakeNostrRelay(require_auth=True)
        await auth_relay.start()
        self.addAsyncCleanup(auth_relay.stop)
        node = await self.start_node()

        async with ProviderClient(node.base_url) as provider:
            descriptor = await provider.descriptor()

        adapter = FrogletNostrRelayAdapter.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            [auth_relay.url],
            provider_base_url=node.base_url,
            auth_seed_path=node.data_dir / "identity" / "nostr-publication.secp256k1.seed",
        )
        async with adapter:
            publish_results = await adapter.publish_provider_events()
            queried = await adapter.query_events([{"kinds": [30390, 30391]}])

        self.assertEqual(len(publish_results), 3)
        self.assertTrue(all(result.accepted for result in publish_results))
        self.assertGreaterEqual(len(queried), 3)
        self.assertGreaterEqual(len(auth_relay.auth_events), 2)
        self.assertTrue(all(verify_event(event) for event in auth_relay.auth_events))
        self.assertTrue(
            all(
                event["pubkey"] == descriptor["payload"]["linked_identities"][0]["identity"]
                for event in auth_relay.auth_events
            )
        )

    async def test_external_adapter_uses_relay_policy_roles(self) -> None:
        read_relay = FakeNostrRelay()
        await read_relay.start()
        self.addAsyncCleanup(read_relay.stop)
        node = await self.start_node()
        config_path = node.temp_root / "nostr-relays.json"

        adapter = FrogletNostrRelayAdapter.from_token_file(
            node.runtime_url,
            _runtime_token_path(node),
            [self.relay.url],
            provider_base_url=node.base_url,
        )
        async with adapter:
            provider_events = await adapter.collect_provider_events()

        read_relay.store_event(provider_events[0])
        config_path.write_text(
            json.dumps(
                {
                    "relays": [
                        {"url": self.relay.url, "read": False, "write": True},
                        {"url": read_relay.url, "read": True, "write": False},
                    ],
                    "retry": {
                        "max_attempts": 2,
                        "initial_backoff_secs": 0.01,
                        "max_backoff_secs": 0.02,
                    },
                }
            ),
            encoding="utf-8",
        )

        policy_adapter = FrogletNostrRelayAdapter.from_config_file(
            node.runtime_url,
            _runtime_token_path(node),
            config_path,
            provider_base_url=node.base_url,
        )
        async with policy_adapter:
            publish_results = await policy_adapter.publish_events(provider_events)
            queried = await policy_adapter.query_events(
                [{"kinds": [provider_events[0]["kind"]]}]
            )

        self.assertEqual(len(publish_results), len(provider_events))
        self.assertTrue(all(result.accepted for result in publish_results))
        self.assertEqual(len(self.relay.events), len(provider_events))
        self.assertEqual(list(read_relay.events), [provider_events[0]["id"]])
        self.assertEqual([event["id"] for event in queried], [provider_events[0]["id"]])

    async def test_external_adapter_requires_matching_read_write_roles(self) -> None:
        node = await self.start_node()

        write_disabled = FrogletNostrRelayAdapter(
            node.runtime_url,
            "test-token",
            [RelayPolicy(relay_url=self.relay.url, read=True, write=False)],
            provider_base_url=node.base_url,
        )
        async with write_disabled.runtime:
            with self.assertRaisesRegex(NostrRelayConfigError, "write-enabled relays"):
                await write_disabled.publish_events([{"id": "event-1"}])

        read_disabled = FrogletNostrRelayAdapter(
            node.runtime_url,
            "test-token",
            [RelayPolicy(relay_url=self.relay.url, read=False, write=True)],
            provider_base_url=node.base_url,
        )
        async with read_disabled.runtime:
            with self.assertRaisesRegex(NostrRelayConfigError, "read-enabled relays"):
                await read_disabled.query_events([{"kinds": [30390]}])

    async def test_external_adapter_cli_publish_and_query_commands_work(self) -> None:
        node = await self.start_node()

        publish_args = argparse.Namespace(
            runtime_url=node.runtime_url,
            provider_url=node.base_url,
            token_file=_runtime_token_path(node),
            relay=[self.relay.url],
            relay_config=None,
            auth_seed_file=None,
            retry_attempts=2,
            initial_backoff_secs=0.01,
            max_backoff_secs=0.02,
            command="publish-provider",
            deal_id=None,
            kind=[],
            author=[],
            limit=None,
        )
        publish_stdout = io.StringIO()
        with contextlib.redirect_stdout(publish_stdout):
            publish_rc = await _run_cli(publish_args)

        query_args = argparse.Namespace(
            runtime_url=node.runtime_url,
            provider_url=node.base_url,
            token_file=_runtime_token_path(node),
            relay=[self.relay.url],
            relay_config=None,
            auth_seed_file=None,
            retry_attempts=None,
            initial_backoff_secs=None,
            max_backoff_secs=None,
            command="query",
            deal_id=None,
            kind=[30390],
            author=[],
            limit=10,
        )
        query_stdout = io.StringIO()
        with contextlib.redirect_stdout(query_stdout):
            query_rc = await _run_cli(query_args)

        published = json.loads(publish_stdout.getvalue())
        queried = json.loads(query_stdout.getvalue())

        self.assertEqual(publish_rc, 0)
        self.assertEqual(query_rc, 0)
        self.assertEqual(len(published), 3)
        self.assertTrue(queried)


class NostrRelayAdapterHardeningTests(unittest.IsolatedAsyncioTestCase):
    async def test_nostr_relay_client_uses_explicit_session_timeouts(self) -> None:
        relay = NostrRelayClient("ws://127.0.0.1:9")
        session = await relay._ensure_session()
        try:
            self.assertIsNotNone(session.timeout.total)
            self.assertIsNotNone(session.timeout.connect)
            self.assertIsNotNone(session.timeout.sock_connect)
            self.assertIsNotNone(session.timeout.sock_read)
        finally:
            await relay.close()

    async def test_publish_events_limits_relay_concurrency(self) -> None:
        relay_policies = [
            RelayPolicy(relay_url=f"ws://relay-{index}.example")
            for index in range(4)
        ]
        adapter = FrogletNostrRelayAdapter(
            "http://127.0.0.1:8000",
            "token",
            relay_policies,
            provider_base_url="http://127.0.0.1:9000",
        )
        adapter._relay_concurrency_limit = 2

        in_flight = 0
        max_in_flight = 0

        async def fake_publish_with_retry(
            relay: NostrRelayClient,
            relay_policy: RelayPolicy,
            event: dict[str, object],
        ) -> RelayPublishResult:
            del relay
            nonlocal in_flight, max_in_flight
            in_flight += 1
            max_in_flight = max(max_in_flight, in_flight)
            try:
                await asyncio.sleep(0.05)
            finally:
                in_flight -= 1
            return RelayPublishResult(
                relay_url=relay_policy.relay_url,
                event_id=str(event["id"]),
                accepted=True,
                message="ok",
            )

        with mock.patch.object(adapter, "_publish_with_retry", side_effect=fake_publish_with_retry):
            results = await adapter.publish_events([{"id": "event-1"}])

        self.assertEqual(len(results), len(relay_policies))
        self.assertLessEqual(max_in_flight, 2)

    async def test_query_events_runs_relays_in_parallel(self) -> None:
        relay_policies = [
            RelayPolicy(relay_url="ws://slow-relay.example"),
            RelayPolicy(relay_url="ws://fast-relay.example"),
        ]
        adapter = FrogletNostrRelayAdapter(
            "http://127.0.0.1:8000",
            "token",
            relay_policies,
            provider_base_url="http://127.0.0.1:9000",
        )

        slow_started = asyncio.Event()
        fast_started = asyncio.Event()
        release_slow = asyncio.Event()

        async def fake_query_with_retry(
            relay: NostrRelayClient,
            relay_policy: RelayPolicy,
            filters: list[dict[str, object]],
        ) -> tuple[list[dict[str, object]], str | None]:
            del relay, filters
            if relay_policy.relay_url == "ws://slow-relay.example":
                slow_started.set()
                await release_slow.wait()
                return [{"id": "slow-event"}], None
            fast_started.set()
            return [{"id": "fast-event"}], None

        with mock.patch.object(adapter, "_query_with_retry", side_effect=fake_query_with_retry):
            query_task = asyncio.create_task(adapter.query_events([{"kinds": [30390]}]))
            await asyncio.wait_for(slow_started.wait(), timeout=1.0)
            await asyncio.wait_for(fast_started.wait(), timeout=1.0)
            release_slow.set()
            events = await asyncio.wait_for(query_task, timeout=1.0)

        self.assertEqual({event["id"] for event in events}, {"slow-event", "fast-event"})


class NostrRelayAdapterHelperTests(unittest.TestCase):
    def test_relay_config_helpers_validate_inputs(self) -> None:
        with self.assertRaisesRegex(ValueError, "relay_url must not be empty"):
            RelayPolicy(relay_url="   ")
        with self.assertRaisesRegex(ValueError, "relay policy must allow read, write, or both"):
            RelayPolicy(relay_url="ws://relay.example", read=False, write=False)
        with self.assertRaisesRegex(ValueError, "max_attempts must be at least 1"):
            RetryPolicy(max_attempts=0)
        with self.assertRaisesRegex(ValueError, "backoff values must be non-negative"):
            RetryPolicy(initial_backoff_secs=-0.1)
        with self.assertRaisesRegex(ValueError, "max_backoff_secs must be >="):
            RetryPolicy(initial_backoff_secs=1.0, max_backoff_secs=0.5)

        with tempfile.TemporaryDirectory(prefix="froglet-nostr-config-") as temp_root:
            temp_path = Path(temp_root)
            config_path = temp_path / "relays.json"
            config_path.write_text(
                json.dumps({"relays": [{"url": "ws://relay.example"}], "retry": []}),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "retry section must be an object"):
                RelayListConfig.from_json_file(config_path)

            seed_path = temp_path / "nostr.seed"
            seed_path.write_text("not-hex", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "invalid hex in Nostr auth seed"):
                NostrAuthSigner.from_seed_file(seed_path)

    def test_cli_parser_and_main_entrypoint(self) -> None:
        parser = _build_parser()
        args = parser.parse_args(
            [
                "--runtime-url",
                "http://127.0.0.1:9000",
                "--provider-url",
                "http://127.0.0.1:8000",
                "--token-file",
                "auth.token",
                "--relay",
                "ws://127.0.0.1:7000/",
                "query",
                "--kind",
                "30390",
                "--author",
                "f" * 64,
                "--limit",
                "5",
            ]
        )
        self.assertEqual(args.command, "query")
        self.assertEqual(args.kind, [30390])
        self.assertEqual(args.author, ["f" * 64])
        self.assertEqual(args.limit, 5)

        fake_parser = mock.Mock()
        fake_args = argparse.Namespace(command="publish-provider")
        fake_parser.parse_args.return_value = fake_args
        with mock.patch("froglet_nostr_adapter._build_parser", return_value=fake_parser), mock.patch(
            "froglet_nostr_adapter._run_cli",
            new=mock.AsyncMock(return_value=0),
        ) as run_cli:
            self.assertEqual(main(), 0)

        run_cli.assert_awaited_once_with(fake_args)


if __name__ == "__main__":
    import unittest

    unittest.main(verbosity=2)
