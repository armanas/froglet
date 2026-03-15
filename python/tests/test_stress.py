import asyncio
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, create_signed_event

CONCURRENCY = 40
TOTAL_REQUESTS = 300
QUERY_WORKERS = 10
QUERIES_PER_WORKER = 20


class StressTests(FrogletAsyncTestCase):
    async def test_concurrent_publish_and_query_load(self) -> None:
        node = await self.start_node()
        queue: asyncio.Queue[dict] = asyncio.Queue()
        for i in range(TOTAL_REQUESTS):
            queue.put_nowait({"event": create_signed_event(f"stress-{i}", kind="stress.test")})

        results = {"publish_ok": 0, "publish_fail": 0, "query_ok": 0, "query_fail": 0, "errors": []}

        async with aiohttp.ClientSession(connector=aiohttp.TCPConnector(limit=CONCURRENCY)) as session:
            async def bombard_publish() -> None:
                while not queue.empty():
                    try:
                        request = queue.get_nowait()
                    except asyncio.QueueEmpty:
                        return
                    try:
                        async with session.post(node.url("/v1/node/events/publish"), json=request) as resp:
                            if resp.status == 201:
                                results["publish_ok"] += 1
                            else:
                                results["publish_fail"] += 1
                                results["errors"].append(await resp.text())
                    except Exception as exc:  # pragma: no cover - load-path diagnostics
                        results["publish_fail"] += 1
                        results["errors"].append(str(exc))
                    finally:
                        queue.task_done()

            async def bombard_query() -> None:
                for _ in range(QUERIES_PER_WORKER):
                    try:
                        async with session.post(
                            node.url("/v1/node/events/query"),
                            json={"kinds": ["stress.test"], "limit": 10},
                        ) as resp:
                            if resp.status == 200:
                                results["query_ok"] += 1
                            else:
                                results["query_fail"] += 1
                                results["errors"].append(await resp.text())
                    except Exception as exc:  # pragma: no cover - load-path diagnostics
                        results["query_fail"] += 1
                        results["errors"].append(str(exc))

            tasks = [asyncio.create_task(bombard_publish()) for _ in range(CONCURRENCY)]
            tasks.extend(asyncio.create_task(bombard_query()) for _ in range(QUERY_WORKERS))
            await asyncio.gather(*tasks)

        self.assertEqual(results["publish_fail"], 0, results["errors"][:3])
        self.assertEqual(results["query_fail"], 0, results["errors"][:3])
        self.assertEqual(results["publish_ok"], TOTAL_REQUESTS)
        self.assertEqual(results["query_ok"], QUERY_WORKERS * QUERIES_PER_WORKER)


if __name__ == "__main__":
    unittest.main(verbosity=2)
