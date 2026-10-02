#!/usr/bin/env python3
"""Package or verify an unpublished local ontology demo; never build or deploy.

Python 3.10+ standard library only. Caller-supplied executable and Wasm digests
pin the inputs. SHA-256 member checks detect changes, not publisher identity.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import struct
import sys
import tarfile
import tempfile

REPO = Path(__file__).resolve().parents[1]
SCHEMA = "froglet.local-demo-bundle/v1"
SCOPE = "Unpublished local demo candidate; no release, attestation or platform qualification claim"
BINARY_LIMIT = 128 * 1024 * 1024
WASM_LIMIT = 262144
SOURCE_LIMIT = 1024 * 1024
MANIFEST_LIMIT = 64 * 1024
HASH = re.compile(r"[0-9a-f]{64}\Z")
CANDIDATE = re.compile(r"(?:local|candidate)\.[a-z0-9][a-z0-9._-]{0,63}\Z")
SOURCES = (
    "LICENSE", "examples/a2a_compute_demo.py", "examples/research_profile.py",
    "examples/a2a_counterparty_demo.py", "scripts/setup_a2a_counterparty.py",
    "docs/A2A_COUNTERPARTY_SETUP.md", "scripts/package_demo_bundle.py",
)
GENERATED = ("README.txt", "RUN-DEMO.sh", "RUN-COUNTERPARTY-CHECK.sh",
             "examples/__init__.py", "scripts/__init__.py")
MEMBERS = (*SOURCES, *GENERATED, "froglet-node", "ontology_check.wasm")
EXECUTABLES = {"froglet-node", "RUN-DEMO.sh", "RUN-COUNTERPARTY-CHECK.sh"}


class BundleError(ValueError):
    """Validation failure without including caller-supplied file contents."""


def digest(value):
    return hashlib.sha256(value).hexdigest()


def encoded(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode()


def absolute_path(value):
    path = Path(value)
    if not path.is_absolute() or ".." in path.parts:
        raise BundleError("use absolute paths without '..'")
    for entry in (path, *path.parents):
        if entry.is_symlink():
            raise BundleError("symbolic links in input/output paths are refused")
    return path


def regular_read(path, limit, executable=False):
    path = absolute_path(path)
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
                         | getattr(os, "O_NONBLOCK", 0))
    with os.fdopen(descriptor, "rb") as source:
        before = os.fstat(source.fileno())
        if not stat.S_ISREG(before.st_mode) or not 0 < before.st_size <= limit:
            raise BundleError("input must be a nonempty bounded regular file")
        if executable and not before.st_mode & 0o111:
            raise BundleError("node binary must have an executable permission bit")
        value = source.read(limit + 1)
        after = os.fstat(source.fileno())
        observed = lambda item: (item.st_dev, item.st_ino, item.st_size, item.st_mtime_ns,
                                 item.st_ctime_ns, item.st_mode)
        if len(value) > limit or len(value) != before.st_size or observed(before) != observed(after):
            raise BundleError("input changed or exceeded its limit while reading")
        return value


def platform_header(binary, platform, arch):
    """Refuse mismatched basic executable headers; this does not test execution."""
    if platform == "darwin":
        if len(binary) < 32 or binary[:4] != b"\xcf\xfa\xed\xfe":
            raise BundleError("Darwin input must be a little-endian 64-bit Mach-O executable")
        cpu, _, filetype = struct.unpack_from("<III", binary, 4)
        if cpu != {"arm64": 0x0100000c, "x86_64": 0x01000007}[arch] or filetype != 2:
            raise BundleError("Mach-O executable header does not match the supplied architecture")
    else:
        if len(binary) < 64 or binary[:7] != b"\x7fELF\x02\x01\x01":
            raise BundleError("Linux input must be a little-endian 64-bit ELF executable")
        kind, machine = struct.unpack_from("<HH", binary, 16)
        if kind not in (2, 3) or machine != {"arm64": 183, "x86_64": 62}[arch]:
            raise BundleError("ELF executable header does not match the supplied architecture")


def check_label(candidate, platform, arch):
    if not isinstance(candidate, str) or not CANDIDATE.fullmatch(candidate):
        raise BundleError("candidate label must start 'local.' or 'candidate.' and use safe lowercase characters")
    if platform not in ("darwin", "linux") or arch not in ("arm64", "x86_64"):
        raise BundleError("select an explicit supported executable platform and architecture")


def launcher(example, scenario=False):
    arguments = ' --scenario ontology' if scenario else ''
    return ("#!/bin/sh\nset -eu\n"
            'bundle=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)\n'
            'export PYTHONDONTWRITEBYTECODE=1\n'
            'python3 "$bundle/scripts/package_demo_bundle.py" --verify-dir "$bundle" >/dev/null\n'
            f'exec python3 "$bundle/examples/{example}" "$@" --binary "$bundle/froglet-node" '
            f'--module "$bundle/ontology_check.wasm"{arguments}\n').encode()


def readme(candidate, platform, arch):
    return f"""Froglet ontology demo — {candidate} ({platform}/{arch})

