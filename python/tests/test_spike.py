"""Spike testing — simulates sudden traffic surges and measures how the
system handles burst load and recovers.
"""

import asyncio
import os
import statistics
import time
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, create_signed_event


class SpikeTests(FrogletAsyncTestCase):
    """Tests system behavior under sudden traffic surges."""

    async def test_spike_from_idle(self) -> None:
        """Ramp from 0 to 200 concurrent requests in 1 second, sustain for
        5 seconds, then drop to 0.  Measure error rate during spike and
        recovery time."""
        node = await self.start_node()
        spike_concurrency = 200
        spike_duration = 5.0
        results = {"ok": 0, "fail": 0, "latencies": []}
        stop = asyncio.Event()

        async def spike_worker(session: aiohttp.ClientSession, worker_id: int) -> None:
            while not stop.is_set():
                event = create_signed_event(
                    f"spike-{worker_id}-{time.time_ns()}", kind="spike.test"
                )
                start = time.perf_counter_ns()
                try:
                    async with session.post(
                        node.url("/v1/node/events/publish"),
                        json={"event": event},
                        timeout=aiohttp.ClientTimeout(total=10),
                    ) as resp:
                        await resp.read()
                        if resp.status == 201:
                            results["ok"] += 1
                        else:
                            results["fail"] += 1
                except Exception:
                    results["fail"] += 1
                    continue
                results["latencies"].append(
                    (time.perf_counter_ns() - start) / 1_000_000
                )

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=spike_concurrency)
        ) as session:
            # Launch all workers at once (spike)
            tasks = [
                asyncio.create_task(spike_worker(session, i))
                for i in range(spike_concurrency)
            ]

            await asyncio.sleep(spike_duration)
            stop.set()
            await asyncio.gather(*tasks, return_exceptions=True)

        total = results["ok"] + results["fail"]
        error_rate = results["fail"] / total if total > 0 else 0

        # Allow up to 5% error rate during spike
        self.assertLess(
            error_rate, 0.05,
            f"Error rate {error_rate:.2%} during spike (ok={results['ok']}, fail={results['fail']})",
        )

        # Verify recovery — health check must pass within 5 seconds
        recovery_start = time.perf_counter()
        recovered = False
        async with aiohttp.ClientSession() as session:
            for _ in range(25):
                try:
                    async with session.get(
                        node.url("/health"),
                        timeout=aiohttp.ClientTimeout(total=2),
                    ) as resp:
                        if resp.status == 200:
                            recovered = True
                            break
                except Exception:
                    pass
                await asyncio.sleep(0.2)

        recovery_time = time.perf_counter() - recovery_start
        self.assertTrue(recovered, f"Server did not recover within {recovery_time:.1f}s")

    async def test_repeated_spikes(self) -> None:
        """Three spikes of 50 concurrent requests with 3-second intervals.
        Verify no progressive degradation."""
        node = await self.start_node()
        spike_results: list[dict] = []

        for spike_num in range(3):
            ok = 0
            fail = 0
            latencies: list[float] = []

            async with aiohttp.ClientSession(
                connector=aiohttp.TCPConnector(limit=50)
            ) as session:
                async def burst(i: int) -> None:
                    nonlocal ok, fail
                    event = create_signed_event(
                        f"repeated-{spike_num}-{i}", kind="spike.repeated"
                    )
                    start = time.perf_counter_ns()
                    try:
                        async with session.post(
                            node.url("/v1/node/events/publish"),
                            json={"event": event},
                            timeout=aiohttp.ClientTimeout(total=10),
                        ) as resp:
                            await resp.read()
                            if resp.status == 201:
                                ok += 1
                            else:
                                fail += 1
                    except Exception:
                        fail += 1
                        return
                    latencies.append((time.perf_counter_ns() - start) / 1_000_000)

                await asyncio.gather(*(burst(i) for i in range(50)))

            spike_results.append({
                "spike": spike_num,
                "ok": ok,
                "fail": fail,
                "mean_ms": round(statistics.mean(latencies), 2) if latencies else 0,
            })

            if spike_num < 2:
                await asyncio.sleep(3)

        # No spike should be dramatically worse than the first
        for result in spike_results:
            self.assertEqual(result["fail"], 0, f"Failures in spike {result['spike']}: {result}")

        # Mean latency of last spike should not be >3x the first
        if spike_results[0]["mean_ms"] > 0 and spike_results[-1]["mean_ms"] > 0:
            ratio = spike_results[-1]["mean_ms"] / spike_results[0]["mean_ms"]
            self.assertLess(
                ratio, 3.0,
                f"Progressive degradation: spike 0 mean={spike_results[0]['mean_ms']}ms, "
                f"spike 2 mean={spike_results[-1]['mean_ms']}ms (ratio={ratio:.1f}x)",
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
