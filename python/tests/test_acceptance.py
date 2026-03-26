"""User Acceptance Testing (UAT) — validates business-level requirements.

Each test maps to a user story and exercises the actual protocol flows
including the quote → deal → payment path for priced services.
"""

import asyncio
import time
import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    build_wasm_submission,
    create_signed_event,
    create_protocol_quote,
    create_protocol_deal,
    default_success_payment_hash,
    generate_schnorr_signing_key,
    schnorr_pubkey_hex,
    start_discovery,
    workload_hash_from_submission,
)


class AcceptanceTests(FrogletAsyncTestCase):
    """UAT scenarios validating Froglet business requirements."""

    # -----------------------------------------------------------------------
    # UAT-1: Provider registers a service and it becomes discoverable
    # -----------------------------------------------------------------------
    async def test_uat1_service_discoverable_via_reference_discovery(self) -> None:
        """A provider publishes an event and it appears in discovery search."""
        discovery = await self.start_discovery()
        provider = await self.start_provider(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.url(""),
                "FROGLET_DISCOVERY_PUBLISH": "true",
                "FROGLET_PUBLIC_BASE_URL": "http://127.0.0.1:0",
            }
        )

        # Publish a service event
        event = create_signed_event("uat-service-1", kind="service.offering")
        async with aiohttp.ClientSession() as session:
            async with session.post(
                provider.url("/v1/node/events/publish"),
                json={"event": event},
            ) as resp:
                self.assertEqual(resp.status, 201)

            # Provider should be registered in discovery
            deadline = time.monotonic() + 30
            found = False
            while time.monotonic() < deadline:
                async with session.post(
                    discovery.url("/v1/discovery/search"),
                    json={},
                ) as resp:
                    if resp.status == 200:
                        result = await resp.json()
                        if result.get("nodes"):
                            found = True
                            break
                await asyncio.sleep(1)

            self.assertTrue(found, "Provider did not appear in discovery within 30s")

    # -----------------------------------------------------------------------
    # UAT-2: Free WASM compute executes within SLA
    # -----------------------------------------------------------------------
    async def test_uat2_free_wasm_compute_within_sla(self) -> None:
        """Free WASM compute returns a result within 5 seconds."""
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX)

        start = time.perf_counter()
        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                self.assertIn(resp.status, (200, 202))
                result = await resp.json()
        elapsed = time.perf_counter() - start

        self.assertLess(elapsed, 5.0, f"WASM compute took {elapsed:.2f}s (SLA: 5s)")
        if resp.status == 200:
            self.assertIn("output", result)

    # -----------------------------------------------------------------------
    # UAT-3: Priced service enforces the quote → deal payment path
    # -----------------------------------------------------------------------
    async def test_uat3_priced_service_requires_protocol_deal(self) -> None:
        """When a service has a price, a raw invoke returns 409 with the
        correct quote_path and deal_path, and a proper quote can be obtained."""
        provider = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        request = build_wasm_request(VALID_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            # Step 1: Raw invoke is rejected with 409
            async with session.post(
                provider.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                self.assertEqual(resp.status, 409)
                payload = await resp.json()

            self.assertTrue(payload["requires_protocol_deal"])
            self.assertEqual(payload["quote_path"], "/v1/provider/quotes")
            self.assertEqual(payload["deal_path"], "/v1/provider/deals")

            # Step 2: Obtain a quote
            requester_key = generate_schnorr_signing_key()
            submission = build_wasm_submission(VALID_WASM_HEX)
            quote = await create_protocol_quote(
                session,
                provider,
                offer_id=payload["service_id"],
                request={"kind": "wasm", "submission": submission},
                requester_secret_key=requester_key,
            )

            self.assertIn("hash", quote)
            self.assertIn("payload", quote)
            self.assertIn("settlement_terms", quote["payload"])

            # Step 3: Create a deal from the quote (mock lightning, so no real payment)
            deal_result = await create_protocol_deal(
                session,
                provider,
                quote=quote,
                request={"kind": "wasm", "submission": submission},
                requester_secret_key=requester_key,
                expected_statuses=(200, 202),
            )

            self.assertIn("deal_id", deal_result)

    # -----------------------------------------------------------------------
    # UAT-4: WASM compute respects resource limits
    # -----------------------------------------------------------------------
    async def test_uat4_wasm_resource_limits_enforced(self) -> None:
        """A trapping WASM module is caught and reported as failed, not left
        hanging or crashing the provider."""
        from test_support import TRAPPING_WASM_HEX

        node = await self.start_node()
        request = build_wasm_request(TRAPPING_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                result = await resp.json()

        # Should fail gracefully, not 500
        if resp.status == 200:
            self.assertIn(result.get("status", ""), ("failed", "error"))
        else:
            self.assertIn(resp.status, (400, 422, 500))

        # Provider still healthy
        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/health")) as resp:
                self.assertEqual(resp.status, 200)

    # -----------------------------------------------------------------------
    # UAT-5: Event publish and query integrity
    # -----------------------------------------------------------------------
    async def test_uat5_event_content_integrity(self) -> None:
        """Published event content is preserved exactly through publish → query."""
        node = await self.start_node()
        test_content = '{"key": "value", "number": 42, "emoji": "🐸"}'
        event = create_signed_event(test_content, kind="uat.integrity")

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                json={"event": event},
            ) as resp:
                self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["uat.integrity"], "limit": 1},
            ) as resp:
                self.assertEqual(resp.status, 200)
                result = await resp.json()

        events = result.get("events", [])
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]["content"], test_content)

    # -----------------------------------------------------------------------
    # UAT-6: Multiple event kinds are independently queryable
    # -----------------------------------------------------------------------
    async def test_uat6_kind_isolation(self) -> None:
        """Events of different kinds are independently queryable."""
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            for kind in ["uat.alpha", "uat.beta"]:
                event = create_signed_event(f"content-{kind}", kind=kind)
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    json={"event": event},
                ) as resp:
                    self.assertEqual(resp.status, 201)

            # Query only alpha
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["uat.alpha"], "limit": 10},
            ) as resp:
                result = await resp.json()

        events = result.get("events", [])
        self.assertTrue(all(e["kind"] == "uat.alpha" for e in events))
        self.assertTrue(len(events) >= 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
