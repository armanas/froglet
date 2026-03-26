"""HTTP API fuzzing — sends malformed data to all endpoints and verifies
the server never crashes and always returns valid HTTP responses.

Runs against process-local binaries (like other python/tests/).
"""

import asyncio
import json
import os
import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, create_signed_event


# ---------------------------------------------------------------------------
# Fuzz vector generators
# ---------------------------------------------------------------------------

def _fuzz_vectors() -> list[tuple[str, object | bytes | str]]:
    """Returns (label, body) pairs.  Body is either a dict/list (sent as JSON),
    raw bytes, or a string (sent as text)."""
    vectors: list[tuple[str, object | bytes | str]] = []

    # --- JSON-level vectors ---
    vectors.append(("empty_object", {}))
    vectors.append(("empty_array", []))
    vectors.append(("null_body", None))
    vectors.append(("nested_1000", _nested_json(1000)))
    vectors.append(("large_string_100k", {"content": "A" * 100_000}))
    vectors.append(("negative_integers", {"limit": -1, "offset": -999}))
    vectors.append(("float_where_int", {"limit": 3.14}))
    vectors.append(("boolean_where_string", {"content": True}))
    vectors.append(("extra_unknown_fields", {"event": {}, "x_evil": True, "__proto__": {}}))
    vectors.append(("missing_required_fields", {"kind": "note"}))
    vectors.append(("unicode_edge_cases", {"content": "\u0000\ufeff\ud800\udfff"}))
    vectors.append(("sql_injection", {"content": "'; DROP TABLE events; --"}))
    vectors.append(("path_traversal", {"service_id": "../../../etc/passwd"}))
    vectors.append(("command_injection", {"content": "; rm -rf /"}))
    vectors.append(("xss_payload", {"content": '<script>alert("xss")</script>'}))

    # --- Raw byte vectors ---
    vectors.append(("binary_garbage", os.urandom(256)))
    vectors.append(("invalid_utf8", b"\x80\x81\x82\xff\xfe"))
    vectors.append(("null_bytes", b"\x00" * 64))
    vectors.append(("oversized_1mb", b"X" * (1024 * 1024)))

    # --- String vectors ---
    vectors.append(("plain_text", "this is not json"))
    vectors.append(("empty_string", ""))

    return vectors


def _nested_json(depth: int) -> dict:
    result: dict = {"leaf": True}
    for _ in range(depth):
        result = {"nested": result}
    return result


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class FuzzApiTests(FrogletAsyncTestCase):
    """Fuzz every public endpoint and verify the server stays healthy."""

    async def _fuzz_endpoint(
        self,
        session: aiohttp.ClientSession,
        node_url: str,
        method: str,
        path: str,
        vectors: list[tuple[str, object | bytes | str]],
        health_url: str,
    ) -> list[dict]:
        results = []
        for label, body in vectors:
            try:
                if isinstance(body, bytes):
                    async with session.request(
                        method,
                        f"{node_url}{path}",
                        data=body,
                        headers={"Content-Type": "application/octet-stream"},
                        timeout=aiohttp.ClientTimeout(total=10),
                    ) as resp:
                        status = resp.status
                        await resp.read()
                elif isinstance(body, str):
                    async with session.request(
                        method,
                        f"{node_url}{path}",
                        data=body,
                        headers={"Content-Type": "text/plain"},
                        timeout=aiohttp.ClientTimeout(total=10),
                    ) as resp:
                        status = resp.status
                        await resp.read()
                else:
                    async with session.request(
                        method,
                        f"{node_url}{path}",
                        json=body,
                        timeout=aiohttp.ClientTimeout(total=10),
                    ) as resp:
                        status = resp.status
                        await resp.read()
            except Exception as exc:
                results.append({"label": label, "path": path, "error": str(exc)})
                continue

            results.append({"label": label, "path": path, "status": status})

        # Verify the server is still alive after the fuzz batch
        try:
            async with session.get(health_url, timeout=aiohttp.ClientTimeout(total=5)) as resp:
                self.assertEqual(resp.status, 200, f"Server crashed after fuzzing {path}")
        except Exception as exc:
            self.fail(f"Server unreachable after fuzzing {path}: {exc}")

        return results

    async def test_fuzz_provider_publish(self) -> None:
        node = await self.start_node()
        vectors = _fuzz_vectors()
        async with aiohttp.ClientSession() as session:
            results = await self._fuzz_endpoint(
                session, node.base_url, "POST", "/v1/node/events/publish",
                vectors, node.url("/health"),
            )
        # No 5xx panics — 4xx is expected for malformed input
        for r in results:
            if "status" in r:
                self.assertNotIn(r["status"], (500, 502, 503), f"Server error on fuzz {r}")

    async def test_fuzz_provider_query(self) -> None:
        node = await self.start_node()
        vectors = _fuzz_vectors()
        async with aiohttp.ClientSession() as session:
            results = await self._fuzz_endpoint(
                session, node.base_url, "POST", "/v1/node/events/query",
                vectors, node.url("/health"),
            )
        for r in results:
            if "status" in r:
                self.assertNotIn(r["status"], (500, 502, 503), f"Server error on fuzz {r}")

    async def test_fuzz_provider_descriptor(self) -> None:
        node = await self.start_node()
        vectors = _fuzz_vectors()
        async with aiohttp.ClientSession() as session:
            results = await self._fuzz_endpoint(
                session, node.base_url, "GET", "/v1/provider/descriptor",
                vectors, node.url("/health"),
            )
        for r in results:
            if "status" in r:
                self.assertNotIn(r["status"], (500, 502, 503), f"Server error on fuzz {r}")

    async def test_fuzz_wrong_content_type(self) -> None:
        """Send JSON to endpoints with wrong Content-Type headers."""
        node = await self.start_node()
        event = create_signed_event("fuzz-content-type")
        payload = json.dumps({"event": event}).encode()

        wrong_types = [
            "text/html",
            "application/xml",
            "multipart/form-data",
            "application/x-www-form-urlencoded",
            "",
        ]

        async with aiohttp.ClientSession() as session:
            for ct in wrong_types:
                headers = {"Content-Type": ct} if ct else {}
                async with session.post(
                    node.url("/v1/node/events/publish"),
                    data=payload,
                    headers=headers,
                    timeout=aiohttp.ClientTimeout(total=10),
                ) as resp:
                    self.assertNotIn(
                        resp.status, (500, 502, 503),
                        f"Server error with Content-Type={ct!r}"
                    )

            # Server still healthy
            async with session.get(node.url("/health")) as resp:
                self.assertEqual(resp.status, 200)

    async def test_fuzz_oversized_bodies(self) -> None:
        """Send bodies exceeding any reasonable limit."""
        node = await self.start_node()
        sizes = [1 * 1024 * 1024, 5 * 1024 * 1024]  # 1MB, 5MB

        async with aiohttp.ClientSession() as session:
            for size in sizes:
                body = b"X" * size
                try:
                    async with session.post(
                        node.url("/v1/node/events/publish"),
                        data=body,
                        headers={"Content-Type": "application/json"},
                        timeout=aiohttp.ClientTimeout(total=30),
                    ) as resp:
                        self.assertNotEqual(resp.status, 500, f"500 on {size}-byte body")
                except aiohttp.ClientError:
                    pass  # Connection reset is acceptable for huge payloads

            # Server still healthy
            async with session.get(
                node.url("/health"), timeout=aiohttp.ClientTimeout(total=5)
            ) as resp:
                self.assertEqual(resp.status, 200)


if __name__ == "__main__":
    unittest.main(verbosity=2)
