import unittest

import aiohttp

from test_support import FrogletAsyncTestCase, VALID_WASM_HEX


class SandboxTests(FrogletAsyncTestCase):
    async def test_lua_arithmetic_executes(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={"script": "return tostring(100 * 42)"},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["result"], "4200")

    async def test_lua_can_read_json_input(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={
                    "script": "return input.greeting .. ', ' .. input.target",
                    "input": {"greeting": "hello", "target": "froglet"},
                },
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["result"], "hello, froglet")

    async def test_lua_io_is_not_available(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={"script": "return io.open('/etc/passwd', 'r')"},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("io", payload["error"])

    async def test_lua_infinite_loop_hits_execution_limit(self) -> None:
        node = await self.start_node()
        loop_script = "while true do\n  local x = 1\nend"

        async with aiohttp.ClientSession(timeout=aiohttp.ClientTimeout(total=10)) as session:
            async with session.post(
                node.url("/v1/node/execute/lua"),
                json={"script": loop_script},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("execution limit exceeded", payload["error"].lower())

    async def test_valid_wasm_executes(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"wasm_hex": VALID_WASM_HEX},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 200)
        self.assertEqual(payload["result"], 42)

    async def test_invalid_wasm_hex_is_rejected(self) -> None:
        node = await self.start_node()

        async with aiohttp.ClientSession() as session:
            async with session.post(
                node.url("/v1/node/execute/wasm"),
                json={"wasm_hex": "zz-not-hex"},
            ) as resp:
                payload = await resp.json()

        self.assertEqual(resp.status, 400)
        self.assertIn("invalid hex", payload["error"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
