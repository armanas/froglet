import hashlib
import json
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import textwrap
import time
import unittest
from pathlib import Path
from python.tests.test_support import _cargo_debug_dir


REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALL_SCRIPT = REPO_ROOT / "scripts" / "install.sh"
AGENT_BOOTSTRAP_SCRIPT = REPO_ROOT / "scripts" / "agent-bootstrap.sh"
RELEASE_MANIFEST_SCRIPT = REPO_ROOT / "scripts" / "release_manifest.py"
SOURCE_REVISION = "0123456789abcdef0123456789abcdef01234567"
IMAGE_REFS = {
    "provider": "ghcr.io/armanas/froglet-provider@sha256:" + "1" * 64,
    "runtime": "ghcr.io/armanas/froglet-runtime@sha256:" + "2" * 64,
    "dual": "ghcr.io/armanas/froglet-dual@sha256:" + "4" * 64,
    "mcp": "ghcr.io/armanas/froglet-mcp@sha256:" + "3" * 64,
}


class InstallScriptTests(unittest.TestCase):
    maxDiff = None

    def setUp(self):
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.assets_root = self.root / "assets"
        self.stub_dir = self.root / "stubs"
        self.home_dir = self.root / "home"
        self.install_dir = self.root / "installed-bin"
        self.curl_log = self.root / "curl.log"

        self.assets_root.mkdir()
        self.stub_dir.mkdir()
        self.home_dir.mkdir()
        self.curl_log.write_text("", encoding="utf-8")

        self._write_stub(
            "curl",
            """#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$CURL_LOG"
if [ "${FROGLET_TEST_REQUIRE_HTTPS_REDIRECTS:-0}" = 1 ]; then
  case " $* " in
    *" --proto =https --proto-redir =https --tlsv1.2 "*) ;;
    *) echo "secure HTTPS redirect flags missing" >&2; exit 91 ;;
  esac
fi
out=""
format=""
url=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      out="$2"
      shift 2
      ;;
    -w)
      format="$2"
      shift 2
      ;;
    -f|-s|-S|-L)
      shift
      ;;
    --fail|--silent|--show-error|--location)
      shift
      ;;
    -H)
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done
if [ -n "$format" ]; then
  printf '%s' "$FAKE_LATEST_URL"
  exit 0
fi
case "$url" in
  */repos/*/releases/tags/*) base="release-api.json" ;;
  */v1/feed*) base="feed.json" ;;
  *) base="${url##*/}" ;;
esac
src="$FROGLET_TEST_ASSET_DIR/$base"
[ -f "$src" ] || {
  echo "missing fixture asset: $src" >&2
  exit 1
}
if [ -n "$out" ]; then
  cp "$src" "$out"
else
  cat "$src"
fi
""",
        )
        self._write_stub(
            "uname",
            """#!/bin/sh
set -eu
case "${1:-}" in
  -s) printf '%s\\n' "$FAKE_UNAME_S" ;;
  -m) printf '%s\\n' "$FAKE_UNAME_M" ;;
  *) /usr/bin/uname "$@" ;;
esac
""",
        )

    def tearDown(self):
        self.temp_dir.cleanup()

    def test_installs_latest_linux_x86_64_release_and_prints_path_hint(self):
        version = "v1.2.3"
        asset_dir = self._create_release_assets(version)

        result = self._run_installer(
            asset_dir,
            extra_env={"FAKE_LATEST_URL": f"https://github.com/armanas/froglet/releases/tag/{version}"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        installed = self.install_dir / "froglet-node"
        self.assertTrue(installed.exists())
        self.assertIn(f"Installed froglet-node to {installed}", result.stdout)
        self.assertIn("Add", result.stdout)
        log = self.curl_log.read_text(encoding="utf-8")
        self.assertIn("https://github.com/armanas/froglet/releases/latest", log)
        self.assertIn(
            f"https://github.com/armanas/froglet/releases/download/{version}/froglet-node-{version}-linux-x86_64.tar.gz",
            log,
        )
        self.assertIn(
            f"https://github.com/armanas/froglet/releases/download/{version}/release-manifest.json",
            log,
        )
        self.assertNotIn("SHA256SUMS", log)

    def test_normalizes_pinned_version_without_v_prefix(self):
        version = "v9.9.9"
        asset_dir = self._create_release_assets(version)

        result = self._run_installer(asset_dir, extra_env={"VERSION": "9.9.9"})

        self.assertEqual(result.returncode, 0, result.stderr)
        log = self.curl_log.read_text(encoding="utf-8")
        self.assertNotIn("/releases/latest", log)
        self.assertIn(
            f"https://github.com/armanas/froglet/releases/download/{version}/froglet-node-{version}-linux-x86_64.tar.gz",
            log,
        )

    def test_requests_linux_arm64_asset(self):
        version = "v2.0.0"
        asset_dir = self._create_release_assets(version)

        result = self._run_installer(
            asset_dir,
            extra_env={"VERSION": version, "FAKE_UNAME_M": "arm64"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        log = self.curl_log.read_text(encoding="utf-8")
        self.assertIn(f"froglet-node-{version}-linux-arm64.tar.gz", log)

    def test_requests_darwin_arm64_asset(self):
        version = "v3.1.4"
        asset_dir = self._create_release_assets(version)

        result = self._run_installer(
            asset_dir,
            extra_env={
                "VERSION": version,
                "FAKE_UNAME_S": "Darwin",
                "FAKE_UNAME_M": "arm64",
            },
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        log = self.curl_log.read_text(encoding="utf-8")
        self.assertIn(f"froglet-node-{version}-darwin-arm64.tar.gz", log)

    def test_fails_when_asset_differs_from_attested_manifest_digest(self):
        version = "v5.0.0"
        asset_dir = self._create_release_assets(version)
        (asset_dir / f"froglet-node-{version}-linux-x86_64.tar.gz").write_bytes(
            b"tampered after manifest generation"
        )

        result = self._run_installer(asset_dir, extra_env={"VERSION": version})

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch", result.stderr)
        self.assertFalse((self.install_dir / "froglet-node").exists())

    def test_installs_without_gh_from_immutable_release_asset_digest(self):
        version = "v5.1.0"
        asset_dir = self._create_release_assets(version)

        result = self._run_installer(
            asset_dir,
            extra_env={
                "VERSION": version,
                "FROGLET_GH_ATTESTATION_MODE": "off",
            },
            use_manifest_pin=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("immutable release asset digest", result.stdout)
        self.assertIn("Skipped optional GitHub artifact-attestation", result.stdout)
        self.assertTrue((self.install_dir / "froglet-node").exists())

    def test_rejects_mutable_github_release_without_manifest_pin(self):
        version = "v5.1.1"
        asset_dir = self._create_release_assets(version)
        release_api = json.loads((asset_dir / "release-api.json").read_text())
        release_api["immutable"] = False
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )

        result = self._run_installer(
            asset_dir, extra_env={"VERSION": version}, use_manifest_pin=False
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is mutable", result.stderr)
        self.assertFalse((self.install_dir / "froglet-node").exists())

    def test_rejects_missing_or_duplicate_manifest_asset(self):
        for suffix, assets, expected in (
            ("missing", [], "found 0"),
            (
                "duplicate",
                [
                    {"name": "release-manifest.json", "digest": "sha256:" + "1" * 64},
                    {"name": "release-manifest.json", "digest": "sha256:" + "2" * 64},
                ],
                "found 2",
            ),
        ):
            with self.subTest(suffix=suffix):
                version = f"v5.1.2-{suffix}"
                asset_dir = self._create_release_assets(version)
                release_api = json.loads((asset_dir / "release-api.json").read_text())
                release_api["assets"] = assets
                (asset_dir / "release-api.json").write_text(
                    json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
                )

                result = self._run_installer(
                    asset_dir,
                    extra_env={"VERSION": version},
                    use_manifest_pin=False,
                )

                self.assertNotEqual(result.returncode, 0)
                self.assertIn(expected, result.stderr)

    def test_rejects_malformed_manifest_asset_digest(self):
        version = "v5.1.3"
        asset_dir = self._create_release_assets(version)
        release_api = json.loads((asset_dir / "release-api.json").read_text())
        release_api["assets"][0]["digest"] = "sha256:not-a-digest"
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )

        result = self._run_installer(
            asset_dir, extra_env={"VERSION": version}, use_manifest_pin=False
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a lowercase SHA-256 digest", result.stderr)

    def test_verifies_offline_attestation_with_pinned_workflow_and_tag(self):
        version = "v5.2.0"
        asset_dir = self._create_release_assets(version)
        gh_log = self.root / "gh.log"
        self._write_stub(
            "gh",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$GH_LOG"
exit 0
""",
        )

        result = self._run_installer(
            asset_dir,
            extra_env={"VERSION": version, "GH_LOG": str(gh_log)},
            use_manifest_pin=False,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        invocation = gh_log.read_text(encoding="utf-8")
        self.assertIn("attestation verify", invocation)
        self.assertIn("--repo armanas/froglet", invocation)
        self.assertIn(
            "--signer-workflow armanas/froglet/.github/workflows/release.yml",
            invocation,
        )
        self.assertIn(f"--source-ref refs/tags/{version}", invocation)
        self.assertIn(f"--source-digest {SOURCE_REVISION}", invocation)
        self.assertIn("--deny-self-hosted-runners", invocation)

    def test_rejects_tampered_manifest_before_parsing(self):
        version = "v5.3.0"
        asset_dir = self._create_release_assets(version)
        manifest = asset_dir / "release-manifest.json"
        trusted_manifest_sha256 = hashlib.sha256(manifest.read_bytes()).hexdigest()
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                IMAGE_REFS["provider"], "ghcr.io/armanas/froglet-provider:latest"
            ),
            encoding="utf-8",
        )

        result = self._run_installer(
            asset_dir,
            extra_env={
                "VERSION": version,
                "FROGLET_RELEASE_MANIFEST_SHA256": trusted_manifest_sha256,
                "FROGLET_TRUSTED_MANIFEST_PIN": "1",
            },
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 mismatch for release-manifest.json", result.stderr)

    def test_manifest_pin_requires_an_explicit_trusted_mode(self):
        version = "v5.3.1"
        asset_dir = self._create_release_assets(version)
        manifest_sha256 = hashlib.sha256(
            (asset_dir / "release-manifest.json").read_bytes()
        ).hexdigest()

        result = self._run_installer(
            asset_dir,
            extra_env={
                "VERSION": version,
                "FROGLET_RELEASE_MANIFEST_SHA256": manifest_sha256,
            },
            use_manifest_pin=False,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires explicit FROGLET_TRUSTED_MANIFEST_PIN=1", result.stderr)
        self.assertFalse((self.install_dir / "froglet-node").exists())

    def test_agent_bootstrap_uses_only_manifest_digest_image_refs(self):
        version = "v5.4.0"
        asset_dir = self._create_release_assets(version)
        manifest = asset_dir / "release-manifest.json"
        manifest_sha256 = hashlib.sha256(manifest.read_bytes()).hexdigest()
        bootstrap_home = self.root / "bootstrap-home"
        bootstrap_dir = bootstrap_home / ".froglet" / "agent"
        install_dir = bootstrap_home / ".local" / "bin"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(bootstrap_home),
                "INSTALL_DIR": str(install_dir),
                "VERSION": version,
                "FROGLET_BOOTSTRAP_DIR": str(bootstrap_dir),
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_RAW_BASE": f"file://{REPO_ROOT}",
                "FROGLET_INSTALL_BASE_URL": f"file://{self.assets_root}",
                "FROGLET_RELEASE_MANIFEST_SHA256": manifest_sha256,
                "FROGLET_TRUSTED_MANIFEST_PIN": "1",
            }
        )

        plan, result = self._run_approved_bootstrap(env, REPO_ROOT)

        self.assertEqual(plan.returncode, 0, plan.stderr)
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout[result.stdout.index("{") :])
        self.assertEqual(payload["release"], version)
        self.assertEqual(payload["source_revision"], SOURCE_REVISION)
        self.assertEqual(payload["provider_image"], IMAGE_REFS["provider"])
        self.assertEqual(payload["runtime_image"], IMAGE_REFS["runtime"])
        self.assertEqual(payload["dual_image"], IMAGE_REFS["dual"])
        self.assertEqual(payload["mcp_image"], IMAGE_REFS["mcp"])
        self.assertEqual(payload["install_mode"], "native")
        self.assertTrue(payload["lifecycle_command"].endswith("froglet-service.sh"))
        self.assertEqual(
            payload["relay_url"], "wss://relay.froglet.dev/v1/tunnel"
        )
        self.assertTrue(payload["relay_configured"])
        self.assertEqual(payload["relay_public_suffix"], "relay.froglet.dev")
        native_environment = (bootstrap_dir / "native.env").read_text(encoding="utf-8")
        self.assertIn(
            'FROGLET_RELAY_URL="wss://relay.froglet.dev/v1/tunnel"',
            native_environment,
        )
        self.assertIn(
            'FROGLET_RELAY_PUBLIC_SUFFIX="relay.froglet.dev"',
            native_environment,
        )
        self.assertNotIn("FROGLET_RELAY_ENABLED", native_environment)
        self.assertNotIn(":latest", json.dumps(payload))
        self.assertIn("verified Release Bundle", result.stderr)
        self.assertNotIn("signed release assets", result.stderr)

    def test_native_install_plan_is_non_mutating_and_binds_exact_platform_bytes(self):
        version = "v5.4.0-plan"
        asset_dir = self._create_release_assets(version)
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        home = self.root / "plan-home"
        bootstrap_dir = home / ".froglet" / "agent"
        install_dir = home / ".local" / "bin"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(install_dir),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(bootstrap_dir),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "plan-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
            }
        )

        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT)],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )

        self.assertEqual(plan.returncode, 0, plan.stderr)
        payload = json.loads(plan.stdout)
        manifest = json.loads(
            (asset_dir / "release-manifest.json").read_text(encoding="utf-8")
        )
        self.assertEqual(payload["status"], "approval_required")
        self.assertEqual(payload["repository"], "armanas/froglet")
        self.assertEqual(payload["release_trust"], "github-immutable-release")
        self.assertEqual(payload["release_tag"], version)
        self.assertEqual(payload["platform"], "linux")
        self.assertEqual(payload["architecture"], "x86_64")
        self.assertEqual(payload["bootstrap_asset"], "agent-bootstrap.sh")
        self.assertEqual(payload["bootstrap_script_path"], str(AGENT_BOOTSTRAP_SCRIPT))
        self.assertEqual(
            payload["binary_asset"],
            manifest["binary_froglet_node_linux_x86_64_asset"],
        )
        self.assertEqual(
            payload["binary_asset_sha256"],
            manifest["binary_froglet_node_linux_x86_64_sha256"],
        )
        for field in (
            "release_manifest_sha256",
            "bootstrap_script_sha256",
            "install_script_sha256",
            "service_script_sha256",
            "setup_agent_script_sha256",
            "setup_payment_script_sha256",
            "install_approval_hash",
        ):
            self.assertRegex(payload[field], r"^[0-9a-f]{64}$")
        self.assertIn(str(install_dir / "froglet-node"), payload["persistent_paths"])
        self.assertEqual(
            payload["lifecycle_lock"]["path"],
            str(bootstrap_dir) + ".lifecycle.lock",
        )
        self.assertFalse(payload["lifecycle_lock"]["created_during_plan"])
        self.assertEqual(payload["service_manager"], "systemd")
        self.assertEqual(payload["agent_project_dir"], str(self.root / "plan-project"))
        self.assertEqual(payload["network_mode"], "clearnet")
        self.assertEqual(payload["marketplace_url"], "https://marketplace.froglet.dev")
        self.assertEqual(
            payload["relay_url"], "wss://relay.froglet.dev/v1/tunnel"
        )
        self.assertEqual(payload["relay_public_suffix"], "relay.froglet.dev")
        self.assertTrue(payload["relay_configured"])
        self.assertFalse(payload["relay_transport_activation_granted"])
        self.assertFalse(payload["relay_connected_before_approval"])
        self.assertEqual(payload["provider_url"], "http://127.0.0.1:8080")
        self.assertEqual(payload["runtime_url"], "http://127.0.0.1:8081")
        self.assertFalse(payload["public_exposure_before_approval"])
        self.assertFalse(payload["start_stack"])
        self.assertEqual(payload["health_attempts"], 60)
        self.assertEqual(payload["health_interval_secs"], 1)
        self.assertEqual(payload["github_attestation_mode"], "auto")
        self.assertIsNone(payload["requester_spend_budget_msat"])
        self.assertIsNone(payload["requester_max_deal_msat"])
        self.assertEqual(payload["seeded_demo_services"], [])
        self.assertEqual(payload["seeded_public_services"], [])
        self.assertEqual(payload["compose_project_name"], "froglet_agent")
        self.assertEqual(payload["mcp_docker_network"], "froglet_agent_default")
        self.assertEqual(payload["paths"]["binary"], str(install_dir / "froglet-node"))
        self.assertEqual(payload["paths"]["bootstrap_dir"], str(bootstrap_dir))
        self.assertEqual(payload["paths"]["data_dir"], str(home / ".froglet" / "data"))
        self.assertEqual(payload["paths"]["agent_config"], "none")
        self.assertEqual(
            payload["script_base"],
            f"https://raw.githubusercontent.com/armanas/froglet/{SOURCE_REVISION}",
        )
        self.assertIn(payload["install_approval_hash"], payload["execute_command"])
        self.assertIn("$FROGLET_INSTALL_APPROVAL_HASH", payload["execute_command_template"])
        self.assertFalse(home.exists(), "plan must not create persistent HOME paths")

        rejected = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", "0" * 64],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("approval hash does not match", rejected.stderr)
        self.assertFalse(home.exists(), "rejected execution must not mutate the host")

        untrusted_override_env = env.copy()
        untrusted_override_env["FROGLET_RAW_BASE"] = "https://mirror.invalid/froglet"
        untrusted_override = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=untrusted_override_env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(untrusted_override.returncode, 0)
        self.assertIn(
            "FROGLET_RAW_BASE is allowed only with explicit", untrusted_override.stderr
        )
        self.assertFalse(home.exists(), "untrusted script base must fail before writes")

        insecure_download_env = env.copy()
        insecure_download_env["FROGLET_INSTALL_BASE_URL"] = "http://fixtures.invalid"
        insecure_download = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=insecure_download_env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(insecure_download.returncode, 0)
        self.assertIn("install material URL must use https://", insecure_download.stderr)
        self.assertFalse(home.exists(), "HTTP install input must fail before writes")

        release_api = json.loads((asset_dir / "release-api.json").read_text())
        bootstrap_asset = next(
            asset
            for asset in release_api["assets"]
            if asset["name"] == "agent-bootstrap.sh"
        )
        bootstrap_asset["state"] = "new"
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )
        non_uploaded = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(non_uploaded.returncode, 0)
        self.assertIn("asset state must be uploaded", non_uploaded.stderr)
        self.assertFalse(home.exists(), "non-uploaded asset must fail before writes")

        bootstrap_asset["state"] = "uploaded"
        release_api["immutable"] = False
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )
        mutable = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(mutable.returncode, 0)
        self.assertIn("is mutable", mutable.stderr)
        self.assertFalse(home.exists(), "mutable release must fail before writes")

    def test_native_bootstrap_rejects_pipe_to_shell_before_network_or_writes(self):
        home = self.root / "pipe-home"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "pipe-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "CURL_LOG": str(self.curl_log),
            }
        )

        result = subprocess.run(
            ["bash", "-s"],
            cwd=self.root,
            env=env,
            input=AGENT_BOOTSTRAP_SCRIPT.read_text(encoding="utf-8"),
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be downloaded to a file", result.stderr)
        self.assertEqual(self.curl_log.read_text(encoding="utf-8"), "")
        self.assertFalse(home.exists())

    def test_native_install_plan_allows_explicit_empty_relay_opt_out(self):
        version = "v5.4.0-relay-opt-out"
        asset_dir = self._create_release_assets(version)
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        home = self.root / "relay-opt-out-home"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "relay-opt-out-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                "FROGLET_RELAY_URL": "",
                "FROGLET_RELAY_PUBLIC_SUFFIX": "",
            }
        )

        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )

        self.assertEqual(plan.returncode, 0, plan.stderr)
        payload = json.loads(plan.stdout)
        self.assertEqual(payload["relay_url"], "")
        self.assertEqual(payload["relay_public_suffix"], "")
        self.assertFalse(payload["relay_configured"])
        self.assertFalse(payload["relay_transport_activation_granted"])
        self.assertFalse(payload["relay_connected_before_approval"])
        self.assertFalse(home.exists(), "relay opt-out planning must remain non-mutating")

    def test_concurrent_execute_fails_second_mutation_while_first_holds_lock(self):
        version = "v5.4.0-concurrent"
        asset_dir = self._create_release_assets(version)
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        home = self.root / "concurrent-home"
        ready = self.root / "first-lock-ready"
        release = self.root / "release-first-lock"
        lock_dir = home / ".froglet" / "agent.lifecycle.lock"
        self._write_stub(
            "mkdir",
            f"""#!/bin/sh
set -eu
if [ "$#" -eq 1 ] && [ "$1" = {str(lock_dir)!r} ]; then
  if /bin/mkdir "$1"; then
    : > {str(ready)!r}
    while [ ! -f {str(release)!r} ]; do /bin/sleep 0.05; done
    exit 0
  fi
  exit 1
fi
exec /bin/mkdir "$@"
""",
        )
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "concurrent-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
            }
        )
        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertEqual(plan.returncode, 0, plan.stderr)
        approval_hash = json.loads(plan.stdout)["install_approval_hash"]

        first = subprocess.Popen(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
            cwd=self.root,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            for _ in range(200):
                if ready.exists():
                    break
                if first.poll() is not None:
                    break
                time.sleep(0.01)
            if not ready.exists():
                release.touch()
                first_stdout, first_stderr = first.communicate(timeout=5)
                self.fail(first_stderr + first_stdout)
            before = sorted(
                str(path.relative_to(home)) for path in home.rglob("*")
            )

            second = subprocess.run(
                ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
                cwd=self.root,
                env=env,
                text=True,
                capture_output=True,
            )

            self.assertNotEqual(second.returncode, 0)
            self.assertIn("lifecycle lock", second.stderr)
            after = sorted(
                str(path.relative_to(home)) for path in home.rglob("*")
            )
            self.assertEqual(after, before)
            self.assertFalse((home / ".local" / "bin" / "froglet-node").exists())
            self.assertFalse((home / ".froglet" / "agent" / "current-release").exists())
        finally:
            release.touch()
        first_stdout, first_stderr = first.communicate(timeout=20)
        self.assertEqual(first.returncode, 0, first_stderr + first_stdout)
        self.assertFalse(lock_dir.exists())

    def test_failure_cleanup_keeps_lifecycle_lock_until_compensation_finishes(self):
        version = "v5.4.0-cleanup-lock"
        asset_dir = self._create_release_assets(version)
        for script_name in ("install.sh", "setup-agent.sh", "setup-payment.sh"):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        cleanup_ready = self.root / "cleanup-ready"
        release_cleanup = self.root / "release-cleanup"
        service_fixture = asset_dir / "froglet-service.sh"
        service_fixture.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  activate) exit 17 ;;
  uninstall)
    : > "$FROGLET_TEST_CLEANUP_READY"
    while [[ ! -f "$FROGLET_TEST_RELEASE_CLEANUP" ]]; do /bin/sleep 0.05; done
    exit 0
    ;;
  *) exit 1 ;;
esac
""",
            encoding="utf-8",
        )
        service_fixture.chmod(0o755)
        home = self.root / "cleanup-lock-home"
        lock_dir = home / ".froglet" / "agent.lifecycle.lock"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "cleanup-lock-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                "FROGLET_TEST_CLEANUP_READY": str(cleanup_ready),
                "FROGLET_TEST_RELEASE_CLEANUP": str(release_cleanup),
            }
        )
        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertEqual(plan.returncode, 0, plan.stderr)
        approval_hash = json.loads(plan.stdout)["install_approval_hash"]
        first = subprocess.Popen(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
            cwd=self.root,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        try:
            for _ in range(300):
                if cleanup_ready.exists():
                    break
                if first.poll() is not None:
                    break
                time.sleep(0.01)
            if not cleanup_ready.exists():
                release_cleanup.touch()
                first_stdout, first_stderr = first.communicate(timeout=5)
                self.fail(first_stderr + first_stdout)
            self.assertTrue(lock_dir.is_dir())
            before = sorted(
                str(path.relative_to(home)) for path in home.rglob("*")
            )

            competitor = subprocess.run(
                ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
                cwd=self.root,
                env=env,
                text=True,
                capture_output=True,
            )

            self.assertNotEqual(competitor.returncode, 0)
            self.assertIn("lifecycle mutation is active", competitor.stderr)
            after = sorted(
                str(path.relative_to(home)) for path in home.rglob("*")
            )
            self.assertEqual(after, before)
            self.assertTrue(lock_dir.is_dir())
        finally:
            release_cleanup.touch()
        first_stdout, first_stderr = first.communicate(timeout=20)
        self.assertNotEqual(first.returncode, 0, first_stderr + first_stdout)
        self.assertFalse(lock_dir.exists())

    def test_documented_first_hop_verifies_immutable_bootstrap_before_execution(self):
        version = "v5.4.0-first-hop"
        asset_dir = self._create_release_assets(version)
        readme = (REPO_ROOT / "README.md").read_text(encoding="utf-8")
        install_section = readme.split("Minimal full local stack from zero:", 1)[1]
        shell_block = install_section.split("```bash\n", 1)[1].split("\n```", 1)[0]
        verification_only = shell_block.split('chmod 0700 "$bootstrap"', 1)[0]
        self.assertNotIn('sh "$bootstrap"', verification_only)
        home = self.root / "first-hop-home"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": f"https://github.com/armanas/froglet/releases/tag/{version}",
                "FROGLET_TEST_REQUIRE_HTTPS_REDIRECTS": "1",
            }
        )

        verified = subprocess.run(
            ["sh", "-eu", "-c", verification_only],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertEqual(verified.returncode, 0, verified.stderr)
        self.assertFalse(home.exists())

        release_api = json.loads((asset_dir / "release-api.json").read_text())
        bootstrap_record = next(
            asset
            for asset in release_api["assets"]
            if asset["name"] == "agent-bootstrap.sh"
        )
        bootstrap_record["digest"] = bootstrap_record["digest"].removeprefix("sha256:")
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )
        malformed_algorithm = subprocess.run(
            ["sh", "-eu", "-c", verification_only],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(malformed_algorithm.returncode, 0)
        self.assertFalse(home.exists(), "first-hop algorithm failure must not mutate HOME")

        for asset in release_api["assets"]:
            if asset["name"] == "agent-bootstrap.sh":
                asset["digest"] = "sha256:" + "0" * 64
        (asset_dir / "release-api.json").write_text(
            json.dumps(release_api, indent=2) + "\n", encoding="utf-8"
        )
        rejected = subprocess.run(
            ["sh", "-eu", "-c", verification_only],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertNotEqual(rejected.returncode, 0)
        self.assertFalse(home.exists(), "first-hop digest failure must not mutate HOME")

    def test_native_install_execution_fails_closed_when_approved_script_drifts(self):
        version = "v5.4.0-drift"
        asset_dir = self._create_release_assets(version)
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        home = self.root / "drift-home"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "drift-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_BOOTSTRAP_START": "0",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
            }
        )
        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )
        self.assertEqual(plan.returncode, 0, plan.stderr)
        approval_hash = json.loads(plan.stdout)["install_approval_hash"]
        with (asset_dir / "install.sh").open("a", encoding="utf-8") as handle:
            handle.write("\n# drift after user approval\n")

        result = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("approval hash does not match", result.stderr)
        self.assertFalse(home.exists())

    def test_native_plan_rejects_running_bootstrap_not_bound_by_manifest(self):
        version = "v5.4.0-bootstrap-mismatch"
        asset_dir = self._create_release_assets(version)
        manifest_path = asset_dir / "release-manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        manifest["agent_bootstrap_sha256"] = "0" * 64
        manifest_path.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        home = self.root / "bootstrap-mismatch-home"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(home),
                "INSTALL_DIR": str(home / ".local" / "bin"),
                "VERSION": version,
                "FROGLET_BOOTSTRAP_DIR": str(home / ".froglet" / "agent"),
                "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(self.root / "bootstrap-mismatch-project"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_RAW_BASE": f"file://{REPO_ROOT}",
                "FROGLET_INSTALL_BASE_URL": f"file://{self.assets_root}",
                "FROGLET_RELEASE_MANIFEST_SHA256": hashlib.sha256(
                    manifest_path.read_bytes()
                ).hexdigest(),
                "FROGLET_TRUSTED_MANIFEST_PIN": "1",
            }
        )

        result = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=self.root,
            env=env,
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("running bootstrap SHA-256 does not match", result.stderr)
        self.assertFalse(home.exists())

    def test_native_bootstrap_reaches_direct_agent_config_and_local_proof_without_runtime_tools(self):
        version = "v5.4.1"
        binary_content = self._bootstrap_binary_content()
        asset_dir = self._create_release_assets(version, binary_content)
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        (asset_dir / "health").write_text('{"status":"ok"}\n', encoding="utf-8")
        systemctl_log = self.root / "systemctl.log"
        self._write_stub(
            "systemctl",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
exit 0
""",
        )
        self._write_stub("loginctl", "#!/bin/sh\nexit 0\n")
        for command in ("python3", "node", "docker", "jq"):
            self._write_stub(
                command,
                f"#!/bin/sh\necho '{command} must not run' >&2\nexit 99\n",
            )

        native_home = self.root / "native-home"
        project_dir = self.root / "agent-project"
        fixture = self.root / "user-supplied.json"
        fixture_original = b'[{"owner":"user","value":17},{"owner":"user","value":23}]\n'
        fixture.write_bytes(fixture_original)
        fixture_copy_count = self.root / "fixture-copy-count"
        self._write_stub(
            "cp",
            """#!/bin/sh
set -eu
if [ "${1:-}" = "$FROGLET_TEST_REPLACE_SOURCE" ]; then
  case "${2:-}" in
    */data-fixture)
      /bin/cp "$@"
      count=0
      [ ! -f "$FROGLET_TEST_COPY_COUNT" ] || count="$(cat "$FROGLET_TEST_COPY_COUNT")"
      count=$((count + 1))
      printf '%s\n' "$count" > "$FROGLET_TEST_COPY_COUNT"
      if [ "$count" = 2 ]; then
        replacement="${FROGLET_TEST_REPLACE_SOURCE}.replacement"
        printf '%s\n' '[{"owner":"attacker","value":999}]' > "$replacement"
        /bin/mv -f "$replacement" "$FROGLET_TEST_REPLACE_SOURCE"
      fi
      exit 0
      ;;
  esac
