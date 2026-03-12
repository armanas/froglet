import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN, VALID_WASM_HEX, build_wasm_request


class PaymentEnforcementTests(FrogletAsyncTestCase):
    async def test_priced_services_default_to_lightning_backend(self) -> None:
        node = await self.start_node(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/node/capabilities")) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["payments"]["backend"], "lightning")
        self.assertEqual(payload["payments"]["accepted_payment_methods"], ["lightning"])

    async def test_priced_query_requires_payment_in_legacy_cashu_mode(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["note"], "limit": 1},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 402)
        self.assertEqual(payload["service_id"], "events.query")
        self.assertEqual(payload["price_sats"], 10)
        self.assertEqual(payload["accepted_payment_methods"], ["cashu"])

    async def test_priced_query_accepts_valid_payment_in_legacy_cashu_mode(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/query"),
                json={
                    "kinds": ["note"],
                    "limit": 1,
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertIn("events", payload)
        self.assertEqual(payload["payment_receipt"]["settlement_status"], "committed")

    async def test_replayed_payment_token_is_rejected_in_legacy_cashu_mode(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )
        request = {
            "kinds": ["note"],
            "limit": 1,
            "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/events/query"), json=request) as resp:
                first_payload = await resp.json()
            async with session.post(node.url("/v1/node/events/query"), json=request) as resp:
                second_payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertIn("token_hash", second_payload)
        self.assertIn("events", first_payload)

    async def test_lightning_priced_query_requires_protocol_deal_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
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
