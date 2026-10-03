"""Run the release workflow's real version check against isolated metadata."""

import copy
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO_ROOT = Path(__file__).resolve().parents[2]


class ReleaseMetadataTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text()
        match = re.search(
            r"- name: Verify MCP versions are consistent\n.*?"
            r"python3 - <<'PY'\n(.*?)\n          PY",
            workflow,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(match, "release workflow MCP check is missing")
        self.script = textwrap.dedent(match.group(1))
        self.output = self.root / "workflow-output"
        version = "0.4.7-beta.1"
        self.documents = {
            "package.json": {"version": version},
            "package-lock.json": {"version": version, "packages": {"": {"version": version}}},
            "integrations/mcp/froglet/package.json": {"version": version},
            "integrations/mcp/froglet/package-lock.json": {"version": version, "packages": {"": {"version": version}}},
            "integrations/mcp/froglet/mcp-manifest.json": {"version": version},
            "server.json": {"version": version, "packages": [{"version": version}]},
        }

    def tearDown(self):
        self.temporary.cleanup()

    def _write(self, documents):
        for name, document in documents.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(document), encoding="utf-8")

    def _run(self, root=None):
        return subprocess.run(
            [sys.executable, "-c", self.script],
            cwd=root or self.root,
            env={**os.environ, "GITHUB_OUTPUT": str(self.output)},
            capture_output=True,
            text=True,
            timeout=10,
        )

    def test_current_checkout_metadata_matches(self):
        result = self._run(REPO_ROOT)
        self.assertEqual(result.returncode, 0, result.stderr)
        version = json.loads((REPO_ROOT / "package.json").read_text())["version"]
        self.assertEqual(self.output.read_text(), f"mcp_version={version}\n")

    def test_matching_prerelease_metadata_produces_version(self):
        self._write(self.documents)
        result = self._run()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), "mcp_version=0.4.7-beta.1\n")

    def test_each_mismatched_distribution_version_blocks_publication(self):
        cases = (
            ("package.json", ("version",)),
            ("package-lock.json", ("version",)),
            ("package-lock.json", ("packages", "", "version")),
            ("integrations/mcp/froglet/package-lock.json", ("version",)),
            ("integrations/mcp/froglet/package-lock.json", ("packages", "", "version")),
            ("integrations/mcp/froglet/mcp-manifest.json", ("version",)),
            ("server.json", ("version",)),
            ("server.json", ("packages", 0, "version")),
        )
        for name, keys in cases:
            with self.subTest(name=name, keys=keys):
                documents = copy.deepcopy(self.documents)
                value = documents[name]
                for key in keys[:-1]:
                    value = value[key]
                value[keys[-1]] = "0.2.1"
                self._write(documents)
                result = self._run()
                self.assertNotEqual(result.returncode, 0, result.stdout)
                self.assertIn("does not match", result.stderr)
                self.assertFalse(self.output.exists())

    def test_missing_root_lock_blocks_publication(self):
        self._write(self.documents)
        (self.root / "package-lock.json").unlink()
        result = self._run()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("package-lock.json", result.stderr)
        self.assertFalse(self.output.exists())

    def test_malformed_registry_metadata_blocks_publication(self):
        self._write(self.documents)
        (self.root / "server.json").write_text("not-json", encoding="utf-8")
        result = self._run()
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn("JSONDecodeError", result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
