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
    read_db_row,
    verify_signed_artifact,
    workload_hash_from_submission,
)


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

        quote_request = {"offer_id": "execute.wasm", **build_wasm_request(VALID_WASM_HEX)}

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
                    **build_wasm_request(VALID_WASM_HEX),
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
        self.assertIsNone(terminal["receipt"]["payload"].get("failure"))
        self.assertTrue(verify_response["valid"])
        self.assertIn(
            terminal["receipt"]["hash"],
            {artifact["hash"] for artifact in feed["artifacts"]},
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
        self.assertIsNone(terminal["receipt"]["payload"].get("settlement"))
        self.assertEqual(
            terminal["receipt"]["payload"]["failure"]["code"], "capacity_exhausted"
        )
        self.assertIsNone(terminal["receipt"]["payload"].get("result_hash"))
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
