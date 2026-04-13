import json
import os
import stat
import subprocess
import tempfile
import textwrap
import tomllib
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
SETUP_AGENT = REPO_ROOT / "scripts" / "setup-agent.sh"
SETUP_PAYMENT = REPO_ROOT / "scripts" / "setup-payment.sh"


class SetupScriptsTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.stub_dir = self.root / "stubs"
        self.stub_dir.mkdir()
        self.curl_log = self.root / "curl.log"
        self.curl_log.write_text("", encoding="utf-8")
        self.repo_copy = self.root / "froglet-space"
        self.repo_copy.mkdir()

        self._write_stub(
            "curl",
            """#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$CURL_LOG"
status="${FAKE_CURL_STATUS:-200}"
body="${FAKE_CURL_BODY:-}"
exit_code="${FAKE_CURL_EXIT_CODE:-0}"
if [ -z "$body" ]; then
  body='{}'
fi
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o|--output)
      out="$2"
      : > "$out"
      shift 2
      ;;
    -w|--write-out)
      format="$2"
      shift 2
      ;;
    --fail|--silent|--show-error)
      shift
      ;;
    --cacert|-H|-d)
      shift
      if [ "$#" -gt 0 ]; then
        shift
      fi
      ;;
    http*)
      url="$1"
      shift
      ;;
    *)
      shift
      ;;
  esac
done
if [ "$exit_code" -ne 0 ]; then
  exit "$exit_code"
fi
if [ -n "${format:-}" ]; then
  printf '%s' "$status"
else
  printf '%s' "$body"
fi
""",
        )

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_setup_agent_generates_claude_code_config(self):
        out_path = self.root / "claude.mcp.json"
        result = self._run(
            [str(SETUP_AGENT), "--target", "claude-code", "--out", str(out_path)]
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(out_path.read_text(encoding="utf-8"))
        server = payload["mcpServers"]["froglet"]
        self.assertEqual(server["type"], "stdio")
        self.assertTrue(server["args"][0].endswith("/integrations/mcp/froglet/server.js"))
        self.assertEqual(
            server["env"]["FROGLET_PROVIDER_URL"],
            "http://127.0.0.1:8080",
        )
        self.assertTrue(
            server["env"]["FROGLET_PROVIDER_AUTH_TOKEN_PATH"].endswith(
                "/data/runtime/froglet-control.token"
            )
        )

    def test_setup_agent_generates_codex_config(self):
        out_path = self.root / "codex.config.toml"
        result = self._run([str(SETUP_AGENT), "--target", "codex", "--out", str(out_path)])
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = tomllib.loads(out_path.read_text(encoding="utf-8"))
        server = payload["mcp_servers"]["froglet"]
        self.assertEqual(server["command"], "node")
        self.assertTrue(server["args"][0].endswith("/integrations/mcp/froglet/server.js"))
        self.assertEqual(server["env"]["FROGLET_RUNTIME_URL"], "http://127.0.0.1:8081")

    def test_setup_agent_generates_openclaw_config(self):
        out_path = self.root / "openclaw.json"
        result = self._run([str(SETUP_AGENT), "--target", "openclaw", "--out", str(out_path)])
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(out_path.read_text(encoding="utf-8"))
        config = payload["plugins"]["entries"]["froglet"]["config"]
        self.assertEqual(config["hostProduct"], "openclaw")
        self.assertTrue(
            payload["plugins"]["load"]["paths"][0].endswith("/integrations/openclaw/froglet")
        )
        self.assertEqual(config["providerUrl"], "http://127.0.0.1:8080")
        self.assertTrue(
            config["providerAuthTokenPath"].endswith("/data/runtime/froglet-control.token")
        )

    def test_setup_payment_generates_lightning_mock_env(self):
        out_path = self.root / "lightning.env"
        result = self._run([str(SETUP_PAYMENT), "lightning", "--out", str(out_path)])
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=lightning", content)
        self.assertIn("FROGLET_LIGHTNING_MODE=mock", content)
        self.assertIn("lightning mock mode", result.stdout)

    def test_setup_payment_generates_stripe_env_and_verifies(self):
        out_path = self.root / "stripe.env"
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(out_path)],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_123",
                "FAKE_CURL_BODY": '{"livemode": false}',
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=stripe", content)
        self.assertIn("FROGLET_STRIPE_SECRET_KEY=sk_test_123", content)
        self.assertIn("/v1/account", self.curl_log.read_text(encoding="utf-8"))

    def test_setup_payment_rejects_stripe_live_key_before_probe(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={"FROGLET_STRIPE_SECRET_KEY": "sk_live_123"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a Stripe test secret key", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_stripe_malformed_key_before_probe(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={"FROGLET_STRIPE_SECRET_KEY": "not-a-stripe-key"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a Stripe test secret key", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_stripe_probe_when_account_is_live(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_123",
                "FAKE_CURL_BODY": '{"livemode": true}',
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("livemode=true", result.stderr)

    def test_setup_payment_generates_x402_env_and_probes_facilitator(self):
        out_path = self.root / "x402.env"
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(out_path)],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FAKE_CURL_STATUS": "400",
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=x402", content)
        self.assertIn("FROGLET_X402_NETWORK=base", content)
        self.assertIn("/verify", self.curl_log.read_text(encoding="utf-8"))

    def test_setup_payment_rejects_x402_invalid_wallet(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={"FROGLET_X402_WALLET_ADDRESS": "0xabc123"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("20-byte Base address", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_x402_unsupported_network(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FROGLET_X402_NETWORK": "ethereum",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be base", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_x402_missing_verify_endpoint(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FAKE_CURL_STATUS": "404",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("/verify endpoint not found", result.stderr)

    def _run(self, args, extra_env=None):
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "CURL_LOG": str(self.curl_log),
            }
        )
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["bash", *args],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
        )

    def _write_stub(self, name: str, content: str):
        path = self.stub_dir / name
        path.write_text(textwrap.dedent(content), encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)


if __name__ == "__main__":
    unittest.main(verbosity=2)
