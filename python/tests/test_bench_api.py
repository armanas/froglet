"""API performance benchmarking — measures latency percentiles and throughput.

Targets a running compose stack (local Docker or GCP VM).  Uses the same
process-local binaries as FrogletAsyncTestCase when no compose stack env
vars are set.

Env vars:
    FROGLET_PERF_REQUESTS     – total requests per endpoint (default 500)
    FROGLET_PERF_CONCURRENCY  – max parallel requests (default 40)
"""

import asyncio
import json
import os
import statistics
import sys
import time
import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    VALID_WASM_HEX,
    build_wasm_request,
    create_signed_event,
)

TOTAL_REQUESTS = int(os.environ.get("FROGLET_PERF_REQUESTS", "500"))
CONCURRENCY = int(os.environ.get("FROGLET_PERF_CONCURRENCY", "40"))


def _percentile(sorted_values: list[float], p: float) -> float:
    if not sorted_values:
        return 0.0
    k = (len(sorted_values) - 1) * (p / 100.0)
    f = int(k)
    c = f + 1
    if c >= len(sorted_values):
        return sorted_values[f]
    return sorted_values[f] + (k - f) * (sorted_values[c] - sorted_values[f])


class PerformanceBenchmarks(FrogletAsyncTestCase):
    """Measures latency and throughput for key API endpoints."""

    async def _benchmark_endpoint(
        self,
        session: aiohttp.ClientSession,
        label: str,
        method: str,
        url: str,
        *,
        json_body: object = None,
        expected_statuses: tuple[int, ...] = (200,),
    ) -> dict:
        sem = asyncio.Semaphore(CONCURRENCY)
        latencies: list[float] = []
        errors = 0

        async def single_request() -> None:
            nonlocal errors
            async with sem:
                start = time.perf_counter_ns()
                try:
                    if method == "GET":
                        async with session.get(url) as resp:
                            await resp.read()
                            if resp.status not in expected_statuses:
                                errors += 1
                    else:
                        async with session.post(url, json=json_body) as resp:
                            await resp.read()
                            if resp.status not in expected_statuses:
                                errors += 1
                except Exception:
                    errors += 1
                    return
                elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
                latencies.append(elapsed_ms)

        wall_start = time.perf_counter()
        await asyncio.gather(*(single_request() for _ in range(TOTAL_REQUESTS)))
        wall_elapsed = time.perf_counter() - wall_start

        latencies.sort()
        result = {
            "endpoint": label,
            "requests": TOTAL_REQUESTS,
            "concurrency": CONCURRENCY,
            "errors": errors,
            "wall_time_s": round(wall_elapsed, 3),
            "throughput_rps": round(TOTAL_REQUESTS / wall_elapsed, 1) if wall_elapsed > 0 else 0,
        }
        if latencies:
            result.update(
                {
                    "p50_ms": round(_percentile(latencies, 50), 2),
                    "p95_ms": round(_percentile(latencies, 95), 2),
                    "p99_ms": round(_percentile(latencies, 99), 2),
                    "min_ms": round(latencies[0], 2),
                    "max_ms": round(latencies[-1], 2),
                    "mean_ms": round(statistics.mean(latencies), 2),
                }
            )
        return result

    async def test_health_latency(self) -> None:
        """Benchmark /health on the provider."""
        node = await self.start_node()
        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=CONCURRENCY)
        ) as session:
            result = await self._benchmark_endpoint(
                session, "GET /health", "GET", node.url("/health")
            )
        self.assertEqual(result["errors"], 0, f"health check errors: {result}")
        print(json.dumps(result), file=sys.stderr)

    async def test_publish_throughput(self) -> None:
        """Benchmark event publishing throughput."""
        node = await self.start_node()
        counter = 0

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=CONCURRENCY)
        ) as session:
            sem = asyncio.Semaphore(CONCURRENCY)
            latencies: list[float] = []
            errors = 0

            async def publish_one() -> None:
                nonlocal counter, errors
                counter += 1
                event = create_signed_event(f"bench-{counter}", kind="bench.perf")
                start = time.perf_counter_ns()
                async with sem:
                    try:
                        async with session.post(
                            node.url("/v1/node/events/publish"),
                            json={"event": event},
                        ) as resp:
                            await resp.read()
                            if resp.status != 201:
                                errors += 1
                    except Exception:
                        errors += 1
                        return
                latencies.append((time.perf_counter_ns() - start) / 1_000_000)

            wall_start = time.perf_counter()
            await asyncio.gather(*(publish_one() for _ in range(TOTAL_REQUESTS)))
            wall_elapsed = time.perf_counter() - wall_start

        latencies.sort()
        result = {
            "endpoint": "POST /v1/node/events/publish",
            "requests": TOTAL_REQUESTS,
            "errors": errors,
            "wall_time_s": round(wall_elapsed, 3),
            "throughput_rps": round(TOTAL_REQUESTS / wall_elapsed, 1) if wall_elapsed > 0 else 0,
            "p50_ms": round(_percentile(latencies, 50), 2) if latencies else 0,
            "p95_ms": round(_percentile(latencies, 95), 2) if latencies else 0,
            "p99_ms": round(_percentile(latencies, 99), 2) if latencies else 0,
        }
        self.assertEqual(errors, 0, f"publish errors: {result}")
        print(json.dumps(result), file=sys.stderr)

    async def test_query_latency(self) -> None:
        """Benchmark query latency after seeding data."""
        node = await self.start_node()

        # Seed some events first
        async with aiohttp.ClientSession() as session:
            for i in range(50):
                event = create_signed_event(f"seed-{i}", kind="bench.query")
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    json={"event": event},
                ) as resp:
                    self.assertEqual(resp.status, 201)

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=CONCURRENCY)
        ) as session:
            result = await self._benchmark_endpoint(
                session,
                "POST /v1/node/events/query",
                "POST",
                node.url("/v1/node/events/query"),
                json_body={"kinds": ["bench.query"], "limit": 10},
            )
        self.assertEqual(result["errors"], 0, f"query errors: {result}")
        print(json.dumps(result), file=sys.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
