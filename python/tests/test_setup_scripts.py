import hashlib
import json
import os
import shlex
import stat
import subprocess
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


if __name__ == "__main__":
    unittest.main(verbosity=2)
