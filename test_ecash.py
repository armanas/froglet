import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN


class EcashApiTests(FrogletAsyncTestCase):
    async def test_valid_cashu_token_returns_amount_and_hash(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/pay/ecash"),
                json={"token": VALID_CASHU_TOKEN},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["amount_satoshis"], 10)
        self.assertEqual(len(payload["token_hash"]), 64)

    async def test_invalid_cashu_token_is_rejected(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/pay/ecash"),
                json={"token": "cashuAinvalidbase64formatjunkstr"},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("error", payload)


if __name__ == "__main__":
    unittest.main(verbosity=2)
