import asyncio
import base64
import json
import shutil
import tempfile
import time
from pathlib import Path

import aiohttp
from aiohttp import web

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN, reserve_tcp_port, verify_signed_artifact


def rewrite_cashu_token_mint(token: str, mint_url: str) -> str:
    if not token.startswith("cashuA"):
        raise ValueError("expected a v3 cashu token")

    payload = json.loads(base64.b64decode(token[len("cashuA") :]).decode("utf-8"))
    for entry in payload["token"]:
        entry["mint"] = mint_url

    encoded = base64.b64encode(
        json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).decode("ascii")
    return f"cashuA{encoded}"


async def start_fake_checkstate_mint(state: str) -> tuple[str, list[dict], web.AppRunner]:
    requests: list[dict] = []

    async def handle_checkstate(request: web.Request) -> web.Response:
        payload = await request.json()
        requests.append(payload)
        return web.json_response(
            {
                "states": [
                    {"Y": proof_y, "state": state, "witness": None}
                    for proof_y in payload.get("Ys", [])
                ]
            }
        )

    port = reserve_tcp_port()
    app = web.Application()
    app.router.add_post("/v1/checkstate", handle_checkstate)
    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, "127.0.0.1", port)
    await site.start()
    return f"http://127.0.0.1:{port}", requests, runner


class HardeningTests(FrogletAsyncTestCase):
    async def test_execute_lua_enforces_wall_clock_timeout(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "1",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/offers")) as resp:
                offers = await resp.json()
            self.assertEqual(resp.status, 200)
            lua_offer = next(
                offer for offer in offers["offers"] if offer["payload"]["offer_id"] == "execute.lua"
            )
            self.assertEqual(lua_offer["payload"]["constraints"]["timeout_secs"], 1)

            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={"script": "while true do end"},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertTrue(
            "timeout" in payload["error"].lower()
            or "limit exceeded" in payload["error"].lower()
        )

    async def test_cashu_mint_allowlist_rejects_unknown_mint(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "cashu",
                "FROGLET_PRICE_EXEC_LUA": "1",
                "FROGLET_CASHU_MINT_ALLOWLIST": "https://mint.example",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={
                    "script": "return 7",
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("not allowed", payload["error"].lower())

    async def test_cashu_remote_checkstate_rejects_spent_proofs(self) -> None:
        mint_url, requests, runner = await start_fake_checkstate_mint("SPENT")
        self.addAsyncCleanup(runner.cleanup)
        token = rewrite_cashu_token_mint(VALID_CASHU_TOKEN, mint_url)

        node = await self.start_node(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "cashu",
                "FROGLET_PRICE_EXEC_LUA": "1",
                "FROGLET_CASHU_MINT_ALLOWLIST": mint_url,
                "FROGLET_CASHU_REMOTE_CHECKSTATE": "true",
                "FROGLET_CASHU_REQUEST_TIMEOUT_SECS": "2",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={
                    "script": "return 1",
                    "payment": {"kind": "cashu", "token": token},
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("not spendable", payload["error"].lower())
        self.assertEqual(len(requests), 1)
        self.assertIn("Ys", requests[0])
        self.assertGreater(len(requests[0]["Ys"]), 0)

    async def test_restart_recovery_emits_signed_failure_receipt_for_incomplete_deal(self) -> None:
        data_root = Path(tempfile.mkdtemp(prefix="froglet-recovery-data-"))
        self.addCleanup(lambda: shutil.rmtree(data_root, ignore_errors=True))

        node = await self.start_node(
            data_dir=data_root,
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "30",
                "FROGLET_PRICE_EXEC_LUA": "10",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={
                    "offer_id": "execute.lua",
                    "kind": "lua",
                    "script": "while true do end",
                },
            ) as resp:
                quote = await resp.json()
            self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": quote,
                    "kind": "lua",
                    "script": "while true do end",
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                deal = await resp.json()
            self.assertEqual(resp.status, 202)

            deadline = time.monotonic() + 5.0
            current = deal
            while time.monotonic() < deadline:
                async with session.get(node.url(f"/v1/deals/{deal['deal_id']}")) as poll_resp:
                    current = await poll_resp.json()
                if current["status"] == "running":
                    break
                await asyncio.sleep(0.1)
            else:
                self.fail(f"deal never reached running state: {current}")

        await node.stop()

        restarted = await self.start_node(
            data_dir=data_root,
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "30",
                "FROGLET_PRICE_EXEC_LUA": "10",
            },
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(restarted.url(f"/v1/deals/{deal['deal_id']}")) as resp:
                recovered = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(recovered["status"], "failed")
        self.assertEqual(recovered["error"], "node restarted before deal completion")
        self.assertTrue(verify_signed_artifact(recovered["receipt"]))
        self.assertEqual(recovered["receipt"]["payload"]["failure"]["code"], "node_restarted")
        self.assertEqual(recovered["receipt"]["payload"]["status"], "failed")
        self.assertEqual(recovered["receipt"]["payload"]["settlement"]["status"], "expired")
