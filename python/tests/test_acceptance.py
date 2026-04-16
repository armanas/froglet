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
    bearer_auth_headers,
    create_signed_event,
    create_protocol_quote,
    create_protocol_deal,
    default_success_payment_hash,
    generate_schnorr_signing_key,
    provider_control_auth_token_path,
    remote_stack_enabled,
    remote_stack_url,
    schnorr_pubkey_hex,
    workload_hash_from_submission,
)


class AcceptanceTests(FrogletAsyncTestCase):
    """UAT scenarios validating Froglet business requirements."""

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
                node.runtime.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                self.assertIn(resp.status, (200, 202))
                result = await resp.json()
        elapsed = time.perf_counter() - start

        self.assertLess(elapsed, 5.0, f"WASM compute took {elapsed:.2f}s (SLA: 5s)")
        if resp.status == 200:
            self.assertIn("result", result)

    # -----------------------------------------------------------------------
    # UAT-3: Priced service enforces the quote → deal payment path
    # -----------------------------------------------------------------------
    async def test_uat3_priced_service_requires_protocol_deal(self) -> None:
        """When a service has a price, a raw invoke returns 409 with the
        correct quote_path and deal_path, and a proper quote can be obtained."""
        provider_extra_env = {
            "FROGLET_PRICE_EXEC_WASM": "10",
            "FROGLET_PAYMENT_BACKEND": "lightning",
            "FROGLET_LIGHTNING_MODE": "mock",
        }
        provider = await self.start_provider(
            extra_env=None if remote_stack_enabled() else provider_extra_env
        )
        runtime = await self.start_runtime(extra_env=provider_extra_env)
        remote_provider_url = remote_stack_url("FROGLET_TEST_PROVIDER_URL") if remote_stack_enabled() else None
        if remote_provider_url and provider.base_url == remote_provider_url:
            priced_service_id = f"uat-priced-compute-{int(time.time() * 1000)}"
            payload = {
                "service_id": "execute.compute",
                "offer_id": "execute.compute",
                "summary": f"UAT-3 priced compute ({priced_service_id})",
                "price_sats": 10,
                "publication_state": "active",
                "runtime": "wasm",
                "package_kind": "inline_module",
                "entrypoint_kind": "handler",
                "entrypoint": "run",
                "contract_version": "froglet.wasm.run_json.v1",
                "wasm_module_hex": VALID_WASM_HEX,
            }
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{provider.base_url}/v1/provider/artifacts/publish",
                    headers=bearer_auth_headers(
                        provider_control_auth_token_path(provider.data_dir),
                    ),
                    json=payload,
                ) as resp:
                    self.assertEqual(resp.status, 201)
                    published = await resp.json()
            published_service_id = (
                published.get("service", {}).get("service_id")
                or published.get("evidence", {}).get("service_id")
                or published.get("project", {}).get("service_id")
            )
            self.assertEqual(published_service_id, "execute.compute")

        request = build_wasm_request(VALID_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            # Step 1: Raw invoke is rejected with 409
            async with session.post(
                runtime.url("/v1/node/execute/wasm"),
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
                node.runtime.url("/v1/node/execute/wasm"),
                json=request,
            ) as resp:
                result = await resp.json()

        # Should fail gracefully, not 500
        if resp.status == 200:
            self.assertIn(result.get("status", ""), ("failed", "error"))
        else:
            self.assertIn(resp.status, (400, 422))

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
