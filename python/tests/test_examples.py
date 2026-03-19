import asyncio
import json
import subprocess
import sys
import unittest
from pathlib import Path

import aiohttp

from test_support import FrogletAsyncTestCase


REPO_ROOT = Path(__file__).resolve().parents[2]


class ExampleScriptTests(FrogletAsyncTestCase):
    async def _wait_for_provider_publication(
        self,
        discovery,
        provider_id: str,
        *,
        timeout_secs: float = 10.0,
    ) -> None:
        deadline = asyncio.get_running_loop().time() + timeout_secs
        async with aiohttp.ClientSession() as session:
            while asyncio.get_running_loop().time() < deadline:
                async with session.post(discovery.url("/v1/discovery/search"), json={}) as resp:
                    self.assertEqual(resp.status, 200)
                    payload = await resp.json()
                if any(
                    node.get("descriptor", {}).get("node_id") == provider_id
                    for node in payload["nodes"]
                ):
                    return
                await asyncio.sleep(0.2)
        raise TimeoutError(f"timed out waiting for provider {provider_id} in discovery")

    async def _run_example(self, script_name: str, *args: str) -> dict:
        script_path = REPO_ROOT / "examples" / script_name
        completed = await asyncio.to_thread(
            subprocess.run,
            [sys.executable, str(script_path), *args],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(completed.stdout)

    async def test_runtime_search_and_buy_example(self) -> None:
        discovery = await self.start_discovery()
        provider = await self.start_provider(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
                "FROGLET_PRICE_EXEC_WASM": "0",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()
        await self._wait_for_provider_publication(
            discovery, provider_caps["identity"]["node_id"]
        )

        output = await self._run_example(
            "runtime_search_and_buy.py",
            "--runtime-url",
            runtime.runtime_url,
            "--token-path",
            str(runtime.runtime_auth_token_path),
        )

        self.assertEqual(output["provider_id"], provider_caps["identity"]["node_id"])
        self.assertEqual(set(output["offer_ids"]), {"events.query", "execute.wasm"})
        self.assertTrue(output["deal_id"])
        self.assertIn(
            output["deal_status"], {"running", "accepted", "result_ready", "succeeded"}
        )

    async def test_runtime_search_and_inspect_example(self) -> None:
        discovery = await self.start_discovery()
        provider = await self.start_provider(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
                "FROGLET_DISCOVERY_PUBLISH": "true",
            }
        )
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "reference",
                "FROGLET_DISCOVERY_URL": discovery.base_url,
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()
        await self._wait_for_provider_publication(
            discovery, provider_caps["identity"]["node_id"]
        )

        output = await self._run_example(
            "runtime_search_and_inspect.py",
            "--runtime-url",
            runtime.runtime_url,
            "--token-path",
            str(runtime.runtime_auth_token_path),
        )

        self.assertEqual(output["provider_id"], provider_caps["identity"]["node_id"])
        self.assertTrue(output["descriptor_hash"])
        self.assertGreaterEqual(len(output["offer_ids"]), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
