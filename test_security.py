import json
import time
import unittest
from io import BytesIO

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    PUBKEY_HEX,
    canonical_event_signing_bytes,
    create_signed_event,
    generate_schnorr_signing_key,
    schnorr_pubkey_hex,
    schnorr_sign_message,
)

ATTACKER_KEY = generate_schnorr_signing_key()
ATTACKER_PUBKEY = schnorr_pubkey_hex(ATTACKER_KEY)
VICTIM_KEY = generate_schnorr_signing_key()


class SecurityApiTests(FrogletAsyncTestCase):
    async def test_rejects_tampered_signature(self) -> None:
        node = await self.start_node()
        event = create_signed_event("legitimate offer")
        event["content"] = "malicious offer"

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/events/publish"), json={"event": event}) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("invalid signature", payload["error"])

    async def test_rejects_spoofed_pubkey(self) -> None:
        node = await self.start_node()
        content = "impersonation attack"
        event = {
            "id": __import__("hashlib").sha256(content.encode("utf-8")).hexdigest(),
            "pubkey": ATTACKER_PUBKEY,
            "created_at": int(time.time()),
            "kind": "market.listing",
            "tags": [["t", "test"]],
            "content": content,
        }
        event["sig"] = schnorr_sign_message(VICTIM_KEY, canonical_event_signing_bytes(event))

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/events/publish"), json={"event": event}) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("invalid signature", payload["error"])

    async def test_sql_injection_payload_is_stored_as_data(self) -> None:
        node = await self.start_node()
        event = create_signed_event("'; DROP TABLE events; --")

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/events/publish"), json={"event": event}) as resp:
                publish_payload = await resp.json()
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["market.listing"], "limit": 10},
            ) as resp:
                query_payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(publish_payload["status"], "success")
        self.assertTrue(any(item["content"] == "'; DROP TABLE events; --" for item in query_payload["events"]))

    async def test_malformed_json_is_rejected(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                data='{"event": {"id": "123", "pubkey": ',
                headers={"Content-Type": "application/json"},
            ) as resp:
                body = await resp.text()

        self.assertIn(resp.status, [400, 422])
        self.assertTrue(body)

    async def test_missing_required_fields_are_rejected(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/events/publish"),
                json={"event": {"id": "123"}},
            ) as resp:
                body = await resp.text()

        self.assertIn(resp.status, [400, 422])
        self.assertTrue(body)

    async def test_large_payload_is_rejected(self) -> None:
        node = await self.start_node()
        huge_event = {
            "event": {
                "id": "massive",
                "pubkey": PUBKEY_HEX,
                "created_at": int(time.time()),
                "kind": "market.listing",
                "tags": [],
                "content": "A" * 3_000_000,
                "sig": "00" * 64,
            }
        }
        encoded_body = json.dumps(huge_event).encode("utf-8")

        async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=15)) as session:
            try:
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    data=BytesIO(encoded_body),
                    headers={"Content-Type": "application/json"},
                ) as resp:
                    body = await resp.text()
                self.assertIn(resp.status, [400, 413])
                self.assertTrue(body)
            except aiohttp.ClientOSError as exc:
                self.assertRegex(
                    str(exc),
                    r"Broken pipe|Connection reset by peer",
                )

    async def test_absurd_query_limit_is_clamped_without_crash(self) -> None:
        node = await self.start_node()
        event = create_signed_event("query seed")

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/events/publish"), json={"event": event}) as resp:
                self.assertEqual(resp.status, 201)
            async with session.post(
                node.url("/v1/node/events/query"),
                json={"kinds": ["market.listing"], "limit": 9_999_999_999},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertIn("events", payload)


if __name__ == "__main__":
    unittest.main(verbosity=2)
