"""Black box API testing — tests the public API surface with zero knowledge
of internals.  Only uses aiohttp against the running node; no internal
module imports beyond test_support for binary management.
"""

import asyncio
import json
import time
import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    create_signed_event,
)


class BlackBoxApiTests(FrogletAsyncTestCase):
    """Tests the Froglet provider/runtime API as an external client would."""

    async def test_health_endpoints(self) -> None:
        """All services expose /health returning 200."""
        node = await self.start_node()
        async with aiohttp.ClientSession() as session:
            for url in [node.url("/health"), node.runtime.url("/health")]:
                async with session.get(url) as resp:
                    self.assertEqual(resp.status, 200)

    async def test_provider_descriptor(self) -> None:
        """GET /v1/provider/descriptor returns a valid descriptor."""
        node = await self.start_node()
        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/provider/descriptor")) as resp:
                self.assertEqual(resp.status, 200)
                descriptor = await resp.json()

        self.assertIn("payload", descriptor)
        self.assertIn("transport_endpoints", descriptor["payload"])
        self.assertIn("node_id", descriptor["payload"])

    async def test_publish_query_roundtrip(self) -> None:
        """Publish an event and retrieve it via query."""
        node = await self.start_node()
        event = create_signed_event("blackbox-roundtrip", kind="blackbox.test")

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                json={"event": event},
            ) as resp:
                self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["blackbox.test"], "limit": 10},
            ) as resp:
                self.assertEqual(resp.status, 200)
                result = await resp.json()

        events = result.get("events", [])
        self.assertTrue(
            any(e["content"] == "blackbox-roundtrip" for e in events),
            f"Published event not found in query results: {events}",
        )

    async def test_concurrent_publish(self) -> None:
        """20 parallel publishes all succeed."""
        node = await self.start_node()

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=20)
        ) as session:
            async def publish(i: int) -> int:
                event = create_signed_event(f"concurrent-{i}", kind="blackbox.concurrent")
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    json={"event": event},
                ) as resp:
                    return resp.status

            statuses = await asyncio.gather(*(publish(i) for i in range(20)))

        self.assertTrue(all(s == 201 for s in statuses), f"Some publishes failed: {statuses}")

    async def test_query_empty_result(self) -> None:
        """Querying a nonexistent kind returns empty results, not an error."""
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["nonexistent.kind.12345"], "limit": 5},
            ) as resp:
                self.assertEqual(resp.status, 200)
                result = await resp.json()

        events = result.get("events", [])
        self.assertEqual(len(events), 0)

    async def test_wasm_compute_execution(self) -> None:
        """Submit a WASM module for execution and verify result."""
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                self.assertIn(resp.status, (200, 202), f"Unexpected status: {resp.status}")
                result = await resp.json()

        # The valid WASM module returns output "42"
        if resp.status == 200:
            self.assertIn("output", result)

    async def test_invalid_event_rejected(self) -> None:
        """Publishing an event with invalid signature is rejected with 400."""
        node = await self.start_node()
        event = create_signed_event("will-tamper")
        event["content"] = "tampered content"  # Invalidates signature

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                json={"event": event},
            ) as resp:
                self.assertEqual(resp.status, 400)
                payload = await resp.json()
                self.assertIn("error", payload)

    async def test_concurrent_query_under_load(self) -> None:
        """Seed data then run 20 parallel queries — all must succeed."""
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            # Seed
            for i in range(10):
                event = create_signed_event(f"load-{i}", kind="blackbox.load")
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    json={"event": event},
                ) as resp:
                    self.assertEqual(resp.status, 201)

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=20)
        ) as session:
            async def query() -> int:
                async with session.post(
                    node.url("/v1/node/events/query"),
                    json={"kinds": ["blackbox.load"], "limit": 10},
                ) as resp:
                    return resp.status

            statuses = await asyncio.gather(*(query() for _ in range(20)))

        self.assertTrue(all(s == 200 for s in statuses), f"Some queries failed: {statuses}")

    async def test_method_not_allowed(self) -> None:
        """GET on a POST-only endpoint returns 405 or 404, not 500."""
        node = await self.start_node()
        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/node/events/publish")) as resp:
                self.assertNotEqual(resp.status, 500)
                self.assertIn(resp.status, (404, 405))


if __name__ == "__main__":
    unittest.main(verbosity=2)
