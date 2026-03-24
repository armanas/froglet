import asyncio
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock

from aiohttp import web

from froglet_client import (
    DiscoveryClient,
    FrogletClientError,
    ProviderClient,
    RuntimeClient,
    decrypt_confidential_envelope,
    encrypt_confidential_payload,
    generate_confidential_keypair,
)


def listening_port(site: web.TCPSite) -> int:
    server = getattr(site, "_server", None)
    sockets = getattr(server, "sockets", None)
    if not sockets:
        raise RuntimeError("test server did not expose a bound socket")
    return int(sockets[0].getsockname()[1])


class ClientSdkTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self._runner = None
        self._site = None

    async def asyncTearDown(self) -> None:
        if self._runner is not None:
            await self._runner.cleanup()

    async def start_server(self, handler) -> str:
        app = web.Application()
        app.router.add_route("*", "/{tail:.*}", handler)
        self._runner = web.AppRunner(app)
        await self._runner.setup()
        self._site = web.TCPSite(self._runner, "127.0.0.1", 0)
        await self._site.start()
        return f"http://127.0.0.1:{listening_port(self._site)}"

    async def test_provider_client_uses_provider_namespace(self) -> None:
        seen: list[tuple[str, str]] = []

        async def handler(request: web.Request) -> web.StreamResponse:
            seen.append((request.method, request.path))
            if request.method == "GET" and request.path == "/v1/provider/descriptor":
                return web.json_response({"payload": {"provider_id": "provider-1"}})
            if request.method == "GET" and request.path == "/v1/provider/offers":
                return web.json_response({"offers": [{"payload": {"offer_id": "execute.compute"}}]})
            if request.method == "POST" and request.path == "/v1/provider/quotes":
                return web.json_response(
                    {
                        "hash": "quote-1",
                        "payload": {
                            "provider_id": "provider-1",
                            "requester_id": "runtime-1",
                            "offer_hash": "offer-1",
                            "workload_hash": "workload-1",
                            "settlement_terms": {
                                "method": "lightning.base_fee_plus_success_fee.v1",
                                "base_fee_msat": 0,
                                "success_fee_msat": 10_000,
                                "max_base_invoice_expiry_secs": 30,
                                "max_success_hold_expiry_secs": 30,
                                "min_final_cltv_expiry": 18,
                            },
                            "execution_limits": {
                                "max_input_bytes": 1,
                                "max_runtime_ms": 1000,
                                "max_memory_bytes": 1,
                                "max_output_bytes": 1,
                                "fuel_limit": 1,
                            },
                            "expires_at": 9999999999,
                            "workload_kind": "compute.wasm.v1",
                        },
                    },
                    status=201,
                )
            if request.method == "POST" and request.path == "/v1/provider/deals":
                return web.json_response(
                    {
                        "deal_id": "deal-1",
                        "status": "payment_pending",
                        "quote": {"hash": "quote-1"},
                        "deal": {"hash": "deal-1"},
                    }
                )
            if request.method == "GET" and request.path == "/v1/provider/deals/deal-1":
                return web.json_response(
                    {
                        "deal_id": "deal-1",
                        "status": "result_ready",
                        "quote": {"hash": "quote-1"},
                        "deal": {"hash": "deal-1"},
                    }
                )
            if request.method == "POST" and request.path == "/v1/provider/deals/deal-1/accept":
                return web.json_response(
                    {
                        "deal_id": "deal-1",
                        "status": "succeeded",
                        "receipt": {"hash": "receipt-1"},
                    }
                )
            if request.method == "GET" and request.path == "/v1/provider/deals/deal-1/invoice-bundle":
                return web.json_response({"session_id": "bundle-1"})
            raise AssertionError(f"unexpected request {request.method} {request.path}")

        base_url = await self.start_server(handler)
        async with ProviderClient(base_url) as provider:
            descriptor = await provider.descriptor()
            offers = await provider.offers()
            quote = await provider.create_quote(
                "execute.compute",
                {"submission": {"wasm_module_hex": "00"}},
                requester_id="runtime-1",
            )
            deal = await provider.create_deal(
                quote,
                {"hash": "deal-1"},
                {"submission": {"wasm_module_hex": "00"}},
            )
            current = await provider.get_deal("deal-1")
            bundle = await provider.get_invoice_bundle("deal-1")
            terminal = await provider.accept_result("deal-1", "11" * 32)

        self.assertEqual(descriptor["payload"]["provider_id"], "provider-1")
        self.assertEqual(offers[0]["payload"]["offer_id"], "execute.compute")
        self.assertEqual(deal["status"], "payment_pending")
        self.assertEqual(current["status"], "result_ready")
        self.assertEqual(bundle["session_id"], "bundle-1")
        self.assertEqual(terminal["status"], "succeeded")
        self.assertIn(("GET", "/v1/provider/descriptor"), seen)
        self.assertIn(("POST", "/v1/provider/quotes"), seen)

    async def test_runtime_client_uses_runtime_only_surface(self) -> None:
        seen: list[tuple[str, str, str | None]] = []

        async def handler(request: web.Request) -> web.StreamResponse:
            seen.append(
                (
                    request.method,
                    request.path,
                    request.headers.get("Authorization"),
                )
            )
            if request.path == "/v1/runtime/wallet/balance":
                return web.json_response(
                    {
                        "backend": "lightning",
                        "mode": "mock",
                        "balance_known": True,
                        "balance_sats": 21,
                        "accepted_payment_methods": ["lightning"],
                        "reservations": True,
                        "receipts": True,
                    }
                )
            if request.path == "/v1/runtime/search":
                return web.json_response({"nodes": [{"descriptor": {"node_id": "provider-1"}}]})
            if request.path == "/v1/runtime/providers/provider-1":
                return web.json_response(
                    {
                        "discovery": {"descriptor": {"node_id": "provider-1"}},
                        "descriptor": {"payload": {"provider_id": "provider-1"}},
                        "offers": [{"payload": {"offer_id": "execute.compute"}}],
                    }
                )
            if request.path == "/v1/runtime/deals" and request.method == "POST":
                return web.json_response(
                    {
                        "quote": {"hash": "quote-1"},
                        "deal": {
                            "deal_id": "deal-1",
                            "status": "payment_pending",
                            "provider_id": "provider-1",
                            "provider_url": "https://provider.example",
                            "receipt": None,
                            "result_hash": None,
                        },
                        "payment_intent_path": "/v1/runtime/deals/deal-1/payment-intent",
                        "payment_intent": {"deal_id": "deal-1", "bundle_hash": "bundle-1"},
                    }
                )
            if request.path == "/v1/runtime/deals/deal-1" and request.method == "GET":
                return web.json_response(
                    {
                        "deal": {
                            "deal_id": "deal-1",
                            "status": "result_ready",
                            "provider_id": "provider-1",
                            "provider_url": "https://provider.example",
                            "receipt": None,
                            "result_hash": "ab" * 32,
                        }
                    }
                )
            if request.path == "/v1/runtime/deals/deal-1/payment-intent":
                return web.json_response(
                    {
                        "payment_intent": {
                            "deal_id": "deal-1",
                            "bundle_hash": "bundle-1",
                            "release_action": {
                                "endpoint_path": "/v1/runtime/deals/deal-1/accept",
                                "expected_result_hash": "ab" * 32,
                            },
                        }
                    }
                )
            if request.path == "/v1/runtime/deals/deal-1/accept":
                return web.json_response(
                    {
                        "deal": {
                            "deal_id": "deal-1",
                            "status": "succeeded",
                            "provider_id": "provider-1",
                            "provider_url": "https://provider.example",
                            "receipt": {"hash": "receipt-1"},
                            "result_hash": "ab" * 32,
                        }
                    }
                )
            raise AssertionError(f"unexpected request {request.method} {request.path}")

        runtime_url = await self.start_server(handler)
        with TemporaryDirectory(prefix="froglet-runtime-client-") as temp_dir:
            token_path = Path(temp_dir) / "auth.token"
            token_path.write_text("runtime-test-token\n", encoding="utf-8")
            async with RuntimeClient.from_token_file(runtime_url, token_path) as runtime:
                wallet = await runtime.wallet_balance()
                nodes = await runtime.search(limit=10)
                provider = await runtime.get_provider("provider-1")
                handle = await runtime.buy_service(
                    {
                        "provider": {"provider_id": "provider-1"},
                        "offer_id": "execute.compute",
                        "submission": {"wasm_module_hex": "00"},
                    },
                    include_payment_intent=True,
                )
                deal = await runtime.get_deal("deal-1")
                waited = await runtime.wait_for_deal(
                    "deal-1", statuses={"result_ready"}, timeout_secs=0.5, poll_interval_secs=0.01
                )
                intent = await runtime.payment_intent("deal-1")
                terminal = await runtime.accept_result("deal-1")

        self.assertEqual(wallet["balance_sats"], 21)
        self.assertEqual(nodes[0]["descriptor"]["node_id"], "provider-1")
        self.assertEqual(provider["descriptor"]["payload"]["provider_id"], "provider-1")
        self.assertFalse(handle.terminal)
        self.assertEqual(handle.payment_intent["bundle_hash"], "bundle-1")
        self.assertEqual(deal["status"], "result_ready")
        self.assertEqual(waited["status"], "result_ready")
        self.assertEqual(intent["deal_id"], "deal-1")
        self.assertEqual(terminal["status"], "succeeded")
        self.assertTrue(all(path.startswith("/v1/runtime/") for _, path, _ in seen))
        self.assertTrue(all(auth == "Bearer runtime-test-token" for _, _, auth in seen))

    async def test_discovery_client_uses_search_post_and_provider_lookup(self) -> None:
        seen: list[tuple[str, str]] = []

        async def handler(request: web.Request) -> web.StreamResponse:
            seen.append((request.method, request.path))
            if request.method == "POST" and request.path == "/v1/discovery/search":
                return web.json_response({"nodes": [{"descriptor": {"node_id": "provider-1"}}]})
            if request.method == "GET" and request.path == "/v1/discovery/providers/provider-1":
                return web.json_response({"descriptor": {"node_id": "provider-1"}})
            raise AssertionError(f"unexpected request {request.method} {request.path}")

        base_url = await self.start_server(handler)
        async with DiscoveryClient(base_url) as client:
            nodes = await client.search_nodes(limit=5)
            node = await client.get_node("provider-1")

        self.assertEqual(nodes[0]["descriptor"]["node_id"], "provider-1")
        self.assertEqual(node["descriptor"]["node_id"], "provider-1")
        self.assertEqual(seen, [
            ("POST", "/v1/discovery/search"),
            ("GET", "/v1/discovery/providers/provider-1"),
        ])

    async def test_confidential_helpers_encrypt_verify_and_decrypt(self) -> None:
        requester = generate_confidential_keypair()
        provider = generate_confidential_keypair()
        session_hash = "ab" * 32
        payload = {"prompt": "hello"}

        envelope = encrypt_confidential_payload(
            session_hash,
            requester["private_key_hex"],
            provider["public_key_hex"],
            payload,
        )
        decrypted = decrypt_confidential_envelope(
            session_hash,
            provider["private_key_hex"],
            requester["public_key_hex"],
            envelope,
            expected_direction="request",
        )

        self.assertEqual(decrypted, payload)


