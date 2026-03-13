import asyncio
import os
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase


@unittest.skipUnless(
    os.getenv("FROGLET_RUN_TOR_INTEGRATION") == "1",
    "requires FROGLET_RUN_TOR_INTEGRATION=1 and outbound Tor bootstrap access",
)
class TorIntegrationTests(FrogletAsyncTestCase):
    async def test_dual_mode_advertises_matching_tor_transport_metadata(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_NETWORK_MODE": "dual",
            }
        )

        descriptor = None
        capabilities = None
        deadline = asyncio.get_running_loop().time() + 90.0

        async with aiohttp.ClientSession() as session:
            while asyncio.get_running_loop().time() < deadline:
                async with session.get(node.url("/v1/descriptor")) as resp:
                    self.assertEqual(resp.status, 200)
                    descriptor = await resp.json()

                async with session.get(node.url("/v1/node/capabilities")) as resp:
                    self.assertEqual(resp.status, 200)
                    capabilities = await resp.json()

                tor_status = capabilities["transports"]["tor"]["status"]
                onion_url = capabilities["transports"]["tor"].get("onion_url")
                if tor_status == "up" and onion_url:
                    break
                await asyncio.sleep(1.0)
            else:
                self.fail("timed out waiting for Tor transport to become available")

            async with session.get(node.url("/v1/offers")) as resp:
                self.assertEqual(resp.status, 200)
                offers = await resp.json()

            async with session.get(node.url("/v1/feed?limit=5")) as resp:
                self.assertEqual(resp.status, 200)
                feed = await resp.json()

        self.assertIsNotNone(descriptor)
        self.assertIsNotNone(capabilities)
        self.assertEqual(capabilities["transports"]["clearnet"]["url"], node.base_url)
        self.assertTrue(capabilities["transports"]["tor"]["enabled"])
        self.assertEqual(capabilities["transports"]["tor"]["status"], "up")
        self.assertTrue(
            capabilities["transports"]["tor"]["onion_url"].startswith("http://")
        )
        self.assertIn(".onion", capabilities["transports"]["tor"]["onion_url"])

        descriptor_transports = {
            endpoint["transport"]: endpoint
            for endpoint in descriptor["payload"]["transport_endpoints"]
        }
        self.assertEqual(
            descriptor_transports["https"]["uri"],
            capabilities["transports"]["clearnet"]["url"],
        )
        self.assertEqual(
            descriptor_transports["tor"]["uri"],
            capabilities["transports"]["tor"]["onion_url"],
        )
        self.assertIn("receipt_poll", descriptor_transports["tor"]["features"])
        self.assertNotEqual(
            descriptor["signer"], descriptor_transports["tor"]["uri"]
        )
        self.assertEqual(len(offers["offers"]), 2)
        self.assertGreaterEqual(len(feed["artifacts"]), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
