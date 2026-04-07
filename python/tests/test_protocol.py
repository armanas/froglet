import asyncio
import json

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    LONG_RUNNING_WASM_HEX,
    VALID_WASM_HEX,
    build_wasm_request,
    create_protocol_deal,
    create_protocol_quote,
    create_signed_event,
    execute_db,
    generate_schnorr_signing_key,
    read_db_row,
    read_db_rows,
    schnorr_pubkey_hex,
    sha256_hex,
    verify_signed_artifact,
    workload_hash_from_submission,
)


def runtime_auth_headers(node) -> dict[str, str]:
    token_path = node.data_dir / "runtime" / "auth.token"
    token = token_path.read_text().strip()
    return {"Authorization": f"Bearer {token}"}


def set_mock_invoice_bundle_states(
    provider,
    session_id: str,
    *,
    base_state: str,
    success_state: str,
) -> None:
    execute_db(
        provider.data_dir / "node.db",
        "UPDATE lightning_invoice_bundles SET base_state = ?, success_state = ?, updated_at = strftime('%s','now') WHERE session_id = ?",
        (base_state, success_state, session_id),
    )


class ProtocolPrimitiveTests(FrogletAsyncTestCase):
    async def _create_quote(
        self,
        session: aiohttp.ClientSession,
        node,
        request: dict,
        *,
        offer_id: str = "execute.compute",
        requester_key: bytes | None = None,
    ) -> tuple[bytes, dict]:
        requester_key = requester_key or generate_schnorr_signing_key()
        quote = await create_protocol_quote(
            session,
            node,
            offer_id=offer_id,
            request=request,
            requester_secret_key=requester_key,
        )
        return requester_key, quote

    async def _create_deal(
        self,
        session: aiohttp.ClientSession,
        node,
        *,
        quote: dict,
        request: dict,
        requester_key: bytes,
        idempotency_key: str | None = None,
        payment: dict | None = None,
        success_payment_hash: str | None = None,
    ) -> dict:
        return await create_protocol_deal(
            session,
            node,
            quote=quote,
            request=request,
            requester_secret_key=requester_key,
            idempotency_key=idempotency_key,
            payment=payment,
            success_payment_hash=success_payment_hash,
        )

    async def test_descriptor_offers_feed_and_artifact_fetch_are_signed(self) -> None:
        node = await self.start_provider()

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/provider/descriptor")) as resp:
                self.assertEqual(resp.status, 200)
                descriptor = await resp.json()

            async with session.get(node.url("/v1/provider/offers")) as resp:
                self.assertEqual(resp.status, 200)
                offers_payload = await resp.json()

            async with session.get(node.url("/v1/feed?limit=1")) as resp:
                self.assertEqual(resp.status, 200)
                first_page = await resp.json()

            next_cursor = first_page["next_cursor"]
            self.assertIsNotNone(next_cursor)

            async with session.get(node.url(f"/v1/feed?cursor={next_cursor}&limit=1")) as resp:
                self.assertEqual(resp.status, 200)
                second_page = await resp.json()

            first_artifact = first_page["artifacts"][0]
            async with session.get(node.url(f"/v1/artifacts/{first_artifact['hash']}")) as resp:
                self.assertEqual(resp.status, 200)
                fetched_artifact = await resp.json()

        self.assertTrue(verify_signed_artifact(descriptor))
        self.assertEqual(descriptor["artifact_type"], "descriptor")
        self.assertEqual(descriptor["schema_version"], "froglet/v1")
        self.assertEqual(descriptor["payload"]["protocol_version"], "froglet/v1")
        self.assertGreaterEqual(descriptor["payload"]["descriptor_seq"], 1)
        self.assertEqual(descriptor["payload"]["provider_id"], descriptor["signer"])
        transport_endpoints = {
            endpoint["transport"]: endpoint
            for endpoint in descriptor["payload"]["transport_endpoints"]
        }
        self.assertEqual(transport_endpoints["http"]["uri"], node.base_url)
        self.assertNotIn("tor", transport_endpoints)
        self.assertNotEqual(
            descriptor["signer"], transport_endpoints["http"]["uri"]
        )
        self.assertEqual(
            descriptor["payload"]["capabilities"]["service_kinds"],
            ["compute.execution.v1", "compute.wasm.v1", "events.query"],
        )
        self.assertEqual(
            descriptor["payload"]["capabilities"]["execution_runtimes"],
            ["any", "builtin", "wasm"],
        )
        linked_identities = descriptor["payload"]["linked_identities"]
        self.assertEqual(len(linked_identities), 1)
        self.assertEqual(linked_identities[0]["identity_kind"], "nostr")
        self.assertEqual(
            linked_identities[0]["scope"],
            ["publication.nostr"],
        )
        self.assertNotEqual(linked_identities[0]["identity"], descriptor["signer"])

        offers = offers_payload["offers"]
        self.assertEqual(len(offers), 3)
        self.assertTrue(all(verify_signed_artifact(offer) for offer in offers))
        self.assertEqual(
            {offer["payload"]["offer_id"] for offer in offers},
            {"events.query", "execute.compute", "execute.compute.generic"},
        )

        self.assertEqual(first_page["cursor_type"], "artifact_sequence")
        self.assertEqual(first_page["cursor_semantics"], "exclusive_after")
        self.assertEqual(first_page["applied_cursor"], 0)
        self.assertEqual(first_page["page_size"], 1)
        self.assertTrue(first_page["has_more"])
        self.assertEqual(len(first_page["artifacts"]), 1)

        self.assertEqual(second_page["applied_cursor"], next_cursor)
        self.assertEqual(second_page["page_size"], 1)
        self.assertEqual(len(second_page["artifacts"]), 1)
        self.assertGreater(second_page["artifacts"][0]["cursor"], first_artifact["cursor"])

        self.assertEqual(fetched_artifact["hash"], first_artifact["hash"])
        self.assertEqual(fetched_artifact["document"]["hash"], first_artifact["hash"])
        self.assertTrue(verify_signed_artifact(fetched_artifact["document"]))

    async def test_compute_quote_deal_and_receipt_flow(self) -> None:
        node = await self.start_provider(extra_env={"FROGLET_PRICE_EXEC_WASM": "0"})

        wasm_request = build_wasm_request(VALID_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            requester_key, quote = await self._create_quote(session, node, wasm_request)

            self.assertTrue(verify_signed_artifact(quote))
            self.assertEqual(
                quote["payload"]["settlement_terms"]["success_fee_msat"], 0
            )
            self.assertEqual(quote["payload"]["workload_kind"], "compute.wasm.v1")

            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=wasm_request,
                requester_key=requester_key,
                idempotency_key="protocol-compute-deal",
            )

            terminal = await self.wait_for_deal(node, deal["deal_id"])

            async with session.post(
                node.url("/v1/receipts/verify"),
                json={"receipt": terminal["receipt"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                verify_response = await resp.json()

            async with session.get(node.url("/v1/feed")) as resp:
                self.assertEqual(resp.status, 200)
                feed = await resp.json()

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(terminal["result"], 42)
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(terminal["receipt"]["payload"]["deal_state"], "succeeded")
        self.assertEqual(terminal["receipt"]["payload"]["execution_state"], "succeeded")
        self.assertEqual(terminal["receipt"]["payload"]["settlement_state"], "none")
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_refs"]["method"], "none"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["deal_hash"], terminal["deal"]["hash"]
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["result_format"], "application/json+jcs"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["runtime"], "wasm"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["abi_version"],
            wasm_request["submission"]["workload"]["abi_version"],
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["module_hash"],
            wasm_request["submission"]["workload"]["module_hash"],
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["max_input_bytes"], 131072
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["max_output_bytes"], 131072
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["fuel_limit"], 50000000
        )
        self.assertIsNone(terminal["receipt"]["payload"].get("failure_code"))
        self.assertTrue(verify_response["valid"])
        self.assertEqual(verify_response["deal_state"], "succeeded")
        self.assertIn(
            terminal["receipt"]["hash"],
            {artifact["hash"] for artifact in feed["artifacts"]},
        )

    async def test_lightning_deal_requires_requester_preimage_release(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_EXECUTION_TIMEOUT_SECS": "120",
            }
        )
        requester_key = generate_schnorr_signing_key()
        requester_id = schnorr_pubkey_hex(requester_key)
        success_preimage = "12" * 32
        success_payment_hash = sha256_hex(bytes.fromhex(success_preimage))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-deal",
                success_payment_hash=success_payment_hash,
            )

            self.assertEqual(deal["status"], "payment_pending")

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            async with session.post(
                node.url("/v1/invoice-bundles/verify"),
                json={
                    "bundle": bundle["bundle"],
                    "quote": quote,
                    "deal": deal["deal"],
                    "requester_id": requester_id,
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                validation = await resp.json()

            self.assertTrue(verify_signed_artifact(bundle["bundle"]))
            self.assertEqual(bundle["base_state"], "open")
            self.assertEqual(bundle["success_state"], "open")
            self.assertEqual(bundle["bundle"]["payload"]["requester_id"], requester_id)
            self.assertEqual(
                bundle["bundle"]["payload"]["success_fee"]["payment_hash"],
                success_payment_hash,
            )
            self.assertTrue(validation["valid"])
            self.assertEqual(validation["issues"], [])

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

            result_ready = await self.wait_for_deal_status(
                node, deal["deal_id"], {"result_ready"}
            )

            async with session.post(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/accept"),
                json={
                    "success_preimage": success_preimage,
                    "expected_result_hash": result_ready["result_hash"],
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                terminal = await resp.json()

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                settled_bundle = await resp.json()

        self.assertEqual(result_ready["status"], "result_ready")
        self.assertEqual(result_ready["result"], 42)
        self.assertIsNone(result_ready.get("receipt"))
        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(terminal["result"], 42)
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_refs"]["method"],
            "lightning.base_fee_plus_success_fee.v1",
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_state"], "settled"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_refs"]["bundle_hash"],
            bundle["bundle"]["hash"],
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_refs"]["success_fee"][
                "payment_hash"
            ],
            success_payment_hash,
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["runtime"], "wasm"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["max_runtime_ms"], 120000
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["fuel_limit"], 50000000
        )
        self.assertEqual(settled_bundle["success_state"], "settled")

    async def test_lightning_release_preimage_rejects_mismatched_secret(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_preimage = "34" * 32
        success_payment_hash = sha256_hex(bytes.fromhex(success_preimage))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-bad-preimage",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

            result_ready = await self.wait_for_deal_status(
                node, deal["deal_id"], {"result_ready"}
            )

            async with session.post(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/accept"),
                json={"success_preimage": "56" * 32},
            ) as resp:
                self.assertEqual(resp.status, 400)
                rejected = await resp.json()

        self.assertEqual(result_ready["status"], "result_ready")
        self.assertEqual(
            rejected["error"], "success_preimage does not match the deal payment lock"
        )

    async def test_lightning_watcher_promotes_funded_deal_without_status_polling(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_payment_hash = sha256_hex(bytes.fromhex("9a" * 32))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-watcher-promotion",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

        status = await self.wait_for_deal_status_in_db(
            node, deal["deal_id"], {"result_ready"}
        )
        self.assertEqual(status, "result_ready")

    async def test_lightning_watcher_finalizes_settled_result_ready_deal(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_payment_hash = sha256_hex(bytes.fromhex("ab" * 32))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-watcher-finalize",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

            status = await self.wait_for_deal_status_in_db(
                node, deal["deal_id"], {"result_ready"}
            )
            self.assertEqual(status, "result_ready")

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="settled",
            )

            terminal_status = await self.wait_for_deal_status_in_db(
                node, deal["deal_id"], {"succeeded"}
            )
            self.assertEqual(terminal_status, "succeeded")

            async with session.get(node.url(f"/v1/provider/deals/{deal['deal_id']}")) as resp:
                self.assertEqual(resp.status, 200)
                terminal = await resp.json()

        self.assertEqual(terminal["status"], "succeeded")
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_state"], "settled"
        )

    async def test_lightning_watcher_fails_payment_pending_deal_when_provider_cancels(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_payment_hash = sha256_hex(bytes.fromhex("cd" * 32))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-watcher-cancel-pending",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="canceled",
                success_state="open",
            )

            terminal = await self.wait_for_deal(node, deal["deal_id"])

        self.assertEqual(terminal["status"], "failed")
        self.assertIsNone(terminal.get("result"))
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(
            terminal["receipt"]["payload"]["failure_code"], "payment_canceled"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["deal_state"], "canceled"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_refs"]["success_fee"][
                "payment_hash"
            ],
            success_payment_hash,
        )

    async def test_lightning_watcher_fails_result_ready_deal_when_requester_withholds_preimage(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_payment_hash = sha256_hex(bytes.fromhex("ef" * 32))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-withheld-preimage",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

            result_ready = await self.wait_for_deal_status(
                node, deal["deal_id"], {"result_ready"}
            )

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="canceled",
            )

            terminal = await self.wait_for_deal(node, deal["deal_id"])

        self.assertEqual(result_ready["status"], "result_ready")
        self.assertEqual(terminal["status"], "failed")
        self.assertEqual(terminal["result"], 42)
        self.assertEqual(terminal["result_hash"], result_ready["result_hash"])
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(
            terminal["receipt"]["payload"]["failure_code"],
            "success_fee_canceled_before_release",
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_state"], "canceled"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["result_hash"], result_ready["result_hash"]
        )

    async def test_lightning_watcher_fails_result_ready_deal_on_success_hold_expiry(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        requester_key = generate_schnorr_signing_key()
        success_payment_hash = sha256_hex(bytes.fromhex("90" * 32))

        async with aiohttp.ClientSession() as session:
            _, quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=requester_key,
                idempotency_key="protocol-lightning-success-expired",
                success_payment_hash=success_payment_hash,
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                bundle = await resp.json()

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="accepted",
            )

            result_ready = await self.wait_for_deal_status(
                node, deal["deal_id"], {"result_ready"}
            )

            set_mock_invoice_bundle_states(
                node,
                bundle["session_id"],
                base_state="settled",
                success_state="expired",
            )

            terminal = await self.wait_for_deal(node, deal["deal_id"])

        self.assertEqual(result_ready["status"], "result_ready")
        self.assertEqual(terminal["status"], "failed")
        self.assertEqual(terminal["result_hash"], result_ready["result_hash"])
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(
            terminal["receipt"]["payload"]["failure_code"],
            "success_fee_expired_before_release",
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement_state"], "expired"
        )

    async def test_lightning_invoice_bundle_rejects_mismatched_stored_bundle(self) -> None:
        node = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )
        first_requester_key = generate_schnorr_signing_key()
        second_requester_key = generate_schnorr_signing_key()
        first_requester_id = schnorr_pubkey_hex(first_requester_key)

        async with aiohttp.ClientSession() as session:
            _, first_quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=first_requester_key,
            )
            first_deal = await self._create_deal(
                session,
                node,
                quote=first_quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=first_requester_key,
                idempotency_key="protocol-lightning-mismatch-1",
                success_payment_hash=sha256_hex(b"protocol-lightning-mismatch-1"),
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{first_deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                first_bundle = await resp.json()

            _, second_quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
                requester_key=second_requester_key,
            )
            second_deal = await self._create_deal(
                session,
                node,
                quote=second_quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=second_requester_key,
                idempotency_key="protocol-lightning-mismatch-2",
                success_payment_hash=sha256_hex(b"protocol-lightning-mismatch-2"),
            )

            async with session.get(
                node.url(f"/v1/provider/deals/{second_deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                second_bundle = await resp.json()

        execute_db(
            node.data_dir / "node.db",
            "UPDATE lightning_invoice_bundles SET bundle_json = ? WHERE session_id = ?",
            (json.dumps(second_bundle["bundle"]), first_bundle["session_id"]),
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(
                node.url(f"/v1/provider/deals/{first_deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 409)
                corrupted = await resp.json()

            async with session.post(
                node.url("/v1/invoice-bundles/verify"),
                json={
                    "bundle": second_bundle["bundle"],
                    "quote": first_quote,
                    "deal": first_deal["deal"],
                    "requester_id": first_requester_id,
                },
            ) as resp:
                self.assertEqual(resp.status, 200)
                validation = await resp.json()

        self.assertEqual(
            corrupted["error"],
            "stored lightning invoice bundle failed commitment validation",
        )
        issue_codes = {issue["code"] for issue in corrupted["validation"]["issues"]}
        self.assertIn("quote_hash_mismatch", issue_codes)
        self.assertFalse(validation["valid"])
        self.assertIn(
            "requester_id_mismatch",
            {issue["code"] for issue in validation["issues"]},
        )

    async def test_data_offer_can_be_quoted_and_executed(self) -> None:
        node = await self.start_provider()
        wasm_request = build_wasm_request(VALID_WASM_HEX)

        async with aiohttp.ClientSession() as session:
            requester_key, quote = await self._create_quote(
                session,
                node,
                wasm_request,
                offer_id="execute.compute",
            )
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=wasm_request,
                requester_key=requester_key,
                idempotency_key="protocol-data-deal",
            )

        terminal = await self.wait_for_deal(node, deal["deal_id"])

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(terminal["result"], 42)
        self.assertEqual(terminal["workload_kind"], "compute.wasm.v1")
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["runtime"],
            "wasm",
        )

    async def test_deal_rejection_emits_signed_terminal_receipt(self) -> None:
        node = await self.start_provider(
            extra_env={"FROGLET_WASM_CONCURRENCY_LIMIT": "1"}
        )

        async with aiohttp.ClientSession() as session:
            first_requester_key, first_quote = await self._create_quote(
                session,
                node,
                build_wasm_request(LONG_RUNNING_WASM_HEX),
            )
            second_requester_key, second_quote = await self._create_quote(
                session,
                node,
                build_wasm_request(VALID_WASM_HEX),
            )
            first_deal = await self._create_deal(
                session,
                node,
                quote=first_quote,
                request=build_wasm_request(LONG_RUNNING_WASM_HEX),
                requester_key=first_requester_key,
            )

            deadline = asyncio.get_running_loop().time() + 10
            while asyncio.get_running_loop().time() < deadline:
                async with session.get(
                    node.url(f"/v1/provider/deals/{first_deal['deal_id']}")
                ) as resp:
                    first_status = await resp.json()
                if first_status["status"] == "running":
                    break
                await asyncio.sleep(0.1)
            else:
                self.fail("first deal never entered running state")

            second_deal = await self._create_deal(
                session,
                node,
                quote=second_quote,
                request=build_wasm_request(VALID_WASM_HEX),
                requester_key=second_requester_key,
            )

            terminal = await self.wait_for_deal(node, second_deal["deal_id"])

            async with session.post(
                node.url("/v1/receipts/verify"),
                json={"receipt": terminal["receipt"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                verify_response = await resp.json()

        self.assertEqual(terminal["status"], "rejected")
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(terminal["receipt"]["payload"]["deal_state"], "rejected")
        self.assertEqual(terminal["receipt"]["payload"]["deal_hash"], terminal["deal"]["hash"])
        self.assertEqual(terminal["receipt"]["payload"]["execution_state"], "not_started")
        self.assertEqual(
            terminal["receipt"]["payload"]["failure_code"], "capacity_exhausted"
        )
        self.assertIsNone(terminal["receipt"]["payload"].get("result_hash"))
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["runtime"], "wasm"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["max_output_bytes"], 131072
        )
        self.assertTrue(verify_response["valid"])

    async def test_quote_workload_hash_is_stable_across_canonical_input_key_order(self) -> None:
        node = await self.start_provider()
        first_request = build_wasm_request(VALID_WASM_HEX, input={"b": 2, "a": 1})
        second_request = build_wasm_request(VALID_WASM_HEX, input={"a": 1, "b": 2})

        async with aiohttp.ClientSession() as session:
            _, first_quote = await self._create_quote(session, node, first_request)
            _, second_quote = await self._create_quote(session, node, second_request)

        self.assertEqual(
            first_quote["payload"]["workload_hash"],
            second_quote["payload"]["workload_hash"],
        )
        self.assertEqual(
            first_quote["payload"]["workload_hash"],
            workload_hash_from_submission(first_request["submission"]),
        )

    async def test_deal_persistence_keeps_submission_evidence_and_quote_hash(self) -> None:
        node = await self.start_provider()
        request = build_wasm_request(VALID_WASM_HEX, input={"job": "deal-persist"})

        async with aiohttp.ClientSession() as session:
            requester_key, quote = await self._create_quote(session, node, request)
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=request,
                requester_key=requester_key,
                idempotency_key="persisted-deal-evidence",
            )

        terminal = await self.wait_for_deal(node, deal["deal_id"])
        self.assertEqual(terminal["status"], "succeeded")

        workload_hash, spec_json, quote_json = read_db_row(
            node.data_dir / "node.db",
            "SELECT workload_hash, spec_json, quote_json FROM deals WHERE deal_id = ?",
            (deal["deal_id"],),
        )
        stored_spec = json.loads(spec_json)
        stored_quote = json.loads(quote_json)

        self.assertEqual(workload_hash, workload_hash_from_submission(request["submission"]))
        self.assertEqual(stored_spec["kind"], "wasm")
        self.assertEqual(stored_spec["submission"]["submission_type"], "wasm_submission")
        self.assertEqual(stored_quote["hash"], quote["hash"])
        artifact_rows = read_db_rows(
            node.data_dir / "node.db",
            "SELECT d.artifact_kind, d.artifact_hash, f.sequence FROM artifact_documents d JOIN artifact_feed f ON f.artifact_hash = d.artifact_hash WHERE d.artifact_hash IN (?, ?, ?) ORDER BY f.sequence ASC",
            (quote["hash"], terminal["deal"]["hash"], terminal["receipt"]["hash"]),
        )
        self.assertEqual([row[0] for row in artifact_rows], ["quote", "deal", "receipt"])
        self.assertEqual(len({row[2] for row in artifact_rows}), 3)
        evidence_kinds = {
            row[0]
            for row in read_db_rows(
                node.data_dir / "node.db",
                "SELECT evidence_kind FROM execution_evidence WHERE subject_kind = 'deal' AND subject_id = ? ORDER BY evidence_id ASC",
                (deal["deal_id"],),
            )
        }
        self.assertEqual(
            evidence_kinds,
            {
                "deal_artifact_ref",
                "execution_result",
                "quote_artifact_ref",
                "receipt_artifact_ref",
                "workload_spec",
            },
        )

    async def test_deal_and_quote_reads_prefer_retained_artifacts_over_cache_columns(self) -> None:
        node = await self.start_provider()
        request = build_wasm_request(VALID_WASM_HEX, input={"job": "reference-first-reads"})

        async with aiohttp.ClientSession() as session:
            requester_key, quote = await self._create_quote(session, node, request)
            deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=request,
                requester_key=requester_key,
                idempotency_key="reference-first-deal",
            )

        terminal = await self.wait_for_deal(node, deal["deal_id"])
        self.assertEqual(terminal["status"], "succeeded")

        execute_db(
            node.data_dir / "node.db",
            "UPDATE quotes SET quote_json = ? WHERE quote_id = ?",
            ("{", quote["hash"]),
        )
        execute_db(
            node.data_dir / "node.db",
            "UPDATE deals SET quote_json = ?, spec_json = ?, deal_artifact_json = ?, result_json = ?, receipt_artifact_json = ? WHERE deal_id = ?",
            ("{", "{", "{", "{", "{", deal["deal_id"]),
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url(f"/v1/provider/deals/{deal['deal_id']}")) as resp:
                reread = await resp.json()

            second_deal = await self._create_deal(
                session,
                node,
                quote=quote,
                request=request,
                requester_key=requester_key,
                idempotency_key="reference-first-deal-second",
            )

        self.assertEqual(reread["status"], "succeeded")
        self.assertEqual(reread["quote"]["hash"], quote["hash"])
        self.assertEqual(reread["deal"]["hash"], terminal["deal"]["hash"])
        self.assertEqual(reread["result"], 42)
        self.assertEqual(reread["receipt"]["hash"], terminal["receipt"]["hash"])

        second_terminal = await self.wait_for_deal(node, second_deal["deal_id"])
        self.assertEqual(second_terminal["status"], "succeeded")
        self.assertEqual(second_terminal["quote"]["hash"], quote["hash"])

    async def test_quote_rejects_unsupported_wasm_abi_version(self) -> None:
        node = await self.start_provider()
        request = build_wasm_request(VALID_WASM_HEX)
        request["submission"]["workload"]["abi_version"] = "froglet.wasm.run_json.v0"

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/provider/quotes"),
                json={
                    "offer_id": "execute.compute",
                    "requester_id": schnorr_pubkey_hex(generate_schnorr_signing_key()),
                    **request,
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("abi_version", payload["error"])
