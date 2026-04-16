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


if __name__ == "__main__":
    unittest.main(verbosity=2)
