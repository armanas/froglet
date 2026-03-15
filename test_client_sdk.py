import time
import unittest

from froglet_client import (
    MarketplaceClient,
    ProviderClient,
    RuntimeClient,
    generate_requester_seed,
    requester_id_from_seed,
    runtime_requester_fields,
)
from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    sign_deal_artifact_from_quote,
    sha256_hex,
    verify_signed_artifact,
)


class ClientSdkTests(FrogletAsyncTestCase):
    async def test_provider_client_supports_quote_deal_wait_accept_flow(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        runtime = RuntimeClient.from_token_file(
            node.runtime_url,
            node.data_dir / "runtime" / "auth.token",
            provider_base_url=node.base_url,
        )
        requester_key = generate_requester_seed()
        requester_id = requester_id_from_seed(requester_key)
        success_preimage = "11" * 32
        success_payment_hash = sha256_hex(bytes.fromhex(success_preimage))
        request = build_wasm_request(VALID_WASM_HEX)

        async with ProviderClient(node.base_url) as provider:
            quote = await provider.create_quote(
                "execute.wasm", request, requester_id=requester_id
            )
            signed_deal = sign_deal_artifact_from_quote(
                quote,
                requester_key,
                success_payment_hash=success_payment_hash,
            )
            deal = await provider.create_deal(
                quote,
                signed_deal,
                request,
                idempotency_key="sdk-provider-flow",
            )
            bundle = await provider.get_invoice_bundle(deal["deal_id"])
            validation = await provider.verify_invoice_bundle(
                bundle["bundle"],
                quote,
                deal["deal"],
                requester_id=requester_id,
            )

            async with runtime:
                await runtime.set_mock_lightning_state(
                    bundle["session_id"],
                    base_state="settled",
                    success_state="accepted",
                )
                result_ready = await provider.wait_for_deal(
                    deal["deal_id"], statuses={"result_ready"}
                )
                terminal = await runtime.accept_result(
                    deal["deal_id"], success_preimage
                )
                receipt_verification = await runtime.verify_receipt(terminal["receipt"])

        self.assertTrue(validation["valid"])
        self.assertEqual(result_ready["status"], "result_ready")
        self.assertEqual(terminal["status"], "succeeded")
        self.assertTrue(verify_signed_artifact(terminal["receipt"]))
        self.assertTrue(receipt_verification["valid"])

    async def test_runtime_client_hides_payment_intent_unless_requested(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
            }
        )
        runtime = RuntimeClient.from_token_file(
            node.runtime_url,
            node.data_dir / "runtime" / "auth.token",
            provider_base_url=node.base_url,
        )

        async with runtime:
            hidden_key = generate_requester_seed()
            hidden = await runtime.buy_service(
                {
                    "offer_id": "execute.wasm",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "sdk-runtime-hidden",
                    **runtime_requester_fields(
                        hidden_key, b"sdk-runtime-hidden".rjust(32, b"\0")
                    ),
                }
            )
            visible_key = generate_requester_seed()
            visible = await runtime.buy_service(
                {
                    "offer_id": "execute.wasm",
                    **build_wasm_request(VALID_WASM_HEX),
                    "idempotency_key": "sdk-runtime-visible",
                    **runtime_requester_fields(
                        visible_key, b"sdk-runtime-visible".rjust(32, b"\0")
                    ),
                },
                include_payment_intent=True,
            )
            intent = await runtime.payment_intent(visible.deal["deal_id"])

        self.assertFalse(hidden.terminal)
        self.assertIsNone(hidden.payment_intent)
        self.assertIsNotNone(hidden.payment_intent_path)
        self.assertFalse(visible.terminal)
        self.assertIsNotNone(visible.payment_intent)
        self.assertEqual(intent["deal_id"], visible.deal["deal_id"])
        self.assertEqual(intent["bundle_hash"], visible.payment_intent["bundle_hash"])

    async def test_runtime_client_issues_curated_list_and_provider_client_verifies_it(self) -> None:
        node = await self.start_node()
        runtime = RuntimeClient.from_token_file(
            node.runtime_url,
            node.data_dir / "runtime" / "auth.token",
            provider_base_url=node.base_url,
        )

        async with ProviderClient(node.base_url) as provider:
            descriptor = await provider.descriptor()
            async with runtime:
                curated_list = await runtime.issue_curated_list(
                    list_id="sdk-curated-list",
                    expires_at=int(time.time()) + 60,
                    entries=[
                        {
                            "provider_id": descriptor["payload"]["provider_id"],
                            "descriptor_hash": descriptor["hash"],
                            "tags": ["bootstrap", "local"],
                            "note": "local test node",
                        }
                    ],
                )
            verification = await provider.verify_curated_list(curated_list)

        self.assertTrue(verify_signed_artifact(curated_list))
        self.assertEqual(curated_list["payload"]["list_id"], "sdk-curated-list")
        self.assertEqual(curated_list["payload"]["entries"][0]["descriptor_hash"], descriptor["hash"])
        self.assertTrue(verification["valid"])
        self.assertEqual(verification["list_id"], "sdk-curated-list")

    async def test_runtime_client_builds_nostr_publications_and_provider_verifies_them(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )
        runtime = RuntimeClient.from_token_file(
            node.runtime_url,
            node.data_dir / "runtime" / "auth.token",
            provider_base_url=node.base_url,
        )
        success_preimage = "22" * 32

        async with ProviderClient(node.base_url) as provider:
            async with runtime:
                descriptor = await provider.descriptor()
                publications = await runtime.nostr_provider_publications()
                descriptor_verification = await provider.verify_nostr_event(
                    publications["descriptor_summary"]
                )

                requester_key = generate_requester_seed()
                handle = await runtime.buy_service(
                    {
                        "offer_id": "execute.wasm",
                        **build_wasm_request(VALID_WASM_HEX),
                        "idempotency_key": "sdk-nostr-receipt",
                        **runtime_requester_fields(
                            requester_key,
                            bytes.fromhex(success_preimage),
                        ),
                    },
                    include_payment_intent=True,
                )
                await runtime.set_mock_lightning_state(
                    handle.payment_intent["session_id"],
                    base_state="settled",
                    success_state="accepted",
                )
                result_ready = await runtime.wait_for_deal(
                    handle.deal["deal_id"], statuses={"result_ready"}
                )
                await runtime.accept_result(
                    handle.deal["deal_id"],
                    success_preimage,
                    expected_result_hash=result_ready["result_hash"],
                )
                receipt_publication = await runtime.nostr_receipt_publication(
                    handle.deal["deal_id"]
                )

            receipt_verification = await provider.verify_nostr_event(
                receipt_publication["receipt_summary"]
            )

        publication_identity = descriptor["payload"]["linked_identities"][0]["identity"]
        self.assertEqual(publications["descriptor_summary"]["pubkey"], publication_identity)
        self.assertNotEqual(publication_identity, descriptor["signer"])
        self.assertTrue(descriptor_verification["valid"])
        self.assertGreaterEqual(len(publications["offer_summaries"]), 1)
        self.assertTrue(receipt_verification["valid"])
        self.assertEqual(receipt_publication["receipt_summary"]["kind"], 1390)
        self.assertEqual(receipt_publication["receipt_summary"]["pubkey"], publication_identity)

    async def test_marketplace_client_searches_registered_nodes(self) -> None:
        marketplace = await self.start_marketplace()
        node = await self.start_node(
            extra_env={
                "FROGLET_DISCOVERY_MODE": "marketplace",
                "FROGLET_MARKETPLACE_URL": marketplace.base_url,
                "FROGLET_MARKETPLACE_PUBLISH": "true",
            }
        )

        async with MarketplaceClient(marketplace.base_url) as client:
            nodes = await client.search_nodes(limit=10)

        self.assertTrue(nodes)
        self.assertTrue(any(entry["descriptor"]["node_id"] for entry in nodes))
        self.assertTrue(any(entry["descriptor"]["transports"]["clearnet_url"] == node.base_url for entry in nodes))


if __name__ == "__main__":
    unittest.main(verbosity=2)
