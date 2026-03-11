import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_CASHU_TOKEN


class JobApiTests(FrogletAsyncTestCase):
    async def test_lua_job_executes_and_idempotency_reuses_same_job(self) -> None:
        node = await self.start_node()
        request = {
            "kind": "lua",
            "script": "return input.greeting .. ', ' .. input.target",
            "input": {"greeting": "hello", "target": "jobs"},
            "idempotency_key": "lua-hello-jobs",
        }

        async with aiohttp.ClientSession() as session:
            async with session.post(node.url("/v1/node/jobs"), json=request) as resp:
                first_payload = await resp.json()
            async with session.post(node.url("/v1/node/jobs"), json=request) as resp:
                second_payload = await resp.json()

        self.assertEqual(first_payload["job_id"], second_payload["job_id"])
        self.assertIn(first_payload["status"], {"queued", "running"})
        completed = await self.wait_for_job(node, first_payload["job_id"])
        self.assertEqual(completed["status"], "succeeded")
        self.assertEqual(completed["result"], "hello, jobs")

    async def test_failed_paid_job_releases_payment_reservation(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_LUA": "10",
                "FROGLET_PRICE_EVENTS_QUERY": "10",
            }
        )

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/jobs"),
                json={
                    "kind": "lua",
                    "script": "return io.open('/etc/passwd', 'r')",
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
