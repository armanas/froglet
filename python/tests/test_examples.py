import asyncio
import json
import subprocess
import sys
import unittest
from pathlib import Path

from test_support import FrogletAsyncTestCase


REPO_ROOT = Path(__file__).resolve().parents[2]


class ExampleScriptTests(FrogletAsyncTestCase):
    async def _run_example(self, script_name: str, *args: str) -> dict:
        script_path = REPO_ROOT / "examples" / script_name
        completed = await asyncio.to_thread(
            subprocess.run,
            [sys.executable, str(script_path), *args],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(completed.stdout)

    async def test_runtime_mock_lightning_buy_accept_example(self) -> None:
        node = await self.start_node(
            extra_env={
                "FROGLET_PRICE_EXEC_WASM": "10",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
            }
        )

        output = await self._run_example(
            "runtime_mock_lightning_buy_accept.py",
            "--runtime-url",
            node.runtime_url,
            "--provider-url",
            node.base_url,
            "--token-path",
            str(node.data_dir / "runtime" / "auth.token"),
            "--idempotency-key",
            "example-test-buy-accept",
        )

        self.assertEqual(output["terminal_status"], "succeeded")
        self.assertTrue(output["receipt_valid"])
        self.assertGreaterEqual(output["archive_artifact_count"], 3)
        self.assertGreaterEqual(output["archive_evidence_count"], 3)

    async def test_runtime_curated_discovery_example(self) -> None:
        node = await self.start_node()

        output = await self._run_example(
            "runtime_curated_discovery.py",
            "--runtime-url",
            node.runtime_url,
            "--provider-url",
            node.base_url,
            "--token-path",
            str(node.data_dir / "runtime" / "auth.token"),
            "--list-id",
            "example-test-curated-list",
        )

        self.assertEqual(output["curated_list_id"], "example-test-curated-list")
        self.assertTrue(output["curated_list_valid"])
        self.assertTrue(output["descriptor_summary_valid"])
        self.assertGreaterEqual(output["offer_count"], 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
