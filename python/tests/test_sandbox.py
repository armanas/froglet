import unittest

import aiohttp

from test_support import (
    FrogletAsyncTestCase,
    LONG_RUNNING_WASM_HEX,
    VALID_WASM_HEX,
    build_wasm_submission,
)


class SandboxTests(FrogletAsyncTestCase):
    async def test_wasm_infinite_loop_hits_runtime_limits(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=10)) as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"submission": build_wasm_submission(LONG_RUNNING_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertTrue(
            "timeout" in payload["error"].lower()
            or "execution limit exceeded" in payload["error"].lower()
        )

    async def test_valid_wasm_executes(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"submission": build_wasm_submission(VALID_WASM_HEX)},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["result"], 42)

    async def test_invalid_wasm_hex_is_rejected(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={
                    "submission": {
                        **build_wasm_submission(VALID_WASM_HEX),
                        "module_bytes_hex": "zz-not-hex",
                    }
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("invalid hex", payload["error"])

    async def test_execute_wasm_rejects_module_hash_mismatch(self) -> None:
        node = await self.start_node()
        submission = build_wasm_submission(VALID_WASM_HEX)
        submission["workload"]["module_hash"] = "11" * 32

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("module hash", payload["error"].lower())

    async def test_execute_wasm_rejects_unsupported_abi_version(self) -> None:
        node = await self.start_node()
        submission = build_wasm_submission(VALID_WASM_HEX)
        submission["workload"]["abi_version"] = "froglet.wasm.run_json.v0"

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("abi_version", payload["error"])

    async def test_execute_wasm_rejects_requested_capabilities(self) -> None:
        node = await self.start_node()
        submission = build_wasm_submission(VALID_WASM_HEX)
        submission["workload"]["requested_capabilities"] = ["net.http.fetch"]

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"submission": submission},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("requested_capabilities", payload["error"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
