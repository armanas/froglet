import stat
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase


class PrivacyAndOpsecTests(FrogletAsyncTestCase):
    async def test_http_headers_are_fixed_and_generic(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/health")) as resp:
                headers = resp.headers
                body = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(headers.get("Server"), "nginx/1.18.0")
        self.assertIsNotNone(headers.get("Date"))
        self.assertEqual(body["status"], "ok")
        self.assertEqual(body["service"], "froglet")

    async def test_data_at_rest_permissions_are_restricted(self) -> None:
        node = await self.start_node()
        db_path = node.data_dir / "node.db"
        identity_dir = node.data_dir / "identity"
        seed_path = identity_dir / "secp256k1.seed"

        self.assertTrue(db_path.exists(), db_path)
        self.assertTrue(identity_dir.exists(), identity_dir)
        self.assertTrue(seed_path.exists(), seed_path)

        db_mode = stat.S_IMODE(db_path.stat().st_mode)
        identity_dir_mode = stat.S_IMODE(identity_dir.stat().st_mode)
        seed_mode = stat.S_IMODE(seed_path.stat().st_mode)

        self.assertEqual(db_mode, 0o600)
        self.assertEqual(identity_dir_mode, 0o700)
        self.assertEqual(seed_mode, 0o600)

    async def test_capabilities_expose_only_local_clearnet_transport_when_tor_is_disabled(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/node/capabilities")) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["transports"]["clearnet"]["url"], node.base_url)
        self.assertFalse(payload["transports"]["tor"]["enabled"])
        self.assertEqual(payload["transports"]["tor"]["status"], "disabled")


if __name__ == "__main__":
    unittest.main(verbosity=2)
