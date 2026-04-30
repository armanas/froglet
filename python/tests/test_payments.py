import unittest
import hashlib
import hmac
import json
import time

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_WASM_HEX, build_wasm_request


class PaymentEnforcementTests(FrogletAsyncTestCase):
    @staticmethod
    def stripe_signature(secret: str, body: bytes, timestamp: int | None = None) -> str:
        timestamp = timestamp or int(time.time())
        signed_payload = str(timestamp).encode("utf-8") + b"." + body
        digest = hmac.new(secret.encode("utf-8"), signed_payload, hashlib.sha256).hexdigest()
        return f"t={timestamp},v1={digest}"

    async def test_priced_services_require_explicit_lightning_mode(self) -> None:
        with self.assertRaisesRegex(
            RuntimeError,
            "FROGLET_LIGHTNING_MODE is required whenever Lightning payments are active",
        ):
            await self.start_provider(extra_env={"FROGLET_PRICE_EVENTS_QUERY": "10"})

    async def test_lightning_priced_query_requires_protocol_deal_flow(self) -> None:
        provider = await self.start_provider(
            extra_env={
                "FROGLET_PRICE_EVENTS_QUERY": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                provider.url("/v1/node/events/query"),
                json={"kinds": ["note"], "limit": 1},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "events.query")
        self.assertEqual(payload["payment_backend"], "lightning")
        self.assertEqual(payload["quote_path"], "/v1/provider/quotes")
        self.assertEqual(payload["deal_path"], "/v1/provider/deals")

    async def test_lightning_priced_execute_helper_requires_protocol_deal_flow(self) -> None:
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/node/execute/wasm"),
                json=build_wasm_request(VALID_WASM_HEX),
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "execute.compute")
        self.assertEqual(payload["legacy_endpoint"], "/v1/node/execute/wasm")

    async def test_lightning_priced_job_helper_requires_protocol_deal_flow(self) -> None:
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/node/jobs"),
                json={"idempotency_key": "legacy-job-helper", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_protocol_deal"])
        self.assertEqual(payload["service_id"], "execute.compute")
        self.assertEqual(payload["legacy_endpoint"], "/v1/node/jobs")

    async def test_x402_priced_job_helper_requires_synchronous_settlement(self) -> None:
        runtime = await self.start_runtime(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "x402",
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/node/jobs"),
                json={"idempotency_key": "paid-x402-job", **build_wasm_request(VALID_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 409)
        self.assertTrue(payload["requires_synchronous_settlement"])
        self.assertEqual(payload["service_id"], "execute.compute")
        self.assertEqual(payload["accepted_payment_methods"], ["x402_usdc"])
        self.assertEqual(payload["legacy_endpoint"], "/v1/node/jobs")
        self.assertIn("/v1/node/execute/wasm", payload["synchronous_endpoints"])

    async def test_stripe_webhook_verifies_signature_and_deduplicates_event_id(self) -> None:
        secret = "whsec_test_secret"
        provider = await self.start_provider(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "stripe",
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_dummy",
                "FROGLET_STRIPE_WEBHOOK_SECRET": secret,
            }
        )
        body = json.dumps(
            {
                "id": "evt_test_123",
                "object": "event",
                "type": "payment_intent.succeeded",
                "data": {
                    "object": {
                        "id": "pi_test_123",
                        "object": "payment_intent",
                        "status": "succeeded",
                    }
                },
            },
            separators=(",", ":"),
        ).encode("utf-8")
        headers = {
            "content-type": "application/json",
            "stripe-signature": self.stripe_signature(secret, body),
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(
                provider.url("/v1/webhooks/stripe"),
                data=body,
                headers=headers,
            ) as resp:
                first_payload = await resp.json()
            async with session.post(
                provider.url("/v1/webhooks/stripe"),
                data=body,
                headers=headers,
            ) as resp2:
                second_payload = await resp2.json()

        self.assertEqual(resp.status, 200)
        self.assertFalse(first_payload["duplicate"])
        self.assertTrue(first_payload["processed"])
        self.assertEqual(first_payload["event_id"], "evt_test_123")
        self.assertEqual(first_payload["event_type"], "payment_intent.succeeded")
        self.assertEqual(first_payload["payment_intent_id"], "pi_test_123")
        self.assertEqual(resp2.status, 200)
        self.assertTrue(second_payload["duplicate"])
        self.assertFalse(second_payload["processed"])

    async def test_stripe_webhook_rejects_invalid_signature(self) -> None:
        provider = await self.start_provider(
            extra_env={
                "FROGLET_PAYMENT_BACKEND": "stripe",
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_dummy",
                "FROGLET_STRIPE_WEBHOOK_SECRET": "whsec_test_secret",
            }
        )
        body = b'{"id":"evt_bad","object":"event","type":"payment_intent.succeeded"}'
        async with aiohttp.ClientSession() as session:
            async with session.post(
                provider.url("/v1/webhooks/stripe"),
                data=body,
                headers={
                    "content-type": "application/json",
                    "stripe-signature": "t=1,v1=not-a-valid-signature",
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertEqual(payload["error"], "invalid stripe webhook signature")


if __name__ == "__main__":
    unittest.main(verbosity=2)
