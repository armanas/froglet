"""Exercise the Fly launch preparation CLI without network or cloud access."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10
    import tomli as tomllib


REPO = Path(__file__).resolve().parents[2]
SCRIPT = REPO / "scripts" / "fly_provider_smoke.sh"
IMAGE = "ghcr.io/example/froglet-provider@sha256:" + "a" * 64
VERSION = "0.1.0-beta.23"


class FlyProviderSmokeTests(unittest.TestCase):
    """Fake only external commands; execute argument parsing and checks as shipped."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="froglet-fly-smoke-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.stubs = self.root / "bin"
        self.stubs.mkdir()
        # Keep all real cloud/network binaries out of PATH, including fallback
        # tests where the preferred fake `fly` is deliberately absent.
        for name in ("date", "mkdir", "mktemp", "chmod", "cat", "tee", "sleep", "sed",
                     "dirname", "basename", "rm", "cp", "mv", "head", "tr", "grep", "jq"):
            executable = shutil.which(name)
            if executable:
                (self.stubs / name).symlink_to(executable)
        (self.stubs / "python3").symlink_to(sys.executable)
        self.log = self.root / "commands.jsonl"
        self.app_file = self.root / "created-app.txt"
        self.counter = 0
        self._write_stub("flyctl", """
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
with open(os.environ["FAKE_COMMAND_LOG"], "a", encoding="utf-8") as output:
    output.write(json.dumps({"command": "fly", "args": args}) + "\\n")
if args[:2] == ["auth", "whoami"] and os.environ.get("FAKE_MODE") == "auth-failed":
    sys.exit(1)
if args[:2] == ["apps", "destroy"] and os.environ.get("FAKE_MODE") == "cleanup-failed":
    sys.exit(1)
if args[:2] == ["apps", "create"]:
    if os.environ.get("FAKE_MODE") == "create-failed":
        sys.exit(1)
    Path(os.environ["FAKE_APP_FILE"]).write_text(args[2], encoding="utf-8")
if args and args[0] == "deploy":
    if os.environ.get("FAKE_MODE") == "deploy-failed":
        sys.exit(1)
    print("Fake deployment completed; no cloud request was made")
elif args and args[0] in ("status", "machines"):
    print(json.dumps({"status": "running", "machines": []}))
else:
    print("fake fly command completed")
""")
        # Exercise the CLI's preferred binary too; neither can reach Fly.
        (self.stubs / "fly").write_bytes((self.stubs / "flyctl").read_bytes())
        (self.stubs / "fly").chmod(0o755)
        self._write_stub("curl", """
import json, os, sys
from pathlib import Path
from urllib.parse import urlsplit
args = sys.argv[1:]
with open(os.environ["FAKE_COMMAND_LOG"], "a", encoding="utf-8") as output:
    output.write(json.dumps({"command": "curl", "args": args}) + "\\n")
url = next((value for value in args if value.startswith("https://")), "")
path = urlsplit(url).path
method = "GET"
for index, argument in enumerate(args[:-1]):
    if argument in ("-X", "--request"):
        method = args[index + 1]
mode = os.environ.get("FAKE_MODE", "")
status = 200
body = {}
app = Path(os.environ["FAKE_APP_FILE"]).read_text(encoding="utf-8")
if path == "/health":
    body = {"status": "ok", "service": "froglet"}
    if mode == "bad-health":
        body["service"] = "something-else"
elif path == "/v1/node/capabilities":
    body = {"version": os.environ["FAKE_EXPECTED_VERSION"],
            "identity": {"node_id": "b" * 64},
            "transports": {"clearnet": {"enabled": True,
                          "url": "https://" + app + ".fly.dev"}}}
    if mode == "bad-version":
        body["version"] = "stale-version"
    elif mode == "bad-identity":
        body["identity"]["node_id"] = "not-a-node-id"
    elif mode == "bad-public-url":
        body["transports"]["clearnet"]["url"] = "https://unrelated.fly.dev"
    elif mode == "clearnet-disabled":
        body["transports"]["clearnet"]["enabled"] = False
elif path in ("/v1/provider/usage", "/v1/provider/control"):
    opened = mode == "owner-open" or (mode == "owner-usage-open" and path.endswith("/usage")) \
        or (mode == "owner-control-open" and path.endswith("/control"))
    status = 200 if opened else 401
    body = {"error": "unauthorized"} if status == 401 else {"ok": True}
elif path in ("/v1/provider/quotes", "/v1/provider/deals"):
    opened = mode == "admission-open" or (mode == "quote-open" and path.endswith("/quotes")) \
        or (mode == "deal-open" and path.endswith("/deals"))
    status = 200 if opened else 403
    body = {"code": "provider_access_required"}
    if mode == "wrong-refusal":
        body = {"code": "unrelated_error"}
else:
    # Unknown endpoints fail closed instead of quietly satisfying a new check.
    status = 404
    body = {"error": "unexpected fixture URL", "url": url}
expected_methods = {"/health": "GET", "/v1/node/capabilities": "GET",
                    "/v1/provider/usage": "GET", "/v1/provider/control": "POST",
                    "/v1/provider/quotes": "POST", "/v1/provider/deals": "POST"}
if expected_methods.get(path) != method:
    status = 405
    body = {"error": "unexpected fixture method", "method": method}
output_path = None
write_format = None
for index, argument in enumerate(args[:-1]):
    if argument in ("-o", "--output"):
        output_path = args[index + 1]
    elif argument in ("-w", "--write-out"):
        write_format = args[index + 1]
serialized = json.dumps(body)
if output_path:
    Path(output_path).write_text(serialized, encoding="utf-8")
else:
    sys.stdout.write(serialized)
if write_format:
    sys.stdout.write(write_format.replace("%{http_code}", str(status)))
if status >= 400 and any(value in ("-f", "--fail") or
                         (value.startswith("-") and not value.startswith("--")
                          and "f" in value) for value in args):
    sys.exit(22)
""")

    def _write_stub(self, name, code):
        path = self.stubs / name
        path.write_text("#!" + sys.executable + "\n" + textwrap.dedent(code),
                        encoding="utf-8")
        path.chmod(0o755)

    def run_cli(self, *arguments, mode="", required=True, organization=True):
        self.counter += 1
        evidence = self.root / ("evidence-" + str(self.counter))
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("FROGLET_", "FLY_"))}
        env.update({"PATH": str(self.stubs),
                    "FAKE_COMMAND_LOG": str(self.log),
                    "FAKE_APP_FILE": str(self.app_file),
                    "FAKE_EXPECTED_VERSION": VERSION, "FAKE_MODE": mode})
        argv = ["/bin/bash", str(SCRIPT), "--evidence-dir", str(evidence)]
        if required:
            argv += ["--image", IMAGE, "--expected-version", VERSION]
        if organization and "--prepare-only" not in arguments:
            argv += ["--org", "fixture-org"]
        result = subprocess.run(argv + list(arguments), cwd=self.root, env=env,
                                text=True, capture_output=True, timeout=12)
        return result, evidence

    def commands(self, command=None):
        rows = [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines()] \
            if self.log.exists() else []
        return [row for row in rows if command is None or row["command"] == command]

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_prepare_only_renders_default_closed_configuration_without_external_commands(self):
        result, evidence = self.run_cli("--prepare-only")
        self.assert_success(result)
        self.assertEqual(self.commands(), [])
        settings = tomllib.loads((evidence / "fly.toml").read_text(encoding="utf-8"))
        self.assertEqual(settings["build"]["image"], IMAGE)
        self.assertEqual(settings["primary_region"], "fra")
        env = settings["env"]
        self.assertEqual(env["FROGLET_PUBLIC_BASE_URL"],
                         "https://" + settings["app"] + ".fly.dev")
        expected = {
            "FROGLET_NODE_ROLE": "provider", "FROGLET_PAYMENT_BACKEND": "none",
            "FROGLET_PROVIDER_ACCESS_MODE": "private",
            "FROGLET_REQUESTER_SPEND_BUDGET_MSAT": "0",
            "FROGLET_RUNTIME_LISTEN_ADDR": "127.0.0.1:0",
            "FROGLET_RUNTIME_ALLOW_NON_LOOPBACK": "false",
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "0",
            "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "0",
            "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "0",
        }
        for key, value in expected.items():
            with self.subTest(key=key):
                self.assertEqual(env[key], value)
        self.assertNotIn("FROGLET_MARKETPLACE_URL", env)
        self.assertEqual(settings["mounts"], [{"source": "froglet_data", "destination": "/data"}])
        self.assertTrue(settings["http_service"]["force_https"])
        report = json.loads((evidence / "qualification.json").read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "prepared")
        self.assertEqual(report["checks"], [])
        self.assertEqual(report["config_sha256"],
                         hashlib.sha256((evidence / "fly.toml").read_bytes()).hexdigest())

    def test_explicit_finite_allowances_and_region_are_retained(self):
        result, evidence = self.run_cli("--prepare-only", "--region", "ams",
                                       "--max-deals", "4", "--max-quotes", "8",
                                       "--max-runtime-ms", "20000", "--app-prefix", "test-froglet")
        self.assert_success(result)
        settings = tomllib.loads((evidence / "fly.toml").read_text(encoding="utf-8"))
        self.assertEqual(settings["primary_region"], "ams")
        self.assertTrue(settings["app"].startswith("test-froglet-"))
        for key, value in (("DEALS", "4"), ("QUOTES", "8"), ("RUNTIME_MS", "20000")):
            self.assertEqual(settings["env"]["FROGLET_PROVIDER_MAX_TOTAL_" + key], value)
        self.assertEqual(self.commands(), [])

    def test_invite_mode_with_finite_allowances_keeps_anonymous_admission_closed(self):
        arguments = ("--access-mode", "invite", "--max-deals", "4", "--max-quotes", "8",
                     "--max-runtime-ms", "20000")
        result, evidence = self.run_cli(*arguments)
        self.assert_success(result)
        settings = tomllib.loads((evidence / "fly.toml").read_text(encoding="utf-8"))
        self.assertEqual(settings["env"]["FROGLET_PROVIDER_ACCESS_MODE"], "invite")
        for key, value in (("DEALS", "4"), ("QUOTES", "8"), ("RUNTIME_MS", "20000")):
            self.assertEqual(settings["env"]["FROGLET_PROVIDER_MAX_TOTAL_" + key], value)
        report = json.loads((evidence / "qualification.json").read_text(encoding="utf-8"))
        checks = {row["check"]: row for row in report["checks"]}
        for name in ("anonymous-quote", "anonymous-deal"):
            self.assertEqual(checks[name]["status"], "pass")
            self.assertIn("HTTP 403", checks[name]["detail"])
        urls = [value for row in self.commands("curl") for value in row["args"]
                if value.startswith("https://")]
        self.assertFalse(any("invite" in value for value in urls))
        self.assertIn("remote computation and signed execution receipts", report["not_verified"])
        # Each anonymous endpoint must independently fail the qualification if
        # it admits requests, even when invite mode has nonzero allowances.
        for mode in ("quote-open", "deal-open"):
            with self.subTest(mode=mode):
                result, _ = self.run_cli(*arguments, mode=mode)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_required_release_coordinates_and_invalid_inputs_never_call_external_commands(self):
        cases = [
            (False, ("--prepare-only",)),
            (False, ("--image", IMAGE, "--prepare-only")),
            (False, ("--expected-version", VERSION, "--prepare-only")),
            (True, ("--image", "ghcr.io/example/froglet-provider:latest")),
            (True, ("--image", "ghcr.io/example/froglet-provider@sha256:" + "A" * 64)),
            (True, ("--image", "docker.io/example/froglet-provider@sha256:" + "a" * 64)),
            (True, ("--image", "ghcr.io/example/froglet-mcp@sha256:" + "a" * 64)),
            (True, ("--expected-version", "")),
            (True, ("--region", "AMS")),
            (True, ("--access-mode", "open")),
            (True, ("--access-mode", "trial")),
            (True, ("--access-mode", "paid")),
            (True, ("--access-mode", "")),
            (True, ("--org", "BadOrg")),
            (True, ("--org", "-leading-dash")),
            (True, ("--org", "trailing-dash-")),
            (True, ("--org", "a" * 65)),
            (True, ("--org", "org;true")),
            (True, ("--app-prefix", "too-long-froglet-prefix")),
            (True, ("--app-prefix", "froglet;true")),
            (True, ("--app-prefix", "BadPrefix")),
            (True, ("--max-deals", "-1")),
            (True, ("--max-deals", "1001")),
            (True, ("--max-quotes", "1001")),
            (True, ("--max-quotes", "unlimited")),
            (True, ("--max-runtime-ms", "5000001")),
            (True, ("--max-runtime-ms", "18446744073709551616")),
            (True, ("--image",)),
            (True, ("--unknown-option",)),
            (True, ("--marketplace-url", "https://example.invalid")),
        ]
        for required, arguments in cases:
            with self.subTest(arguments=arguments):
                result, _ = self.run_cli(*arguments, required=required)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(self.commands(), [])

    def test_success_uses_pinned_deployment_and_created_volume_then_cleans_up(self):
        result, evidence = self.run_cli()
        self.assert_success(result)
        fly = [entry["args"] for entry in self.commands("fly")]
        create = next(args for args in fly if args[:2] == ["apps", "create"])
        created = create[2]
        self.assertEqual(create[create.index("--org") + 1], "fixture-org")
        self.assertIn("--yes", create)
        self.assertNotIn("--machines", create)
        volume = next(args for args in fly if args[:2] == ["volumes", "create"])
        self.assertEqual(volume[volume.index("--size") + 1], "3")
        self.assertEqual(volume[volume.index("--app") + 1], created)
        deploy = next(args for args in fly if args[0] == "deploy")
        self.assertIn("--ha=false", deploy)
        self.assertEqual(deploy[deploy.index("--image") + 1], IMAGE)
        self.assertEqual(Path(deploy[deploy.index("--config") + 1]), evidence / "fly.toml")
        self.assertEqual(deploy[deploy.index("--app") + 1], created)
        self.assertEqual([args for args in fly if args[:2] == ["apps", "destroy"]],
                         [["apps", "destroy", created, "--yes"]])
        self.assertTrue((evidence / "fly.toml").is_file())
        report = json.loads((evidence / "qualification.json").read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "passed")
        self.assertEqual(report["scope"], "public reachability and anonymous refusal only")
        self.assertIn("remote computation and signed execution receipts", report["not_verified"])
        self.assertIn("production readiness", report["not_verified"])
        curls = self.commands("curl")
        for row in curls:
            self.assertEqual(row["args"][0], "--disable")
            self.assertTrue(set(row["args"]).isdisjoint(
                {"-L", "--location", "--location-trusted", "-k", "--insecure"}))
        urls = [value for row in curls for value in row["args"] if value.startswith("https://")]
        self.assertTrue(urls)
        self.assertTrue(all(value.startswith("https://" + created + ".fly.dev/") for value in urls))
        self.assertFalse(any("registrations" in value or "marketplace" in value for value in urls))
        self.assertTrue(any(value.endswith("/health") for value in urls))
        self.assertTrue(any(value.endswith("/v1/node/capabilities") for value in urls))
        self.assertTrue(any(value.endswith("/v1/provider/usage") for value in urls))
        self.assertTrue(any(value.endswith("/v1/provider/control") for value in urls))
        self.assertTrue(any(value.endswith("/v1/provider/quotes") for value in urls))
        self.assertTrue(any(value.endswith("/v1/provider/deals") for value in urls))

    def test_existing_evidence_is_preserved_without_any_external_command(self):
        evidence = self.root / "existing-evidence"
        evidence.mkdir()
        sentinel = evidence / "qualification.json"
        sentinel.write_text("prior evidence must remain unchanged", encoding="utf-8")
        result, _ = self.run_cli("--evidence-dir", str(evidence))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sentinel.read_text(encoding="utf-8"), "prior evidence must remain unchanged")
        self.assertEqual(self.commands(), [])

    def test_keep_preserves_created_app(self):
        result, _ = self.run_cli("--keep")
        self.assert_success(result)
        self.assertTrue(self.app_file.exists())
        self.assertFalse(any(row["args"][:2] == ["apps", "destroy"]
                             for row in self.commands("fly")))

    def test_help_needs_no_release_coordinates_or_cloud_commands(self):
        result, _ = self.run_cli("--help", required=False)
        self.assert_success(result)
        self.assertEqual(self.commands(), [])

    def test_flyctl_fallback_has_the_same_success_and_cleanup_behavior(self):
        (self.stubs / "fly").unlink()
        result, _ = self.run_cli()
        self.assert_success(result)
        self.assertTrue(any(row["args"][:2] == ["apps", "destroy"]
                            for row in self.commands("fly")))

    def test_failed_authentication_never_creates_or_destroys_an_app(self):
        result, _ = self.run_cli(mode="auth-failed")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(row["args"][:1] == ["apps"]
                             for row in self.commands("fly")))
        self.assertEqual(self.commands("curl"), [])

    def test_deployment_requires_explicit_organization_before_any_external_command(self):
        result, _ = self.run_cli(organization=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.commands(), [])

    def test_failed_deployment_cleans_up_only_the_created_app(self):
        result, _ = self.run_cli(mode="deploy-failed")
        self.assertNotEqual(result.returncode, 0)
        created = self.app_file.read_text(encoding="utf-8")
        self.assertEqual([row["args"] for row in self.commands("fly")
                          if row["args"][:2] == ["apps", "destroy"]],
                         [["apps", "destroy", created, "--yes"]])
        self.assertEqual(self.commands("curl"), [])

    def test_failed_cleanup_is_recorded_as_failure_instead_of_success(self):
        result, evidence = self.run_cli(mode="cleanup-failed")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("billable", result.stderr)
        report = json.loads((evidence / "qualification.json").read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "failed")
        self.assertTrue(any(row["check"] == "cleanup" and row["status"] == "fail"
                            for row in report["checks"]))

    def test_keep_retains_created_app_after_failed_probe_but_does_not_claim_pass(self):
        result, evidence = self.run_cli("--keep", mode="bad-version")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(any(row["args"][:2] == ["apps", "destroy"]
                             for row in self.commands("fly")))
        report = json.loads((evidence / "qualification.json").read_text(encoding="utf-8"))
        self.assertEqual(report["status"], "failed")
        self.assertTrue(report["keep_requested"])

    def test_failed_creation_never_destroys_an_app_it_did_not_create(self):
        for arguments in ((), ("--keep",)):
            with self.subTest(arguments=arguments):
                result, _ = self.run_cli(*arguments, mode="create-failed")
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(any(row["args"][:2] == ["apps", "destroy"]
                                     for row in self.commands("fly")))
                self.assertEqual(self.commands("curl"), [])

    def test_health_identity_version_and_public_url_mismatches_fail_closed(self):
        for mode in ("bad-health", "bad-version", "bad-identity", "bad-public-url",
                     "clearnet-disabled"):
            with self.subTest(mode=mode):
                result, _ = self.run_cli(mode=mode)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                created = self.app_file.read_text(encoding="utf-8")
                self.assertTrue(any(row["args"][:3] == ["apps", "destroy", created]
                                    for row in self.commands("fly")))

    def test_owner_and_admission_endpoints_must_refuse_unauthenticated_access(self):
        for mode in ("owner-usage-open", "owner-control-open", "quote-open", "deal-open",
                     "wrong-refusal"):
            with self.subTest(mode=mode):
                result, _ = self.run_cli(mode=mode)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