fi
exec /bin/cp "$@"
""",
        )
        bootstrap_dir = native_home / ".froglet" / "agent"
        install_dir = native_home / ".local" / "bin"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(native_home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(install_dir),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(bootstrap_dir),
                "FROGLET_DATA_DIR": str(native_home / ".froglet" / "data"),
                "FROGLET_AGENT_PROJECT_DIR": str(project_dir),
                "FROGLET_AGENT_TARGET": "claude-code",
                "FROGLET_BOOTSTRAP_DATA_FIXTURE": str(fixture),
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_PAYMENT_BACKEND": "lightning",
                "FROGLET_LIGHTNING_MODE": "mock",
                "FROGLET_NETWORK_MODE": "dual",
                "FROGLET_MARKETPLACE_URL": "https://market.example/v1",
                "FROGLET_REQUESTER_SPEND_BUDGET_MSAT": "120000",
                "FROGLET_REQUESTER_MAX_DEAL_MSAT": "9000",
                "FROGLET_RELAY_URL": "wss://relay.example/v1/tunnel",
                "FROGLET_RELAY_PUBLIC_SUFFIX": "relay.example",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                "FROGLET_HEALTH_ATTEMPTS": "1",
                "FROGLET_HEALTH_INTERVAL_SECS": "0",
                "FROGLET_GH_ATTESTATION_MODE": "off",
                "SYSTEMCTL_LOG": str(systemctl_log),
                "FROGLET_TEST_REPLACE_SOURCE": str(fixture),
                "FROGLET_TEST_COPY_COUNT": str(fixture_copy_count),
            }
        )

        plan, result = self._run_approved_bootstrap(env, self.root)

        self.assertEqual(plan.returncode, 0, plan.stderr)
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout[result.stdout.index("{") :])
        self.assertEqual(payload["install_mode"], "native")
        self.assertTrue(payload["service_started"])
        self.assertTrue(payload["local_proof"])
        self.assertTrue(payload["data_proof"])
        self.assertEqual(payload["relay_url"], "wss://relay.example/v1/tunnel")
        self.assertTrue(payload["relay_configured"])
        self.assertEqual(payload["relay_public_suffix"], "relay.example")
        self.assertFalse(payload["relay_connected"])
        self.assertEqual(payload["network_mode"], "dual")
        self.assertEqual(payload["payment_backend"], "lightning")
        self.assertEqual(payload["marketplace_url"], "https://market.example/v1")
        self.assertEqual(payload["requester_spend_budget_msat"], 120000)
        self.assertEqual(payload["requester_max_deal_msat"], 9000)
        self.assertEqual(payload["data_service_id"], "froglet-install-data")
        config = json.loads((project_dir / ".mcp.json").read_text(encoding="utf-8"))
        server = config["mcpServers"]["froglet"]
        self.assertEqual(server["command"], str(install_dir / "froglet-node"))
        self.assertEqual(server["args"], ["mcp"])
        self.assertTrue((bootstrap_dir / "local-proof.json").exists())
        data_proof_path = Path(payload["data_proof_path"])
        data_proof = json.loads(data_proof_path.read_text(encoding="utf-8"))
        self.assertEqual(data_proof["release"], version)
        self.assertEqual(data_proof["state_path"], str(native_home / ".froglet/data"))
        self.assertEqual(data_proof["fixture_source"], str(fixture))
        self.assertEqual(data_proof["invocation"]["result"]["source_kind"], "json")
        self.assertFalse(data_proof["unpublish"]["result"]["isError"])
        self.assertTrue(data_proof["active_offer_catalog_restored"])
        self.assertEqual(
            data_proof["final_active_offer_ids_sha256"],
            data_proof["preflight_active_offer_ids_sha256"],
        )
        staged_fixture = Path(data_proof["staged_fixture_path"])
        self.assertEqual(
            data_proof["staged_fixture_sha256"],
            hashlib.sha256(fixture_original).hexdigest(),
        )
        self.assertTrue(data_proof["staged_fixture_removed"])
        self.assertFalse(staged_fixture.exists())
        self.assertNotEqual(fixture.read_bytes(), fixture_original)
        self.assertEqual(fixture_copy_count.read_text().strip(), "2")
        self.assertFalse(staged_fixture.with_name("froglet-service.toml").exists())
        native_environment = (bootstrap_dir / "native.env").read_text(encoding="utf-8")
        self.assertIn(
            'FROGLET_RELAY_URL="wss://relay.example/v1/tunnel"',
            native_environment,
        )
        self.assertIn(
            'FROGLET_RELAY_PUBLIC_SUFFIX="relay.example"', native_environment
        )
        self.assertNotIn("FROGLET_RELAY_ENABLED", native_environment)
        self.assertIn('FROGLET_NETWORK_MODE="dual"', native_environment)
        self.assertIn(
            'FROGLET_MARKETPLACE_URL="https://market.example/v1"', native_environment
        )
        self.assertIn(
            'FROGLET_REQUESTER_SPEND_BUDGET_MSAT="120000"', native_environment
        )
        self.assertIn('FROGLET_REQUESTER_MAX_DEAL_MSAT="9000"', native_environment)
        payment_environment = (bootstrap_dir / "payment.env").read_text(encoding="utf-8")
        self.assertEqual(
            payment_environment,
            "FROGLET_PAYMENT_BACKEND=lightning\nFROGLET_LIGHTNING_MODE=mock\n",
        )
        self.assertEqual(stat.S_IMODE((bootstrap_dir / "payment.env").stat().st_mode), 0o600)
        self.assertNotIn("must not run", result.stderr)
        self.assertIn("immutable release asset digest", result.stdout)
        self.assertIn("--user enable --now froglet.service", systemctl_log.read_text())

        # Reconnecting the same release preserves every persistent setting,
        # unrelated agent server, and byte of an already merged configuration.
        config["mcpServers"]["unrelated"] = {"command": "keep-me"}
        config["theme"] = "personal"
        config_path = project_dir / ".mcp.json"
        config_path.write_text(json.dumps(config, indent=4) + "\n")
        before_config = config_path.read_bytes()
        before_manager = systemctl_log.read_text()
        repeat_plan, repeated = self._run_approved_bootstrap(env, self.root)
        self.assertEqual(repeat_plan.returncode, 0, repeat_plan.stderr)
        self.assertEqual(repeated.returncode, 0, repeated.stderr)
        self.assertTrue(json.loads(repeated.stdout[repeated.stdout.index("{"):])["existing_installation_reused"])
        self.assertEqual(config_path.read_bytes(), before_config)
        self.assertEqual((bootstrap_dir / "native.env").read_text(), native_environment)
        self.assertEqual((bootstrap_dir / "payment.env").read_text(), payment_environment)
        new_manager_calls = systemctl_log.read_text()[len(before_manager):]
        self.assertNotIn("restart", new_manager_calls)
        self.assertNotIn("disable", new_manager_calls)

        # A configuration failure on reconnect must never uninstall the
        # working node or erase the malformed file the user needs to repair.
        config_path.write_text('{"unfinished":')
        _, failed_reconnect = self._run_approved_bootstrap(env, self.root)
        self.assertNotEqual(failed_reconnect.returncode, 0)
        self.assertTrue((bootstrap_dir / "current/froglet-node").exists())
        self.assertTrue((install_dir / "froglet-node").exists())
        self.assertEqual(config_path.read_text(), '{"unfinished":')
        self.assertEqual((bootstrap_dir / "native.env").read_text(), native_environment)
        self.assertNotIn("uninstalling", failed_reconnect.stderr)

    def test_failed_native_bootstrap_uninstalls_partially_activated_service(self):
        version = "v5.4.1-cleanup"
        failing_binary = self._bootstrap_binary_content().replace(
            "              publish)\n",
            "              publish)\n                exit 41\n",
        )
        asset_dir = self._create_release_assets(
            version, failing_binary
        )
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        (asset_dir / "health").write_text('{"status":"ok"}\n', encoding="utf-8")
        systemctl_log = self.root / "failed-bootstrap-systemctl.log"
        self._write_stub(
            "systemctl",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
exit 0
""",
        )
        self._write_stub("loginctl", "#!/bin/sh\nexit 0\n")

        native_home = self.root / "failed-bootstrap-home"
        fixture = self.root / "failed-bootstrap-data.json"
        fixture.write_text('[{"owner":"user","value":17}]\n', encoding="utf-8")
        bootstrap_dir = native_home / ".froglet" / "agent"
        install_dir = native_home / ".local" / "bin"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(native_home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(install_dir),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(bootstrap_dir),
                "FROGLET_DATA_DIR": str(native_home / ".froglet" / "data"),
                "FROGLET_AGENT_TARGET": "claude-code",
                "FROGLET_BOOTSTRAP_DATA_FIXTURE": str(fixture),
                "FROGLET_BOOTSTRAP_MODE": "native",
                "FROGLET_SERVICE_MANAGER": "systemd",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                "FROGLET_HEALTH_ATTEMPTS": "1",
                "FROGLET_HEALTH_INTERVAL_SECS": "0",
                "FROGLET_GH_ATTESTATION_MODE": "off",
                "SYSTEMCTL_LOG": str(systemctl_log),
            }
        )

        plan, result = self._run_approved_bootstrap(env, self.root)

        self.assertEqual(plan.returncode, 0, plan.stderr)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bootstrap failed; uninstalling", result.stderr)
        self.assertFalse(bootstrap_dir.exists())
        self.assertFalse(Path(str(bootstrap_dir) + ".lifecycle.lock").exists())
        self.assertFalse((install_dir / "froglet-node").exists())
        self.assertIn("--user disable --now froglet.service", systemctl_log.read_text())

        docker_log = self.root / "failed-bootstrap-docker.log"
        self._write_stub(
            "docker",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$DOCKER_LOG"
case " $* " in
  *" exec "*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"status":"ok","native_mcp":true},"isError":false}}' ;;
