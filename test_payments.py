import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN


class PaymentEnforcementTests(FrogletAsyncTestCase):
    async def test_priced_query_requires_payment(self) -> None:
        node = await self.start_node(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["note"], "limit": 1},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 402)
        self.assertEqual(payload["service_id"], "events.query")
        self.assertEqual(payload["price_sats"], 10)
        self.assertEqual(payload["payment_kind"], "cashu")

    async def test_priced_query_accepts_valid_payment(self) -> None:
        node = await self.start_node(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})

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

    async def test_replayed_payment_token_is_rejected(self) -> None:
        node = await self.start_node(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