UNPUBLISHED LOCAL CANDIDATE. This is not an immutable release, attested build,
notarized application, or proof of compatibility with another machine. The
packager checks the basic executable header, not its dependencies or runtime.
Binary and Wasm inputs were pinned by caller-supplied SHA-256. MANIFEST.json lists
every payload member's bytes, permissions and SHA-256; it is integrity metadata,
not a signature or publisher authentication. No wallet, Node state, private
credential, installer fixture or release manifest is collected by this packager.

Requires Python 3.10+ and a compatible POSIX machine. No Cargo, Rustup, Node.js,
npm, checkout, account login or paid backend is required for this local demo.

After reviewing this candidate and its hashes, extract it into a new private
directory. Run from any directory:
  /absolute/bundle/RUN-DEMO.sh --output /absolute/report.json
  /absolute/bundle/RUN-DEMO.sh --serve

The ontology demo starts independent disposable Alice/Bob Nodes on loopback,
publishes selected synthetic terminology, retrieves it and runs Alice's bounded
Wasm audit. It checks retries, changed-input refusal, an execution-limit failure,
and restart recovery. Successful private state is removed by default. --serve
keeps those disposable Nodes available until Ctrl-C; --keep-state retains their
private state. Agent-host configuration is printed as private file paths. The
launcher does not edit host settings, install services or use existing Nodes.
Keep this bundle unchanged: launchers verify all members before starting work.
Reports must be written outside the bundle so later checks still pass.

Optional, separate counterparty qualification (OpenSSL additionally required):
  /absolute/bundle/RUN-COUNTERPARTY-CHECK.sh --output /absolute/invite-report.json
This creates its own disposable Nodes and short-lived loopback TLS credentials;
it does not use or reconfigure your real providers or trust store.

Real counterparty setup is an explicit operator action, separate from both demos:
  python3 /absolute/bundle/scripts/setup_a2a_counterparty.py --request /absolute/private-request.json --plan
Read docs/A2A_COUNTERPARTY_SETUP.md first. The plan is read-only; applying it
requires its exact approved SHA-256 and writes a new private configuration
directory. The helper never issues an invitation or restarts a Node. An existing
HTTP admission invitation is provider-wide, even beside an Offer-scoped A2A
credential. Nothing here authorizes anonymous public compute or real payments.

Standalone integrity check:
  python3 /absolute/bundle/scripts/package_demo_bundle.py --verify-dir /absolute/bundle