esac
exit 0
""",
        )
        docker_home = self.root / "failed-docker-home"
        docker_bootstrap = docker_home / ".froglet" / "agent"
        docker_env = env.copy()
        docker_env.update(
            {
                "HOME": str(docker_home),
                "INSTALL_DIR": str(docker_home / ".local" / "bin"),
                "FROGLET_BOOTSTRAP_DIR": str(docker_bootstrap),
                "FROGLET_DATA_DIR": str(docker_home / ".froglet" / "data"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "docker",
                "DOCKER_LOG": str(docker_log),
            }
        )

        docker_plan, docker_result = self._run_approved_bootstrap(
            docker_env, self.root
        )

        self.assertEqual(docker_plan.returncode, 0, docker_plan.stderr)
        self.assertNotEqual(docker_result.returncode, 0)
        self.assertIn("stopping the partially activated Docker service", docker_result.stderr)
        transient_root = (
            docker_bootstrap / "proofs" / version / "native-data"
        )
        self.assertFalse(transient_root.exists())
        self.assertFalse(Path(str(docker_bootstrap) + ".lifecycle.lock").exists())
        self.assertIn("down --remove-orphans", docker_log.read_text(encoding="utf-8"))

    def test_transient_proof_compensates_failures_and_rejects_service_id_collision(self):
        version = "v5.4.1-proof-compensation"
        asset_dir = self._create_release_assets(
            version, self._bootstrap_binary_content()
        )
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        (asset_dir / "health").write_text('{"status":"ok"}\n', encoding="utf-8")
        systemctl_log = self.root / "proof-compensation-systemctl.log"
        docker_log = self.root / "proof-compensation-docker.log"
        self._write_stub(
            "systemctl",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SYSTEMCTL_LOG"
exit 0
""",
        )
        self._write_stub("loginctl", "#!/bin/sh\nexit 0\n")
        self._write_stub(
            "docker",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$DOCKER_LOG"
case " $* " in
  *" exec "*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"status":"ok","native_mcp":true},"isError":false}}' ;;
