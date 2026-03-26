"""Soak / endurance testing — runs sustained moderate load for a configurable
duration and monitors for latency degradation and error rate creep.

Env vars:
    FROGLET_SOAK_DURATION_MINUTES  – default 5 (use 30+ for real endurance runs)
    FROGLET_SOAK_CONCURRENCY       – default 10
    FROGLET_SOAK_INTERVAL_SECS     – sample interval, default 15
"""

import asyncio
import json
import os
import statistics
import sys
import time
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, create_signed_event

SOAK_DURATION_MINUTES = int(os.environ.get("FROGLET_SOAK_DURATION_MINUTES", "5"))
SOAK_CONCURRENCY = int(os.environ.get("FROGLET_SOAK_CONCURRENCY", "10"))
SAMPLE_INTERVAL = int(os.environ.get("FROGLET_SOAK_INTERVAL_SECS", "15"))


class SoakTests(FrogletAsyncTestCase):
    """Sustained load for stability and degradation detection."""

    async def test_sustained_publish_and_query(self) -> None:
        """Run continuous publish + query load and sample metrics periodically."""
        node = await self.start_node()
        duration_secs = SOAK_DURATION_MINUTES * 60
        stop = asyncio.Event()
        samples: list[dict] = []

        # Shared counters for current sample window
        window = {
            "ok": 0,
            "fail": 0,
            "latencies": [],
            "lock": asyncio.Lock(),
        }

        async def worker(session: aiohttp.ClientSession, wid: int) -> None:
            counter = 0
            while not stop.is_set():
                counter += 1
                event = create_signed_event(
                    f"soak-{wid}-{counter}", kind="soak.test"
                )
                start = time.perf_counter_ns()
                try:
                    async with session.post(
                        node.url("/v1/node/events/publish"),
                        json={"event": event},
                        timeout=aiohttp.ClientTimeout(total=10),
                    ) as resp:
                        await resp.read()
                        status = resp.status
                except Exception:
                    async with window["lock"]:
                        window["fail"] += 1
                    continue

                elapsed_ms = (time.perf_counter_ns() - start) / 1_000_000
                async with window["lock"]:
                    if status == 201:
                        window["ok"] += 1
                    else:
                        window["fail"] += 1
                    window["latencies"].append(elapsed_ms)

                # Small delay to keep load moderate
                await asyncio.sleep(0.05)

        async def sampler() -> None:
            while not stop.is_set():
                await asyncio.sleep(SAMPLE_INTERVAL)
                if stop.is_set():
                    break

                async with window["lock"]:
                    lats = list(window["latencies"])
                    sample = {
                        "timestamp": time.time(),
                        "ok": window["ok"],
                        "fail": window["fail"],
                    }
                    window["ok"] = 0
                    window["fail"] = 0
                    window["latencies"] = []

                if lats:
                    lats.sort()
                    sample["p50_ms"] = round(lats[len(lats) // 2], 2)
                    sample["p99_ms"] = round(lats[int(len(lats) * 0.99)], 2) if len(lats) > 1 else round(lats[0], 2)
                    sample["mean_ms"] = round(statistics.mean(lats), 2)

                samples.append(sample)

        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(limit=SOAK_CONCURRENCY)
        ) as session:
            workers = [
                asyncio.create_task(worker(session, i))
                for i in range(SOAK_CONCURRENCY)
            ]
            sampler_task = asyncio.create_task(sampler())

            await asyncio.sleep(duration_secs)
            stop.set()
            await asyncio.gather(*workers, return_exceptions=True)
            sampler_task.cancel()

        # --- Assertions ---
        self.assertGreater(len(samples), 0, "No samples collected")

        # Error rate must stay below 1% across all windows
        total_ok = sum(s["ok"] for s in samples)
        total_fail = sum(s["fail"] for s in samples)
        total = total_ok + total_fail
        if total > 0:
            error_rate = total_fail / total
            self.assertLess(
                error_rate, 0.01,
                f"Error rate {error_rate:.2%} exceeds 1% threshold",
            )

        # p99 latency must not increase more than 100% from first sample to last
        p99_samples = [s["p99_ms"] for s in samples if "p99_ms" in s]
        if len(p99_samples) >= 3:
            baseline = statistics.mean(p99_samples[:2])
            final = statistics.mean(p99_samples[-2:])
            if baseline > 0:
                increase = (final - baseline) / baseline
                self.assertLess(
                    increase, 1.0,
                    f"p99 latency increased {increase:.0%} from {baseline:.1f}ms to {final:.1f}ms",
                )

        # Print samples for diagnostics
        print(json.dumps(samples, indent=2), file=sys.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
