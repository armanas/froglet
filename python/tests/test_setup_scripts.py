import hashlib
import json
import os
import shlex
import stat
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

from python.tests.test_support import _cargo_debug_dir

try:
    import tomllib
except ModuleNotFoundError:  # Python < 3.11
    import tomli as tomllib


REPO_ROOT = Path(__file__).resolve().parents[2]
SETUP_AGENT = REPO_ROOT / "scripts" / "setup-agent.sh"
SETUP_PAYMENT = REPO_ROOT / "scripts" / "setup-payment.sh"
FROGLET_SERVICE = REPO_ROOT / "scripts" / "froglet-service.sh"
FRESH_HOST_SMOKE = REPO_ROOT / "scripts" / "fresh_host_quickstart_smoke.sh"
IMMUTABLE_MCP_IMAGE = "ghcr.io/armanas/froglet-mcp@sha256:" + "3" * 64


class CargoTargetDirTests(unittest.TestCase):
    def test_absolute_cargo_target_dir_is_honored(self):
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": directory}):
                self.assertEqual(_cargo_debug_dir(), Path(directory) / "debug")

    def test_relative_cargo_target_dir_resolves_from_repo_root(self):
        with mock.patch.dict(os.environ, {"CARGO_TARGET_DIR": "_tmp/python-cargo"}):
            self.assertEqual(
                _cargo_debug_dir(), REPO_ROOT / "_tmp" / "python-cargo" / "debug"
            )


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

    def test_setup_agent_escapes_json_and_toml_values(self):
        json_out = self.root / "escaped.mcp.json"
        provider_url = 'http://127.0.0.1:8080/path"quoted\\slash'
        result = self._run(
            [str(SETUP_AGENT), "--target", "claude-code", "--out", str(json_out)],
            extra_env={"FROGLET_PROVIDER_URL": provider_url},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        server = json.loads(json_out.read_text(encoding="utf-8"))["mcpServers"]["froglet"]
        self.assertEqual(server["env"]["FROGLET_PROVIDER_URL"], provider_url)

        toml_out = self.root / "escaped.config.toml"
        runtime_url = 'http://127.0.0.1:8081/path"quoted\\slash'
        result = self._run(
            [str(SETUP_AGENT), "--target", "codex", "--out", str(toml_out)],
            extra_env={"FROGLET_RUNTIME_URL": runtime_url},
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = tomllib.loads(toml_out.read_text(encoding="utf-8"))
        self.assertEqual(
            payload["mcp_servers"]["froglet"]["env"]["FROGLET_RUNTIME_URL"],
            runtime_url,
        )

    def test_setup_agent_rejects_control_characters(self):
        result = self._run(
            [str(SETUP_AGENT), "--target", "claude-code", "--out", str(self.root / "bad.json")],
            extra_env={"FROGLET_PROVIDER_URL": "http://127.0.0.1:8080\nbad"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("control characters", result.stderr)

    def test_setup_agent_binary_mode_adds_linux_host_gateway(self):
        script = self.root / "setup-agent.sh"
        script.write_text(SETUP_AGENT.read_text(encoding="utf-8"), encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IXUSR)

        token_dir = self.root / "tokens"
        token_dir.mkdir()
        out_path = self.root / "binary.mcp.json"
        result = self._run(
            [str(script), "--target", "claude-code", "--out", str(out_path)],
            extra_env={
                "FROGLET_PROVIDER_AUTH_TOKEN_PATH": str(token_dir / "provider.token"),
                "FROGLET_RUNTIME_AUTH_TOKEN_PATH": str(token_dir / "runtime.token"),
                "FROGLET_MCP_DOCKER_NETWORK": "froglet_agent_default",
                "FROGLET_MCP_IMAGE": IMMUTABLE_MCP_IMAGE,
            },
            cwd=self.root,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        server = json.loads(out_path.read_text(encoding="utf-8"))["mcpServers"]["froglet"]
        self.assertEqual(server["command"], "docker")
        self.assertIn("--network", server["args"])
        self.assertIn("froglet_agent_default", server["args"])
        self.assertIn("--add-host", server["args"])
        self.assertIn("host.docker.internal:host-gateway", server["args"])
        self.assertEqual(server["env"]["FROGLET_PROVIDER_URL"], "http://provider:8080")
        self.assertEqual(server["env"]["FROGLET_RUNTIME_URL"], "http://runtime:8081")
        self.assertIn(IMMUTABLE_MCP_IMAGE, server["args"])

    def test_setup_agent_binary_mode_rejects_mutable_mcp_image(self):
        script = self.root / "setup-agent.sh"
        script.write_text(SETUP_AGENT.read_text(encoding="utf-8"), encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IXUSR)

        result = self._run(
            [str(script), "--target", "claude-code", "--out", str(self.root / "bad.json")],
            extra_env={"FROGLET_MCP_IMAGE": "ghcr.io/armanas/froglet-mcp:latest"},
            cwd=self.root,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("immutable OCI sha256 digest reference", result.stderr)

    def test_setup_agent_native_mode_needs_no_python_node_docker_or_jq(self):
        script = self.root / "setup-agent.sh"
        script.write_text(SETUP_AGENT.read_text(encoding="utf-8"), encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IXUSR)
        node_bin = self.root / "froglet-node"
        merger = _cargo_debug_dir() / "froglet-node"
        self.assertTrue(merger.is_file(), "build froglet-node before setup integration tests")
        node_bin.write_text(f"#!/bin/sh\nexec {shlex.quote(str(merger))} \"$@\"\n", encoding="utf-8")
        node_bin.chmod(node_bin.stat().st_mode | stat.S_IXUSR)
        for command in ("python3", "node", "docker", "jq"):
            self._write_stub(command, f"#!/bin/sh\necho '{command} must not run' >&2\nexit 99\n")

        out_path = self.root / "native.mcp.json"
        result = self._run(
            [str(script), "--target", "claude-code", "--out", str(out_path)],
            extra_env={
                "FROGLET_MCP_MODE": "native",
                "FROGLET_NODE_BIN": str(node_bin),
                "FROGLET_DATA_DIR": str(self.root / "state"),
            },
            cwd=self.root,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        server = json.loads(out_path.read_text(encoding="utf-8"))["mcpServers"][
            "froglet"
        ]
        self.assertEqual(server["command"], str(node_bin))
        self.assertEqual(server["args"], ["mcp"])
        self.assertEqual(server["env"]["FROGLET_DATA_DIR"], str(self.root / "state"))
        self.assertEqual(
            server["env"]["FROGLET_DAEMON_URL"],
            server["env"]["FROGLET_PROVIDER_URL"],
        )
        self.assertEqual(
            server["env"]["FROGLET_PROVIDER_CONTROL_TOKEN_PATH"],
            server["env"]["FROGLET_PROVIDER_AUTH_TOKEN_PATH"],
        )
        self.assertNotIn("Docker", result.stdout)

    def test_setup_payment_generates_lightning_mock_env(self):
        out_path = self.root / "lightning.env"
        result = self._run([str(SETUP_PAYMENT), "lightning", "--out", str(out_path)])
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=lightning", content)
        self.assertIn("FROGLET_LIGHTNING_MODE=mock", content)
        self.assertIn("lightning mock mode", result.stdout)

    def test_setup_payment_writes_private_env_file(self):
        out_path = self.root / "lightning-private.env"
        result = self._run([str(SETUP_PAYMENT), "lightning", "--out", str(out_path)])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(out_path.stat().st_mode), 0o600)

    def test_setup_payment_shell_quotes_values_when_sourced(self):
        out_path = self.root / "stripe-quoted.env"
        secret_key = "sk_test_secret with spaces"
        webhook_secret = "whsec_secret with spaces"
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(out_path), "--no-verify"],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": secret_key,
                "FROGLET_STRIPE_WEBHOOK_SECRET": webhook_secret,
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        sourced = subprocess.run(
            [
                "bash",
                "-c",
                (
                    f"set -a; . {shlex.quote(str(out_path))}; set +a; "
                    'printf "%s\\n%s\\n" "$FROGLET_STRIPE_SECRET_KEY" "$FROGLET_STRIPE_WEBHOOK_SECRET"'
                ),
            ],
            text=True,
            capture_output=True,
        )
        self.assertEqual(sourced.returncode, 0, sourced.stderr)
        self.assertEqual(sourced.stdout.splitlines(), [secret_key, webhook_secret])

    def test_setup_payment_preserves_a_leading_tilde_when_sourced(self):
        out_path = self.root / "phoenixd-quoted.env"
        password = "~literal password"
        result = self._run(
            [
                str(SETUP_PAYMENT),
                "lightning",
                "--mode",
                "phoenixd",
                "--out",
                str(out_path),
                "--no-verify",
            ],
            extra_env={
                "FROGLET_LIGHTNING_PHOENIXD_URL": "http://127.0.0.1:9740",
                "FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD": password,
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        sourced = subprocess.run(
            [
                "bash",
                "-c",
                (
                    f"set -a; . {shlex.quote(str(out_path))}; set +a; "
                    'printf "%s" "$FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD"'
                ),
            ],
            text=True,
            capture_output=True,
        )
        self.assertEqual(sourced.returncode, 0, sourced.stderr)
        self.assertEqual(sourced.stdout, password)

    def test_setup_payment_generates_stripe_env_and_verifies(self):
        out_path = self.root / "stripe.env"
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(out_path)],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_123",
                "FROGLET_STRIPE_WEBHOOK_SECRET": "whsec_test_123",
                "FAKE_CURL_BODY": '{"object":"account","livemode":false}',
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=stripe", content)
        self.assertIn("FROGLET_STRIPE_SECRET_KEY=sk_test_123", content)
        self.assertIn("FROGLET_STRIPE_API_VERSION=2026-04-22.preview", content)
        self.assertIn("FROGLET_STRIPE_WEBHOOK_SECRET=whsec_test_123", content)
        self.assertIn("/v1/account", self.curl_log.read_text(encoding="utf-8"))

    def test_setup_payment_rejects_stripe_live_key_before_probe(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={"FROGLET_STRIPE_SECRET_KEY": "sk_live_123"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FROGLET_STRIPE_LIVE_CONFIRM=fresh", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_stripe_malformed_key_before_probe(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={"FROGLET_STRIPE_SECRET_KEY": "not-a-stripe-key"},
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be sk_test_...", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_accepts_stripe_live_key_with_fresh_confirm(self):
        out_path = self.root / "stripe-live.env"
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(out_path)],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_live_123",
                "FROGLET_STRIPE_LIVE_CONFIRM": "fresh",
                "FAKE_CURL_BODY": '{"object":"account","livemode":true}',
            },
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text(encoding="utf-8")
        self.assertIn("FROGLET_STRIPE_SECRET_KEY=sk_live_123", content)
        self.assertIn("livemode=true", result.stdout)

    def test_setup_payment_rejects_stripe_probe_when_account_is_live(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_123",
                "FAKE_CURL_BODY": '{"object":"account","livemode":true}',
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("livemode=true", result.stderr)

    def test_setup_payment_rejects_malformed_stripe_webhook_secret(self):
        result = self._run(
            [str(SETUP_PAYMENT), "stripe", "--out", str(self.root / "stripe.env")],
            extra_env={
                "FROGLET_STRIPE_SECRET_KEY": "sk_test_123",
                "FROGLET_STRIPE_WEBHOOK_SECRET": "not-whsec",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must start with whsec_", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_paid_only_setup_refuses_mock_and_test_money_before_writing(self):
        out_path = self.root / "paid.env"
        for args in (["lightning", "--mode", "mock"], ["stripe"]):
            result = self._run([str(SETUP_PAYMENT), *args, "--paid-only", "--out", str(out_path)],
                               extra_env={"FROGLET_STRIPE_SECRET_KEY": "sk_test_example"})
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(out_path.exists())
            self.assertEqual(self.curl_log.read_text(), "")

    def test_paid_only_setup_requires_explicit_caps_and_writes_closed_profile(self):
        out_path = self.root / "paid.env"
        config = {
            "FROGLET_LIGHTNING_PHOENIXD_URL": "http://127.0.0.1:9740",
            "FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD": "test-only-password",
            "FROGLET_PRICE_EVENTS_QUERY": "100",
            "FROGLET_PRICE_EXEC_WASM": "100",
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "10",
            "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "50000",
            "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "30",
        }
        args = [str(SETUP_PAYMENT), "lightning", "--mode", "phoenixd", "--paid-only", "--no-verify", "--out", str(out_path)]
        missing = dict(config)
        del missing["FROGLET_PROVIDER_MAX_TOTAL_DEALS"]
        result = self._run(args, extra_env=missing)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(out_path.exists())
        result = self._run(args, extra_env=config)
        self.assertEqual(result.returncode, 0, result.stderr)
        content = out_path.read_text()
        self.assertIn("FROGLET_PROVIDER_REQUIRE_PAYMENT=true", content)
        self.assertIn("FROGLET_PROVIDER_MAX_TOTAL_DEALS=10", content)
        self.assertIn("FROGLET_REQUESTER_SPEND_BUDGET_MSAT=0", content)
        self.assertEqual(stat.S_IMODE(out_path.stat().st_mode), 0o600)
        self.assertNotIn("test-only-password", result.stdout + result.stderr)

    def test_setup_payment_generates_x402_env_and_probes_facilitator(self):
        out_path = self.root / "x402.env"
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(out_path)],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FROGLET_X402_FACILITATOR_URL": "https://facilitator.example.test/x402",
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
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0xabc123",
                "FROGLET_X402_FACILITATOR_URL": "https://facilitator.example.test/x402",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("20-byte Base address", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_x402_unsupported_network(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FROGLET_X402_FACILITATOR_URL": "https://facilitator.example.test/x402",
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
                "FROGLET_X402_FACILITATOR_URL": "https://facilitator.example.test/x402",
                "FAKE_CURL_STATUS": "404",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("/verify endpoint not found", result.stderr)

    def test_setup_payment_rejects_unauthenticated_x402_facilitator(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FROGLET_X402_FACILITATOR_URL": "https://facilitator.example.test/x402",
                "FAKE_CURL_STATUS": "401",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rejected Froglet as unauthenticated", result.stderr)

    def test_setup_payment_rejects_x402_without_facilitator(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FROGLET_X402_FACILITATOR_URL is required", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def test_setup_payment_rejects_public_plaintext_x402_facilitator(self):
        result = self._run(
            [str(SETUP_PAYMENT), "x402", "--out", str(self.root / "x402.env")],
            extra_env={
                "FROGLET_X402_WALLET_ADDRESS": "0x1111111111111111111111111111111111111111",
                "FROGLET_X402_FACILITATOR_URL": "http://facilitator.example.test/x402",
            },
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must use HTTPS", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")

    def _run(self, args, extra_env=None, cwd=REPO_ROOT):
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
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
        )

    def _write_stub(self, name: str, content: str):
        path = self.stub_dir / name
        path.write_text(textwrap.dedent(content), encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)


class FreshHostSmokeCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.workspace = self.root / "workspace"
        self.stub_dir = self.root / "stubs"
        self.stub_dir.mkdir()
        self.cleanup_log = self.root / "cleanup.log"
        self.cleanup_log.write_text("", encoding="utf-8")

        agent = self.root / "agent-bootstrap.sh"
        agent.write_text(
            textwrap.dedent(
                """\
                #!/bin/sh
                set -eu
                mkdir -p "$INSTALL_DIR" "$FROGLET_BOOTSTRAP_DIR"
                printf '#!/bin/sh\nexit 0\n' > "$INSTALL_DIR/froglet-node"
                chmod 0755 "$INSTALL_DIR/froglet-node"
                cp "$FAKE_SERVICE_SCRIPT" "$FROGLET_BOOTSTRAP_DIR/froglet-service.sh"
                chmod 0755 "$FROGLET_BOOTSTRAP_DIR/froglet-service.sh"
                printf 'vfixture\n' > "$FROGLET_BOOTSTRAP_DIR/current-release"
                exit 17
                """
            ),
            encoding="utf-8",
        )
        agent.chmod(agent.stat().st_mode | stat.S_IXUSR)
        self.agent = agent

        service = self.root / "froglet-service.sh"
        service.write_text(
            textwrap.dedent(
                """\
                #!/bin/sh
                set -eu
                [ "${1:-}" = "uninstall" ]
                printf 'uninstall\n' >> "$CLEANUP_LOG"
                rm -f "$INSTALL_DIR/froglet-node"
                rm -rf "$FROGLET_BOOTSTRAP_DIR"
                """
            ),
            encoding="utf-8",
        )
        service.chmod(service.stat().st_mode | stat.S_IXUSR)
        self.service = service

        self._write_stub(
            "curl",
            """#!/bin/sh
            set -eu
            out=""
            while [ "$#" -gt 0 ]; do
              case "$1" in
                -o) out="$2"; shift 2 ;;
                -*) shift ;;
                *) shift ;;
              esac
            done
            [ -n "$out" ]
            cp "$FAKE_AGENT_SCRIPT" "$out"
            """,
        )
        self._write_stub(
            "uname",
            """#!/bin/sh
            case "${1:-}" in
              -s) printf 'Linux\n' ;;
              -m) printf 'x86_64\n' ;;
              *) /usr/bin/uname "$@" ;;
            esac
            """,
        )
        self._write_stub("systemctl", "#!/bin/sh\nexit 3\n")

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_bootstrap_failure_after_native_activation_still_uninstalls(self):
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "FAKE_AGENT_SCRIPT": str(self.agent),
                "FAKE_SERVICE_SCRIPT": str(self.service),
                "CLEANUP_LOG": str(self.cleanup_log),
                "FROGLET_FRESH_HOST_WORKDIR": str(self.workspace),
            }
        )
        result = subprocess.run(
            [
                "bash",
                str(FRESH_HOST_SMOKE),
                "--agent-url",
                "https://fixtures.invalid/agent-bootstrap.sh",
                "--agent-sha256",
                hashlib.sha256(self.agent.read_bytes()).hexdigest(),
                "--raw-base",
                "https://fixtures.invalid",
                "--target-agent",
                "manual",
                "--mode",
                "native",
            ],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(self.cleanup_log.read_text(encoding="utf-8"), "uninstall\n")
        self.assertFalse(self.workspace.exists())

    def _write_stub(self, name: str, content: str) -> None:
        path = self.stub_dir / name
        path.write_text(textwrap.dedent(content), encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)


class NativeLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.home = self.root / "home"
        self.bootstrap = self.home / ".froglet" / "agent"
        self.data = self.home / ".froglet" / "data"
        self.bin_dir = self.home / ".local" / "bin"
        self.stub_dir = self.root / "stubs"
        self.systemctl_log = self.root / "systemctl.log"
        self.launchctl_log = self.root / "launchctl.log"
        for path in (self.home, self.bootstrap, self.data, self.bin_dir, self.stub_dir):
            path.mkdir(parents=True, exist_ok=True)
        self._write_stub(
            "systemctl",
            """#!/bin/sh
            printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
            if [ "$*" = "--user is-active froglet.service" ]; then
              printf 'active\n'
            fi
            exit 0
            """,
        )
        self._write_stub("loginctl", "#!/bin/sh\nexit 0\n")
        self._write_stub(
            "launchctl",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$LAUNCHCTL_LOG\"\nexit 0\n",
        )
        self._write_stub("curl", "#!/bin/sh\nexit 0\n")
        for command in ("python3", "node", "docker", "jq"):
            self._write_stub(command, f"#!/bin/sh\necho '{command} must not run' >&2\nexit 99\n")

    def tearDown(self):
        self.temp_dir.cleanup()

    def _write_stub(self, name: str, content: str) -> None:
        path = self.stub_dir / name
        path.write_text(textwrap.dedent(content), encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def _environment(self, **overrides: str) -> dict[str, str]:
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(self.home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "SYSTEMCTL_LOG": str(self.systemctl_log),
                "LAUNCHCTL_LOG": str(self.launchctl_log),
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_DIR": str(self.bootstrap),
                "FROGLET_DATA_DIR": str(self.data),
                "INSTALL_DIR": str(self.bin_dir),
                "XDG_CONFIG_HOME": str(self.home / ".config"),
                "FROGLET_HEALTH_ATTEMPTS": "1",
                "FROGLET_HEALTH_INTERVAL_SECS": "0",
            }
        )
        env.update(overrides)
        return env

    def _release_fixture(self, release: str, proof_ok: bool = True) -> tuple[Path, Path]:
        fixture = self.root / f"fixture-{release}"
        fixture.mkdir(exist_ok=True)
        binary = fixture / "froglet-node"
        proof = (
            "printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"structuredContent\":{\"status\":\"ok\",\"native_mcp\":true},\"isError\":false}}'"
            if proof_ok
            else "exit 31"
        )
        binary.write_text(
            textwrap.dedent(
                f"""\
                #!/bin/sh
                if [ "${{1:-}}" = "mcp" ]; then
                  cat >/dev/null
                  {proof}
                  exit $?
                fi
                if [ -n "${{FROGLET_TEST_CAPTURE_ENV:-}}" ]; then
                  env | sort > "$FROGLET_TEST_CAPTURE_ENV"
                fi
                exit 0
                """
            ),
            encoding="utf-8",
        )
        binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
        manifest = fixture / "release-manifest.json"
        manifest.write_text(
            json.dumps({"release": release}, indent=2) + "\n", encoding="utf-8"
        )
        return binary, manifest

    def _run(self, *args: str, **env_overrides: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(FROGLET_SERVICE), *args],
            cwd=self.root,
            env=self._environment(**env_overrides),
            text=True,
            capture_output=True,
        )

    def _activate(self, release: str = "v1.0.0", proof_ok: bool = True):
        binary, manifest = self._release_fixture(release, proof_ok)
        return self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            release,
            "--manifest",
            str(manifest),
        )

    def _payment_environment(self, *lines: str) -> Path:
        path = self.root / "selected-payment.env"
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        path.chmod(0o600)
        return path

    def _write_upgrade_installer(self) -> None:
        installer = self.bootstrap / "install.sh"
        installer.write_text(
            textwrap.dedent(
                """\
                #!/bin/sh
                set -eu
                release="${VERSION:-v2.0.0}"
                case "$release" in v*) ;; *) release="v$release" ;; esac
                mkdir -p "$INSTALL_DIR"
                cat > "$INSTALL_DIR/froglet-node" <<EOF
                #!/bin/sh
                if [ "\\${1:-}" = "mcp" ]; then
                  cat >/dev/null
                  if [ "\\${FAKE_UPGRADE_PROOF:-ok}" = "ok" ]; then
                    printf '%s\\n' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"status":"ok","native_mcp":true},"isError":false}}'
                    exit 0
                  fi
                  exit 31
                fi
                exit 0
                EOF
                chmod 0755 "$INSTALL_DIR/froglet-node"
                printf '{\\n  "release": "%s"\\n}\\n' "$release" > "$FROGLET_RELEASE_MANIFEST_OUT"
                """
            ),
            encoding="utf-8",
        )
        installer.chmod(installer.stat().st_mode | stat.S_IXUSR)

    def test_custom_loopback_ports_survive_restart(self):
        binary, manifest = self._release_fixture("v1.0.0")
        result = self._run("activate", "--binary", str(binary), "--release", "v1.0.0", "--manifest", str(manifest),
            FROGLET_PROVIDER_URL="http://127.0.0.1:18940", FROGLET_RUNTIME_URL="http://127.0.0.1:18941")
        self.assertEqual(result.returncode, 0, result.stderr)
        before = (self.bootstrap / "native.env").read_text()
        self.assertIn('FROGLET_LISTEN_ADDR="127.0.0.1:18940"', before)
        self.assertIn('FROGLET_RUNTIME_LISTEN_ADDR="127.0.0.1:18941"', before)
        result = self._run("restart")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bootstrap / "native.env").read_text(), before)

    def test_systemd_activation_installs_restart_policy_and_local_proof(self):
        seed = self.data / "identity" / "secp256k1.seed"
        seed.parent.mkdir(parents=True)
        seed.write_text("stable-identity", encoding="utf-8")

        result = self._activate()

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.bootstrap / "current-release").read_text().strip(), "v1.0.0")
        self.assertTrue((self.bootstrap / "current").is_symlink())
        self.assertTrue((self.bin_dir / "froglet-node").is_symlink())
        proof = json.loads((self.bootstrap / "local-proof.json").read_text())
        self.assertEqual(proof["result"]["structuredContent"]["status"], "ok")
        self.assertTrue(proof["result"]["structuredContent"]["native_mcp"])
        self.assertFalse(
            (self.bootstrap / "backups").exists(),
            "native activation must not create a plaintext same-disk identity backup",
        )
        self.assertIn("identity backup is not created automatically", result.stderr)
        self.assertIn("froglet-node identity backup", result.stderr)
        unit = (self.home / ".config/systemd/user/froglet.service").read_text()
        self.assertIn("Restart=on-failure", unit)
        self.assertIn("WantedBy=default.target", unit)
        self.assertIn(f'ExecStart="{self.bootstrap / "run-native.sh"}"', unit)
        environment = (self.bootstrap / "native.env").read_text()
        self.assertNotIn("FROGLET_PUBLISH_DEMO_SERVICES", environment)
        self.assertIn('FROGLET_NETWORK_MODE="clearnet"', environment)
        self.assertIn(
            'FROGLET_MARKETPLACE_URL="https://marketplace.froglet.dev"', environment
        )
        self.assertEqual(
            (self.bootstrap / "payment.env").read_text(encoding="utf-8"),
            "FROGLET_PAYMENT_BACKEND=none\n",
        )
        self.assertEqual(stat.S_IMODE((self.bootstrap / "payment.env").stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE((self.bootstrap / "run-native.sh").stat().st_mode), 0o700)
        calls = self.systemctl_log.read_text(encoding="utf-8")
        self.assertIn("--user enable --now froglet.service", calls)

    def test_native_configuration_survives_upgrade_and_rollback_without_reexport(self):
        payment = self._payment_environment(
            "FROGLET_PAYMENT_BACKEND=stripe",
            "FROGLET_STRIPE_SECRET_KEY=sk_test_persisted",
            "FROGLET_STRIPE_API_VERSION=2026-04-22.preview",
        )
        binary, manifest = self._release_fixture("v1.0.0")
        initial = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_PAYMENT_ENV_FILE=str(payment),
            FROGLET_NETWORK_MODE="dual",
            FROGLET_MARKETPLACE_URL="https://market.example/v1",
            FROGLET_REQUESTER_SPEND_BUDGET_MSAT="50000",
            FROGLET_REQUESTER_MAX_DEAL_MSAT="7000",
            FROGLET_RELAY_URL="wss://relay.example/v1/tunnel",
            FROGLET_RELAY_PUBLIC_SUFFIX="relay.example",
        )
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._write_upgrade_installer()

        upgraded = self._run("upgrade", "v2.0.0")

        self.assertEqual(upgraded.returncode, 0, upgraded.stderr)
        environment = (self.bootstrap / "native.env").read_text(encoding="utf-8")
        self.assertIn(
            'FROGLET_RELAY_URL="wss://relay.example/v1/tunnel"', environment
        )
        self.assertIn('FROGLET_RELAY_PUBLIC_SUFFIX="relay.example"', environment)
        self.assertNotIn("FROGLET_RELAY_ENABLED", environment)
        self.assertIn('FROGLET_NETWORK_MODE="dual"', environment)
        self.assertIn(
            'FROGLET_MARKETPLACE_URL="https://market.example/v1"', environment
        )
        self.assertIn('FROGLET_REQUESTER_SPEND_BUDGET_MSAT="50000"', environment)
        self.assertIn('FROGLET_REQUESTER_MAX_DEAL_MSAT="7000"', environment)
        self.assertIn(
            "FROGLET_STRIPE_SECRET_KEY=sk_test_persisted",
            (self.bootstrap / "payment.env").read_text(encoding="utf-8"),
        )

        rolled_back = self._run("rollback")
        self.assertEqual(rolled_back.returncode, 0, rolled_back.stderr)
        self.assertEqual((self.bootstrap / "current-release").read_text().strip(), "v1.0.0")
        after_rollback = (self.bootstrap / "native.env").read_text(encoding="utf-8")
        self.assertIn('FROGLET_NETWORK_MODE="dual"', after_rollback)
        self.assertIn('FROGLET_REQUESTER_SPEND_BUDGET_MSAT="50000"', after_rollback)

    def test_private_paid_config_reaches_the_native_launcher_without_secret_duplication(self):
        payment = self._payment_environment(
            "FROGLET_PAYMENT_BACKEND=stripe",
            "FROGLET_STRIPE_SECRET_KEY=sk_test_native\\ secret",
            "FROGLET_STRIPE_API_VERSION=2026-04-22.preview",
        )
        binary, manifest = self._release_fixture("v1.0.0")
        result = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_SERVICE_START="0",
            FROGLET_PAYMENT_ENV_FILE=str(payment),
            FROGLET_NETWORK_MODE="dual",
            FROGLET_MARKETPLACE_URL="https://market.example/v1",
            FROGLET_REQUESTER_SPEND_BUDGET_MSAT="90000",
            FROGLET_REQUESTER_MAX_DEAL_MSAT="8000",
            FROGLET_RELAY_URL="wss://relay.example/v1/tunnel",
            FROGLET_RELAY_PUBLIC_SUFFIX="relay.example",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("sk_test_native", result.stdout + result.stderr)
        capture = self.root / "launched.env"
        launched = subprocess.run(
            [str(self.bootstrap / "run-native.sh")],
            env=self._environment(FROGLET_TEST_CAPTURE_ENV=str(capture)),
            text=True,
            capture_output=True,
        )
        self.assertEqual(launched.returncode, 0, launched.stderr)
        launched_environment = capture.read_text(encoding="utf-8")
        for expected in (
            "FROGLET_PAYMENT_BACKEND=stripe",
            "FROGLET_STRIPE_SECRET_KEY=sk_test_native secret",
            "FROGLET_NETWORK_MODE=dual",
            "FROGLET_MARKETPLACE_URL=https://market.example/v1",
            "FROGLET_REQUESTER_SPEND_BUDGET_MSAT=90000",
            "FROGLET_REQUESTER_MAX_DEAL_MSAT=8000",
            "FROGLET_RELAY_URL=wss://relay.example/v1/tunnel",
            "FROGLET_RELAY_PUBLIC_SUFFIX=relay.example",
        ):
            self.assertIn(expected + "\n", launched_environment)
        self.assertNotIn("FROGLET_RELAY_ENABLED", launched_environment)
        unit = (self.home / ".config/systemd/user/froglet.service").read_text()
        native_environment = (self.bootstrap / "native.env").read_text()
        launcher = (self.bootstrap / "run-native.sh").read_text()
        self.assertNotIn("sk_test_native", unit)
        self.assertNotIn("sk_test_native", native_environment)
        self.assertNotIn("sk_test_native", launcher)

    def test_existing_provider_safeguards_survive_activation_upgrade_and_rollback(self):
        initial = self._activate()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        safeguards = {
            "FROGLET_PROVIDER_ACCESS_MODE": "private",
            "FROGLET_PROVIDER_REQUIRE_PAYMENT": "false",
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "0",
            "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "40000",
            "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "20",
            "FROGLET_PROVIDER_MIN_FREE_BYTES": "1048576",
            "FROGLET_PROVIDER_MAX_DATABASE_BYTES": "4194304",
            "FROGLET_FILE_MAX_TOTAL_DOWNLOADS": "10",
            "FROGLET_FILE_MAX_TOTAL_BYTES": "2097152",
            "FROGLET_FILE_MAX_STORAGE_BYTES": "1048576",
            "FROGLET_EXECUTION_TIMEOUT_SECS": "3",
            "FROGLET_PROCESS_CONCURRENCY": "1",
            "FROGLET_PROCESS_OUTPUT_MAX_BYTES": "8192",
            "FROGLET_PROCESS_MEMORY_MAX_BYTES": "16777216",
            "FROGLET_PROCESS_PIDS_LIMIT": "16",
            "FROGLET_PROCESS_CPU_LIMIT": "0.5",
            "FROGLET_WASM_CONCURRENCY_LIMIT": "2",
            "FROGLET_WASM_POLICY_PATH": str(self.root / "owner's policy.json"),
            "FROGLET_A2A_CONFIG_PATH": str(self.root / "access.json"),
            "FROGLET_PRICE_EXEC_WASM": "1000",
            "FROGLET_PUBLIC_REQUEST_QUOTA": "12",
            "FROGLET_TRUST_FORWARD_PUBLIC_QUOTA_HEADERS": "false",
            "FROGLET_IDENTITY_AUTO_GENERATE": "false",
            "FROGLET_HOST_READABLE_CONTROL_TOKEN": "false",
            "FROGLET_DB_PATH": str(self.root / "existing ledger.db"),
        }
        environment = self.bootstrap / "native.env"
        lines = environment.read_text().splitlines()
        lines = [line for line in lines if line.split("=", 1)[0] not in safeguards]
        lines.extend(f'{name}="{value}"' for name, value in safeguards.items())
        environment.write_text("\n".join(lines) + "\n")
        self._write_upgrade_installer()
        binary, manifest = self._release_fixture("v1.1.0")
        steps = (
            ("activate", "--binary", str(binary), "--release", "v1.1.0", "--manifest", str(manifest)),
            ("upgrade", "v2.0.0"),
            ("rollback",),
        )
        for args in steps:
            with self.subTest(operation=args[0]):
                result = self._run(*args)
                self.assertEqual(result.returncode, 0, result.stderr)
                persisted = environment.read_text()
                for name, value in safeguards.items():
                    self.assertIn(f'{name}="{value}"\n', persisted)
        # The rolled-back fixture captures what a clean service launch receives.
        capture = self.root / "preserved-launch.env"
        launched = subprocess.run(
            [str(self.bootstrap / "run-native.sh")],
            env=self._environment(FROGLET_TEST_CAPTURE_ENV=str(capture)),
            text=True, capture_output=True,
        )
        self.assertEqual(launched.returncode, 0, launched.stderr)
        received = dict(line.split("=", 1) for line in capture.read_text().splitlines())
        for name, value in safeguards.items():
            self.assertEqual(received[name], value)

    def test_native_environment_injection_fails_before_release_switch(self):
        initial = self._activate()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        marker = self.root / "native-env-must-not-execute"
        environment = self.bootstrap / "native.env"
        with environment.open("a") as output:
            output.write(f'FROGLET_PROVIDER_ACCESS_MODE="$(touch {marker})"\n')
        before = environment.read_bytes()
        current = (self.bootstrap / "current").readlink()
        calls = self.systemctl_log.read_bytes()
        self._write_upgrade_installer()
        result = self._run("upgrade", "v2.0.0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("native environment", result.stderr)
        self.assertFalse(marker.exists())
        self.assertEqual((self.bootstrap / "current").readlink(), current)
        self.assertEqual(environment.read_bytes(), before)
        self.assertEqual(self.systemctl_log.read_bytes(), calls)
        self.assertFalse((self.bootstrap / "releases/v2.0.0").exists())

    def _run_scoped_native(self, *args: str, **overrides: str):
        environment = self._environment()
        for name in tuple(environment):
            if name.startswith("FROGLET_"):
                del environment[name]
        environment.update({
            "FROGLET_SERVICE_MANAGER": "systemd",
            "FROGLET_BOOTSTRAP_DIR": str(self.bootstrap),
            "FROGLET_DATA_DIR": str(self.data),
            "FROGLET_HEALTH_ATTEMPTS": "1",
            "FROGLET_HEALTH_INTERVAL_SECS": "0",
        })
        environment.update(overrides)
        return subprocess.run(
            ["bash", str(FROGLET_SERVICE), *args], cwd=self.root,
            env=environment, text=True, capture_output=True, timeout=15,
        )

    def _activate_scoped_native(self, release: str = "v1.0.0"):
        binary, manifest = self._release_fixture(release)
        return self._run_scoped_native(
            "activate", "--binary", str(binary), "--release", release,
            "--manifest", str(manifest),
        )

    @staticmethod
    def _native_literal(value: str) -> str:
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        escaped = escaped.replace("$", "\\$").replace("`", "\\`")
        return f'"{escaped}"'

    def _replace_native_settings(self, settings: dict[str, str]) -> None:
        environment = self.bootstrap / "native.env"
        lines = [line for line in environment.read_text().splitlines()
                 if line.split("=", 1)[0] not in settings]
        lines.extend(f"{name}={self._native_literal(value)}"
                     for name, value in settings.items())
        environment.write_text("\n".join(lines) + "\n", encoding="utf-8")
        environment.chmod(0o600)

    def _native_lifecycle_snapshot(self):
        paths = [self.bootstrap / name for name in (
            "native.env", "payment.env", "current", "previous", "current-release",
            "release-manifest.json", "run-native.sh", "local-proof.json",
        )]
        paths.extend((
            self.bin_dir / "froglet-node", self.systemctl_log, self.launchctl_log,
            self.home / ".config/systemd/user/froglet.service",
            self.home / "Library/LaunchAgents/dev.froglet.node.plist",
        ))
        paths.extend(self.data.rglob("*"))
        snapshot = {}
        for path in paths:
            if not path.exists() and not path.is_symlink():
                snapshot[str(path)] = None
                continue
            metadata = path.lstat()
            if stat.S_ISLNK(metadata.st_mode):
                value = ("symlink", str(path.readlink()))
            elif stat.S_ISREG(metadata.st_mode):
                value = ("file", path.read_bytes())
            else:
                value = ("special",)
            snapshot[str(path)] = (metadata.st_mode, metadata.st_uid, value)
        releases = self.bootstrap / "releases"
        snapshot["release_names"] = sorted(path.name for path in releases.iterdir()) if releases.exists() else []
        return snapshot

    def test_native_preservation_only_overrides_reject_new_or_changed_settings(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._replace_native_settings({"FROGLET_PROCESS_CONCURRENCY": "1"})
        binary, manifest = self._release_fixture("v1.1.0")
        args = ("activate", "--binary", str(binary), "--release", "v1.1.0",
                "--manifest", str(manifest))
        for label, overrides in (
            ("new", {"FROGLET_PROVIDER_MIN_FREE_BYTES": "1048576"}),
            ("changed", {"FROGLET_PROCESS_CONCURRENCY": "2"}),
        ):
            with self.subTest(override=label):
                before = self._native_lifecycle_snapshot()
                result = self._run_scoped_native(*args, **overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("requires an approved installed configuration", result.stderr)
                self.assertEqual(self._native_lifecycle_snapshot(), before)
        matching = self._run_scoped_native(*args, FROGLET_PROCESS_CONCURRENCY="1")
        self.assertEqual(matching.returncode, 0, matching.stderr)
        self.assertIn('FROGLET_PROCESS_CONCURRENCY="1"\n',
                      (self.bootstrap / "native.env").read_text())
        self.assertNotIn("FROGLET_PROVIDER_MIN_FREE_BYTES=", (self.bootstrap / "native.env").read_text())

    def test_native_literal_paths_round_trip_without_shell_execution(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        dollar_marker = self.root / "dollar-must-not-execute"
        backtick_marker = self.root / "backtick-must-not-execute"
        literal = (f'{self.root}/policy "quoted" \\ path $HOME '
                   f'$(touch {dollar_marker}) `touch {backtick_marker}`')
        environment = self.bootstrap / "native.env"
        with environment.open("a", encoding="utf-8") as output:
            output.write(f"FROGLET_WASM_POLICY_PATH='{literal}'\n")
        for release in ("v1.1.0", "v1.2.0"):
            with self.subTest(release=release):
                result = self._activate_scoped_native(release)
                self.assertEqual(result.returncode, 0, result.stderr)
                canonical = environment.read_text()
                self.assertIn(f"FROGLET_WASM_POLICY_PATH={self._native_literal(literal)}\n", canonical)
                self.assertFalse(dollar_marker.exists())
                self.assertFalse(backtick_marker.exists())
                capture = self.root / f"literal-{release}.env"
                launched = subprocess.run(
                    [str(self.bootstrap / "run-native.sh")], cwd=self.root,
                    env={"PATH": f"{self.stub_dir}:/usr/bin:/bin", "HOME": str(self.home),
                         "FROGLET_TEST_CAPTURE_ENV": str(capture)},
                    text=True, capture_output=True, timeout=15,
                )
                self.assertEqual(launched.returncode, 0, launched.stderr)
                received = dict(line.split("=", 1) for line in capture.read_text().splitlines())
                self.assertEqual(received["FROGLET_WASM_POLICY_PATH"], literal)
                self.assertFalse(dollar_marker.exists())
                self.assertFalse(backtick_marker.exists())

    def test_failed_upgrade_preserves_installed_safeguards_and_requester_zero(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        safeguards = {
            "FROGLET_PROVIDER_ACCESS_MODE": "private",
            "FROGLET_PROVIDER_REQUIRE_PAYMENT": "false",
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "0",
            "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "0",
            "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "0",
            "FROGLET_PROVIDER_MIN_FREE_BYTES": "1048576",
            "FROGLET_FILE_MAX_TOTAL_DOWNLOADS": "2",
            "FROGLET_FILE_MAX_TOTAL_BYTES": "1024",
            "FROGLET_FILE_MAX_STORAGE_BYTES": "2048",
            "FROGLET_PROCESS_CONCURRENCY": "1",
            "FROGLET_REQUESTER_SPEND_BUDGET_MSAT": "0",
            "FROGLET_REQUESTER_MAX_DEAL_MSAT": "0",
        }
        self._replace_native_settings(safeguards)
        state = self.data / "accounting-fixture.json"
        state.write_text('{"reserved_deals":1,"reserved_runtime_ms":250,"issued_quotes":2}\n')
        original_state = state.read_bytes()
        original_payment = (self.bootstrap / "payment.env").read_bytes()
        original_target = (self.bootstrap / "current").readlink()
        self._write_upgrade_installer()
        result = self._run_scoped_native("upgrade", "v2.0.0", FAKE_UPGRADE_PROOF="fail")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rolled back to v1.0.0", result.stderr)
        self.assertEqual((self.bootstrap / "current").readlink(), original_target)
        self.assertEqual((self.bootstrap / "current-release").read_text().strip(), "v1.0.0")
        self.assertFalse((self.bootstrap / "releases/v2.0.0").exists())
        self.assertEqual(state.read_bytes(), original_state)
        self.assertEqual((self.bootstrap / "payment.env").read_bytes(), original_payment)
        capture = self.root / "failed-upgrade-launch.env"
        launched = subprocess.run(
            [str(self.bootstrap / "run-native.sh")], cwd=self.root,
            env={"PATH": f"{self.stub_dir}:/usr/bin:/bin", "HOME": str(self.home),
                 "FROGLET_TEST_CAPTURE_ENV": str(capture)},
            text=True, capture_output=True, timeout=15,
        )
        self.assertEqual(launched.returncode, 0, launched.stderr)
        received = dict(line.split("=", 1) for line in capture.read_text().splitlines())
        for name, value in safeguards.items():
            self.assertEqual(received[name], value)

    def test_native_environment_invalid_records_fail_without_lifecycle_changes(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._write_upgrade_installer()
        environment = self.bootstrap / "native.env"
        baseline = environment.read_bytes()
        (self.data / "state-fixture").write_bytes(b"state must stay unchanged\n")
        limits = (b'FROGLET_PROVIDER_MAX_TOTAL_DEALS="0"\n'
                  b'FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS="0"\n'
                  b'FROGLET_PROVIDER_MAX_TOTAL_QUOTES="0"\n')
        cases = (
            ("malformed", b"not-an-assignment\n", "malformed line"),
            ("unknown", b'FROGLET_UNSUPPORTED_SAFETY_LIMIT="1"\n', "unsupported variable"),
            ("duplicate_same", b'FROGLET_NETWORK_MODE="clearnet"\n', "repeats variable"),
            ("duplicate_changed", b'FROGLET_NETWORK_MODE="dual"\n', "repeats variable"),
            ("nul_comment", b"# ignored comment\x00payload\n", "NUL bytes"),
            ("control", b'FROGLET_WASM_POLICY_PATH="bad\tpath"\n', "control characters"),
            ("unterminated", b'FROGLET_WASM_POLICY_PATH="missing\n', "unterminated value"),
            ("quoted_escape", b'FROGLET_WASM_POLICY_PATH="bad\\qpath"\n', "unsupported quoted escape"),
            ("partial_file", b'FROGLET_FILE_MAX_TOTAL_DOWNLOADS="1"\n', "configured together"),
            ("zero_file", b'FROGLET_FILE_MAX_TOTAL_DOWNLOADS="0"\n', "file limits must be positive"),
            ("file_download_overflow", b'FROGLET_FILE_MAX_TOTAL_DOWNLOADS="1000001"\n', "integer range"),
            ("sqlite_overflow", b'FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS="9223372036854775808"\n', "integer range"),
            ("u64_overflow", b'FROGLET_PROVIDER_MIN_FREE_BYTES="18446744073709551616"\n', "integer range"),
            ("empty_uint", b'FROGLET_PROVIDER_MAX_DATABASE_BYTES=""\n', "unsigned integer"),
            ("protected_missing_limits", b'FROGLET_PROVIDER_ACCESS_MODE="private"\n', "protected provider requires"),
            ("paid_without_payment", limits + b'FROGLET_PROVIDER_ACCESS_MODE="paid"\n', "paid access requires payment"),
            ("trial_requires_payment", limits + b'FROGLET_PROVIDER_ACCESS_MODE="trial"\nFROGLET_PROVIDER_REQUIRE_PAYMENT="true"\n', "trial access cannot require payment"),
            ("bad_boolean", b'FROGLET_PROVIDER_REQUIRE_PAYMENT="maybe"\n', "must be a boolean"),
            ("wasm_zero", b'FROGLET_WASM_CONCURRENCY_LIMIT="0"\n', "Wasm concurrency must be positive"),
            ("cpu_nan", b'FROGLET_PROCESS_CPU_LIMIT="NaN"\n', "positive finite decimal"),
            ("different_data_root", f'FROGLET_DATA_ROOT="{self.root / "other-state"}"\n'.encode(), "must match lifecycle"),
        )
        for label, suffix, error in cases:
            with self.subTest(record=label):
                environment.write_bytes(baseline + suffix)
                before = self._native_lifecycle_snapshot()
                result = self._run_scoped_native("upgrade", "v2.0.0")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(error, result.stderr)
                self.assertEqual(self._native_lifecycle_snapshot(), before)
        with self.subTest(record="unsupported_role"):
            environment.write_bytes(baseline.replace(b'FROGLET_NODE_ROLE="dual"', b'FROGLET_NODE_ROLE="provider"'))
            before = self._native_lifecycle_snapshot()
            result = self._run_scoped_native("upgrade", "v2.0.0")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("requires FROGLET_NODE_ROLE=dual", result.stderr)
            self.assertEqual(self._native_lifecycle_snapshot(), before)

    def test_native_environment_unsafe_files_fail_before_service_changes(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        environment = self.bootstrap / "native.env"
        baseline = environment.read_bytes()
        target = self.root / "native-target.env"
        target.write_bytes(baseline)
        target.chmod(0o600)
        for label in ("symlink", "dangling_symlink", "fifo", "group_readable", "world_readable"):
            with self.subTest(file=label):
                environment.unlink()
                if label == "symlink":
                    environment.symlink_to(target)
                elif label == "dangling_symlink":
                    environment.symlink_to(self.root / "missing.env")
                elif label == "fifo":
                    os.mkfifo(environment, 0o600)
                else:
                    environment.write_bytes(baseline)
                    environment.chmod(0o640 if label == "group_readable" else 0o644)
                before = self._native_lifecycle_snapshot()
                result = self._run_scoped_native("restart")
                self.assertNotEqual(result.returncode, 0)
                expected = "regular non-symlink file" if label in ("symlink", "dangling_symlink", "fifo") else "readable by group or other users"
                self.assertIn(expected, result.stderr)
                self.assertEqual(self._native_lifecycle_snapshot(), before)
                self.assertEqual(target.read_bytes(), baseline)
        environment.unlink()
        environment.write_bytes(baseline)
        environment.chmod(0o600)

    @unittest.skipUnless(hasattr(os, "geteuid") and os.geteuid() == 0,
                         "creating a genuinely unowned temporary fixture requires root")
    def test_native_environment_wrong_owner_fails_before_service_changes(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        environment = self.bootstrap / "native.env"
        os.chown(environment, 1, -1)
        before = self._native_lifecycle_snapshot()
        result = self._run_scoped_native("restart")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be owned by the current user", result.stderr)
        self.assertEqual(self._native_lifecycle_snapshot(), before)

    def test_installed_native_environment_requires_file_and_data_directory_record(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._write_upgrade_installer()
        environment = self.bootstrap / "native.env"
        baseline = environment.read_bytes()
        for label in ("missing_file", "missing_data_directory"):
            with self.subTest(configuration=label):
                if label == "missing_file":
                    environment.unlink()
                    expected = "installed native environment is missing"
                else:
                    environment.write_bytes(b"\n".join(
                        line for line in baseline.splitlines()
                        if not line.startswith(b"FROGLET_DATA_DIR=")
                    ) + b"\n")
                    environment.chmod(0o600)
                    expected = "missing FROGLET_DATA_DIR"
                before = self._native_lifecycle_snapshot()
                result = self._run_scoped_native("upgrade", "v2.0.0")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)
                self.assertEqual(self._native_lifecycle_snapshot(), before)

    def test_absent_installed_native_defaults_stay_absent_through_activation(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        absent = {"FROGLET_NODE_ROLE", "FROGLET_IDENTITY_AUTO_GENERATE",
                  "FROGLET_HOST_READABLE_CONTROL_TOKEN"}
        environment = self.bootstrap / "native.env"
        environment.write_text("\n".join(
            line for line in environment.read_text().splitlines()
            if line.split("=", 1)[0] not in absent
        ) + "\n", encoding="utf-8")
        for release in ("v1.1.0", "v1.2.0"):
            with self.subTest(release=release):
                result = self._activate_scoped_native(release)
                self.assertEqual(result.returncode, 0, result.stderr)
                persisted = environment.read_text()
                for name in absent:
                    self.assertNotIn(name + "=", persisted)
                capture = self.root / f"absent-{release}.env"
                launched = subprocess.run(
                    [str(self.bootstrap / "run-native.sh")], cwd=self.root,
                    env={"PATH": f"{self.stub_dir}:/usr/bin:/bin", "HOME": str(self.home),
                         "FROGLET_TEST_CAPTURE_ENV": str(capture)},
                    text=True, capture_output=True, timeout=15,
                )
                self.assertEqual(launched.returncode, 0, launched.stderr)
                received = dict(line.split("=", 1) for line in capture.read_text().splitlines())
                for name in absent:
                    self.assertNotIn(name, received)

    def _assert_native_lock_drift_refused(self, *, reentrant=False,
                                         initially_absent=False, metadata_only=False):
        environment = self.bootstrap / "native.env"
        if not initially_absent:
            initial = self._activate_scoped_native()
            self.assertEqual(initial.returncode, 0, initial.stderr)
            self._replace_native_settings({
                "FROGLET_PROVIDER_ACCESS_MODE": "private",
                "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "10",
                "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "1000",
                "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "10",
            })
            original = environment.read_bytes()
            replacement = original.replace(b'FROGLET_PROVIDER_MAX_TOTAL_DEALS="10"',
                                           b'FROGLET_PROVIDER_MAX_TOTAL_DEALS="0"')
        else:
            settings = {
                "FROGLET_DATA_DIR": str(self.data),
                "FROGLET_PUBLIC_BASE_URL": "http://127.0.0.1:8080",
                "FROGLET_LISTEN_ADDR": "127.0.0.1:8080",
                "FROGLET_RUNTIME_LISTEN_ADDR": "127.0.0.1:8081",
                "FROGLET_RUNTIME_PROVIDER_BASE_URL": "http://127.0.0.1:8080",
                "FROGLET_PROVIDER_ACCESS_MODE": "private",
                "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "0",
                "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "0",
                "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "0",
            }
            replacement = "".join(f"{name}={self._native_literal(value)}\n"
                                  for name, value in settings.items()).encode()
        state = self.data / "lock-race-state-fixture"
        state.write_bytes(b"existing accounting fixture must remain unchanged\n")
        binary, manifest = self._release_fixture("v1.1.0")
        lock = Path(str(self.bootstrap) + ".lifecycle.lock")
        token = "fixture-reentrant-lock"
        if reentrant:
            lock.mkdir(mode=0o700)
            owner = lock / "owner"
            owner.write_text(f"pid={os.getpid()}\nstarted=1\ntoken={token}\n")
            owner.chmod(0o600)
            owner_before = owner.read_bytes()
        replacement_file = self.root / "lock-replacement.env"
        replacement_file.write_bytes(replacement)
        replacement_file.chmod(0o600)
        marker = self.root / "lock-drift-injected"
        if metadata_only:
            mutation = f"chmod 0644 {shlex.quote(str(environment))}"
        else:
            mutation = (f"umask 077\n"
                        f"cat {shlex.quote(str(replacement_file))} > {shlex.quote(str(environment))}")
        # Intercept the exact lock attempt after configuration parsing, without
        # scheduling-dependent concurrent processes or changes to script source.
        self._write_stub("mkdir", f"""#!/bin/sh
            /bin/mkdir "$@"
            result=$?
            if [ "$#" -eq 1 ] && [ "$1" = {shlex.quote(str(lock))} ] && [ ! -e {shlex.quote(str(marker))} ]; then
              {mutation}
              : > {shlex.quote(str(marker))}
            fi
            exit "$result"
            """)
        before = self._native_lifecycle_snapshot()
        overrides = {"FROGLET_LIFECYCLE_LOCK_TOKEN": token} if reentrant else {}
        result = self._run_scoped_native(
            "activate", "--binary", str(binary), "--release", "v1.1.0",
            "--manifest", str(manifest), **overrides,
        )
        self.assertTrue(marker.exists(), "controlled lock drift was not injected")
        actual_limits = [line for line in environment.read_text().splitlines()
                         if line.startswith("FROGLET_PROVIDER_MAX_TOTAL_DEALS=")]
        self.assertNotEqual(result.returncode, 0,
                            f"lifecycle accepted configuration drift; installed limit: {actual_limits!r}")
        self.assertIn("native environment", result.stderr)
        expected = before.copy()
        if initially_absent:
            expected[str(environment)] = (stat.S_IFREG | 0o600, os.getuid(), ("file", replacement))
        elif metadata_only:
            expected[str(environment)] = (stat.S_IFREG | 0o644, before[str(environment)][1], ("file", original))
        else:
            expected[str(environment)] = (before[str(environment)][0], before[str(environment)][1], ("file", replacement))
        self.assertEqual(self._native_lifecycle_snapshot(), expected)
        if reentrant:
            self.assertTrue(lock.is_dir())
            self.assertEqual((lock / "owner").read_bytes(), owner_before)
        else:
            self.assertFalse(lock.exists(), "a rejected acquired operation retained its lock")

    def test_native_config_drift_after_lock_acquisition_is_refused(self):
        self._assert_native_lock_drift_refused()

    def test_native_config_drift_at_reentrant_lock_is_refused(self):
        self._assert_native_lock_drift_refused(reentrant=True)

    def test_first_install_native_environment_appearance_at_lock_is_refused(self):
        self._assert_native_lock_drift_refused(initially_absent=True)

    def test_native_environment_permission_drift_at_lock_is_refused(self):
        self._assert_native_lock_drift_refused(metadata_only=True)

    def test_native_environment_drift_during_validation_is_refused_before_lock(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._replace_native_settings({
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "10",
        })
        environment = self.bootstrap / "native.env"
        original = environment.read_bytes()
        replacement = original.replace(b'FROGLET_PROVIDER_MAX_TOTAL_DEALS="10"',
                                       b'FROGLET_PROVIDER_MAX_TOTAL_DEALS="0"')
        replacement_file = self.root / "validation-replacement.env"
        replacement_file.write_bytes(replacement)
        replacement_file.chmod(0o600)
        marker = self.root / "validation-drift-injected"
        lock_attempt = self.root / "validation-lock-attempted"
        lock = Path(str(self.bootstrap) + ".lifecycle.lock")
        self._write_stub("tr", f"""#!/bin/sh
            if [ ! -e {shlex.quote(str(marker))} ]; then
              cat {shlex.quote(str(replacement_file))} > {shlex.quote(str(environment))}
              : > {shlex.quote(str(marker))}
            fi
            exec /usr/bin/tr "$@"
            """)
        # The NUL check's two pipeline processes must both see the replacement;
        # otherwise its existing byte comparison rejects before the race under test.
        self._write_stub("cmp", f"""#!/bin/sh
            if [ "$#" -eq 3 ] && [ "$1" = -s ] && [ "$2" = {shlex.quote(str(environment))} ] && [ "$3" = - ]; then
              attempts=0
              while [ ! -e {shlex.quote(str(marker))} ] && [ "$attempts" -lt 100 ]; do
                /bin/sleep 0.01
                attempts=$((attempts + 1))
              done
            fi
            exec /usr/bin/cmp "$@"
            """)
        self._write_stub("mkdir", f"""#!/bin/sh
            if [ "$#" -eq 1 ] && [ "$1" = {shlex.quote(str(lock))} ]; then
              : > {shlex.quote(str(lock_attempt))}
            fi
            exec /bin/mkdir "$@"
            """)
        binary, manifest = self._release_fixture("v1.1.0")
        before = self._native_lifecycle_snapshot()
        result = self._run_scoped_native(
            "activate", "--binary", str(binary), "--release", "v1.1.0",
            "--manifest", str(manifest),
        )
        self.assertTrue(marker.exists(), "controlled validation drift was not injected")
        self.assertNotEqual(result.returncode, 0, "lifecycle accepted configuration changed during validation")
        self.assertIn("native environment", result.stderr)
        expected = before.copy()
        expected[str(environment)] = (before[str(environment)][0], before[str(environment)][1],
                                      ("file", replacement))
        self.assertEqual(self._native_lifecycle_snapshot(), expected)
        self.assertFalse(lock_attempt.exists(), "configuration drift was detected only after trying the lock")
        self.assertFalse(lock.exists())

    def test_native_listener_invalid_and_overflow_ports_fail_without_lifecycle_changes(self):
        initial = self._activate_scoped_native()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        (self.data / "port-state-fixture").write_bytes(b"port validation must not alter state\n")
        binary, manifest = self._release_fixture("v1.1.0")
        ports = (str((1 << 64) + 8080), "65536", "0", "00065536",
                 "0000", "000" + str((1 << 64) + 8080))
        for name in ("FROGLET_PROVIDER_URL", "FROGLET_RUNTIME_URL"):
            for port in ports:
                with self.subTest(endpoint=name, port=port):
                    before = self._native_lifecycle_snapshot()
                    result = self._run_scoped_native(
                        "activate", "--binary", str(binary), "--release", "v1.1.0",
                        "--manifest", str(manifest), **{name: f"http://127.0.0.1:{port}"},
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(f"{name} has an invalid port", result.stderr)
                    self.assertEqual(self._native_lifecycle_snapshot(), before)

    def test_native_listener_boundary_and_padded_ports_persist_and_reach_launcher(self):
        for release, provider_port, runtime_port in (
            ("v1.0.0", "1", "65535"),
            ("v1.1.0", "65535", "1"),
            ("v1.2.0", "0008080", "0008081"),
        ):
            with self.subTest(provider_port=provider_port, runtime_port=runtime_port):
                binary, manifest = self._release_fixture(release)
                provider = f"http://127.0.0.1:{provider_port}"
                runtime = f"http://127.0.0.1:{runtime_port}"
                result = self._run_scoped_native(
                    "activate", "--binary", str(binary), "--release", release,
                    "--manifest", str(manifest), FROGLET_PROVIDER_URL=provider,
                    FROGLET_RUNTIME_URL=runtime,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                expected = {
                    "FROGLET_LISTEN_ADDR": f"127.0.0.1:{provider_port}",
                    "FROGLET_RUNTIME_LISTEN_ADDR": f"127.0.0.1:{runtime_port}",
                    "FROGLET_PUBLIC_BASE_URL": provider,
                    "FROGLET_RUNTIME_PROVIDER_BASE_URL": provider,
                }
                environment = self.bootstrap / "native.env"
                persisted = environment.read_text()
                for name, value in expected.items():
                    self.assertIn(f'{name}="{value}"\n', persisted)
                restarted = self._run_scoped_native("restart")
                self.assertEqual(restarted.returncode, 0, restarted.stderr)
                self.assertEqual(environment.read_text(), persisted)
                capture = self.root / f"ports-{release}.env"
                launched = subprocess.run(
                    [str(self.bootstrap / "run-native.sh")], cwd=self.root,
                    env={"PATH": f"{self.stub_dir}:/usr/bin:/bin", "HOME": str(self.home),
                         "FROGLET_TEST_CAPTURE_ENV": str(capture)},
                    text=True, capture_output=True, timeout=15,
                )
                self.assertEqual(launched.returncode, 0, launched.stderr)
                received = dict(line.split("=", 1) for line in capture.read_text().splitlines())
                for name, value in expected.items():
                    self.assertEqual(received[name], value)

    def test_payment_environment_rejects_shell_injection_before_activation(self):
        marker = self.root / "must-not-exist"
        payment = self._payment_environment(
            "FROGLET_PAYMENT_BACKEND=stripe",
            f"FROGLET_STRIPE_SECRET_KEY=$(touch {marker})",
        )
        binary, manifest = self._release_fixture("v1.0.0")
        result = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_SERVICE_START="0",
            FROGLET_PAYMENT_ENV_FILE=str(payment),
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unescaped shell metacharacter", result.stderr)
        self.assertFalse(marker.exists())

    def test_payment_environment_rejects_group_or_world_readable_secrets(self):
        payment = self._payment_environment(
            "FROGLET_PAYMENT_BACKEND=stripe",
            "FROGLET_STRIPE_SECRET_KEY=sk_test_exposed",
        )
        payment.chmod(0o644)
        binary, manifest = self._release_fixture("v1.0.0")
        result = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_SERVICE_START="0",
            FROGLET_PAYMENT_ENV_FILE=str(payment),
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must not be readable by group or other users", result.stderr)
        self.assertFalse((self.bootstrap / "payment.env").exists())

    def test_failed_upgrade_rolls_back_and_preserves_identity_state(self):
        seed = self.data / "identity" / "secp256k1.seed"
        seed.parent.mkdir(parents=True)
        seed.write_text("do-not-change", encoding="utf-8")
        initial = self._activate()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._write_upgrade_installer()

        result = self._run("upgrade", "v2.0.0", FAKE_UPGRADE_PROOF="fail")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rolled back to v1.0.0", result.stderr)
        self.assertEqual((self.bootstrap / "current-release").read_text().strip(), "v1.0.0")
        self.assertEqual(seed.read_text(encoding="utf-8"), "do-not-change")
        self.assertFalse((self.bootstrap / "releases/v2.0.0").exists())

    def test_successful_upgrade_and_uninstall_preserve_state_by_default(self):
        state = self.data / "identity" / "secp256k1.seed"
        state.parent.mkdir(parents=True)
        state.write_text("identity", encoding="utf-8")
        initial = self._activate()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        self._write_upgrade_installer()

        upgraded = self._run("upgrade", "v2.0.0")
        self.assertEqual(upgraded.returncode, 0, upgraded.stderr)
        self.assertEqual((self.bootstrap / "current-release").read_text().strip(), "v2.0.0")
        self.assertEqual((self.bootstrap / "previous").resolve().name, "v1.0.0")

        status_result = self._run("status")
        self.assertEqual(status_result.returncode, 0, status_result.stderr)
        self.assertIn("release=v2.0.0", status_result.stdout)
        self.assertIn(f"state_path={self.data}", status_result.stdout)

        restarted = self._run("restart")
        self.assertEqual(restarted.returncode, 0, restarted.stderr)
        self.assertIn("native MCP command-line probe and local health passed", restarted.stderr)

        uninstalled = self._run("uninstall")
        self.assertEqual(uninstalled.returncode, 0, uninstalled.stderr)
        self.assertTrue(state.exists())
        self.assertFalse(self.bootstrap.exists())
        self.assertFalse((self.bin_dir / "froglet-node").exists())

    def test_existing_release_target_rejects_different_manifest(self):
        initial = self._activate()
        self.assertEqual(initial.returncode, 0, initial.stderr)
        binary, manifest = self._release_fixture("v1.0.0")
        manifest.write_text(
            json.dumps({"release": "v1.0.0", "changed": True}, indent=2) + "\n",
            encoding="utf-8",
        )

        result = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_SERVICE_START="0",
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("different release manifest", result.stderr)

    def test_launchd_definition_uses_the_same_private_launcher_as_systemd(self):
        payment = self._payment_environment(
            "FROGLET_PAYMENT_BACKEND=lightning",
            "FROGLET_LIGHTNING_MODE=mock",
        )
        binary, manifest = self._release_fixture("v1.0.0")
        result = self._run(
            "activate",
            "--binary",
            str(binary),
            "--release",
            "v1.0.0",
            "--manifest",
            str(manifest),
            FROGLET_SERVICE_MANAGER="launchd",
            FROGLET_PAYMENT_ENV_FILE=str(payment),
            FROGLET_NETWORK_MODE="tor",
            FROGLET_MARKETPLACE_URL="https://market.example/v1",
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        plist = (self.home / "Library/LaunchAgents/dev.froglet.node.plist").read_text()
        self.assertIn("<key>RunAtLoad</key><true/>", plist)
        self.assertIn("<key>KeepAlive</key>", plist)
        self.assertIn("<key>SuccessfulExit</key><false/>", plist)
        self.assertIn(
            f"<array><string>{self.bootstrap / 'run-native.sh'}</string></array>",
            plist,
        )
        self.assertNotIn("FROGLET_PAYMENT_BACKEND", plist)
        self.assertNotIn("FROGLET_MARKETPLACE_URL", plist)
        capture = self.root / "launchd.env"
        launched = subprocess.run(
            [str(self.bootstrap / "run-native.sh")],
            env=self._environment(FROGLET_TEST_CAPTURE_ENV=str(capture)),
            text=True,
            capture_output=True,
        )
        self.assertEqual(launched.returncode, 0, launched.stderr)
        launch_environment = capture.read_text(encoding="utf-8")
        self.assertIn("FROGLET_PAYMENT_BACKEND=lightning\n", launch_environment)
        self.assertIn("FROGLET_LIGHTNING_MODE=mock\n", launch_environment)
        self.assertIn("FROGLET_NETWORK_MODE=tor\n", launch_environment)
        self.assertIn(
            "FROGLET_MARKETPLACE_URL=https://market.example/v1\n", launch_environment
        )
        restarted = self._run("restart", FROGLET_SERVICE_MANAGER="launchd")
        self.assertEqual(restarted.returncode, 0, restarted.stderr)
        calls = self.launchctl_log.read_text(encoding="utf-8")
        self.assertGreaterEqual(calls.count("bootstrap "), 2, calls)
        self.assertIn("bootout ", calls)
        self.assertIn("kickstart -k ", calls)


class A2aCounterpartySetupTests(unittest.TestCase):
    """Private setup/review invariants, independent of runtime transport tests."""

    def setUp(self):
        from scripts import setup_a2a_counterparty as setup
        self.setup = setup
        self.directory = tempfile.TemporaryDirectory(prefix="froglet-counterparty-test-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.invite = self.root / "issued-invitation.token"
        self.write_private(self.invite, b"admission-only-" + b"x" * 40)
        self.request = {"kind": "provider", "destination": str(self.root / "bob-settings"),
                        "provider_id": "a" * 64, "requester_id": "b" * 64,
                        "provider_url": "https://bob.example", "offer_hashes": ["c" * 64],
                        "admission_token_file": str(self.invite),
                        "allowances": {"max_total_quotes": 20, "max_total_deals": 10,
                                       "max_total_runtime_ms": 20000}}

    @staticmethod
    def write_private(path, value):
        with path.open("xb") as output:
            output.write(value)
        path.chmod(0o600)

    def provider(self):
        plan, _, _ = self.setup.prepare(self.request)
        result = self.setup.apply(self.request, plan["plan_sha256"])
        return plan, result, Path(result["private_handoff_file"])

    def test_plan_is_read_only_and_never_discloses_credentials(self):
        plan, _, _ = self.setup.prepare(self.request)
        self.assertFalse(Path(self.request["destination"]).exists())
        self.assertEqual(set(self.root.iterdir()), {self.invite})
        self.assertNotIn(self.invite.read_text(), json.dumps(plan))
        self.assertEqual(plan["offer_hashes"], ["c" * 64])
        self.assertFalse(plan["admission_verified"])

    def test_provider_requester_round_trip_preserves_separate_authority(self):
        plan, result, handoff_file = self.provider()
        handoff = json.loads(handoff_file.read_bytes())
        self.assertNotEqual(handoff["a2a_token"], handoff["admission_token"])
        self.assertNotIn(handoff["a2a_token"], json.dumps(result))
        self.assertNotIn(handoff["admission_token"], json.dumps(result))
        request = {"kind": "requester", "destination": str(self.root / "alice-settings"),
                   "handoff_file": str(handoff_file), "expected_provider_id": "a" * 64,
                   "expected_requester_id": "b" * 64}
        alice_plan, _, _ = self.setup.prepare(request)
        alice = self.setup.apply(request, alice_plan["plan_sha256"])
        config = json.loads(Path(alice["configuration"]).read_bytes())
        self.assertEqual(config["clients"], [])
        self.assertEqual(config["providers"], [{"provider_url": "https://bob.example",
                                               "token": handoff["a2a_token"],
                                               "allow_loopback": False}])
        self.assertEqual(Path(alice["access_token_file"]).read_text(), self.invite.read_text())
        bob_config = json.loads(Path(result["configuration"]).read_bytes())
        self.assertEqual(bob_config["clients"][0]["requester_id"], "b" * 64)
        self.assertEqual(bob_config["clients"][0]["offer_hashes"], ["c" * 64])
        activation = (Path(self.request["destination"]) / "activate.sh").read_text()
        self.assertIn("FROGLET_PROVIDER_ACCESS_MODE=invite", activation)
        self.assertIn("FROGLET_PROVIDER_MAX_TOTAL_DEALS=10", activation)
        self.assertIn("FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS=20000", activation)
        for directory in (Path(self.request["destination"]), Path(request["destination"])):
            self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
            for file in directory.iterdir():
                self.assertEqual(stat.S_IMODE(file.stat().st_mode), 0o600)
        self.assertTrue(result["restart_required"])

    def test_stale_plan_refuses_changed_scope_credential_and_existing_config(self):
        plan, _, _ = self.setup.prepare(self.request)
        changed = dict(self.request, offer_hashes=["d" * 64])
        with self.assertRaisesRegex(self.setup.SetupError, "setup_plan_changed"):
            self.setup.apply(changed, plan["plan_sha256"])
        self.invite.write_bytes(b"new-admission-token-" + b"y" * 40)
        with self.assertRaisesRegex(self.setup.SetupError, "setup_plan_changed"):
            self.setup.apply(self.request, plan["plan_sha256"])
        self.assertFalse(Path(self.request["destination"]).exists())
        old = self.root / "existing-a2a.json"
        self.write_private(old, b'{}\n')
        self.request["existing_config"] = str(old)
        plan, _, _ = self.setup.prepare(self.request)
        old.write_bytes(b'{"clients": [], "providers": []}\n')
        with self.assertRaisesRegex(self.setup.SetupError, "setup_plan_changed"):
            self.setup.apply(self.request, plan["plan_sha256"])
        self.assertFalse(Path(self.request["destination"]).exists())

    def test_merge_preserves_unrelated_counterparties_without_mutating_source(self):
        old = {"clients": [{"requester_id": "d" * 64, "token": "other-client-" + "z" * 40,
                             "offer_hashes": ["e" * 64]}],
               "providers": [{"provider_url": "https://other.example", "token": "p" * 40}]}
        path = self.root / "existing-a2a.json"
        original = json.dumps(old).encode()
        self.write_private(path, original)
        self.request["existing_config"] = str(path)
        _, result, _ = self.provider()
        merged = json.loads(Path(result["configuration"]).read_bytes())
        self.assertEqual(merged["clients"][0], old["clients"][0])
        self.assertEqual(merged["providers"], old["providers"])
        self.assertEqual(path.read_bytes(), original)
        with self.assertRaisesRegex(self.setup.SetupError, "already exists"):
            self.setup.apply(self.request, result["plan_sha256"])

    def test_refuses_wrong_id_unapproved_origin_and_operator_token(self):
        _, _, handoff = self.provider()
        request = {"kind": "requester", "destination": str(self.root / "alice-settings"),
                   "handoff_file": str(handoff), "expected_provider_id": "f" * 64,
                   "expected_requester_id": "b" * 64}
        with self.assertRaisesRegex(self.setup.SetupError, "identities"):
            self.setup.prepare(request)
        for url in ("http://bob.example", "https://user:secret@bob.example",
                    "https://bob.example/a2a", "https://bob.example?secret=value"):
            changed = dict(self.request, destination=str(self.root / "next"), provider_url=url)
            with self.subTest(url=url), self.assertRaises(self.setup.SetupError):
                self.setup.prepare(changed)
        with mock.patch.dict(os.environ, {"FROGLET_RUNTIME_AUTH_TOKEN": self.invite.read_text()}):
            changed = dict(self.request, destination=str(self.root / "next"))
            with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime"):
                self.setup.prepare(changed)

    def test_requires_finite_allowances_and_exact_nonempty_scopes(self):
        for allowances in ({}, {"max_total_quotes": 0, "max_total_deals": 10,
                               "max_total_runtime_ms": 20000},
                           {"max_total_quotes": True, "max_total_deals": 10,
                            "max_total_runtime_ms": 20000},
                           {"max_total_quotes": 2**63, "max_total_deals": 10,
                            "max_total_runtime_ms": 20000}):
            with self.subTest(allowances=allowances), self.assertRaises(self.setup.SetupError):
                self.setup.prepare(dict(self.request, allowances=allowances))
        for scope in ([], ["*"], ["c" * 64, "c" * 64], ["C" * 64]):
            with self.subTest(scope=scope), self.assertRaises(self.setup.SetupError):
                self.setup.prepare(dict(self.request, offer_hashes=scope))

    def test_private_files_symlinks_and_invalid_json_fail_without_source_disclosure(self):
        self.invite.chmod(0o644)
        with self.assertRaisesRegex(self.setup.SetupError, "0600"):
            self.setup.prepare(self.request)
        self.invite.chmod(0o600)
        link = self.root / "symlink.token"
        link.symlink_to(self.invite)
        with self.assertRaises(OSError):
            self.setup.prepare(dict(self.request, admission_token_file=str(link)))
        invalid = self.root / "invalid.json"
        self.write_private(invalid, b'{"token":"secret-source-never-output", broken}')
        request_file = self.root / "request.json"
        self.write_private(request_file, self.setup.encoded(dict(self.request, existing_config=str(invalid))))
        result = subprocess.run([sys.executable, str(REPO_ROOT / "scripts/setup_a2a_counterparty.py"),
                                 "--request", str(request_file), "--plan"],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("secret-source-never-output", result.stdout + result.stderr)
        self.assertEqual(result.stderr, "")

    def test_atomic_publication_failure_removes_partial_bundle(self):
        plan, _, _ = self.setup.prepare(self.request)
        with mock.patch.object(self.setup.os, "rename", side_effect=OSError("simulated failure")):
            with self.assertRaises(OSError):
                self.setup.apply(self.request, plan["plan_sha256"])
        self.assertFalse(Path(self.request["destination"]).exists())
        self.assertFalse(any(path.name.startswith(".froglet-a2a-") for path in self.root.iterdir()))

    def test_apply_invalid_request_root_returns_redacted_structured_error(self):
        path = self.root / "invalid-root.json"
        self.write_private(path, b"[]\n")
        result = subprocess.run([sys.executable, str(REPO_ROOT / "scripts/setup_a2a_counterparty.py"),
                                 "--request", str(path), "--approved-plan-sha256", "a" * 64],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(json.loads(result.stdout), {"status": "error", "error": "setup request must be an object"})
        self.assertEqual(result.stderr, "")

    def test_file_based_operator_credentials_cannot_be_delivered_as_invitations(self):
        operator = self.root / "auth.token"
        self.write_private(operator, self.invite.read_bytes())
        with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime token file"):
            self.setup.prepare(dict(self.request, admission_token_file=str(operator)))
        with mock.patch.dict(os.environ, {"FROGLET_RUNTIME_AUTH_TOKEN_PATH": str(operator)}):
            with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime credentials"):
                self.setup.prepare(self.request)
        for name in ("FROGLET_RUNTIME_AUTH_TOKEN", "FROGLET_PROVIDER_CONTROL_TOKEN"):
            with self.subTest(name=name), mock.patch.dict(os.environ, {name: " " + self.invite.read_text() + "\n"}):
                with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime credentials"):
                    self.setup.prepare(self.request)
        for alias in ("FROGLET_PROVIDER_AUTH_TOKEN_PATH", "FROGLET_AUTH_TOKEN_PATH"):
            with self.subTest(alias=alias), mock.patch.dict(os.environ, {alias: str(operator)}):
                with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime credentials"):
                    self.setup.prepare(self.request)

    def test_provider_home_fallback_is_checked_when_only_data_root_is_set(self):
        home = self.root / "home"
        runtime = home / ".froglet/runtime"
        runtime.mkdir(parents=True)
        self.write_private(runtime / "froglet-control.token", self.invite.read_bytes())
        with mock.patch.dict(os.environ, {"HOME": str(home), "FROGLET_DATA_ROOT": str(self.root / "other-data")}):
            os.environ.pop("FROGLET_DATA_DIR", None)
            with self.assertRaisesRegex(self.setup.SetupError, "operator/runtime credentials"):
                self.setup.prepare(self.request)

    def test_concurrent_operation_is_refused(self):
        import fcntl
        plan, _, _ = self.setup.prepare(self.request)
        path = Path(self.request["destination"] + ".froglet-lock")
        self.write_private(path, b"")
        with path.open("rb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaisesRegex(self.setup.SetupError, "another setup"):
                self.setup.apply(self.request, plan["plan_sha256"])
        self.assertFalse(Path(self.request["destination"]).exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
