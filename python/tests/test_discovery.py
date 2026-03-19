import asyncio
import shutil
import tempfile
import unittest
from pathlib import Path

import aiohttp

from test_support import FrogletAsyncTestCase


class DiscoveryIntegrationTests(FrogletAsyncTestCase):
    async def test_node_registers_and_reuses_same_identity_on_restart(self) -> None:
        discovery = await self.start_discovery()
        persistent_root = Path(tempfile.mkdtemp(prefix="froglet-discovery-node-"))
        self.addCleanup(lambda: shutil.rmtree(persistent_root, ignore_errors=True))
        data_dir = persistent_root / "data"

        provider_one = await self.start_provider(
            data_dir=data_dir,
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider_one.url("/v1/node/capabilities")) as resp:
                first_caps = await resp.json()
            async with session.post(discovery.url("/v1/discovery/search"), json={}) as resp:
                first_search = await resp.json()

        node_id = first_caps["identity"]["node_id"]
        self.assertTrue(first_caps["reference_discovery"]["connected"])
        self.assertTrue(any(node["descriptor"]["node_id"] == node_id for node in first_search["nodes"]))

        await provider_one.stop()
        replacement_port = None
        provider_two = await self.start_provider(
            port=replacement_port,
            data_dir=data_dir,
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
            },
        )

        await asyncio.sleep(1)
        async with aiohttp.ClientSession() as session:
            async with session.get(provider_two.url("/v1/node/capabilities")) as resp:
                second_caps = await resp.json()
            async with session.get(discovery.url("/v1/discovery/providers/" + node_id)) as resp:
                listing = await resp.json()

        self.assertEqual(second_caps["identity"]["node_id"], node_id)
        self.assertEqual(
            listing["descriptor"]["transports"]["clearnet_url"],
            provider_two.base_url,
        )

    async def test_stale_node_listing_is_reclaimed_on_restart(self) -> None:
        discovery = await self.start_discovery(
            extra_env={"FROGLET_DISCOVERY_STALE_AFTER_SECS": "2"}
        )
        persistent_root = Path(tempfile.mkdtemp(prefix="froglet-discovery-reclaim-"))
        self.addCleanup(lambda: shutil.rmtree(persistent_root, ignore_errors=True))
        data_dir = persistent_root / "data"

        provider_one = await self.start_provider(
            data_dir=data_dir,
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider_one.url("/v1/node/capabilities")) as resp:
                first_caps = await resp.json()

        node_id = first_caps["identity"]["node_id"]
        await provider_one.stop()
        await asyncio.sleep(3)

        async with aiohttp.ClientSession() as session:
            async with session.get(discovery.url(f"/v1/discovery/providers/{node_id}")) as resp:
                stale_listing = await resp.json()

        self.assertEqual(stale_listing["status"], "inactive")

        provider_two = await self.start_provider(
            data_dir=data_dir,
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider_two.url("/v1/node/capabilities")) as resp:
                second_caps = await resp.json()
            async with session.get(discovery.url(f"/v1/discovery/providers/{node_id}")) as resp:
                recovered_listing = await resp.json()

        self.assertTrue(second_caps["reference_discovery"]["connected"])
        self.assertEqual(second_caps["identity"]["node_id"], node_id)
        self.assertEqual(recovered_listing["status"], "active")
        self.assertEqual(
            recovered_listing["descriptor"]["transports"]["clearnet_url"],
            provider_two.base_url,
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
