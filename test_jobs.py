import json
import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    TRAPPING_WASM_HEX,
    VALID_CASHU_TOKEN,
    VALID_WASM_HEX,
    build_wasm_request,
    build_wasm_submission,
    read_db_row,
    workload_hash_from_submission,
)


class JobApiTests(FrogletAsyncTestCase):
    async def test_wasm_job_executes_and_idempotency_reuses_same_job(self) -> None:
        node = await self.start_node()
        request = build_wasm_request(VALID_WASM_HEX)
        request["idempotency_key"] = "wasm-hello-jobs"

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/jobs"), json=request) as resp:
                first_payload = await resp.json()
            async with session.post(node.url("/v1/node/jobs"), json=request) as resp:
                second_payload = await resp.json()

        self.assertEqual(first_payload["job_id"], second_payload["job_id"])
        self.assertIn(first_payload["status"], {"queued", "running"})
        completed = await self.wait_for_job(node, first_payload["job_id"])
        self.assertEqual(completed["status"], "succeeded")
        self.assertEqual(completed["result"], 42)

    async def test_job_idempotency_uses_canonical_workload_not_transport_hex_casing(self) -> None:
        node = await self.start_node()
        first_request = build_wasm_request(VALID_WASM_HEX, input={"b": 2, "a": 1})
        first_request["idempotency_key"] = "wasm-canonical-idempotency"
        second_request = build_wasm_request(VALID_WASM_HEX.upper(), input={"a": 1, "b": 2})
        second_request["idempotency_key"] = first_request["idempotency_key"]
        second_request["submission"]["workload"] = first_request["submission"]["workload"]

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/jobs"), json=first_request) as resp:
                first_payload = await resp.json()
            async with session.post(node.url("/v1/node/jobs"), json=second_request) as resp:
                second_payload = await resp.json()

        self.assertEqual(first_payload["job_id"], second_payload["job_id"])
        completed = await self.wait_for_job(node, first_payload["job_id"])
        self.assertEqual(completed["status"], "succeeded")

    async def test_job_persistence_keeps_wasm_submission_and_workload_hash(self) -> None:
        node = await self.start_node()
        submission = build_wasm_submission(VALID_WASM_HEX, input={"task": "persist"})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={
                    "kind": "wasm",
                    "submission": submission,
                    "idempotency_key": "persisted-wasm-job",
                },
            ) as resp:
                created = await resp.json()

        completed = await self.wait_for_job(node, created["job_id"])
        self.assertEqual(completed["status"], "succeeded")

        request_hash, payload_json = read_db_row(
            node.data_dir / "node.db",
            "SELECT request_hash, payload_json FROM jobs WHERE job_id = ?",
            (created["job_id"],),
        )
        stored_payload = json.loads(payload_json)

        self.assertEqual(request_hash, workload_hash_from_submission(submission))
        self.assertEqual(stored_payload["kind"], "wasm")
        self.assertEqual(stored_payload["submission"]["submission_type"], "wasm_submission")
        self.assertEqual(
            stored_payload["submission"]["workload"]["module_hash"],
            submission["workload"]["module_hash"],
        )

    async def test_job_rejects_input_hash_mismatch(self) -> None:
        node = await self.start_node()
        submission = build_wasm_submission(VALID_WASM_HEX, input={"answer": 42})
        submission["workload"]["input_hash"] = "33" * 32

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={"kind": "wasm", "submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("input hash", payload["error"].lower())

    async def test_failed_paid_job_releases_payment_reservation(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PRICE_EVENTS_QUERY": "10",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={
                    **build_wasm_request(TRAPPING_WASM_HEX),
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                failed_job = await resp.json()

            completed = await self.wait_for_job(node, failed_job["job_id"])
            async with session.post(
                node.url("/v1/node/events/query"),
                json={
                    "kinds": ["note"],
                    "limit": 1,
                    "payment": {"kind": "cashu", "token": VALID_CASHU_TOKEN},
                },
            ) as resp:
                query_payload = await resp.json()

        self.assertEqual(completed["status"], "failed")
        self.assertEqual(resp.status, 200)
        self.assertIn("events", query_payload)
        self.assertIsNotNone(query_payload["payment_receipt"])
        self.assertEqual(query_payload["payment_receipt"]["settlement_status"], "committed")


if __name__ == "__main__":
    unittest.main(verbosity=2)