esac
exit 0
""",
        )

        def run_case(name: str, mode: str, flag: str):
            home = self.root / f"{name}-home"
            bootstrap = home / ".froglet" / "agent"
            if flag == "FROGLET_TEST_UNPUBLISH_FAILURE":
                data_dir = home / ".froglet" / "data"
                data_dir.mkdir(parents=True)
                marker = data_dir / "froglet-install-proof-published"
                marker.write_text(
                    f"state=cleanup-required\nservice_id=froglet-install-data\nrelease={version}\n",
                    encoding="utf-8",
                )
                marker.chmod(0o600)
            publish_marker = self.root / f"{name}-published"
            invoke_marker = self.root / f"{name}-invoked"
            unpublish_marker = self.root / f"{name}-unpublished"
            env = os.environ.copy()
            env.update(
                {
                    "HOME": str(home),
                    "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                    "INSTALL_DIR": str(home / ".local" / "bin"),
                    "VERSION": version,
                    "CURL_LOG": str(self.curl_log),
                    "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                    "FAKE_LATEST_URL": "unused",
                    "FAKE_UNAME_S": "Linux",
                    "FAKE_UNAME_M": "x86_64",
                    "FROGLET_BOOTSTRAP_DIR": str(bootstrap),
                    "FROGLET_DATA_DIR": str(home / ".froglet" / "data"),
                    "FROGLET_AGENT_TARGET": "manual",
                    "FROGLET_BOOTSTRAP_MODE": mode,
                    "FROGLET_SERVICE_MANAGER": "systemd",
                    "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                    "FROGLET_HEALTH_ATTEMPTS": "1",
                    "FROGLET_HEALTH_INTERVAL_SECS": "0",
                    "FROGLET_GH_ATTESTATION_MODE": "off",
                    "SYSTEMCTL_LOG": str(systemctl_log),
                    "DOCKER_LOG": str(docker_log),
                    "FROGLET_TEST_PUBLISH_MARKER": str(publish_marker),
                    "FROGLET_TEST_INVOKE_MARKER": str(invoke_marker),
                    "FROGLET_TEST_UNPUBLISH_MARKER": str(unpublish_marker),
                    flag: "1",
                }
            )
            plan, result = self._run_approved_bootstrap(env, self.root)
            self.assertEqual(plan.returncode, 0, plan.stderr)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(Path(str(bootstrap) + ".lifecycle.lock").exists())
            return (
                home,
                bootstrap,
                publish_marker,
                invoke_marker,
                unpublish_marker,
                result,
            )

        (
            publication_home,
            _,
            publication_marker,
            publication_invoke_marker,
            publication_unpublish_marker,
            publication_result,
        ) = run_case(
            "after-publication", "native", "FROGLET_TEST_FAIL_AFTER_PUBLICATION"
        )
        self.assertTrue(publication_marker.exists())
        self.assertFalse(publication_invoke_marker.exists())
        self.assertTrue(publication_unpublish_marker.exists())
        self.assertIn("attempting exact unpublish", publication_result.stderr)
        self.assertIn(
            "restored the active-offer catalog baseline", publication_result.stderr
        )
        self.assertFalse(
            (publication_home / ".froglet/data/froglet-install-compensation-required").exists()
        )
        self.assertFalse(
            (publication_home / ".froglet/data/froglet-install-proof-published").exists()
        )

        (
            invocation_home,
            invocation_bootstrap,
            invocation_publish_marker,
            invocation_marker,
            invocation_unpublish_marker,
            invocation_result,
        ) = run_case(
            "after-invocation", "docker", "FROGLET_TEST_FAIL_AFTER_INVOCATION"
        )
        self.assertTrue(invocation_publish_marker.exists())
        self.assertTrue(invocation_marker.exists())
        self.assertTrue(invocation_unpublish_marker.exists())
        self.assertIn("attempting exact unpublish", invocation_result.stderr)
        self.assertFalse(
            (invocation_home / ".froglet/data/froglet-install-compensation-required").exists()
        )
        self.assertFalse(
            (invocation_home / ".froglet/data/froglet-install-proof-published").exists()
        )
        self.assertFalse(
            (invocation_bootstrap / "proofs" / version / "native-data").exists()
        )

        (
            _,
            _,
            collision_publish_marker,
            collision_invoke_marker,
            collision_unpublish_marker,
            collision_result,
        ) = run_case(
            "service-id-collision", "native", "FROGLET_TEST_PUBLICATION_COLLISION"
        )
        self.assertIn("service ID already exists", collision_result.stderr)
        self.assertFalse(collision_publish_marker.exists())
        self.assertFalse(collision_invoke_marker.exists())
        self.assertFalse(collision_unpublish_marker.exists())

        (
            recovery_home,
            _,
            recovery_publish_marker,
            recovery_invoke_marker,
            recovery_unpublish_marker,
            recovery_result,
        ) = run_case(
            "unproved-recovery", "native", "FROGLET_TEST_UNPUBLISH_FAILURE"
        )
        self.assertIn("cleanup intent could not be resolved", recovery_result.stderr)
        self.assertFalse(recovery_publish_marker.exists())
        self.assertFalse(recovery_invoke_marker.exists())
        self.assertFalse(recovery_unpublish_marker.exists())
        self.assertTrue(
            (recovery_home / ".froglet/data/froglet-install-proof-published").exists()
        )
        self.assertTrue(
            (
                recovery_home
                / ".froglet/data/froglet-install-compensation-required"
            ).exists()
        )

    def test_docker_fallback_uses_released_dual_role_image_and_same_local_proof(self):
        version = "v5.4.2"
        asset_dir = self._create_release_assets(
            version, self._bootstrap_binary_content()
        )
        for script_name in (
            "install.sh",
            "froglet-service.sh",
            "setup-agent.sh",
            "setup-payment.sh",
        ):
            shutil.copy(REPO_ROOT / "scripts" / script_name, asset_dir / script_name)
        (asset_dir / "health").write_text('{"status":"ok"}\n', encoding="utf-8")
        manifest_sha256 = hashlib.sha256(
            (asset_dir / "release-manifest.json").read_bytes()
        ).hexdigest()
        docker_log = self.root / "docker.log"
        self._write_stub(
            "docker",
            """#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$DOCKER_LOG"
