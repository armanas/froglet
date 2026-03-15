import asyncio
import os
import stat

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    create_protocol_quote,
    generate_schnorr_signing_key,
    sha256_hex,
    sign_deal_artifact_from_quote,
    verify_signed_artifact,
)


def runtime_auth_headers(node) -> dict[str, str]:
    token_path = node.data_dir / "runtime" / "auth.token"
    token = token_path.read_text().strip()
    return {"Authorization": f"Bearer {token}"}


async def runtime_buy_request(
    session: aiohttp.ClientSession,
    node,
    *,
    request: dict[str, object],
    requester_key: bytes,
    success_payment_hash: str,
) -> dict[str, object]:
    quote = await create_protocol_quote(
        session,
        node,
        offer_id=str(request["offer_id"]),
        request={key: value for key, value in request.items() if key != "offer_id"},
        requester_secret_key=requester_key,
    )
    return {
        **{key: value for key, value in request.items() if key != "offer_id"},
        "quote": quote,
        "deal": sign_deal_artifact_from_quote(
            quote,
            requester_key,
            success_payment_hash=success_payment_hash,
        ),
    }


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

    async def test_runtime_services_buy_requires_presigned_artifacts(self) -> None:
        node = await self.start_node()
        headers = runtime_auth_headers(node)
        requester_key = generate_schnorr_signing_key()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json={
                    "offer_id": "execute.wasm",
                    **build_wasm_request(VALID_WASM_HEX),
                    "requester_seed_hex": requester_key.hex(),
                    "success_payment_hash": sha256_hex(b"legacy-runtime-seed"),
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("pre-signed quote artifact", payload["error"])

    async def test_all_privileged_runtime_routes_require_local_auth(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        headers = runtime_auth_headers(node)
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-auth-lightning-deal",
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(b"runtime-auth-success"),
            )
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                authorized = await resp.json()

            deal_id = authorized["deal"]["deal_id"]
            session_id = authorized["payment_intent"]["session_id"]

            routes = [
                ("get", "/v1/runtime/wallet/balance", None),
                ("post", "/v1/runtime/provider/start", None),
                ("post", "/v1/runtime/services/publish", None),
                ("post", "/v1/runtime/services/buy", buy_request),
                (
                    "post",
                    "/v1/runtime/discovery/curated-lists/issue",
                    {
                        "expires_at": 4_102_444_800,
                        "entries": [
                            {
                                "provider_id": authorized["deal"]["deal"]["payload"][
                                    "provider_id"
                                ],
                                "descriptor_hash": authorized["quote"]["hash"],
                            }
                        ],
                    },
                ),
                ("get", "/v1/runtime/nostr/publications/provider", None),
                ("get", f"/v1/runtime/deals/{deal_id}/payment-intent", None),
                ("get", f"/v1/runtime/archive/deal/{deal_id}", None),
                ("get", f"/v1/runtime/nostr/publications/deals/{deal_id}/receipt", None),
                (
                    "post",
                    f"/v1/runtime/lightning/invoice-bundles/{session_id}/state",
                    {"base_state": "open", "success_state": "open"},
                ),
            ]

            for method, path, payload in routes:
                async with getattr(session, method)(
                    node.url(path),
                    json=payload,
                ) as resp:
                    self.assertEqual(resp.status, 401, path)

                async with getattr(session, method)(
                    node.url(path),
                    headers={"Authorization": "Bearer wrong-token"},
                    json=payload,
                ) as resp:
                    self.assertEqual(resp.status, 401, path)

    async def test_runtime_nostr_publication_surfaces_build_signed_summary_events(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        headers = runtime_auth_headers(node)
        success_preimage = "aa" * 32
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-nostr-summary-1",
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(bytes.fromhex(success_preimage)),
            )
            async with session.get(node.url("/v1/descriptor")) as resp:
                self.assertEqual(resp.status, 200)
                descriptor = await resp.json()

            async with session.get(
                node.url("/v1/runtime/nostr/publications/provider"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                provider_publications = await resp.json()

            async with session.post(
                node.url("/v1/nostr/events/verify"),
                json={"event": provider_publications["descriptor_summary"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                descriptor_verification = await resp.json()

            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                response = await resp.json()

            async with session.post(
                node.url(
                    f"/v1/runtime/lightning/invoice-bundles/{response['payment_intent']['session_id']}/state"
                ),
                headers=headers,
                json={"base_state": "settled", "success_state": "accepted"},
            ) as resp:
                self.assertEqual(resp.status, 200)

            result_ready = await self.wait_for_deal_status(
                node, response["deal"]["deal_id"], {"result_ready"}
            )

            async with session.post(
                node.url(f"/v1/deals/{response['deal']['deal_id']}/release-preimage"),
                json={
                    "success_preimage": success_preimage,
                    "expected_result_hash": result_ready["result_hash"],
                },
            ) as resp:
                self.assertEqual(resp.status, 200)

            async with session.get(
                node.url(
                    f"/v1/runtime/nostr/publications/deals/{response['deal']['deal_id']}/receipt"
                ),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                receipt_publication = await resp.json()

            async with session.post(
                node.url("/v1/nostr/events/verify"),
                json={"event": receipt_publication["receipt_summary"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                receipt_verification = await resp.json()

        publication_identity = descriptor["payload"]["linked_identities"][0]["identity"]
        self.assertEqual(
            provider_publications["descriptor_summary"]["pubkey"],
            publication_identity,
        )
        self.assertNotEqual(publication_identity, descriptor["signer"])
        self.assertTrue(descriptor_verification["valid"])
        self.assertGreaterEqual(len(provider_publications["offer_summaries"]), 1)
        self.assertTrue(receipt_verification["valid"])
        self.assertEqual(receipt_publication["receipt_summary"]["kind"], 1390)
        self.assertEqual(receipt_publication["receipt_summary"]["pubkey"], publication_identity)

    async def test_runtime_services_buy_waits_and_reuses_idempotent_deal(self) -> None:
        node = await self.start_node(
            extra_env={"FROGLET_PRICE_EXEC_WASM": "0"}
        )
        headers = runtime_auth_headers(node)
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-service-buy-1",
            "wait_for_receipt": True,
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(b"runtime-service-buy-1"),
            )
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                first = await resp.json()

            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
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
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-service-buy-lightning-1",
            "wait_for_receipt": True,
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(b"runtime-lightning-success"),
            )
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                response = await resp.json()

        self.assertFalse(response["terminal"])
        self.assertEqual(response["deal"]["status"], "payment_pending")
        self.assertTrue(verify_signed_artifact(response["quote"]))
        self.assertEqual(
            response["payment_intent_path"],
            f"/v1/runtime/deals/{response['deal']['deal_id']}/payment-intent",
        )
        intent = response["payment_intent"]
        self.assertEqual(intent["backend"], "lightning")
        self.assertEqual(intent["deal_id"], response["deal"]["deal_id"])
        self.assertEqual(intent["deal_status"], "payment_pending")
        self.assertFalse(intent["admission_ready"])
        self.assertFalse(intent["result_ready"])
        self.assertFalse(intent["can_release_preimage"])
        self.assertIsNone(intent.get("release_action"))
        self.assertEqual(len(intent["payment_requests"]), 1)
        self.assertEqual(intent["payment_requests"][0]["role"], "success_fee_hold")
        self.assertEqual(
            intent["payment_requests"][0]["payment_hash"],
            buy_request["deal"]["payload"]["success_payment_hash"],
        )
        self.assertTrue(
            intent["payment_requests"][0]["invoice"].startswith("lnmock-")
        )

    async def test_runtime_payment_intent_tracks_lightning_release_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        headers = runtime_auth_headers(node)
        success_preimage = "9b" * 32
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-service-buy-lightning-2",
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(bytes.fromhex(success_preimage)),
            )
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                initial = await resp.json()

            session_id = initial["payment_intent"]["session_id"]
            deal_id = initial["deal"]["deal_id"]

            async with session.post(
                node.url(
                    f"/v1/runtime/lightning/invoice-bundles/{session_id}/state"
                ),
                headers=headers,
                json={"base_state": "settled", "success_state": "accepted"},
            ) as resp:
                self.assertEqual(resp.status, 200)

            result_ready = await self.wait_for_deal_status(
                node, deal_id, {"result_ready"}
            )

            async with session.get(
                node.url(f"/v1/runtime/deals/{deal_id}/payment-intent"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                current = await resp.json()

            intent = current["payment_intent"]
            self.assertEqual(intent["deal_status"], "result_ready")
            self.assertTrue(intent["admission_ready"])
            self.assertTrue(intent["result_ready"])
            self.assertTrue(intent["can_release_preimage"])
            self.assertEqual(intent["payment_requests"][0]["state"], "accepted")
            self.assertEqual(
                intent["release_action"]["expected_result_hash"],
                result_ready["result_hash"],
            )
            self.assertEqual(
                intent["release_action"]["endpoint_path"],
                f"/v1/deals/{deal_id}/release-preimage",
            )

            async with session.post(
                node.url(intent["release_action"]["endpoint_path"]),
                json={
                    "success_preimage": success_preimage,
                    "expected_result_hash": intent["release_action"]["expected_result_hash"],
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                terminal = await resp.json()

            async with session.get(
                node.url(f"/v1/runtime/deals/{deal_id}/payment-intent"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                final = await resp.json()

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(final["payment_intent"]["deal_status"], "succeeded")
        self.assertFalse(final["payment_intent"]["can_release_preimage"])
        self.assertIsNone(final["payment_intent"].get("release_action"))
        self.assertEqual(final["payment_intent"]["payment_requests"][0]["state"], "settled")

    async def test_runtime_archive_exports_lightning_deal_material(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        headers = runtime_auth_headers(node)
        success_preimage = "78" * 32
        requester_key = generate_schnorr_signing_key()
        request = {
            "offer_id": "execute.wasm",
            **build_wasm_request(VALID_WASM_HEX),
            "idempotency_key": "runtime-archive-lightning-deal-1",
        }

        async with aiohttp.ClientSession() as session:
            buy_request = await runtime_buy_request(
                session,
                node,
                request=request,
                requester_key=requester_key,
                success_payment_hash=sha256_hex(bytes.fromhex(success_preimage)),
            )
            async with session.post(
                node.url("/v1/runtime/services/buy"),
                headers=headers,
                json=buy_request,
            ) as resp:
                self.assertEqual(resp.status, 200)
                response = await resp.json()

            deal_id = response["deal"]["deal_id"]
            payment_intent = response["payment_intent"]

            async with session.post(
                node.url(
                    f"/v1/runtime/lightning/invoice-bundles/{payment_intent['session_id']}/state"
                ),
                headers=headers,
                json={"base_state": "settled", "success_state": "accepted"},
            ) as resp:
                self.assertEqual(resp.status, 200)

            result_ready = await self.wait_for_deal_status(
                node, deal_id, {"result_ready"}
            )

            async with session.get(
                node.url(f"/v1/runtime/deals/{deal_id}/payment-intent"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                current_intent = await resp.json()

            async with session.post(
                node.url(current_intent["payment_intent"]["release_action"]["endpoint_path"]),
                json={
                    "success_preimage": success_preimage,
                    "expected_result_hash": current_intent["payment_intent"]["release_action"][
                        "expected_result_hash"
                    ],
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                terminal = await resp.json()

            async with session.get(
                node.url(f"/v1/runtime/archive/deal/{deal_id}"),
                headers=headers,
            ) as resp:
                self.assertEqual(resp.status, 200)
                archive = await resp.json()

        self.assertEqual(terminal["status"], "succeeded")
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
            archive["lightning_invoice_bundles"][0]["session_id"], payment_intent["session_id"]
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
