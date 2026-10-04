"""Publication scan scope and fail-closed behavior in disposable Git fixtures."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest


REPO = Path(__file__).resolve().parents[2]
MARKER = "SCAN_TEST_MARKER"


@unittest.skipUnless(shutil.which("git") and shutil.which("bash"), "Git and Bash required")
class GitleaksGateTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="froglet scan gate ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.repo = self.root / "repo"
        (self.repo / "scripts").mkdir(parents=True)
        shutil.copyfile(REPO / "scripts/gitleaks_gate.sh", self.repo / "scripts/gitleaks_gate.sh")
        (self.repo / ".gitleaks.toml").write_text('title = "Isolated fixture"\n')
        (self.repo / ".gitignore").write_text("/_tmp/\n/private_work/\n/data/\n/node.db*\n")
        (self.repo / "README.md").write_text("Public seed\n")
        self.stubs = self.root / "stubs"
        self.stubs.mkdir()
        self.records = self.root / "scanner-calls.jsonl"
        self.env = dict(os.environ, PATH=str(self.stubs) + os.pathsep + os.environ.get("PATH", ""),
                        GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
                        SCAN_FIXTURE_RECORDS=str(self.records))
        self.env.pop("FROGLET_GITLEAKS_VISIBLE_REFS", None)
        self.git("init", "-q")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.git("config", "commit.gpgSign", "false")
        self.git("config", "core.hooksPath", os.devnull)
        self.commit("seed")
        self.git("branch", "origin/main")
        for tag in ("v0.1.0-alpha.0", "v0.1.0-alpha.1", "v0.1.0-alpha.2"):
            self.git("tag", tag)
        self.evidence = self.repo / "_tmp/evidence"
        scanner = self.stubs / "gitleaks"
        scanner.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''\
            import json, os
            from pathlib import Path
            import subprocess, sys

            args = sys.argv[1:]
            report = Path(args[args.index("--report-path") + 1])
            call = {"arguments": args, "report": str(report)}
            matches = []
            if args[0] == "dir":
                source = Path(args[1])
                paths = sorted(path for path in source.rglob("*") if path.is_file())
                call["source"] = str(source)
                call["files"] = [str(path.relative_to(source)) for path in paths]
                matches = [path for path in paths if b"SCAN_TEST_MARKER" in path.read_bytes()]
            else:
                refs = next(arg for arg in args if arg.startswith("--log-opts=")).split("=", 1)[1].split()
                call["refs"] = refs
                history = subprocess.check_output(["git", "log", "-p", "--format=", *refs, "--"], text=True)
                matches = [line for line in history.splitlines()
                           if line.startswith("+") and not line.startswith("+++") and "SCAN_TEST_MARKER" in line]
            with open(os.environ["SCAN_FIXTURE_RECORDS"], "a", encoding="utf-8") as output:
                output.write(json.dumps(call) + "\\n")
            # Deliberately synthetic canaries; reports never contain source values.
            findings = [{"RuleID": "fixture-marker", "File": "fixture", "Secret": "REDACTED"}
                        for _ in matches]
            if not os.environ.get("SCAN_FIXTURE_OMIT_REPORT"):
                report.write_text("invalid JSON" if os.environ.get("SCAN_FIXTURE_BAD_REPORT") else json.dumps(findings))
            sys.exit(0 if os.environ.get("SCAN_FIXTURE_ZERO_RC") or not matches else 1)
            '''))
        scanner.chmod(0o755)

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.repo, env=self.env, check=True,
                              capture_output=True, text=True)

    def commit(self, message):
        self.git("add", "--all")
        self.git("commit", "-qm", message)

    def run_gate(self, **environment):
        return subprocess.run(["bash", "scripts/gitleaks_gate.sh", "--evidence-dir", str(self.evidence)],
                              cwd=self.repo, env=dict(self.env, **environment),
                              capture_output=True, text=True)

    def calls(self):
        return [json.loads(line) for line in self.records.read_text().splitlines()]

    def test_nonignored_untracked_public_source_is_scanned_and_refused(self):
        (self.repo / "new source.py").write_text(MARKER + "\n")
        result = self.run_gate()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("new source.py", self.calls()[0]["files"])
        self.assertIn("[FAIL] current-tree", result.stderr)

    def test_ignored_private_evidence_and_runtime_state_are_excluded(self):
        for name in ("_tmp/private.txt", "private_work/private.txt", "data/private.txt", "node.db"):
            path = self.repo / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(MARKER + "\n")
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        files = self.calls()[0]["files"]
        self.assertIn("README.md", files)
        self.assertFalse(any(path.startswith(("_tmp/", "private_work/", "data/", "node.db")) for path in files))

    def test_head_is_mandatory_even_when_public_refs_are_overridden(self):
        (self.repo / "README.md").write_text("Candidate source\n")
        self.commit("candidate")
        result = self.run_gate(FROGLET_GITLEAKS_VISIBLE_REFS="origin/main HEAD")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls()[1]["refs"], ["HEAD", "origin/main"])

    def test_removed_canary_in_unpushed_candidate_history_still_refuses_publication(self):
        path = self.repo / "transient.txt"
        path.write_text(MARKER + "\n")
        self.commit("introduce synthetic canary")
        path.unlink()
        self.commit("remove synthetic canary")
        result = self.run_gate(FROGLET_GITLEAKS_VISIBLE_REFS="origin/main")
        self.assertNotEqual(result.returncode, 0)
        calls = self.calls()
        self.assertEqual(json.loads(Path(calls[0]["report"]).read_text()), [])
        self.assertGreater(len(json.loads(Path(calls[1]["report"]).read_text())), 0)
        self.assertIn("[FAIL] visible-history", result.stderr)

    def test_zero_scanner_exit_with_findings_still_refuses_publication(self):
        (self.repo / "new.py").write_text(MARKER + "\n")
        result = self.run_gate(SCAN_FIXTURE_ZERO_RC="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("finding(s), rc=0", result.stderr)

    def test_reused_evidence_root_keeps_prior_reports_without_stale_source(self):
        source = self.repo / "new.py"
        source.write_text(MARKER + "\n")
        self.assertNotEqual(self.run_gate().returncode, 0)
        first = self.calls()[0]
        previous_report = Path(first["report"]).read_bytes()
        source.unlink()
        result = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        second = self.calls()[2]
        self.assertNotEqual(first["source"], second["source"])
        self.assertNotEqual(first["report"], second["report"])
        self.assertTrue((Path(first["source"]) / "new.py").is_file())
        self.assertNotIn("new.py", second["files"])
        self.assertEqual(Path(first["report"]).read_bytes(), previous_report)

    def test_missing_or_invalid_report_fails_closed_even_with_zero_exit(self):
        for mode in ("SCAN_FIXTURE_OMIT_REPORT", "SCAN_FIXTURE_BAD_REPORT"):
            with self.subTest(mode=mode):
                result = self.run_gate(**{mode: "1", "SCAN_FIXTURE_ZERO_RC": "1"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("[FAIL] current-tree", result.stderr)


if __name__ == "__main__":
    unittest.main()
