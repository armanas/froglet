"""Exercise release gates in an isolated repository, without builds or network."""

import hashlib
import io
import os
import json
import re
import shutil
import struct
import sys
import subprocess
import tarfile
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


class ReleaseGateTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repository"
        self.scripts = self.repo / "scripts"
        self.scripts.mkdir(parents=True)
        for name in ("release_gate.sh", "package_release_assets.sh", "verify_release_assets.sh"):
            shutil.copy2(REPO_ROOT / "scripts" / name, self.scripts / name)
        self._script(self.scripts / "gitleaks_gate.sh", 'exit "${TEST_SECRETS_RC:-0}"\n')
        self._script(self.scripts / "strict_checks.sh", '[[ "${FROGLET_REQUIRE_SOFTWARE_CHECKS:-0}" == 1 ]] || exit 98\nprintf "%s" "${FROGLET_QUALIFICATION_NODE_BIN:-}" > "$TEST_QUALIFICATION_LOG"\nexit "${TEST_STRICT_RC:-0}"\n')
        self._script(self.scripts / "agent-bootstrap.sh", "exit 0\n")
        self._script(self.scripts / "smoke_install_from_assets.sh", 'printf "%s\\n" "$@" > "$TEST_INSTALL_LOG"\n')
        self.repo.joinpath("LICENSE").write_text("fixture license\n", encoding="utf-8")
        self.repo.joinpath("Cargo.toml").write_text('[package]\nname = "froglet"\nversion = "0.1.0"\n', encoding="utf-8")
        binary = self.repo / "target" / "release" / "froglet-node"
        binary.parent.mkdir(parents=True)
        self._binary(binary, suffix=b"old unqualified binary")
        self.fresh_binary = self.root / "fresh-binary"
        self._binary(self.fresh_binary, suffix=b"fresh source build")
        self.commands = self.root / "commands"
        self.commands.mkdir()
        # Keep PATH independent of the developer's npm installation.
        # GNU tar runs gzip via PATH; BSD tar's built-in compression can hide
        # an omitted compressor in this otherwise isolated tool fixture.
        for name in ("bash", "sh", "env", "dirname", "date", "mkdir", "cp", "chmod", "tar", "gzip", "mktemp", "rm", "touch", "grep", "sha256sum", "shasum", "python3"):
            executable = shutil.which(name)
            if executable:
                self.commands.joinpath(name).symlink_to(executable)
        self._script(self.commands / "npm", 'printf "%s\\n" "${FROGLET_NODE_BIN:-unset}" >> "$TEST_DOCS_NODE_LOG"\nexit "${TEST_NPM_RC:-0}"\n')
        self._script(self.commands / "uname", 'case "$1" in -s) printf "Linux\\n";; -m) printf "x86_64\\n";; *) exit 99;; esac\n')
        self._script(self.commands / "rustc", 'printf "host: %s\\n" "${TEST_RUST_HOST:-x86_64-unknown-linux-gnu}"\n')
        self._script(self.commands / "cargo", '''printf '%s\\n' "$@" > "$TEST_BUILD_LOG"
exit_code="${TEST_BUILD_RC:-0}"
[[ "$exit_code" == 0 ]] || exit "$exit_code"
target=""
target_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --target) target="$2"; shift 2;;
    --target-dir) target_dir="$2"; shift 2;;
    *) shift;;
  esac
done
[[ -n "$target" && -n "$target_dir" ]] || exit 99
mkdir -p "$target_dir/$target/release"
cp "$TEST_FRESH_BINARY" "$target_dir/$target/release/froglet-node"
''')
        self.evidence = self.root / "evidence"

    def tearDown(self):
        self.temporary.cleanup()

    def _script(self, path, body):
        if path.is_symlink():
            path.unlink()
        path.write_text("#!/bin/bash\nset -eu\n" + body, encoding="utf-8")
        path.chmod(0o755)

    def _binary(self, path, *, platform="linux", arch="x86_64", suffix=b""):
        header = bytearray(64)
        if platform == "linux":
            header[:7] = b"\x7fELF\x02\x01\x01"
            struct.pack_into("<HH", header, 16, 3, {"x86_64": 62, "arm64": 183}[arch])
        else:
            header[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<III", header, 4, {"x86_64": 0x01000007, "arm64": 0x0100000c}[arch], 0, 2)
        path.write_bytes(header + suffix)
        path.chmod(0o755)

    def _run(self, arguments=(), *, script="release_gate.sh", extra_env=None):
        environment = dict(os.environ, PATH=str(self.commands), TEST_BUILD_LOG=str(self.root / "cargo-build.log"), TEST_FRESH_BINARY=str(self.fresh_binary), TEST_INSTALL_LOG=str(self.root / "installer.log"), TEST_QUALIFICATION_LOG=str(self.root / "qualification-node.log"), TEST_DOCS_NODE_LOG=str(self.root / "docs-node.log"), TEST_REAL_TAR=shutil.which("tar"), TEST_PYTHON3=sys.executable)
        environment.pop("CARGO_TARGET_DIR", None)
        environment.update(extra_env or {})
        if script == "release_gate.sh":
            arguments = ("--evidence-dir", str(self.evidence), *arguments)
        return subprocess.run(
            [str(self.scripts / script), *arguments], cwd=self.repo, env=environment,
            capture_output=True, text=True, timeout=20,
        )

    def test_default_gate_passes_with_required_checks_and_labels_its_scope(self):
        result = self._run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Result: PASS", result.stdout)
        self.assertIn("software checks only", result.stdout)

    def test_required_explicit_skip_is_incomplete(self):
        result = self._run(("--skip", "strict"))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("Result: INCOMPLETE", result.stdout)
        self.assertIn("strict\tSKIP", self.evidence.joinpath("summary.tsv").read_text())

    def test_missing_npm_is_incomplete(self):
        self.commands.joinpath("npm").unlink()
        result = self._run()
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("npm not installed", result.stdout)

    def test_failure_takes_precedence_over_incomplete(self):
        result = self._run(("--skip", "strict"), extra_env={"TEST_SECRETS_RC": "7"})
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("Result: FAIL", result.stderr)

    def test_requested_package_skip_is_incomplete(self):
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64", "--skip", "package"))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_selected_package_is_qualified_before_strict_and_docs(self):
        result = self._run(("--install-smoke", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"), extra_env={"FROGLET_QUALIFICATION_NODE_BIN": "/old/candidate", "FROGLET_NODE_BIN": "/old/docs-node", "FROGLET_PLAYGROUND_REAL_PROVIDER": "1", "FROGLET_PLAYGROUND_REAL_REQUESTER": "1"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = self.evidence.joinpath("summary.tsv").read_text().splitlines()[1:]
        self.assertEqual([row.split("\t")[0] for row in rows], ["secrets", "package", "strict", "docs-build", "docs-test", "install-smoke"])
        candidate = str(self.repo / "target" / "x86_64-unknown-linux-gnu" / "release" / "froglet-node")
        self.assertEqual(self.root.joinpath("qualification-node.log").read_text(), candidate)
        self.assertEqual(self.root.joinpath("docs-node.log").read_text().splitlines(), [candidate, candidate])

    def test_failed_or_skipped_package_cannot_supply_old_qualification_paths(self):
        for condition in ("fail", "skip"):
            with self.subTest(condition=condition):
                arguments = ("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64", *(("--skip", "package") if condition == "skip" else ()))
                result = self._run(arguments, extra_env={"TEST_BUILD_RC": "17" if condition == "fail" else "0", "FROGLET_QUALIFICATION_NODE_BIN": "/old/candidate", "FROGLET_NODE_BIN": "/old/docs-node"})
                self.assertEqual(result.returncode, 1 if condition == "fail" else 2)
                self.assertEqual(self.root.joinpath("qualification-node.log").read_text(), "")
                self.assertNotIn("/old/docs-node", self.root.joinpath("docs-node.log").read_text())

    def test_default_gate_does_not_accept_caller_qualification_candidate(self):
        result = self._run(extra_env={"FROGLET_QUALIFICATION_NODE_BIN": "/old/candidate"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.root.joinpath("qualification-node.log").read_text(), "")

    def test_unknown_skip_is_rejected_before_checks(self):
        result = self._run(("--skip", "strcit"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown step", result.stderr)
        self.assertFalse(self.evidence.exists())

    def test_missing_argument_values_are_reported(self):
        for option in ("--version", "--platform", "--arch", "--evidence-dir", "--skip"):
            with self.subTest(option=option):
                result = self._run((option,))
                self.assertEqual(result.returncode, 1)
                self.assertIn("requires a value", result.stderr)
                self.assertNotIn("unbound variable", result.stderr)

    def test_controlled_package_helpers_reject_missing_values(self):
        for script, option in (("package_release_assets.sh", "--out-dir"), ("verify_release_assets.sh", "--target")):
            with self.subTest(script=script):
                result = self._run((option,), script=script)
                self.assertEqual(result.returncode, 1)
                self.assertIn("requires a value", result.stderr)

    def _strict_preflight(self, **overrides):
        shutil.copy2(REPO_ROOT / "scripts" / "strict_checks.sh", self.scripts / "strict-real.sh")
        self._script(self.commands / "cargo", 'if [[ "$1" == clippy && "${2:-}" == --version ]]; then exit "${TEST_CLIPPY_RC:-0}"; fi\nprintf "%s\\n" "$*" >> "$TEST_BUILD_LOG"\nexit 97\n')
        self._script(self.commands / "rustup", 'if [[ "${TEST_WASM_TARGET:-1}" == 1 ]]; then printf "wasm32-unknown-unknown\\n"; fi\n')
        self._script(self.commands / "node", 'printf "%s\\n" "${TEST_NODE_MAJOR:-22}"\n')
        self._script(self.commands / "python3", "exit 0\n")
        self._script(self.commands / "openssl", "exit 0\n")
        build_log = self.root / "builds.log"
        environment = {"FROGLET_REQUIRE_SOFTWARE_CHECKS": "1", "TEST_BUILD_LOG": str(build_log), **overrides}
        return environment, build_log

    def test_real_strict_preflight_rejects_missing_tools_before_builds(self):
        for missing in ("cargo", "rustc", "rustup", "node", "npm", "python3", "openssl"):
            with self.subTest(missing=missing):
                environment, build_log = self._strict_preflight()
                missing_path = self.commands / missing
                missing_path.rename(self.commands / "missing-tool")
                try:
                    result = self._run(script="strict-real.sh", extra_env=environment)
                finally:
                    self.commands.joinpath("missing-tool").rename(missing_path)
                self.assertEqual(result.returncode, 1)
                self.assertIn(f"required release tool is missing: {missing}", result.stderr)
                self.assertFalse(build_log.exists(), "preflight must stop before builds")

    def test_real_strict_preflight_rejects_missing_clippy_before_builds(self):
        environment, build_log = self._strict_preflight(TEST_CLIPPY_RC="1")
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 1)
        self.assertIn("require cargo-clippy", result.stderr)
        self.assertFalse(build_log.exists())

    def test_real_strict_preflight_rejects_missing_wasm_target_before_builds(self):
        environment, build_log = self._strict_preflight(TEST_WASM_TARGET="0")
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 1)
        self.assertIn("require target wasm32-unknown-unknown", result.stderr)
        self.assertFalse(build_log.exists())

    def test_real_strict_preflight_rejects_unsupported_node_before_builds(self):
        for major in ("17", "invalid"):
            with self.subTest(major=major):
                environment, build_log = self._strict_preflight(TEST_NODE_MAJOR=major)
                result = self._run(script="strict-real.sh", extra_env=environment)
                self.assertEqual(result.returncode, 1)
                self.assertIn("require Node.js 18 or newer", result.stderr)
                self.assertFalse(build_log.exists())

    def test_real_strict_preflight_accepts_tools_and_reaches_first_check(self):
        environment, build_log = self._strict_preflight()
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 97, result.stdout + result.stderr)
        self.assertEqual(build_log.read_text(), "fmt --all --check\n")

    def test_real_strict_rejects_missing_qualification_candidate_before_builds(self):
        environment, build_log = self._strict_preflight()
        environment["FROGLET_QUALIFICATION_NODE_BIN"] = str(self.root / "missing-node")
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 1)
        self.assertIn("qualification node must be an executable file", result.stderr)
        self.assertFalse(build_log.exists())

    def _strict_demo_matrix(self, *, candidate=None, required="1", build_rc="0"):
        strict_source = (REPO_ROOT / "scripts" / "strict_checks.sh").read_text()
        shutil.copy2(REPO_ROOT / "scripts" / "strict_checks.sh", self.scripts / "strict-real.sh")
        for name in re.findall(r"^(?:sh|bash) -n (\S+)$", strict_source, re.MULTILINE):
            destination = self.repo / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            if destination != self.scripts / "strict-real.sh":
                shutil.copy2(REPO_ROOT / name, destination)
        # Avoid dependency installation; all external programs below are local
        # call recorders, so this tests the actual shell routing and arguments.
        marker = self.repo / "integrations/mcp/froglet/node_modules/@modelcontextprotocol/sdk/package.json"
        marker.parent.mkdir(parents=True)
        marker.write_text("{}")
        self.repo.joinpath("python/froglet-verify").mkdir(parents=True)
        module = self.root / "sample.wasm"
        module.write_bytes(b"\0asm\x01\0\0\0")
        self._script(self.commands / "cargo", '''"$TEST_PYTHON3" - "$TEST_CARGO_LOG" "$@" <<'PY'
import json,sys
with open(sys.argv[1],"a") as log: log.write(json.dumps(sys.argv[2:])+"\\n")
PY
if [[ "$1" != build ]]; then exit 0; fi
[[ "${TEST_BUILD_RC:-0}" == 0 ]] || exit "$TEST_BUILD_RC"
target=""
target_dir=""
while [[ $# -gt 0 ]]; do
  case "$1" in --target) target="$2"; shift 2;; --target-dir) target_dir="$2"; shift 2;; *) shift;; esac
done
mkdir -p "$target_dir/$target/release"
if [[ "$target" == wasm32-unknown-unknown ]]; then
  cp "$TEST_SAMPLE_MODULE" "$target_dir/$target/release/adder.wasm"
  cp "$TEST_SAMPLE_MODULE" "$target_dir/$target/release/ontology_check.wasm"
else
  cp "$TEST_FRESH_BINARY" "$target_dir/$target/release/froglet-node"
fi
''')
        self._script(self.commands / "rustup", 'printf "wasm32-unknown-unknown\\n"\n')
        self._script(self.commands / "node", 'case "${1:-}" in -e|-p) printf "22\\n";; esac\n')
        self._script(self.commands / "git", "exit 0\n")
        self._script(self.commands / "openssl", "exit 0\n")
        self._script(self.commands / "python3", '''"$TEST_PYTHON3" - "$TEST_PYTHON_LOG" "$@" <<'PY'
import json,sys
with open(sys.argv[1],"a") as log: log.write(json.dumps(sys.argv[2:])+"\\n")
PY
''')
        return {"FROGLET_REQUIRE_SOFTWARE_CHECKS": required, "FROGLET_SKIP_GITLEAKS": "1", "FROGLET_QUALIFICATION_NODE_BIN": str(candidate) if candidate else "", "FROGLET_RUN_COMPOSE_SMOKE": "0", "FROGLET_RUN_TOR_INTEGRATION": "0", "FROGLET_RUN_LND_REGTEST": "0", "FROGLET_RUN_LINUX_SANDBOX_TESTS": "0", "TEST_CARGO_LOG": str(self.root / "strict-cargo.log"), "TEST_PYTHON_LOG": str(self.root / "strict-python.log"), "TEST_SAMPLE_MODULE": str(module), "TEST_BUILD_RC": build_rc}

    def _recorded_calls(self, name):
        return [json.loads(line) for line in self.root.joinpath(name).read_text().splitlines()]

    def test_real_strict_uses_supplied_candidate_for_all_three_demos_without_native_rebuild(self):
        environment = self._strict_demo_matrix(candidate=self.fresh_binary)
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        builds = [call for call in self._recorded_calls("strict-cargo.log") if call[0] == "build"]
        self.assertEqual(len(builds), 1)
        self.assertIn("wasm32-unknown-unknown", builds[0])
        self.assertIn("--locked", builds[0])
        demos = [call for call in self._recorded_calls("strict-python.log") if any(name in call for name in ("examples/a2a_compute_demo.py", "examples/a2a_counterparty_demo.py"))]
        self.assertEqual(len(demos), 3)
        expected_modules = ["adder.wasm", "ontology_check.wasm", "ontology_check.wasm"]
        for call, module in zip(demos, expected_modules):
            self.assertEqual(call[call.index("--binary") + 1], str(self.fresh_binary))
            self.assertEqual(call[call.index("--module") + 1], str(self.repo / "examples/wasm-services/target/wasm32-unknown-unknown/release" / module))

    def test_real_required_strict_builds_native_optimized_candidate_when_none_supplied(self):
        environment = self._strict_demo_matrix()
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        builds = [call for call in self._recorded_calls("strict-cargo.log") if call[0] == "build"]
        self.assertEqual(len(builds), 2)
        self.assertIn("--release", builds[0])
        self.assertIn("--locked", builds[0])
        self.assertEqual(builds[0][builds[0].index("--target") + 1], "x86_64-unknown-linux-gnu")
        candidate = str(self.repo / "target/x86_64-unknown-linux-gnu/release/froglet-node")
        demos = [call for call in self._recorded_calls("strict-python.log") if "--binary" in call]
        self.assertEqual(len(demos), 3)
        self.assertTrue(all(call[call.index("--binary") + 1] == candidate for call in demos))

    def test_real_developer_strict_keeps_default_demo_routing(self):
        environment = self._strict_demo_matrix(required="0")
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(any(call[0] == "build" for call in self._recorded_calls("strict-cargo.log")))
        demos = [call for call in self._recorded_calls("strict-python.log") if any(name in call for name in ("examples/a2a_compute_demo.py", "examples/a2a_counterparty_demo.py"))]
        self.assertEqual(len(demos), 3)
        self.assertTrue(all("--binary" not in call and "--module" not in call for call in demos))

    def test_real_required_strict_build_failure_cannot_reuse_stale_candidate(self):
        old = self.repo / "target/x86_64-unknown-linux-gnu/release/froglet-node"
        old.parent.mkdir(parents=True)
        self._binary(old, suffix=b"old candidate")
        environment = self._strict_demo_matrix(build_rc="19")
        result = self._run(script="strict-real.sh", extra_env=environment)
        self.assertEqual(result.returncode, 19)
        self.assertFalse(any("--binary" in call for call in self._recorded_calls("strict-python.log")))

    def test_invalid_package_values_are_rejected_before_checks(self):
        for version, platform, arch in (("v1'bad", "linux", "x86_64"), ("v1", "invalid", "x86_64"), ("v1", "darwin", "x86_64")):
            with self.subTest(version=version, platform=platform, arch=arch):
                result = self._run(("--package-assets", "--version", version, "--platform", platform, "--arch", arch))
                self.assertEqual(result.returncode, 1)
                self.assertFalse(self.evidence.exists())

    def test_package_normalizes_version(self):
        result = self._run(("--package-assets", "--version", "0.1.0", "--platform", "linux", "--arch", "x86_64"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(self.evidence.joinpath("release-assets", "froglet-node-v0.1.0-linux-x86_64.tar.gz").is_file())

    def test_package_rebuilds_current_source_and_ignores_other_output_overrides(self):
        stale_target = self.repo / "target" / "x86_64-unknown-linux-gnu" / "release" / "froglet-node"
        stale_target.parent.mkdir(parents=True)
        self._binary(stale_target, suffix=b"stale target output")
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"), extra_env={"CARGO_BUILD_TARGET": "aarch64-apple-darwin", "CARGO_TARGET_DIR": str(self.root / "wrong-target-dir")})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        arguments = self.root.joinpath("cargo-build.log").read_text().splitlines()
        self.assertIn("--locked", arguments)
        self.assertIn("--release", arguments)
        self.assertEqual(arguments[arguments.index("--target") + 1], "x86_64-unknown-linux-gnu")
        self.assertEqual(arguments[arguments.index("--target-dir") + 1], str(self.repo / "target"))
        with tarfile.open(self.evidence / "release-assets" / "froglet-node-v0.1.0-linux-x86_64.tar.gz") as tar:
            self.assertEqual(tar.extractfile("froglet-node").read(), self.fresh_binary.read_bytes())

    def test_failed_build_cannot_package_existing_stale_binary(self):
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"), extra_env={"TEST_BUILD_RC": "17"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("package\tFAIL", self.evidence.joinpath("summary.tsv").read_text())
        self.assertFalse(self.evidence.joinpath("release-assets", "froglet-node-v0.1.0-linux-x86_64.tar.gz").exists())

    def test_installer_smoke_does_not_use_old_assets_when_current_build_fails(self):
        old_assets = self.evidence / "release-assets"
        old_assets.mkdir(parents=True)
        old_assets.joinpath("froglet-node-v0.1.0-linux-x86_64.tar.gz").write_bytes(b"old archive")
        result = self._run(("--install-smoke", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"), extra_env={"TEST_BUILD_RC": "17"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("install-smoke\tPENDING", self.evidence.joinpath("summary.tsv").read_text())
        self.assertFalse(self.root.joinpath("installer.log").exists())

    def test_installer_smoke_receives_successfully_rebuilt_assets(self):
        result = self._run(("--install-smoke", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.root.joinpath("installer.log").read_text().splitlines(), ["--assets-dir", str(self.evidence / "release-assets"), "--version", "v0.1.0"])
        with tarfile.open(self.evidence / "release-assets" / "froglet-node-v0.1.0-linux-x86_64.tar.gz") as tar:
            self.assertEqual(tar.extractfile("froglet-node").read(), self.fresh_binary.read_bytes())

    def test_wrong_cargo_version_is_rejected_before_build_or_evidence(self):
        result = self._run(("--package-assets", "--version", "v0.2.0", "--platform", "linux", "--arch", "x86_64"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("does not match Cargo package version", result.stderr)
        self.assertFalse(self.evidence.exists())
        self.assertFalse(self.root.joinpath("cargo-build.log").exists())

    def test_cross_target_local_package_is_rejected_before_build(self):
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "darwin", "--arch", "arm64"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("requires native target linux:x86_64", result.stderr)
        self.assertFalse(self.root.joinpath("cargo-build.log").exists())

    def test_wrong_rust_compiler_host_is_rejected(self):
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"), extra_env={"TEST_RUST_HOST": "aarch64-apple-darwin"})
        self.assertEqual(result.returncode, 1)
        self.assertIn("compiler host does not match", result.stderr)

    def test_prebuilt_packager_rejects_mislabeled_os_and_architecture(self):
        for platform, arch in (("darwin", "arm64"), ("linux", "arm64")):
            with self.subTest(platform=platform, arch=arch):
                self._binary(self.fresh_binary, platform=platform, arch=arch)
                result = self._run(("--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64", "--out-dir", str(self.root / "prebuilt"), "--binary", str(self.fresh_binary)), script="package_release_assets.sh")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("binary header does not match", result.stderr)
                self.assertFalse(self.root.joinpath("prebuilt", "froglet-node-v0.1.0-linux-x86_64.tar.gz").exists())

    def test_packager_disables_appledouble_creation(self):
        # Model BSD tar: metadata is emitted unless COPYFILE_DISABLE is set.
        self._script(self.commands / "tar", '''"$TEST_REAL_TAR" "$@"
if [[ "${COPYFILE_DISABLE:-0}" != 1 ]]; then
  archive=""
  while [[ $# -gt 0 ]]; do
    case "$1" in -czf) archive="$2"; break;; *) shift;; esac
  done
  "$TEST_PYTHON3" - "$archive" <<'PY'
import io, sys, tarfile
path=sys.argv[1]
with tarfile.open(path) as archive:
    payloads=[(member, archive.extractfile(member).read()) for member in archive.getmembers()]
with tarfile.open(path, "w:gz") as archive:
    for member, data in payloads:
        archive.addfile(member, io.BytesIO(data))
    metadata=tarfile.TarInfo("._froglet-node")
    metadata.size=8
    archive.addfile(metadata, io.BytesIO(b"metadata"))
PY
fi
''')
        result = self._run(("--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64", "--out-dir", str(self.root / "metadata-package")), script="package_release_assets.sh")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        with tarfile.open(self.root / "metadata-package" / "froglet-node-v0.1.0-linux-x86_64.tar.gz") as archive:
            self.assertEqual([member.name for member in archive.getmembers()], ["froglet-node", "LICENSE"])
            self.assertTrue(all(not member.pax_headers for member in archive.getmembers()))

    def test_quoted_evidence_path_is_literal_and_not_shell_code(self):
        marker = self.root / "unwanted"
        self.evidence = self.root / f"evidence'; touch '{marker}'; #"
        result = self._run(("--package-assets", "--version", "v0.1.0", "--platform", "linux", "--arch", "x86_64"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(marker.exists())
        self.assertTrue(self.evidence.joinpath("release-assets", "SHA256SUMS").is_file())

    def _assets(self, *, checksum_binary=True, extra_member=False, duplicate=False, symlink=False, appledouble=False, duplicate_member=False, xattr=False):
        assets = self.root / "assets"
        assets.mkdir()
        bootstrap = assets / "agent-bootstrap.sh"
        shutil.copy2(self.scripts / "agent-bootstrap.sh", bootstrap)
        archive = assets / "froglet-node-v0.1.0-linux-x86_64.tar.gz"
        with tarfile.open(archive, "w:gz") as tar:
            for name in ("froglet-node", "LICENSE", *(("surprise",) if extra_member else ()), *(("._froglet-node", "._LICENSE") if appledouble else ()), *(("froglet-node",) if duplicate_member else ())):
                value = b"fixture\n"
                member = tarfile.TarInfo(name)
                if xattr and name == "froglet-node":
                    member.pax_headers = {"SCHILY.xattr.com.apple.provenance": "metadata"}
                if symlink and name == "froglet-node":
                    member.type = tarfile.SYMTYPE
                    member.linkname = "outside-binary"
                    tar.addfile(member)
                    continue
                member.size = len(value)
                tar.addfile(member, io.BytesIO(value))
        members = [bootstrap, *([archive] if checksum_binary else [])]
        sums = [f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in members]
        if duplicate:
            sums.append(sums[-1])
        assets.joinpath("SHA256SUMS").write_text("".join(sums), encoding="utf-8")
        return assets

    def _verify(self, assets):
        return self._run(("--dir", str(assets), "--version", "v0.1.0", "--target", "linux:x86_64"), script="verify_release_assets.sh")

    def test_asset_verification_requires_checksums(self):
        assets = self._assets()
        assets.joinpath("SHA256SUMS").unlink()
        result = self._verify(assets)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA256SUMS", result.stderr)

    def test_asset_verification_requires_binary_checksum_coverage(self):
        result = self._verify(self._assets(checksum_binary=False))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum", result.stderr)

    def test_asset_verification_rejects_unexpected_archive_member(self):
        result = self._verify(self._assets(extra_member=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("archive", result.stderr)

    def test_asset_verification_rejects_duplicate_checksum(self):
        result = self._verify(self._assets(duplicate=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate", result.stderr)

    def test_asset_verification_rejects_symlink_binary(self):
        result = self._verify(self._assets(symlink=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("regular files", result.stderr)

    def test_asset_verification_rejects_raw_appledouble_members(self):
        # BSD listings can hide valid AppleDouble; never use their text as the
        # authoritative archive inventory. Simulate that filtered view here.
        self._script(self.commands / "tar", '''case "$1" in
  -tzf) printf '%s\\n' froglet-node LICENSE;;
  -tvzf) printf '%s\\n' '-rwxr-xr-x froglet-node' '-rw-r--r-- LICENSE';;
  *) exit 99;;
esac
''')
        result = self._verify(self._assets(appledouble=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("archive", result.stderr)

    def test_asset_verification_rejects_duplicate_raw_members(self):
        result = self._verify(self._assets(duplicate_member=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("archive", result.stderr)

    def test_asset_verification_rejects_pax_xattr_metadata(self):
        result = self._verify(self._assets(xattr=True))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("metadata", result.stderr)

    def test_asset_verification_rejects_wrong_checksum(self):
        assets = self._assets()
        archive = assets / "froglet-node-v0.1.0-linux-x86_64.tar.gz"
        archive.write_bytes(archive.read_bytes() + b"changed")
        result = self._verify(assets)
        self.assertNotEqual(result.returncode, 0)

    def test_complete_selected_asset_set_passes(self):
        result = self._verify(self._assets())
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_oci_publication_waits_for_operator_draft_gate(self):
        workflow = REPO_ROOT.joinpath(".github", "workflows", "release.yml").read_text()
        image_job = workflow.split("\n  publish-images:\n", 1)[1].split("\n  publish-checksums:\n", 1)[0]
        needs = next(line for line in image_job.splitlines() if line.strip().startswith("needs:"))
        self.assertIn("ensure-release", needs)


if __name__ == "__main__":
    unittest.main()
