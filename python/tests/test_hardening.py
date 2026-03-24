import asyncio
import os
import signal
import shutil
import tempfile
import time
from pathlib import Path

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    LONG_RUNNING_WASM_HEX,
    VALID_WASM_HEX,
    build_wasm_request,
    build_wasm_submission,
    create_protocol_deal,
    create_protocol_quote,
    generate_schnorr_signing_key,
    verify_signed_artifact,
)


class HardeningTests(FrogletAsyncTestCase):
    async def test_execute_wasm_offer_exposes_execution_profile_timeout(self) -> None:
        provider = await self.start_provider(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_EXECUTION_TIMEOUT_SECS": "120",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/provider/offers")) as resp:
                offers = await resp.json()

        self.assertEqual(resp.status, 200)
        wasm_offer = next(
            offer
            for offer in offers["offers"]
            if offer["payload"]["offer_id"] == "execute.compute"
        )
        self.assertEqual(
            wasm_offer["payload"]["execution_profile"]["max_runtime_ms"], 120_000
        )

    async def test_execute_wasm_enforces_wall_clock_timeout(self) -> None:
        provider = await self.start_provider(
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "1",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(provider.url("/v1/provider/offers")) as resp:
                offers = await resp.json()
            self.assertEqual(resp.status, 200)
            wasm_offer = next(
                offer
                for offer in offers["offers"]
                if offer["payload"]["offer_id"] == "execute.compute"
            )
            self.assertEqual(
                wasm_offer["payload"]["execution_profile"]["max_runtime_ms"], 1_000
            )

            async with session.post(
                provider.url("/v1/node/execute/wasm"),
                json={"submission": build_wasm_submission(LONG_RUNNING_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertTrue(
            "timeout" in payload["error"].lower()
            or "limit exceeded" in payload["error"].lower()
        )

    async def test_restart_recovery_emits_signed_failure_receipt_for_incomplete_deal(self) -> None:
        data_root = Path(tempfile.mkdtemp(prefix="froglet-recovery-data-"))
        self.addCleanup(lambda: shutil.rmtree(data_root, ignore_errors=True))

        provider = await self.start_provider(
            data_dir=data_root,
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "30",
                "FROGLET_PRICE_EXEC_WASM": "0",
            },
        )

        async with aiohttp.ClientSession() as session:
            requester_key = generate_schnorr_signing_key()
            quote = await create_protocol_quote(
                session,
                provider,
                offer_id="execute.compute",
                request=build_wasm_request(LONG_RUNNING_WASM_HEX),
                requester_secret_key=requester_key,
            )
            deal = await create_protocol_deal(
                session,
                provider,
                quote=quote,
                request=build_wasm_request(LONG_RUNNING_WASM_HEX),
                requester_secret_key=requester_key,
            )

            deadline = time.monotonic() + 5.0
            current = deal
            while time.monotonic() < deadline:
                async with session.get(
                    provider.url(f"/v1/provider/deals/{deal['deal_id']}")
                ) as poll_resp:
                    current = await poll_resp.json()
                if current["status"] == "running":
                    break
                await asyncio.sleep(0.1)
            else:
                self.fail(f"deal never reached running state: {current}")

        os.killpg(provider.process.pid, signal.SIGKILL)
        await asyncio.to_thread(provider.process.wait, 5)

        restarted = await self.start_provider(
            data_dir=data_root,
            extra_env={
                "FROGLET_EXECUTION_TIMEOUT_SECS": "30",
                "FROGLET_PRICE_EXEC_WASM": "0",
            },
        )

        async with aiohttp.ClientSession() as session:
            deadline = time.monotonic() + 10.0
            recovered = None
            status = None
            while time.monotonic() < deadline:
                async with session.get(
                    restarted.url(f"/v1/provider/deals/{deal['deal_id']}")
                ) as resp:
                    recovered = await resp.json()
                status = recovered["status"]
                if status == "failed":
                    break
                await asyncio.sleep(0.2)
            else:
                self.fail(f"deal never reached failed after restart recovery: {recovered}")

        self.assertEqual(resp.status, 200)
        self.assertEqual(recovered["status"], "failed")
        self.assertEqual(recovered["error"], "Wasm module execution limit exceeded")
        self.assertTrue(verify_signed_artifact(recovered["receipt"]))
        self.assertEqual(
            recovered["receipt"]["payload"]["failure_code"], "execution_limit_exceeded"
        )
        self.assertEqual(recovered["receipt"]["payload"]["deal_state"], "failed")
        self.assertEqual(recovered["receipt"]["payload"]["deal_hash"], recovered["deal"]["hash"])
        self.assertEqual(recovered["receipt"]["payload"]["settlement_state"], "none")
        self.assertEqual(recovered["receipt"]["payload"]["executor"]["runtime"], "wasm")
        self.assertEqual(
            recovered["receipt"]["payload"]["limits_applied"]["max_output_bytes"], 131072
        )

    async def test_execute_wasm_rejects_module_hash_mismatch(self) -> None:
        provider = await self.start_provider()
        submission = build_wasm_submission(VALID_WASM_HEX)
        submission["workload"]["module_hash"] = "00" * 32

        async with aiohttp.ClientSession() as session:
            async with session.post(
                provider.url("/v1/node/execute/wasm"),
                json={"submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("module hash", payload["error"].lower())