class ClientSdkHardeningTests(unittest.IsolatedAsyncioTestCase):
    async def test_non_json_error_bodies_raise_normalized_froglet_error(self) -> None:
        app = web.Application()

        async def handler(_request: web.Request) -> web.StreamResponse:
            return web.Response(status=502, text="gateway exploded", content_type="text/plain")

        app.router.add_route("*", "/{tail:.*}", handler)
        runner = web.AppRunner(app)
        await runner.setup()
        site = web.TCPSite(runner, "127.0.0.1", 0)
        await site.start()
        base_url = f"http://127.0.0.1:{listening_port(site)}"
        try:
            async with ProviderClient(base_url) as provider:
                with self.assertRaises(FrogletClientError) as raised:
                    await provider.descriptor()
        finally:
            await runner.cleanup()

        self.assertEqual(raised.exception.status, 502)
        self.assertEqual(raised.exception.payload["error"], "non_json_error_response")
        self.assertIn("gateway exploded", raised.exception.payload["body"])

    async def test_sdk_client_sessions_use_explicit_timeouts(self) -> None:
        async with ProviderClient("http://127.0.0.1:1") as provider:
            session = await provider._ensure_session()
            timeout = session.timeout

        self.assertEqual(timeout.total, 30.0)
        self.assertEqual(timeout.connect, 5.0)
        self.assertEqual(timeout.sock_connect, 5.0)
        self.assertEqual(timeout.sock_read, 30.0)

    async def test_wait_for_deal_uses_capped_backoff_with_jitter(self) -> None:
        client = RuntimeClient("http://runtime.invalid", "token")
        statuses = [
            {"deal_id": "deal-1", "status": "payment_pending"},
            {"deal_id": "deal-1", "status": "payment_pending"},
            {"deal_id": "deal-1", "status": "succeeded"},
        ]

        async def fake_get_deal(_deal_id: str) -> dict:
            return statuses.pop(0)

        with mock.patch.object(client, "get_deal", side_effect=fake_get_deal), mock.patch(
            "froglet_client.random.uniform",
            side_effect=lambda low, high: (low + high) / 2,
        ), mock.patch("asyncio.sleep", new=mock.AsyncMock()) as sleep_mock:
            terminal = await client.wait_for_deal(
                "deal-1",
                statuses={"succeeded"},
                timeout_secs=2.0,
                poll_interval_secs=0.1,
            )

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(sleep_mock.await_count, 2)
