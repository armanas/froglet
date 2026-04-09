import asyncio
import os
import shutil
import stat
import tempfile
import unittest
from pathlib import Path

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    reserve_tcp_port,
)


def runtime_auth_headers(runtime) -> dict[str, str]:
    token = runtime.runtime_auth_token_path.read_text(encoding="utf-8").strip()
    return {"Authorization": f"Bearer {token}"}


class RuntimeApiTests(FrogletAsyncTestCase):
    async def start_runtime_for_local_provider(
        self, provider, *, extra_env: dict[str, str] | None = None
    ):
        temp_root = Path(tempfile.mkdtemp(prefix="froglet-runtime-local-provider-"))
        self.addCleanup(shutil.rmtree, temp_root, ignore_errors=True)
        data_dir = temp_root / "data"
        identity_dir = data_dir / "identity"
        identity_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(
            provider.data_dir / "identity" / "secp256k1.seed",
            identity_dir / "secp256k1.seed",
        )
        return await self.start_runtime(data_dir=data_dir, extra_env=extra_env)

    async def start_marketplace_backed_stack(
        self,
        *,
        provider_extra_env: dict[str, str] | None = None,
        runtime_extra_env: dict[str, str] | None = None,
    ):
        provider_port = reserve_tcp_port()
        provider_url = f"http://127.0.0.1:{provider_port}"
        marketplace = await self.start_marketplace(feed_sources=[provider_url])
        provider_env = {"FROGLET_MARKETPLACE_URL": marketplace.base_url}
        if provider_extra_env:
            provider_env.update(provider_extra_env)
        provider = await self.start_provider(port=provider_port, extra_env=provider_env)
        runtime_env = {
            "FROGLET_MARKETPLACE_URL": marketplace.base_url,
            "FROGLET_RUNTIME_PROVIDER_BASE_URL": provider.base_url,
        }
        if runtime_extra_env:
            runtime_env.update(runtime_extra_env)
        runtime = await self.start_runtime_for_local_provider(
            provider,
            extra_env=runtime_env,
        )
        return marketplace, provider, runtime

    async def wait_for_provider_publication(
        self, runtime, provider_id: str, timeout: float = 10.0
    ) -> dict:
        deadline = asyncio.get_running_loop().time() + timeout
        headers = runtime_auth_headers(runtime)
        last_error = "no marketplace response yet"
        async with aiohttp.ClientSession() as session:
            while asyncio.get_running_loop().time() < deadline:
                async with session.post(
                    runtime.url("/v1/runtime/search"),
                    headers=headers,
                    json={"limit": 10},
                ) as resp:
                    if resp.status != 200:
                        last_error = f"status {resp.status}: {await resp.text()}"
                        await asyncio.sleep(0.2)
                        continue
                    payload = await resp.json()
                for provider in payload.get("providers", []):
                    if provider.get("provider_id") == provider_id:
                        return provider
                await asyncio.sleep(0.2)
        raise TimeoutError(
            f"timed out waiting for provider {provider_id} in marketplace search ({last_error})"
        )

    async def wait_for_runtime_deal(
        self,
        runtime,
        deal_id: str,
        *,
        statuses: set[str],
        timeout: float = 15.0,
    ) -> dict:
        deadline = asyncio.get_running_loop().time() + timeout
        headers = runtime_auth_headers(runtime)
        async with aiohttp.ClientSession() as session:
            while asyncio.get_running_loop().time() < deadline:
                async with session.get(
                    runtime.url(f"/v1/runtime/deals/{deal_id}"),
                    headers=headers,
                ) as resp:
                    self.assertEqual(resp.status, 200)
                    payload = await resp.json()
                if payload["deal"]["status"] in statuses:
                    return payload["deal"]
                await asyncio.sleep(0.2)
        raise TimeoutError(f"timed out waiting for runtime deal {deal_id} to reach {statuses}")

    async def test_runtime_auth_and_wallet_balance(self) -> None:
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        self.assertTrue(runtime.runtime_auth_token_path.exists())
        self.assertTrue(runtime.runtime_auth_token_path.read_text(encoding="utf-8").strip())
        if os.name == "posix":
            self.assertEqual(
                stat.S_IMODE(runtime.runtime_auth_token_path.stat().st_mode),
                0o600,
            )

        async with aiohttp.ClientSession() as session:
            async with session.get(runtime.url("/v1/runtime/wallet/balance")) as resp:
                self.assertEqual(resp.status, 401)

            async with session.get(
                runtime.url("/v1/runtime/wallet/balance"),
                headers=runtime_auth_headers(runtime),
            ) as resp:
                self.assertEqual(resp.status, 200)
                balance = await resp.json()

        self.assertEqual(balance["backend"], "lightning")
        self.assertEqual(balance["mode"], "mock_hold_invoice")
        self.assertIn("balance_known", balance)

    async def test_runtime_searches_discovery_and_reads_provider_details(self) -> None:
        _marketplace, provider, runtime = await self.start_marketplace_backed_stack()

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()

        provider_id = provider_caps["identity"]["node_id"]
        await self.wait_for_provider_publication(runtime, provider_id)

        headers = runtime_auth_headers(runtime)
        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/runtime/search"),
                headers=headers,
                json={"limit": 10},
            ) as resp:
                self.assertEqual(resp.status, 200)
                search = await resp.json()

            async with session.get(
                runtime.url(f"/v1/runtime/providers/{provider_id}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                provider_view = await resp.json()

        self.assertTrue(
            any(provider["provider_id"] == provider_id for provider in search["providers"])
        )
        self.assertEqual(provider_view["provider"]["provider_id"], provider_id)
        self.assertEqual(
            {offer["offer_id"] for offer in provider_view["provider"]["offers"]},
            {"events.query", "execute.compute", "execute.compute.generic"},
        )

    async def test_runtime_creates_remote_deal_and_persists_requester_state(self) -> None:
        _marketplace, provider, runtime = await self.start_marketplace_backed_stack(
            provider_extra_env={"FROGLET_PRICE_EXEC_WASM": "0"}
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()
        provider_id = provider_caps["identity"]["node_id"]
        await self.wait_for_provider_publication(runtime, provider_id)

        headers = runtime_auth_headers(runtime)
        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_id": provider_id,
                        "provider_url": provider.base_url,
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "runtime-remote-deal",
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                created = await resp.json()

            terminal = await self.wait_for_runtime_deal(
                runtime,
                created["deal"]["deal_id"],
                statuses={"succeeded", "failed"},
            )

            async with session.get(
                runtime.url(f"/v1/runtime/archive/deal/{created['deal']['deal_id']}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                archive = await resp.json()

        self.assertEqual(created["provider_id"], provider_id)
        self.assertEqual(created["deal"]["provider_id"], provider_id)
        self.assertEqual(terminal["status"], "succeeded")
        self.assertIsNotNone(terminal["receipt"])
        self.assertGreaterEqual(len(archive["artifact_documents"]), 2)
        self.assertGreaterEqual(len(archive["artifact_feed"]), 2)

    async def test_runtime_rejects_provider_id_mismatch_for_explicit_provider_url(self) -> None:
        provider = await self.start_provider()
        runtime = await self.start_runtime()
        headers = runtime_auth_headers(runtime)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_id": "wrong-provider-id",
                        "provider_url": provider.base_url,
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                },
            ) as resp:
                self.assertEqual(resp.status, 400)
                payload = await resp.json()

        self.assertIn(
            "provider URL targets a local or private-network address",
            payload["error"],
        )

    async def test_runtime_rejects_missing_provider_id_for_private_provider_url(self) -> None:
        provider = await self.start_provider()
        runtime = await self.start_runtime()
        headers = runtime_auth_headers(runtime)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_url": provider.base_url,
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                },
            ) as resp:
                self.assertEqual(resp.status, 400)
                payload = await resp.json()

        self.assertIn(
            "provider URL targets a local or private-network address",
            payload["error"],
        )

    async def test_runtime_rejects_explicit_local_provider_without_runtime_base_url(
        self,
    ) -> None:
        runtime = await self.start_runtime()
        headers = runtime_auth_headers(runtime)

        async with aiohttp.ClientSession() as session:
            async with session.get(runtime.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                runtime_caps = await resp.json()

            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_id": runtime_caps["identity"]["node_id"],
                        "provider_url": "http://127.0.0.1:8080",
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                },
            ) as resp:
                self.assertEqual(resp.status, 400)
                payload = await resp.json()

        self.assertIn(
            "provider URL targets a local or private-network address",
            payload["error"],
        )

    async def test_runtime_rewrites_explicit_local_provider_to_runtime_base_url(
        self,
    ) -> None:
        provider = await self.start_provider(extra_env={"FROGLET_PRICE_EXEC_WASM": "0"})
        runtime = await self.start_runtime_for_local_provider(
            provider,
            extra_env={"FROGLET_RUNTIME_PROVIDER_BASE_URL": provider.base_url},
        )
        headers = runtime_auth_headers(runtime)

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()
            provider_id = provider_caps["identity"]["node_id"]

            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_id": provider_id,
                        "provider_url": "http://127.0.0.1:8080",
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "runtime-local-provider-rewrite",
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                created = await resp.json()

            terminal = await self.wait_for_runtime_deal(
                runtime,
                created["deal"]["deal_id"],
                statuses={"succeeded", "failed"},
            )

        self.assertEqual(created["provider_id"], provider_id)
        self.assertEqual(created["deal"]["provider_id"], provider_id)
        self.assertEqual(terminal["status"], "succeeded")

    async def test_runtime_search_reconciles_marketplace_source_identity_rotation(
        self,
    ) -> None:
        provider_port = reserve_tcp_port()
        provider_url = f"http://127.0.0.1:{provider_port}"
        marketplace = await self.start_marketplace(feed_sources=[provider_url])
        provider_a = await self.start_provider(
            port=provider_port,
            extra_env={"FROGLET_MARKETPLACE_URL": marketplace.base_url},
        )
        runtime = await self.start_runtime(
            extra_env={"FROGLET_MARKETPLACE_URL": marketplace.base_url}
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider_a.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_a_caps = await resp.json()
        provider_a_id = provider_a_caps["identity"]["node_id"]
        await self.wait_for_provider_publication(runtime, provider_a_id)
        await provider_a.stop()

        provider_b = await self.start_provider(
            port=provider_port,
            extra_env={"FROGLET_MARKETPLACE_URL": marketplace.base_url},
        )
        async with aiohttp.ClientSession() as session:
            async with session.get(provider_b.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_b_caps = await resp.json()
        provider_b_id = provider_b_caps["identity"]["node_id"]
        self.assertNotEqual(provider_a_id, provider_b_id)
        await self.wait_for_provider_publication(runtime, provider_b_id)

        headers = runtime_auth_headers(runtime)
        deadline = asyncio.get_running_loop().time() + 12.0
        last_search: dict | None = None
        async with aiohttp.ClientSession() as session:
            while asyncio.get_running_loop().time() < deadline:
                async with session.post(
                    runtime.url("/v1/runtime/search"),
                    headers=headers,
                    json={"limit": 20},
                ) as resp:
                    self.assertEqual(resp.status, 200)
                    last_search = await resp.json()
                provider_ids = {
                    provider["provider_id"] for provider in last_search.get("providers", [])
                }
                if provider_b_id in provider_ids and provider_a_id not in provider_ids:
                    break
                await asyncio.sleep(0.3)
            else:
                self.fail(
                    f"stale provider identity remained visible after source rotation: {last_search}"
                )

            async with session.get(
                runtime.url(f"/v1/runtime/providers/{provider_b_id}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                provider_b_view = await resp.json()

            async with session.get(
                runtime.url(f"/v1/runtime/providers/{provider_a_id}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                provider_a_view = await resp.json()

        self.assertEqual(provider_b_view["provider"]["provider_id"], provider_b_id)
        self.assertGreater(len(provider_b_view["provider"]["offers"]), 0)
        self.assertIsNone(provider_a_view["provider"])

    async def test_runtime_exposes_payment_intent_for_priced_remote_provider(self) -> None:
        _marketplace, provider, runtime = await self.start_marketplace_backed_stack(
            provider_extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            },
            runtime_extra_env={
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/node/capabilities")) as resp:
                self.assertEqual(resp.status, 200)
                provider_caps = await resp.json()
        provider_id = provider_caps["identity"]["node_id"]
        await self.wait_for_provider_publication(runtime, provider_id)

        headers = runtime_auth_headers(runtime)
        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/runtime/deals"),
                headers=headers,
                json={
                    "provider": {
                        "provider_id": provider_id,
                        "provider_url": provider.base_url,
                    },
                    "offer_id": "execute.compute",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "runtime-priced-deal",
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                created = await resp.json()

            async with session.get(
                runtime.url(
                    f"/v1/runtime/deals/{created['deal']['deal_id']}/payment-intent"
                ),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                payment_intent = await resp.json()

        self.assertEqual(created["deal"]["status"], "payment_pending")
        self.assertIsNotNone(created["payment_intent"])
        release_action = payment_intent["payment_intent"].get("release_action")
        if release_action is not None:
            self.assertEqual(
                release_action["endpoint_path"],
                f"/v1/runtime/deals/{created['deal']['deal_id']}/accept",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
