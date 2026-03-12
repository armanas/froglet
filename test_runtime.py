import asyncio
import os
import stat

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_CASHU_TOKEN,
    VALID_WASM_HEX,
    build_wasm_request,
    generate_schnorr_signing_key,
    schnorr_pubkey_hex,
    sha256_hex,
    verify_signed_artifact,
)


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
        self.assertEqual(len(snapshot["offers"]), 2)
        self.assertTrue(all(verify_signed_artifact(offer) for offer in snapshot["offers"]))

    async def test_runtime_services_buy_waits_and_reuses_idempotent_deal(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )
        headers = runtime_auth_headers(node)
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
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
                    "offer_id": "execute.wasm",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": request["idempotency_key"],
                    "wait_for_receipt": True,
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                second = await resp.json()

        self.assertTrue(first["terminal"])
        self.assertEqual(first["deal"]["status"], "succeeded")
        self.assertEqual(first["deal"]["result"], 42)
        self.assertTrue(verify_signed_artifact(first["quote"]))
        self.assertTrue(verify_signed_artifact(first["deal"]["receipt"]))

        self.assertTrue(second["terminal"])
        self.assertEqual(second["deal"]["deal_id"], first["deal"]["deal_id"])
        self.assertEqual(second["quote"]["hash"], first["quote"]["hash"])
        self.assertEqual(second["deal"]["result"], 42)

    async def test_runtime_services_buy_returns_payment_pending_for_lightning(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        headers = runtime_auth_headers(node)
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-service-buy-lightning-1",
            "requester_id": schnorr_pubkey_hex(generate_schnorr_signing_key()),
            "success_payment_hash": sha256_hex(b"runtime-lightning-success"),
            "wait_for_receipt": True,
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                response = await resp.json()

        self.assertFalse(response["terminal"])
        self.assertEqual(response["deal"]["status"], "payment_pending")
        self.assertTrue(verify_signed_artifact(response["quote"]))

    async def test_runtime_archive_exports_lightning_deal_material(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        headers = runtime_auth_headers(node)
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-archive-lightning-deal-1",
            "requester_id": schnorr_pubkey_hex(generate_schnorr_signing_key()),
            "success_payment_hash": sha256_hex(b"runtime-archive-lightning-success"),
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                response = await resp.json()

            deal_id = response["deal"]["deal_id"]

            async with session.get(node.url(f"/v1/deals/{deal_id}/invoice-bundle")) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            async with session.post(
                node.url(
                    f"/v1/runtime/lightning/invoice-bundles/{bundle['session_id']}/state"
                ),
                headers=headers,
                json={"base_state": "settled", "success_state": "accepted"},
            ) as resp:
                self.assertEqual(resp.status, 200)

            await self.wait_for_deal(node, deal_id)

            async with session.get(
                node.url(f"/v1/runtime/archive/deal/{deal_id}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                archive = await resp.json()

        self.assertEqual(archive["schema_version"], "froglet/v1")
        self.assertEqual(archive["export_type"], "runtime_archive_bundle")
        self.assertEqual(archive["subject_kind"], "deal")
        self.assertEqual(archive["subject_id"], deal_id)
        self.assertEqual(
            [artifact["artifact_kind"] for artifact in archive["artifact_documents"]],
            ["quote", "deal", "receipt"],
        )
        self.assertEqual(
            [entry["sequence"] for entry in archive["artifact_feed"]],
            sorted(entry["sequence"] for entry in archive["artifact_feed"]),
        )
        self.assertEqual(len(archive["lightning_invoice_bundles"]), 1)
        self.assertEqual(
            archive["lightning_invoice_bundles"][0]["session_id"], bundle["session_id"]
        )
        self.assertEqual(
            archive["lightning_invoice_bundles"][0]["success_state"], "settled"
        )
        self.assertIn(
            "lightning_invoice_bundle_ref",
            {item["evidence_kind"] for item in archive["execution_evidence"]},
        )
        self.assertIn(
            "execution_result",
            {item["evidence_kind"] for item in archive["execution_evidence"]},
        )

    async def test_runtime_archive_exports_job_evidence(self) -> None:
        node = await self.start_node()
        headers = runtime_auth_headers(node)

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "runtime-archive-job-1",
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                job = await resp.json()

            deadline = asyncio.get_running_loop().time() + 10
            current = job
            while asyncio.get_running_loop().time() < deadline:
                async with session.get(node.url(f"/v1/node/jobs/{job['job_id']}")) as resp:
                    self.assertEqual(resp.status, 200)
                    current = await resp.json()
                if current["status"] in {"succeeded", "failed"}:
                    break
                await asyncio.sleep(0.2)
            else:
                self.fail(f"job never reached terminal state: {current}")

            async with session.get(
                node.url(f"/v1/runtime/archive/job/{job['job_id']}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                archive = await resp.json()

        self.assertEqual(archive["subject_kind"], "job")
        self.assertEqual(archive["subject_id"], job["job_id"])
        self.assertEqual(archive["artifact_documents"], [])
        self.assertEqual(archive["artifact_feed"], [])
        self.assertEqual(archive["lightning_invoice_bundles"], [])
        self.assertIn(
            "workload_spec",
            {item["evidence_kind"] for item in archive["execution_evidence"]},
        )
