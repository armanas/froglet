import os
import stat

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN, verify_signed_artifact


def runtime_auth_headers(node) -> dict[str, str]:
    token_path = node.data_dir / "runtime" / "auth.token"
    token = token_path.read_text().strip()
    return {"Authorization": f"Bearer {token}"}


class RuntimeApiTests(FrogletAsyncTestCase):
    async def test_runtime_auth_and_provider_snapshot(self) -> None:
        node = await self.start_node()
        token_path = node.data_dir / "runtime" / "auth.token"

        self.assertTrue(token_path.exists())
        self.assertTrue(token_path.read_text().strip())
        if os.name == "posix":
            self.assertEqual(stat.S_IMODE(token_path.stat().st_mode), 0o600)

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/runtime/wallet/balance")) as resp:
                self.assertEqual(resp.status, 401)

            async with session.post(
                node.url("/v1/runtime/provider/start"),
                headers={"Authorization": "Bearer wrong-token"},
            ) as resp:
                self.assertEqual(resp.status, 401)

            async with session.post(
                node.url("/v1/runtime/provider/start"),
                headers=runtime_auth_headers(node),
            ) as resp:
                self.assertEqual(resp.status, 200)
                snapshot = await resp.json()

        self.assertEqual(snapshot["status"], "running")
        self.assertTrue(verify_signed_artifact(snapshot["descriptor"]))
        self.assertEqual(snapshot["runtime_auth"]["scheme"], "bearer")
        self.assertEqual(snapshot["runtime_auth"]["token_path"], str(token_path))
        self.assertEqual(len(snapshot["offers"]), 3)
        self.assertTrue(all(verify_signed_artifact(offer) for offer in snapshot["offers"]))

    async def test_runtime_services_buy_waits_and_reuses_idempotent_deal(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_LUA": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )
        headers = runtime_auth_headers(node)
        request = {
            "offer_id": "execute.lua",
            "kind": "lua",
            "script": "return input.greeting .. ' ' .. input.target",
            "input": {"greeting": "hello", "target": "runtime"},
            "idempotency_key": "runtime-service-buy-1",
            "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
            "wait_for_receipt": True,
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                first = await resp.json()

            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json={
                    "offer_id": "execute.lua",
                    "kind": "lua",
                    "script": request["script"],
                    "input": request["input"],
                    "idempotency_key": request["idempotency_key"],
                    "wait_for_receipt": True,
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                second = await resp.json()

        self.assertTrue(first["terminal"])
        self.assertEqual(first["deal"]["status"], "succeeded")
        self.assertEqual(first["deal"]["result"], "hello runtime")
        self.assertTrue(verify_signed_artifact(first["quote"]))
        self.assertTrue(verify_signed_artifact(first["deal"]["receipt"]))

        self.assertTrue(second["terminal"])
        self.assertEqual(second["deal"]["deal_id"], first["deal"]["deal_id"])
        self.assertEqual(second["quote"]["hash"], first["quote"]["hash"])
        self.assertEqual(second["deal"]["result"], "hello runtime")
