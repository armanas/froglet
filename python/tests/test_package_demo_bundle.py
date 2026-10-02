"""Candidate packaging boundaries, byte reproducibility and extracted verification."""
import json
import os
from pathlib import Path
import shutil
import ssl
import struct
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import MagicMock, patch

from scripts import package_demo_bundle as bundle


def strict_tls_handshake(root):
    """Exercise the real TLS client/server entirely in memory, with no sockets."""
    server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    server_context.load_cert_chain(root / "server.crt", root / "server.key")
    client_context = ssl.create_default_context(cafile=str(root / "ca.crt"))
    client_context.verify_flags |= ssl.VERIFY_X509_STRICT
    server_in, server_out, client_in, client_out = (ssl.MemoryBIO() for _ in range(4))
    server = server_context.wrap_bio(server_in, server_out, server_side=True)
    client = client_context.wrap_bio(client_in, client_out, server_hostname="127.0.0.1")
    completed = set()
    for _ in range(20):
        for label, peer, outgoing, incoming in (
                ("client", client, client_out, server_in), ("server", server, server_out, client_in)):
            if label not in completed:
                try:
                    peer.do_handshake()
                    completed.add(label)
                except ssl.SSLWantReadError:
                    pass
            pending = outgoing.read()
            if pending:
                incoming.write(pending)
        if len(completed) == 2:
            return client.version()
    raise RuntimeError("in-memory fixture TLS handshake did not finish")


class LocalDemoBundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="froglet-demo-package-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.root.chmod(0o700)
        self.source = self.root / "source"
        self.source.mkdir()
        for name in bundle.SOURCES:
            destination = self.source / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(bundle.REPO / name, destination)
        self.binary = self.root / "node"
        self.binary.write_bytes(b"\xcf\xfa\xed\xfe" + struct.pack("<III", 0x0100000c, 0, 2)
                                + bytes(16))
        self.binary.chmod(0o755)
        self.module = self.root / "module.wasm"
        self.module.write_bytes(b"\0asm\x01\0\0\0")
        self.output = self.root / "output"
        self.output.mkdir(mode=0o700)
        self.arguments = {"binary": self.binary, "module": self.module,
            "binary_sha256": bundle.digest(self.binary.read_bytes()),
            "module_sha256": bundle.digest(self.module.read_bytes()),
            "candidate": "local.unit-test", "platform": "darwin", "arch": "arm64",
            "out_dir": self.output, "repo": self.source}

    def package(self, **overrides):
        return bundle.package(**dict(self.arguments, **overrides))

    def extract(self, report):
        destination = self.root / "extracted"
        destination.mkdir(mode=0o700)
        # Known helper-generated allowlisted members; do not extract arbitrary input.
        with tarfile.open(report["archive"]) as archive:
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertIn(member.name, {*bundle.MEMBERS, "MANIFEST.json"})
                path = destination / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as source:
                    path.write_bytes(source.read())
                path.chmod(member.mode)
        return destination

    def test_same_exact_inputs_make_byte_identical_archives_despite_source_metadata(self):
        first = self.package()
        second_directory = self.root / "second-output"
        second_directory.mkdir(mode=0o700)
        for path in self.source.rglob("*"):
            os.utime(path, (123456789, 123456789))
        self.binary.chmod(0o700)
        second = self.package(out_dir=second_directory)
        self.assertEqual(first["archive_sha256"], second["archive_sha256"])
        self.assertEqual(Path(first["archive"]).read_bytes(), Path(second["archive"]).read_bytes())
        with tarfile.open(first["archive"]) as archive:
            for member in archive.getmembers():
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                self.assertEqual(member.mode, 0o755 if member.name in bundle.EXECUTABLES else 0o644)

    def test_archives_do_not_collect_private_state_or_installer_fixtures(self):
        secret = b"PRIVATE-CREDENTIAL-NOT-TO-BE-PACKAGED"
        for root in (self.source, self.binary.parent):
            for name in ("auth.token", "node.db", "recipient-handoff.json", "release-manifest.json"):
                (root / name).write_bytes(secret)
            private = root / "private_work"
            private.mkdir()
            (private / "nested-secret").write_bytes(secret)
        report = self.package()
        with tarfile.open(report["archive"]) as archive:
            self.assertEqual(set(archive.getnames()), {*bundle.MEMBERS, "MANIFEST.json"})
            for member in archive:
                with archive.extractfile(member) as source:
                    self.assertNotIn(secret, source.read())

    def test_existing_candidate_archive_is_preserved(self):
        first = self.package()
        with self.assertRaises(bundle.BundleError):
            self.package()
        self.assertEqual(bundle.digest(Path(first["archive"]).read_bytes()), first["archive_sha256"])
        self.assertEqual(len(list(self.output.iterdir())), 1)

    def test_changed_binary_or_wasm_under_pinned_hash_is_refused_before_archive(self):
        for key in ("binary", "module"):
            with self.subTest(key=key):
                path = self.arguments[key]
                original = path.read_bytes()
                path.write_bytes(original + b"changed")
                with self.assertRaises(bundle.BundleError):
                    self.package()
                self.assertEqual(list(self.output.iterdir()), [])
                path.write_bytes(original)

    def test_nonexecutable_binary_and_wrong_platform_header_are_refused(self):
        self.binary.chmod(0o644)
        with self.assertRaises(bundle.BundleError):
            self.package()
        self.binary.chmod(0o755)
        for overrides in ({"arch": "x86_64"}, {"platform": "linux"}):
            with self.subTest(overrides=overrides), self.assertRaises(bundle.BundleError):
                self.package(**overrides)

    def test_regular_bounded_read_rejects_fifo_without_waiting_for_a_writer(self):
        fifo = self.root / "fifo"
        os.mkfifo(fifo)
        with self.assertRaises(bundle.BundleError):
            bundle.regular_read(fifo, 1024)

    def test_final_symlink_parent_symlink_relative_path_and_dotdot_are_refused(self):
        alias = self.root / "alias"
        alias.symlink_to(self.binary)
        parent_alias = self.root / "directory-alias"
        parent_alias.symlink_to(self.root, target_is_directory=True)
        for path in (alias, parent_alias / self.binary.name, Path("relative"),
                     self.root / "unused/../node"):
            with self.subTest(path=path), self.assertRaises(bundle.BundleError):
                bundle.regular_read(path, 1024)

    def test_size_limits_empty_files_and_bad_wasm_are_refused(self):
        for data, limit in ((b"", 10), (b"01234567890", 10)):
            path = self.root / "bounded"
            path.write_bytes(data)
            with self.assertRaises(bundle.BundleError):
                bundle.regular_read(path, limit)
        self.module.write_bytes(b"not wasm")
        with self.assertRaises(bundle.BundleError):
            self.package(module_sha256=bundle.digest(self.module.read_bytes()))
        with patch.object(bundle, "BINARY_LIMIT", 31), self.assertRaises(bundle.BundleError):
            self.package()

    def test_linux_elf_architecture_must_match(self):
        value = bytearray(64)
        value[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<HH", value, 16, 3, 62)
        self.binary.write_bytes(value)
        pin = bundle.digest(value)
        self.package(platform="linux", arch="x86_64", binary_sha256=pin)
        with self.assertRaises(bundle.BundleError):
            self.package(platform="linux", arch="arm64", binary_sha256=pin)

    def test_only_explicit_candidate_labels_and_full_lowercase_pins_are_accepted(self):
        for candidate in ("v1.0.0", "release", "local../escape", "local.Upper", "candidate."):
            with self.subTest(candidate=candidate), self.assertRaises(bundle.BundleError):
                self.package(candidate=candidate)
        for pin in ("0" * 63, "A" * 64, None):
            with self.subTest(pin=pin), self.assertRaises(bundle.BundleError):
                self.package(binary_sha256=pin)

    def test_group_writable_output_directory_is_refused(self):
        self.output.chmod(0o770)
        with self.assertRaises(bundle.BundleError):
            self.package()

    def test_extracted_bundle_verifies_hashes_and_exact_member_permissions(self):
        directory = self.extract(self.package())
        verification = bundle.verify_bundle(directory)
        self.assertEqual(verification["member_hashes_verified"], len(bundle.MEMBERS))
        self.assertEqual(verification["binary_sha256"], self.arguments["binary_sha256"])
        manifest = json.loads((directory / "MANIFEST.json").read_text())
        self.assertEqual(manifest["scope"], bundle.SCOPE)
        self.assertNotIn("source_revision", manifest)
        self.assertNotIn("release", manifest)

    def test_member_tampering_is_refused(self):
        directory = self.extract(self.package())
        (directory / "ontology_check.wasm").write_bytes(b"\0asm\x01\0\0\0tampered")
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)

    def test_extra_file_directory_symlink_and_permission_changes_are_refused(self):
        directory = self.extract(self.package())
        extra = directory / "auth.token"
        extra.write_bytes(b"must never pass")
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)
        extra.unlink()
        extra.mkdir()
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)
        extra.rmdir()
        extra.symlink_to(self.binary)
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)
        extra.unlink()
        (directory / "scripts/setup_a2a_counterparty.py").chmod(0o666)
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)

    def test_manifest_member_removal_size_boolean_and_duplicate_fields_are_refused(self):
        directory = self.extract(self.package())
        path = directory / "MANIFEST.json"
        original = path.read_bytes()
        manifest = json.loads(original)
        for invalid in (
            dict(manifest, files={name: item for name, item in manifest["files"].items()
                                  if name != "LICENSE"}),
            dict(manifest, files=dict(manifest["files"], LICENSE={
                **manifest["files"]["LICENSE"], "bytes": True})),
        ):
            path.write_bytes(bundle.encoded(invalid))
            with self.assertRaises(bundle.BundleError):
                bundle.verify_bundle(directory)
        path.write_bytes(b'{"schema":"duplicate","schema":"duplicate"}')
        with self.assertRaises(bundle.BundleError):
            bundle.verify_bundle(directory)

    def test_both_launchers_and_operator_helper_have_separate_help_paths_and_do_not_start_nodes(self):
        directory = self.extract(self.package())
        environment = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        for name in ("RUN-DEMO.sh", "RUN-COUNTERPARTY-CHECK.sh"):
            result = subprocess.run([str(directory / name), "--help"], cwd=self.root,
                                    env=environment, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertIn(b"usage:", result.stdout)
        helper = subprocess.run(["python3", str(directory / "scripts/setup_a2a_counterparty.py"),
                                 "--help"], env=environment, capture_output=True, timeout=10)
        self.assertEqual(helper.returncode, 0)
        bundle.verify_bundle(directory)

    def test_launcher_refuses_tampered_members_before_even_displaying_demo_help(self):
        directory = self.extract(self.package())
        (directory / "ontology_check.wasm").write_bytes(b"tampered")
        result = subprocess.run([str(directory / "RUN-DEMO.sh"), "--help"], cwd=self.root,
                                capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn(b"usage:", result.stdout)

    def test_counterparty_launcher_uses_packaged_modules_despite_conflicting_installed_packages(self):
        directory = self.extract(self.package())
        unrelated = self.root / "unrelated-packages"
        for name in ("examples", "scripts"):
            package = unrelated / name
            package.mkdir(parents=True)
            (package / "__init__.py").write_text('raise RuntimeError("UNRELATED PACKAGE LOADED")\n')
        environment = dict(os.environ, PYTHONPATH=str(unrelated))
        result = subprocess.run([str(directory / "RUN-COUNTERPARTY-CHECK.sh"), "--help"],
                                cwd=self.root, env=environment, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertNotIn(b"UNRELATED PACKAGE LOADED", result.stderr)
        bundle.verify_bundle(directory)

    def test_launcher_enforces_packaged_binary_module_and_ontology_after_user_options(self):
        # Inspect the actual arguments reaching a child, without executing the fixture binary.
        probe = self.source / "examples/a2a_compute_demo.py"
        probe.write_text('import argparse,json\np=argparse.ArgumentParser()\n'
                         'p.add_argument("--binary");p.add_argument("--module");'
                         'p.add_argument("--scenario");print(json.dumps(vars(p.parse_args())))\n')
        directory = self.extract(self.package())
        result = subprocess.run([str(directory / "RUN-DEMO.sh"), "--binary", "/unreviewed/node",
                                 "--module", "/unreviewed/program", "--scenario", "adder"],
                                cwd=self.root, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(json.loads(result.stdout), {"binary": str(directory / "froglet-node"),
            "module": str(directory / "ontology_check.wasm"), "scenario": "ontology"})

    @unittest.skipUnless(shutil.which("openssl"), "fixture certificate qualification requires OpenSSL")
    def test_actual_counterparty_fixture_certificate_passes_strict_x509_validation(self):
        # Keep actual certificate generation and chain loading, but never bind a socket.
        from examples import a2a_counterparty_demo as counterparty
        # macOS's system LibreSSL does not auto-add the missing identifiers that
        # newer OpenSSL happens to supply; exercise it too when available.
        executables = {shutil.which("openssl")}
        if Path("/usr/bin/openssl").is_file():
            executables.add("/usr/bin/openssl")
        for index, executable in enumerate(sorted(executables)):
            with self.subTest(openssl=executable):
                root = self.root / f"certificate-{index}"
                root.mkdir()
                server = MagicMock()
                server.server_port = 12345
                with patch.object(counterparty, "ThreadingHTTPServer", return_value=server), \
                        patch.object(counterparty.ssl.SSLContext, "wrap_socket", return_value=MagicMock()), \
                        patch.object(counterparty.threading, "Thread"), \
                        patch.object(counterparty.shutil, "which", return_value=executable):
                    fixture = counterparty.LocalTlsProxy(root)
                result = subprocess.run([executable, "verify", "-x509_strict", "-CAfile",
                                         str(fixture.root / "ca.crt"), str(fixture.root / "server.crt")],
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(": OK", result.stdout)
                self.assertIn(strict_tls_handshake(fixture.root), ("TLSv1.2", "TLSv1.3"))


if __name__ == "__main__":
    unittest.main()
