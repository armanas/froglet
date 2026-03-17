import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_WASM_HEX, build_wasm_request


class PaymentEnforcementTests(FrogletAsyncTestCase):
    async def test_priced_services_require_explicit_lightning_mode(self) -> None:
        with self.assertRaisesRegex(
            RuntimeError,
            "FROGLET_LIGHTNING_MODE is required whenever Lightning payments are active",
        ):
            await self.start_node(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})

    async def test_lightning_priced_query_requires_protocol_deal_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["note"], "limit": 1},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "events.query")
        self.assertEqual(payload["payment_backend"], "lightning")
        self.assertEqual(payload["quote_path"], "/v1/quotes")
        self.assertEqual(payload["deal_path"], "/v1/deals")

    async def test_lightning_priced_execute_helper_requires_protocol_deal_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json=build_wasm_request(VALID_WASM_HEX),
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "execute.wasm")
        self.assertEqual(payload["legacy_endpoint"], "/v1/node/execute/wasm")

    async def test_lightning_priced_job_helper_requires_protocol_deal_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={"idempotency_key": "legacy-job-helper", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "execute.wasm")
        self.assertEqual(payload["legacy_endpoint"], "/v1/node/jobs")


if __name__ == "__main__":
    unittest.main(verbosity=2)
