import json
import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    TRAPPING_WASM_HEX,
    VALID_WASM_HEX,
    build_wasm_request,
    build_wasm_submission,
    execute_db,
    read_db_row,
    read_db_rows,
    workload_hash_from_submission,
)


class JobApiTests(FrogletAsyncTestCase):
    async def test_wasm_job_executes_and_idempotency_reuses_same_job(self) -> None:
        runtime = await self.start_runtime()
        request = build_wasm_request(VALID_WASM_HEX)
        request["idempotency_key"] = "wasm-hello-jobs"

        async with aiohttp.ClientSession() as session:
            async with session.post(runtime.url("/v1/node/jobs"), json=request) as resp:
                first_payload = await resp.json()
            async with session.post(runtime.url("/v1/node/jobs"), json=request) as resp:
                second_payload = await resp.json()

        self.assertEqual(first_payload["job_id"], second_payload["job_id"])
        self.assertIn(first_payload["status"], {"queued", "running"})
        completed = await self.wait_for_runtime_job(runtime, first_payload["job_id"])
        self.assertEqual(completed["status"], "succeeded")
        self.assertEqual(completed["result"], 42)

    async def test_job_idempotency_uses_canonical_workload_not_transport_hex_casing(self) -> None:
        runtime = await self.start_runtime()
        first_request = build_wasm_request(VALID_WASM_HEX, input={"b": 2, "a": 1})
        first_request["idempotency_key"] = "wasm-canonical-idempotency"
        second_request = build_wasm_request(VALID_WASM_HEX.upper(), input={"a": 1, "b": 2})
        second_request["idempotency_key"] = first_request["idempotency_key"]
        second_request["submission"]["workload"] = first_request["submission"]["workload"]

        async with aiohttp.ClientSession() as session:
            async with session.post(runtime.url("/v1/node/jobs"), json=first_request) as resp:
                first_payload = await resp.json()
            async with session.post(runtime.url("/v1/node/jobs"), json=second_request) as resp:
                second_payload = await resp.json()

        self.assertEqual(first_payload["job_id"], second_payload["job_id"])
        completed = await self.wait_for_runtime_job(runtime, first_payload["job_id"])
        self.assertEqual(completed["status"], "succeeded")

    async def test_job_persistence_keeps_wasm_submission_and_workload_hash(self) -> None:
        runtime = await self.start_runtime()
        submission = build_wasm_submission(VALID_WASM_HEX, input={"task": "persist"})

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/node/jobs"),
                json={
                    "kind": "wasm",
                    "submission": submission,
                    "idempotency_key": "persisted-wasm-job",
                },
            ) as resp:
                created = await resp.json()

        completed = await self.wait_for_runtime_job(runtime, created["job_id"])
        self.assertEqual(completed["status"], "succeeded")

        request_hash, payload_json = read_db_row(
            runtime.data_dir / "node.db",
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
        evidence_kinds = {
            row[0]
            for row in read_db_rows(
                runtime.data_dir / "node.db",
                "SELECT evidence_kind FROM execution_evidence WHERE subject_kind = 'job' AND subject_id = ? ORDER BY evidence_id ASC",
                (created["job_id"],),
            )
        }
        self.assertEqual(evidence_kinds, {"execution_result", "workload_spec"})

    async def test_job_reads_from_evidence_when_cache_columns_are_corrupted(self) -> None:
        runtime = await self.start_runtime()
        request = build_wasm_request(VALID_WASM_HEX, input={"task": "cache-corruption"})

        async with aiohttp.ClientSession() as session:
            async with session.post(runtime.url("/v1/node/jobs"), json=request) as resp:
                created = await resp.json()

        completed = await self.wait_for_runtime_job(runtime, created["job_id"])
        self.assertEqual(completed["status"], "succeeded")

        execute_db(
            runtime.data_dir / "node.db",
            "UPDATE jobs SET payload_json = ?, result_json = ? WHERE job_id = ?",
            ("{", "{", created["job_id"]),
        )

        async with aiohttp.ClientSession() as session:
            async with session.get(runtime.url(f"/v1/node/jobs/{created['job_id']}")) as resp:
                reread = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(reread["status"], "succeeded")
        self.assertEqual(reread["kind"], "wasm")
        self.assertEqual(reread["result"], 42)

    async def test_job_rejects_input_hash_mismatch(self) -> None:
        runtime = await self.start_runtime()
        submission = build_wasm_submission(VALID_WASM_HEX, input={"answer": 42})
        submission["workload"]["input_hash"] = "33" * 32

        async with aiohttp.ClientSession() as session:
            async with session.post(
                runtime.url("/v1/node/jobs"),
                json={"kind": "wasm", "submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("input hash", payload["error"].lower())

if __name__ == "__main__":
    unittest.main(verbosity=2)