case " $* " in
  *" exec "*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"status":"ok","native_mcp":true},"isError":false}}' ;;
esac
exit 0
""",
        )
        for command in ("python3", "node", "jq"):
            self._write_stub(
                command,
                f"#!/bin/sh\necho '{command} must not run' >&2\nexit 99\n",
            )

        fallback_home = self.root / "fallback-home"
        bootstrap_dir = fallback_home / ".froglet" / "agent"
        install_dir = fallback_home / ".local" / "bin"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(fallback_home),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "INSTALL_DIR": str(install_dir),
                "VERSION": version,
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "unused",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
                "FROGLET_BOOTSTRAP_DIR": str(bootstrap_dir),
                "FROGLET_DATA_DIR": str(fallback_home / ".froglet" / "data"),
                "FROGLET_AGENT_TARGET": "manual",
                "FROGLET_BOOTSTRAP_MODE": "docker",
                "FROGLET_RELAY_URL": "wss://relay.example/v1/tunnel",
                "FROGLET_RELAY_PUBLIC_SUFFIX": "relay.example",
                "FROGLET_RAW_BASE": "https://fixtures.invalid",
                "FROGLET_INSTALL_BASE_URL": "https://fixtures.invalid",
                "FROGLET_RELEASE_MANIFEST_SHA256": manifest_sha256,
                "FROGLET_TRUSTED_MANIFEST_PIN": "1",
                "FROGLET_HEALTH_ATTEMPTS": "1",
                "FROGLET_HEALTH_INTERVAL_SECS": "0",
                "DOCKER_LOG": str(docker_log),
            }
        )

        plan, result = self._run_approved_bootstrap(env, self.root)

        self.assertEqual(plan.returncode, 0, plan.stderr)
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads(result.stdout[result.stdout.index("{") :])
        self.assertEqual(payload["install_mode"], "docker")
        self.assertTrue(payload["compose_started"])
        self.assertTrue(payload["local_proof"])
        self.assertTrue(payload["data_proof"])
        self.assertEqual(payload["relay_url"], "wss://relay.example/v1/tunnel")
        self.assertTrue(payload["relay_configured"])
        self.assertEqual(payload["relay_public_suffix"], "relay.example")
        self.assertFalse(payload["relay_connected"])
        data_proof = json.loads(
            Path(payload["data_proof_path"]).read_text(encoding="utf-8")
        )
        self.assertEqual(data_proof["release"], version)
        self.assertEqual(data_proof["invocation"]["result"]["rows"][0]["id"], "proof")
        self.assertFalse(data_proof["unpublish"]["result"]["isError"])
        self.assertTrue(data_proof["active_offer_catalog_restored"])
        self.assertEqual(
            data_proof["final_active_offer_ids_sha256"],
            data_proof["preflight_active_offer_ids_sha256"],
        )
        self.assertTrue(data_proof["staged_fixture_removed"])
        self.assertFalse(Path(data_proof["staged_fixture_path"]).exists())
        compose = (bootstrap_dir / "compose.yaml").read_text(encoding="utf-8")
        self.assertIn(f'image: "{IMAGE_REFS["dual"]}"', compose)
        self.assertIn("  froglet:\n", compose)
        self.assertNotIn("  provider:\n", compose)
        self.assertNotIn("  runtime:\n", compose)
        self.assertNotIn("FROGLET_PUBLISH_DEMO_SERVICES", compose)
        self.assertIn(
            'FROGLET_RELAY_URL: "wss://relay.example/v1/tunnel"', compose
        )
        self.assertIn('FROGLET_RELAY_PUBLIC_SUFFIX: "relay.example"', compose)
        self.assertNotIn("FROGLET_RELAY_ENABLED", compose)
        self.assertIn("froglet froglet-node mcp", docker_log.read_text())

    def test_manifest_generator_rejects_mutable_image_tags(self):
        version = "v5.5.0"
        asset_dir = self._create_release_assets(version)
        result = subprocess.run(
            [
                "python3",
                str(RELEASE_MANIFEST_SCRIPT),
                "generate",
                "--release",
                version,
                "--repository",
                "armanas/froglet",
                "--source-revision",
                SOURCE_REVISION,
                "--checksums",
                str(asset_dir / "SHA256SUMS"),
                "--provider-image",
                "ghcr.io/armanas/froglet-provider:latest",
                "--runtime-image",
                IMAGE_REFS["runtime"],
                "--dual-image",
                IMAGE_REFS["dual"],
                "--mcp-image",
                IMAGE_REFS["mcp"],
                "--out",
                str(self.root / "bad-manifest.json"),
            ],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("immutable OCI sha256 digest reference", result.stderr)

    def test_manifest_records_only_digest_identical_image_mirrors(self):
        version = "v5.6.0"
        asset_dir = self._create_release_assets(version)
        mirror = "registry.example/froglet-provider@sha256:" + "1" * 64
        manifest = self.root / "manifest-with-mirror.json"
        result = subprocess.run(
            [
                "python3",
                str(RELEASE_MANIFEST_SCRIPT),
                "generate",
                "--release",
                version,
                "--repository",
                "armanas/froglet",
                "--source-revision",
                SOURCE_REVISION,
                "--checksums",
                str(asset_dir / "SHA256SUMS"),
                "--provider-image",
                IMAGE_REFS["provider"],
                "--provider-image-mirror",
                mirror,
                "--runtime-image",
                IMAGE_REFS["runtime"],
                "--dual-image",
                IMAGE_REFS["dual"],
                "--mcp-image",
                IMAGE_REFS["mcp"],
                "--out",
                str(manifest),
            ],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        document = json.loads(manifest.read_text(encoding="utf-8"))
        self.assertEqual(document["agent_bootstrap_asset"], "agent-bootstrap.sh")
        self.assertEqual(
            document["agent_bootstrap_sha256"],
            hashlib.sha256(AGENT_BOOTSTRAP_SCRIPT.read_bytes()).hexdigest(),
        )
        self.assertEqual(document["image_provider_mirrors"], [mirror])
        self.assertEqual(document["image_runtime_mirrors"], [])
        self.assertEqual(document["image_mcp_mirrors"], [])

        mismatched = "registry.example/froglet-provider@sha256:" + "9" * 64
        result = subprocess.run(
            [
                "python3",
                str(RELEASE_MANIFEST_SCRIPT),
                "generate",
                "--release",
                version,
                "--repository",
                "armanas/froglet",
                "--source-revision",
                SOURCE_REVISION,
                "--checksums",
                str(asset_dir / "SHA256SUMS"),
                "--provider-image",
                IMAGE_REFS["provider"],
                "--provider-image-mirror",
                mismatched,
                "--runtime-image",
                IMAGE_REFS["runtime"],
                "--dual-image",
                IMAGE_REFS["dual"],
                "--mcp-image",
                IMAGE_REFS["mcp"],
                "--out",
                str(self.root / "mismatched-mirror.json"),
            ],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
        )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "image_provider_mirrors digest must match image_provider", result.stderr
        )

    def test_release_workflow_checksums_and_attests_bootstrap_asset(self):
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            "sha256sum *.tar.gz agent-bootstrap.sh > SHA256SUMS", workflow
        )
        self.assertIn("dist/agent-bootstrap.sh", workflow)
        self.assertIn("subject-path: dist/agent-bootstrap.sh", workflow)
        self.assertIn("dist/agent-bootstrap.intoto.jsonl", workflow)
        self.assertIn("--assets-dir dist", workflow)
        draft_check = workflow.index(
            "Require a draft created after operator immutable-release check"
        )
        asset_build = workflow.index("  build-release-assets:")
        self.assertLess(draft_check, asset_build)
        draft_block = workflow[draft_check:asset_build]
        self.assertIn("--json isDraft -q .isDraft", draft_block)
        self.assertIn('[[ "$current_draft" == "true" ]]', draft_block)
        self.assertIn('[[ "$remote_tag_commit" == "$GITHUB_SHA" ]]', draft_block)
        self.assertIn("published release is not immutable", workflow)

    def _run_installer(self, asset_dir: Path, extra_env=None, use_manifest_pin=True):
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(self.home_dir),
                "INSTALL_DIR": str(self.install_dir),
                "PATH": f"{self.stub_dir}:/usr/bin:/bin",
                "CURL_LOG": str(self.curl_log),
                "FROGLET_TEST_ASSET_DIR": str(asset_dir),
                "FAKE_LATEST_URL": "https://github.com/armanas/froglet/releases/tag/v0.0.0",
                "FAKE_UNAME_S": "Linux",
                "FAKE_UNAME_M": "x86_64",
            }
        )
        if use_manifest_pin:
            env["FROGLET_RELEASE_MANIFEST_SHA256"] = hashlib.sha256(
                (asset_dir / "release-manifest.json").read_bytes()
            ).hexdigest()
            env["FROGLET_TRUSTED_MANIFEST_PIN"] = "1"
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["sh", str(INSTALL_SCRIPT)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
        )

    def _run_approved_bootstrap(self, env, cwd):
        plan = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "plan"],
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
        )
        if plan.returncode != 0:
            return plan, plan
        payload = json.loads(plan.stdout[plan.stdout.index("{") :])
        approval_hash = payload["install_approval_hash"]
        result = subprocess.run(
            ["sh", str(AGENT_BOOTSTRAP_SCRIPT), "execute", approval_hash],
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
        )
        return plan, result

    @staticmethod
    def _bootstrap_binary_content() -> str:
        script = textwrap.dedent(
            """\
            #!/bin/sh
            set -eu
            case "${1:-}" in
              configure-agent)
                exec FROGLET_TEST_CONFIG_MERGER "$@"
                ;;
              mcp)
                if [ "${2:-}" = "--probe" ]; then
                  printf '%s\n' '{"status":"ok","expected_sum":42}'
                  exit 0
                fi
                request="$(cat)"
                case "$request" in
                  *publication_status*)
                    if [ "${FROGLET_TEST_PUBLICATION_COLLISION:-0}" = 1 ]; then
                      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"structuredContent":{"status":"paused","service_id":"froglet-install-data"},"isError":false}}'
                    else
                      printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"structuredContent":{"status":"error","error":"publication_status returned HTTP 404 Not Found: publication not found"},"isError":true}}'
                    fi
                    ;;
                  *publication_unpublish*)
                    if [ "${FROGLET_TEST_UNPUBLISH_FAILURE:-0}" = 1 ]; then
                      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"structuredContent":{"status":"error","error":"publication_unpublish returned HTTP 500 Internal Server Error"},"isError":true}}'
                      exit 0
                    fi
                    if [ -n "${FROGLET_TEST_UNPUBLISH_MARKER:-}" ]; then
                      : > "$FROGLET_TEST_UNPUBLISH_MARKER"
                    fi
                    printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"structuredContent":{"status":"unpublished"},"isError":false}}'
                    ;;
                  *)
                    printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"structuredContent":{"status":"ok","native_mcp":true},"isError":false}}'
                    ;;
                esac
                exit 0
                ;;
              publish)
                [ -f froglet-service.toml ]
                [ -f fixture.json ]
                if [ -n "${FROGLET_TEST_PUBLISH_MARKER:-}" ]; then
                  : > "$FROGLET_TEST_PUBLISH_MARKER"
                fi
                if [ "${FROGLET_TEST_FAIL_AFTER_PUBLICATION:-0}" = 1 ]; then
                  printf '%s\n' '{}'
                  exit 0
                fi
                printf '%s\n' '{"provider_id":"provider","public_url":"http://127.0.0.1:8080","offer_hash":"offer","marketplace_offer_url":null,"invoke_command":"froglet-node invoke froglet-install-data","status_url":null,"local_verification":{"input_hash":"fixture"},"publication_revision":{"revision_hash":"revision"},"warnings":[]}'
                exit 0
                ;;
              invoke)
                if [ -n "${FROGLET_TEST_INVOKE_MARKER:-}" ]; then
                  : > "$FROGLET_TEST_INVOKE_MARKER"
                fi
                if [ "${FROGLET_TEST_FAIL_AFTER_INVOCATION:-0}" = 1 ]; then
                  exit 42
                fi
                printf '%s\n' '{"service_id":"froglet-install-data","provider_id":"provider","provider_url":"http://127.0.0.1:8080","deal_id":"deal","status":"succeeded","terminal":true,"result":{"source_kind":"json","rows":[{"id":"proof"}]}}'
                exit 0
                ;;
            esac
            exit 0
            """
        )
        import shlex
        return script.replace("FROGLET_TEST_CONFIG_MERGER", shlex.quote(str(_cargo_debug_dir() / "froglet-node")))

    def _create_release_assets(self, version: str, binary_content: str | None = None) -> Path:
        version_dir = self.assets_root / version
        version_dir.mkdir(parents=True, exist_ok=True)

        sums = []
        for platform, arch in (
            ("linux", "x86_64"),
            ("linux", "arm64"),
            ("darwin", "arm64"),
        ):
            archive = version_dir / f"froglet-node-{version}-{platform}-{arch}.tar.gz"
            self._write_tarball(archive, "froglet-node", binary_content)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            sums.append(f"{digest}  {archive.name}")

        bootstrap_asset = version_dir / "agent-bootstrap.sh"
        shutil.copy(AGENT_BOOTSTRAP_SCRIPT, bootstrap_asset)
        bootstrap_asset.chmod(bootstrap_asset.stat().st_mode | stat.S_IXUSR)
        sums.append(
            f"{hashlib.sha256(bootstrap_asset.read_bytes()).hexdigest()}  {bootstrap_asset.name}"
        )

        (version_dir / "SHA256SUMS").write_text("\n".join(sums) + "\n", encoding="utf-8")
        subprocess.run(
            [
                "python3",
                str(RELEASE_MANIFEST_SCRIPT),
                "generate",
                "--release",
                version,
                "--repository",
                "armanas/froglet",
                "--source-revision",
                SOURCE_REVISION,
                "--checksums",
                str(version_dir / "SHA256SUMS"),
                "--provider-image",
                IMAGE_REFS["provider"],
                "--runtime-image",
                IMAGE_REFS["runtime"],
                "--dual-image",
                IMAGE_REFS["dual"],
                "--mcp-image",
                IMAGE_REFS["mcp"],
                "--out",
                str(version_dir / "release-manifest.json"),
            ],
            cwd=REPO_ROOT,
            check=True,
        )
        (version_dir / "release-manifest.intoto.jsonl").write_text(
            '{"fixture":"offline-attestation"}\n', encoding="utf-8"
        )
        (version_dir / "feed.json").write_text(
            json.dumps(
                {
                    "artifacts": [
                        {
                            "document": {
                                "payload": {"offer_id": "events.query"}
                            }
                        }
                    ],
                    "active_offer_hashes": ["a" * 64],
                    "has_more": False,
                },
                separators=(",", ":"),
            )
            + "\n",
            encoding="utf-8",
        )
        manifest_sha256 = hashlib.sha256(
            (version_dir / "release-manifest.json").read_bytes()
        ).hexdigest()
        release_assets = [
            {
                "name": "release-manifest.json",
                "digest": f"sha256:{manifest_sha256}",
                "state": "uploaded",
            }
        ]
        for line in sums:
            digest, asset_name = line.split("  ", 1)
            release_assets.append(
                {
                    "name": asset_name,
                    "digest": f"sha256:{digest}",
                    "state": "uploaded",
                }
            )
        (version_dir / "release-api.json").write_text(
            json.dumps(
                {
                    "tag_name": version,
                    "immutable": True,
                    "assets": release_assets,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        return version_dir

    def _write_tarball(
        self, archive_path: Path, binary_name: str, content: str | None = None
    ):
        source = self.root / binary_name
        source.write_text(
            content
            or textwrap.dedent(
                f"""\
                #!/bin/sh
                echo "{binary_name} stub"
                """
            ),
            encoding="utf-8",
        )
        source.chmod(source.stat().st_mode | stat.S_IXUSR)

        license_file = self.root / "LICENSE"
        if not license_file.exists():
            shutil.copy(REPO_ROOT / "LICENSE", license_file)

        with tarfile.open(archive_path, "w:gz") as tar:
            tar.add(source, arcname=binary_name)
            tar.add(license_file, arcname="LICENSE")

    def _write_stub(self, name: str, content: str):
        path = self.stub_dir / name
        path.write_text(content, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)


if __name__ == "__main__":
    unittest.main(verbosity=2)