Synthetic labels and verified transaction evidence do not establish scientific
truth, user demand, public deployment capacity, or performance on another host.
""".encode()


def payloads(repo, binary, module, binary_sha256, module_sha256, candidate, platform, arch):
    check_label(candidate, platform, arch)
    if not all(isinstance(item, str) and HASH.fullmatch(item)
               for item in (binary_sha256, module_sha256)):
        raise BundleError("binary and module must each have an explicit lowercase SHA-256 pin")
    executable = regular_read(binary, BINARY_LIMIT, executable=True)
    program = regular_read(module, WASM_LIMIT)
    if digest(executable) != binary_sha256 or digest(program) != module_sha256:
        raise BundleError("binary or module SHA-256 does not match the caller-supplied pin")
    platform_header(executable, platform, arch)
    if not program.startswith(b"\0asm\x01\0\0\0"):
        raise BundleError("module must have a Wasm v1 header")
    repo = absolute_path(repo)
    result = {name: regular_read(repo / name, SOURCE_LIMIT) for name in SOURCES}
    result.update({"froglet-node": executable, "ontology_check.wasm": program,
        "README.txt": readme(candidate, platform, arch),
        "RUN-DEMO.sh": launcher("a2a_compute_demo.py", scenario=True),
        "RUN-COUNTERPARTY-CHECK.sh": launcher("a2a_counterparty_demo.py"),
        "examples/__init__.py": b'"""Exact local demo modules packaged for this candidate."""\n',
        "scripts/__init__.py": b'"""Exact opt-in operator helpers packaged for this candidate."""\n'})
    return result


def manifest_for(contents, candidate, platform, arch):
    return {"schema": SCHEMA, "scope": SCOPE, "candidate": candidate,
            "platform": platform, "arch": arch, "files": {
                name: {"sha256": digest(value), "bytes": len(value),
                       "mode": "0755" if name in EXECUTABLES else "0644"}
                for name, value in sorted(contents.items())}}


def package(binary, module, binary_sha256, module_sha256, candidate, platform, arch,
            out_dir, repo=REPO):
    contents = payloads(repo, binary, module, binary_sha256, module_sha256,
                        candidate, platform, arch)
    manifest = manifest_for(contents, candidate, platform, arch)
    contents["MANIFEST.json"] = encoded(manifest)
    out_dir = absolute_path(out_dir)
    metadata = out_dir.stat()
    if not stat.S_ISDIR(metadata.st_mode) or (os.name == "posix" and
            (metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022)):
        raise BundleError("output directory must exist, be owned by this user and not writable by others")
    archive = out_dir / f"froglet-ontology-demo-{candidate}-{platform}-{arch}.tar.gz"
    if archive.exists() or archive.is_symlink():
        raise BundleError("candidate archive already exists; nothing is overwritten")
    descriptor, temporary = tempfile.mkstemp(prefix=".froglet-demo-", dir=out_dir)
    try:
        with os.fdopen(descriptor, "wb") as output:
            with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
                with tarfile.open(mode="w", fileobj=compressed, format=tarfile.USTAR_FORMAT) as tar:
                    for name, value in sorted(contents.items()):
                        member = tarfile.TarInfo(name)
                        member.size, member.mtime, member.uid, member.gid = len(value), 0, 0, 0
                        member.mode = 0o755 if name in EXECUTABLES else 0o644
                        tar.addfile(member, io.BytesIO(value))
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.link(temporary, archive)  # Atomic publication, with no overwrite even on a race.
    finally:
        os.unlink(temporary)
    return {"scope": SCOPE, "candidate": candidate, "archive": str(archive),
            "archive_sha256": digest(regular_read(archive, BINARY_LIMIT + 16 * SOURCE_LIMIT)),
            "binary_sha256": binary_sha256, "module_sha256": module_sha256,
            "manifest_sha256": digest(contents["MANIFEST.json"]),
            "members": sorted(contents)}


def verify_bundle(directory):
    directory = absolute_path(directory)
    manifest_bytes = regular_read(directory / "MANIFEST.json", MANIFEST_LIMIT)
    try:
        def unique_pairs(pairs):
            result = {}
            for key, value in pairs:
                if key in result:
                    raise ValueError("duplicate manifest field")
                result[key] = value
            return result
        manifest = json.loads(manifest_bytes, object_pairs_hook=unique_pairs)
    except (ValueError, UnicodeError):
        raise BundleError("invalid bundle integrity manifest") from None
    if not isinstance(manifest, dict) or set(manifest) != {
            "schema", "scope", "candidate", "platform", "arch", "files"} \
            or manifest["schema"] != SCHEMA or manifest["scope"] != SCOPE:
        raise BundleError("unexpected bundle manifest schema or fields")
    check_label(manifest["candidate"], manifest["platform"], manifest["arch"])
    if not isinstance(manifest["files"], dict) or set(manifest["files"]) != set(MEMBERS):
        raise BundleError("manifest must contain exactly the allowlisted payload members")
    expected_paths = {*MEMBERS, "MANIFEST.json"}
    expected_dirs = {str(Path(name).parent) for name in expected_paths if "/" in name}
    for entry in directory.rglob("*"):
        relative = str(entry.relative_to(directory))
        if entry.is_symlink() or (entry.is_dir() and relative not in expected_dirs) \
                or (not entry.is_dir() and relative not in expected_paths):
            raise BundleError("bundle contains an unexpected file, directory or symbolic link")
    for name, declared in manifest["files"].items():
        limit = BINARY_LIMIT if name == "froglet-node" else (
            WASM_LIMIT if name == "ontology_check.wasm" else SOURCE_LIMIT)
        value = regular_read(directory / name, limit, executable=name in EXECUTABLES)
        mode = "0755" if name in EXECUTABLES else "0644"
        if not isinstance(declared, dict) or set(declared) != {"sha256", "bytes", "mode"} \
                or not isinstance(declared["sha256"], str) or not HASH.fullmatch(declared["sha256"]) \
                or type(declared["bytes"]) is not int or declared["bytes"] != len(value) \
                or declared["sha256"] != digest(value) or declared["mode"] != mode \
                or stat.S_IMODE((directory / name).stat().st_mode) != int(mode, 8):
            raise BundleError("bundle payload bytes, digest or permissions do not match its manifest")
    return {"candidate": manifest["candidate"], "member_hashes_verified": len(MEMBERS),
            "scope": SCOPE, "binary_sha256": manifest["files"]["froglet-node"]["sha256"],
            "module_sha256": manifest["files"]["ontology_check.wasm"]["sha256"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verify-dir", type=Path, help="verify an extracted bundle without starting anything")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--module", type=Path)
    parser.add_argument("--binary-sha256")
    parser.add_argument("--module-sha256")
    parser.add_argument("--candidate")
    parser.add_argument("--platform", choices=("darwin", "linux"))
    parser.add_argument("--arch", choices=("arm64", "x86_64"))
    parser.add_argument("--out-dir", type=Path)
    arguments = parser.parse_args()
    package_fields = ("binary", "module", "binary_sha256", "module_sha256", "candidate",
                      "platform", "arch", "out_dir")
    if arguments.verify_dir:
        if any(getattr(arguments, name) is not None for name in package_fields):
            parser.error("--verify-dir cannot be combined with packaging arguments")
        result = verify_bundle(arguments.verify_dir)
    else:
        if any(getattr(arguments, name) is None for name in package_fields):
            parser.error("packaging requires every binary/module pin, candidate, platform, architecture and output argument")
        result = package(**{name: getattr(arguments, name) for name in package_fields})
    print(json.dumps(result, sort_keys=True, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (OSError, BundleError) as error:
        print(f"demo packaging failed: {error}", file=sys.stderr)
        sys.exit(1)
