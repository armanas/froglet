import hashlib
import os
import shutil
import stat
import subprocess
import tarfile
import tempfile
import textwrap
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
INSTALL_SCRIPT = REPO_ROOT / "scripts" / "install.sh"


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
base="${url##*/}"
src="$FROGLET_TEST_ASSET_DIR/$base"
[ -f "$src" ] || {
  echo "missing fixture asset: $src" >&2
  exit 1
}
cp "$src" "$out"
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
        asset_dir = self._create_release_assets(version, "linux", "x86_64")

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

    def test_normalizes_pinned_version_without_v_prefix(self):
        version = "v9.9.9"
        asset_dir = self._create_release_assets(version, "linux", "x86_64")

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
        asset_dir = self._create_release_assets(version, "linux", "arm64")

        result = self._run_installer(
            asset_dir,
            extra_env={"VERSION": version, "FAKE_UNAME_M": "arm64"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        log = self.curl_log.read_text(encoding="utf-8")
        self.assertIn(f"froglet-node-{version}-linux-arm64.tar.gz", log)

    def test_requests_darwin_arm64_asset(self):
        version = "v3.1.4"
        asset_dir = self._create_release_assets(version, "darwin", "arm64")

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

    def test_optionally_installs_marketplace_binary(self):
        version = "v4.0.0"
        asset_dir = self._create_release_assets(version, "linux", "x86_64")

        result = self._run_installer(
            asset_dir,
            extra_env={"VERSION": version, "INSTALL_MARKETPLACE": "1"},
        )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.install_dir / "froglet-node").exists())
        self.assertTrue((self.install_dir / "froglet-marketplace").exists())

    def test_fails_on_checksum_mismatch(self):
        version = "v5.0.0"
        asset_dir = self._create_release_assets(version, "linux", "x86_64")
        (asset_dir / "SHA256SUMS").write_text(
            "0000000000000000000000000000000000000000000000000000000000000000  froglet-node-v5.0.0-linux-x86_64.tar.gz\n",
            encoding="utf-8",
        )

        result = self._run_installer(asset_dir, extra_env={"VERSION": version})

        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.install_dir / "froglet-node").exists())

    def _run_installer(self, asset_dir: Path, extra_env=None):
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
        if extra_env:
            env.update(extra_env)
        return subprocess.run(
            ["sh", str(INSTALL_SCRIPT)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
        )

    def _create_release_assets(self, version: str, platform: str, arch: str) -> Path:
        version_dir = self.assets_root / version
        version_dir.mkdir(parents=True, exist_ok=True)

        sums = []
        for binary in ("froglet-node", "froglet-marketplace"):
            archive = version_dir / f"{binary}-{version}-{platform}-{arch}.tar.gz"
            self._write_tarball(archive, binary)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            sums.append(f"{digest}  {archive.name}")

        (version_dir / "SHA256SUMS").write_text("\n".join(sums) + "\n", encoding="utf-8")
        return version_dir

    def _write_tarball(self, archive_path: Path, binary_name: str):
        source = self.root / binary_name
        source.write_text(
            textwrap.dedent(
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
