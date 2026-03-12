import asyncio
import json

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    LONG_RUNNING_WASM_HEX,
    VALID_CASHU_TOKEN,
    VALID_WASM_HEX,
    build_wasm_request,
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


class ProtocolPrimitiveTests(FrogletAsyncTestCase):
    async def test_descriptor_offers_feed_and_artifact_fetch_are_signed(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url("/v1/descriptor")) as resp:
                self.assertEqual(resp.status, 200)
                descriptor = await resp.json()

            async with session.get(node.url("/v1/offers")) as resp:
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
        self.assertEqual(descriptor["kind"], "descriptor")
        self.assertEqual(descriptor["payload"]["protocol_version"], "v0.2")
        self.assertEqual(descriptor["payload"]["feeds"]["cursor_type"], "artifact_sequence")
        self.assertEqual(
            descriptor["payload"]["feeds"]["cursor_semantics"], "exclusive_after"
        )
        self.assertEqual(descriptor["payload"]["feeds"]["feed_path"], "/v1/feed")
        self.assertEqual(
            descriptor["payload"]["feeds"]["artifact_path_template"],
            "/v1/artifacts/{artifact_hash}",
        )
        self.assertEqual(descriptor["payload"]["feeds"]["max_page_size"], 100)

        offers = offers_payload["offers"]
        self.assertEqual(len(offers), 2)
        self.assertTrue(all(verify_signed_artifact(offer) for offer in offers))
        self.assertEqual(
            {offer["payload"]["offer_id"] for offer in offers},
            {"events.query", "execute.wasm"},
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
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "cashu",
            }
        )

        wasm_request = build_wasm_request(VALID_WASM_HEX)
        quote_request = {"offer_id": "execute.wasm", **wasm_request}

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/quotes"), json=quote_request) as resp:
                self.assertEqual(resp.status, 201)
                quote = await resp.json()

            self.assertTrue(verify_signed_artifact(quote))
            self.assertEqual(quote["payload"]["price_sats"], 10)
            self.assertEqual(quote["payload"]["workload_kind"], "compute.wasm.v1")

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": quote,
                    **wasm_request,
                    "idempotency_key": "protocol-compute-deal",
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                deal = await resp.json()

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
        self.assertEqual(terminal["receipt"]["payload"]["status"], "succeeded")
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["reserved_amount_sats"], 10
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["status"], "committed"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["committed_amount_sats"], 10
        )
        self.assertEqual(terminal["receipt"]["payload"]["amount_paid_sats"], 10)
        self.assertEqual(terminal["receipt"]["payload"]["deal_hash"], terminal["deal"]["hash"])
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
        self.assertIsNone(terminal["receipt"]["payload"].get("failure"))
        self.assertTrue(verify_response["valid"])
        self.assertIn(
            terminal["receipt"]["hash"],
            {artifact["hash"] for artifact in feed["artifacts"]},
        )

    async def test_lightning_deal_waits_for_invoice_bundle_funding(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        requester_id = schnorr_pubkey_hex(generate_schnorr_signing_key())
        success_payment_hash = sha256_hex(b"protocol-lightning-success")

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 201)
                quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": quote,
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "protocol-lightning-deal",
                    "requester_id": requester_id,
                    "success_payment_hash": success_payment_hash,
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                deal = await resp.json()

            self.assertEqual(deal["status"], "payment_pending")

            async with session.get(
                node.url(f"/v1/deals/{deal['deal_id']}/invoice-bundle")
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
                bundle["bundle"]["payload"]["success_hold_invoice"]["payment_hash"],
                success_payment_hash,
            )
            self.assertTrue(validation["valid"])
            self.assertEqual(validation["issues"], [])

            async with session.post(
                node.url(
                    f"/v1/runtime/lightning/invoice-bundles/{bundle['session_id']}/state"
                ),
                headers=runtime_auth_headers(node),
                json={"base_state": "settled", "success_state": "accepted"},
            ) as resp:
                self.assertEqual(resp.status, 200)
                updated = await resp.json()

            self.assertEqual(updated["base_state"], "settled")
            self.assertEqual(updated["success_state"], "accepted")

            terminal = await self.wait_for_deal(node, deal["deal_id"])

            async with session.get(
                node.url(f"/v1/deals/{deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                settled_bundle = await resp.json()

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(terminal["result"], 42)
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["method"], "lightning"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["status"], "committed"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["settlement_reference"],
            bundle["session_id"],
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["settlement"]["payment_lock"]["token_hash"],
            success_payment_hash,
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["executor"]["runtime"], "wasm"
        )
        self.assertEqual(
            terminal["receipt"]["payload"]["limits_applied"]["fuel_limit"], 50000000
        )
        self.assertEqual(settled_bundle["success_state"], "settled")

    async def test_lightning_invoice_bundle_rejects_mismatched_stored_bundle(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        first_requester_id = schnorr_pubkey_hex(generate_schnorr_signing_key())
        second_requester_id = schnorr_pubkey_hex(generate_schnorr_signing_key())

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 201)
                first_quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": first_quote,
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "protocol-lightning-mismatch-1",
                    "requester_id": first_requester_id,
                    "success_payment_hash": sha256_hex(b"protocol-lightning-mismatch-1"),
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                first_deal = await resp.json()

            async with session.get(
                node.url(f"/v1/deals/{first_deal['deal_id']}/invoice-bundle")
            ) as resp:
                self.assertEqual(resp.status, 200)
                first_bundle = await resp.json()

            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 201)
                second_quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": second_quote,
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "protocol-lightning-mismatch-2",
                    "requester_id": second_requester_id,
                    "success_payment_hash": sha256_hex(b"protocol-lightning-mismatch-2"),
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                second_deal = await resp.json()

            async with session.get(
                node.url(f"/v1/deals/{second_deal['deal_id']}/invoice-bundle")
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
                node.url(f"/v1/deals/{first_deal['deal_id']}/invoice-bundle")
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
        node = await self.start_node()
        event = create_signed_event("hello data deal", kind="protocol.test")

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                json={"event": event},
            ) as resp:
                self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/quotes"),
                json={
                    "offer_id": "events.query",
                    "kind": "events_query",
                    "kinds": ["protocol.test"],
                    "limit": 1,
                },
            ) as resp:
                self.assertEqual(resp.status, 201)
                quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": quote,
                    "kind": "events_query",
                    "kinds": ["protocol.test"],
                    "limit": 1,
                    "idempotency_key": "protocol-data-deal",
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                deal = await resp.json()

        terminal = await self.wait_for_deal(node, deal["deal_id"])

        self.assertEqual(terminal["status"], "succeeded")
        self.assertEqual(len(terminal["result"]["events"]), 1)
        self.assertEqual(terminal["result"]["events"][0]["content"], "hello data deal")
        self.assertEqual(terminal["receipt"]["payload"]["service_id"], "events.query")

    async def test_deal_rejection_emits_signed_terminal_receipt(self) -> None:
        node = await self.start_node(
            extra_env={"FROGLET_WASM_CONCURRENCY_LIMIT": "1"}
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **build_wasm_request(LONG_RUNNING_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 201)
                first_quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={"quote": first_quote, **build_wasm_request(LONG_RUNNING_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 202)
                first_deal = await resp.json()

            deadline = asyncio.get_running_loop().time() + 10
            while asyncio.get_running_loop().time() < deadline:
                async with session.get(
                    node.url(f"/v1/deals/{first_deal['deal_id']}")
                ) as resp:
                    first_status = await resp.json()
                if first_status["status"] == "running":
                    break
                await asyncio.sleep(0.1)
            else:
                self.fail("first deal never entered running state")

            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 201)
                second_quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={"quote": second_quote, **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                self.assertEqual(resp.status, 202)
                second_deal = await resp.json()

            terminal = await self.wait_for_deal(node, second_deal["deal_id"])

            async with session.post(
                node.url("/v1/receipts/verify"),
                json={"receipt": terminal["receipt"]},
            ) as resp:
                self.assertEqual(resp.status, 200)
                verify_response = await resp.json()

        self.assertEqual(terminal["status"], "rejected")
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertEqual(terminal["receipt"]["payload"]["status"], "rejected")
        self.assertEqual(terminal["receipt"]["payload"]["deal_hash"], terminal["deal"]["hash"])
        self.assertIsNone(terminal["receipt"]["payload"].get("settlement"))
        self.assertEqual(
            terminal["receipt"]["payload"]["failure"]["code"], "capacity_exhausted"
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
        node = await self.start_node()
        first_request = build_wasm_request(VALID_WASM_HEX, input={"b": 2, "a": 1})
        second_request = build_wasm_request(VALID_WASM_HEX, input={"a": 1, "b": 2})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **first_request},
            ) as resp:
                first_quote = await resp.json()
            self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **second_request},
            ) as resp:
                second_quote = await resp.json()
            self.assertEqual(resp.status, 201)

        self.assertEqual(
            first_quote["payload"]["workload_hash"],
            second_quote["payload"]["workload_hash"],
        )
        self.assertEqual(
            first_quote["payload"]["workload_hash"],
            workload_hash_from_submission(first_request["submission"]),
        )

    async def test_deal_persistence_keeps_submission_evidence_and_quote_hash(self) -> None:
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX, input={"job": "deal-persist"})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **request},
            ) as resp:
                quote = await resp.json()
            self.assertEqual(resp.status, 201)

            async with session.post(
                node.url("/v1/deals"),
                json={"quote": quote, **request, "idempotency_key": "persisted-deal-evidence"},
            ) as resp:
                deal = await resp.json()
            self.assertEqual(resp.status, 202)

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
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX, input={"job": "reference-first-reads"})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **request},
            ) as resp:
                self.assertEqual(resp.status, 201)
                quote = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={"quote": quote, **request, "idempotency_key": "reference-first-deal"},
            ) as resp:
                self.assertEqual(resp.status, 202)
                deal = await resp.json()

        terminal = await self.wait_for_deal(node, deal["deal_id"])
        self.assertEqual(terminal["status"], "succeeded")

        execute_db(
            node.data_dir / "node.db",
            "UPDATE quotes SET quote_json = ? WHERE quote_id = ?",
            ("{", quote["payload"]["quote_id"]),
        )
        execute_db(
            node.data_dir / "node.db",
            "UPDATE deals SET quote_json = ?, spec_json = ?, deal_artifact_json = ?, result_json = ?, receipt_artifact_json = ? WHERE deal_id = ?",
            ("{", "{", "{", "{", "{", deal["deal_id"]),
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(node.url(f"/v1/deals/{deal['deal_id']}")) as resp:
                reread = await resp.json()

            async with session.post(
                node.url("/v1/deals"),
                json={
                    "quote": quote,
                    **request,
                    "idempotency_key": "reference-first-deal-second",
                },
            ) as resp:
                self.assertEqual(resp.status, 202)
                second_deal = await resp.json()

        self.assertEqual(reread["status"], "succeeded")
        self.assertEqual(reread["quote"]["hash"], quote["hash"])
        self.assertEqual(reread["deal"]["hash"], terminal["deal"]["hash"])
        self.assertEqual(reread["result"], 42)
        self.assertEqual(reread["receipt"]["hash"], terminal["receipt"]["hash"])

        second_terminal = await self.wait_for_deal(node, second_deal["deal_id"])
        self.assertEqual(second_terminal["status"], "succeeded")
        self.assertEqual(second_terminal["quote"]["hash"], quote["hash"])

    async def test_quote_rejects_unsupported_wasm_abi_version(self) -> None:
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX)
        request["submission"]["workload"]["abi_version"] = "froglet.wasm.run_json.v0"

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/quotes"),
                json={"offer_id": "execute.wasm", **request},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("abi_version", payload["error"])
