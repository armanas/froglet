#!/usr/bin/env python3
"""Generate and validate Froglet's deterministic release bundle manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from typing import NoReturn


SCHEMA = "froglet.release-bundle.v1"
AGENT_BOOTSTRAP_ASSET = "agent-bootstrap.sh"
PLATFORMS = (("linux", "x86_64"), ("linux", "arm64"), ("darwin", "arm64"))
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
REVISION_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")
REPOSITORY_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
RELEASE_RE = re.compile(r"^v[0-9A-Za-z][0-9A-Za-z.+-]*$")
IMAGE_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"release manifest error: {message}")


def require_string(document: dict[str, object], key: str) -> str:
    value = document.get(key)
    if not isinstance(value, str) or not value:
        fail(f"{key} must be a non-empty string")
    return value


def require_string_list(document: dict[str, object], key: str) -> list[str]:
    value = document.get(key)
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        fail(f"{key} must be an array of strings")
    return value


def image_digest(reference: str, field: str) -> str:
    if not IMAGE_RE.fullmatch(reference):
        fail(f"{field} must be an immutable OCI sha256 digest reference")
    return reference.rsplit("@", 1)[1]


def expected_asset(release: str, platform: str, arch: str) -> str:
    return f"froglet-node-{release}-{platform}-{arch}.tar.gz"


def binary_key(platform: str, arch: str, suffix: str) -> str:
    return f"binary_froglet_node_{platform}_{arch}_{suffix}"


def parse_checksums(path: Path) -> dict[str, str]:
    checksums: dict[str, str] = {}
    for line_number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = raw_line.strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 2:
            fail(f"{path}:{line_number}: expected '<sha256>  <asset>'")
        digest, name = parts
        name = name.removeprefix("*")
        if not SHA256_RE.fullmatch(digest):
            fail(f"{path}:{line_number}: invalid SHA-256 digest")
        if name in checksums:
            fail(f"{path}:{line_number}: duplicate checksum for {name}")
        checksums[name] = digest
    return checksums


def validate(document: dict[str, object], assets_dir: Path | None = None) -> None:
    allowed_keys = {
        "schema",
        "release",
        "source_repository",
        "source_revision",
        "source_ref",
        "attestation_signer_workflow",
        "agent_bootstrap_asset",
        "agent_bootstrap_sha256",
        "image_provider",
        "image_provider_mirrors",
        "image_runtime",
        "image_runtime_mirrors",
        "image_dual",
        "image_dual_mirrors",
        "image_mcp",
        "image_mcp_mirrors",
    }
    for platform, arch in PLATFORMS:
        allowed_keys.add(binary_key(platform, arch, "asset"))
        allowed_keys.add(binary_key(platform, arch, "sha256"))

    unknown = sorted(set(document) - allowed_keys)
    missing = sorted(allowed_keys - set(document))
    if unknown:
        fail(f"unknown fields: {', '.join(unknown)}")
    if missing:
        fail(f"missing fields: {', '.join(missing)}")

    if require_string(document, "schema") != SCHEMA:
        fail(f"schema must be {SCHEMA}")

    release = require_string(document, "release")
    if not RELEASE_RE.fullmatch(release):
        fail("release must be a normalized v-prefixed tag")

    repository = require_string(document, "source_repository")
    if not REPOSITORY_RE.fullmatch(repository):
        fail("source_repository must be an owner/repository pair")
    revision = require_string(document, "source_revision")
    if not REVISION_RE.fullmatch(revision):
        fail("source_revision must be a 40- or 64-character lowercase hex digest")
    if require_string(document, "source_ref") != f"refs/tags/{release}":
        fail("source_ref must match release")
    if require_string(document, "attestation_signer_workflow") != (
        f"{repository}/.github/workflows/release.yml"
    ):
        fail("attestation_signer_workflow must be this repository's release workflow")

    bootstrap_asset = require_string(document, "agent_bootstrap_asset")
    if bootstrap_asset != AGENT_BOOTSTRAP_ASSET:
        fail(f"agent_bootstrap_asset must be {AGENT_BOOTSTRAP_ASSET}")
    bootstrap_digest = require_string(document, "agent_bootstrap_sha256")
    if not SHA256_RE.fullmatch(bootstrap_digest):
        fail("agent_bootstrap_sha256 must be a lowercase SHA-256 digest")
    if assets_dir is not None:
        bootstrap_path = assets_dir / bootstrap_asset
        if not bootstrap_path.is_file():
            fail(f"missing release asset: {bootstrap_path}")
        actual = hashlib.sha256(bootstrap_path.read_bytes()).hexdigest()
        if actual != bootstrap_digest:
            fail(f"SHA-256 mismatch for {bootstrap_asset}")

    for role in ("provider", "runtime", "dual", "mcp"):
        source_field = f"image_{role}"
        source = require_string(document, source_field)
        source_digest = image_digest(source, source_field)
        mirror_field = f"image_{role}_mirrors"
        mirrors = require_string_list(document, mirror_field)
        if len(mirrors) != len(set(mirrors)):
            fail(f"{mirror_field} must not contain duplicates")
        for mirror in mirrors:
            if mirror == source:
                fail(f"{mirror_field} must not repeat the canonical source")
            if image_digest(mirror, mirror_field) != source_digest:
                fail(f"{mirror_field} digest must match {source_field}")

    for platform, arch in PLATFORMS:
        asset_key = binary_key(platform, arch, "asset")
        digest_key = binary_key(platform, arch, "sha256")
        asset = require_string(document, asset_key)
        digest = require_string(document, digest_key)
        expected = expected_asset(release, platform, arch)
        if asset != expected:
            fail(f"{asset_key} must be {expected}")
        if not SHA256_RE.fullmatch(digest):
            fail(f"{digest_key} must be a lowercase SHA-256 digest")
        if assets_dir is not None:
            asset_path = assets_dir / asset
            if not asset_path.is_file():
                fail(f"missing release asset: {asset_path}")
            actual = hashlib.sha256(asset_path.read_bytes()).hexdigest()
            if actual != digest:
                fail(f"SHA-256 mismatch for {asset}")


def generate(args: argparse.Namespace) -> None:
    release = args.release if args.release.startswith("v") else f"v{args.release}"
    checksums = parse_checksums(args.checksums)
    expected_names = {
        expected_asset(release, platform, arch) for platform, arch in PLATFORMS
    }
    expected_names.add(AGENT_BOOTSTRAP_ASSET)
    if set(checksums) != expected_names:
        fail(
            "checksum asset set differs from expected release assets: "
            f"actual={sorted(checksums)!r} expected={sorted(expected_names)!r}"
        )

    document: dict[str, object] = {
        "schema": SCHEMA,
        "release": release,
        "source_repository": args.repository,
        "source_revision": args.source_revision,
        "source_ref": f"refs/tags/{release}",
        "attestation_signer_workflow": (
            f"{args.repository}/.github/workflows/release.yml"
        ),
        "agent_bootstrap_asset": AGENT_BOOTSTRAP_ASSET,
        "agent_bootstrap_sha256": checksums[AGENT_BOOTSTRAP_ASSET],
        "image_provider": args.provider_image,
        "image_provider_mirrors": args.provider_image_mirror,
        "image_runtime": args.runtime_image,
        "image_runtime_mirrors": args.runtime_image_mirror,
        "image_dual": args.dual_image,
        "image_dual_mirrors": args.dual_image_mirror,
        "image_mcp": args.mcp_image,
        "image_mcp_mirrors": args.mcp_image_mirror,
    }
    for platform, arch in PLATFORMS:
        asset = expected_asset(release, platform, arch)
        document[binary_key(platform, arch, "asset")] = asset
        document[binary_key(platform, arch, "sha256")] = checksums[asset]

    validate(document)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def verify(args: argparse.Namespace) -> None:
    try:
        document = json.loads(args.manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot read {args.manifest}: {error}")
    if not isinstance(document, dict):
        fail("manifest root must be a JSON object")
    validate(document, args.assets_dir)


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    generate_parser = commands.add_parser("generate")
    generate_parser.add_argument("--release", required=True)
    generate_parser.add_argument("--repository", required=True)
    generate_parser.add_argument("--source-revision", required=True)
    generate_parser.add_argument("--checksums", required=True, type=Path)
    generate_parser.add_argument("--provider-image", required=True)
    generate_parser.add_argument(
        "--provider-image-mirror", action="append", default=[]
    )
    generate_parser.add_argument("--runtime-image", required=True)
    generate_parser.add_argument("--runtime-image-mirror", action="append", default=[])
    generate_parser.add_argument("--dual-image", required=True)
    generate_parser.add_argument("--dual-image-mirror", action="append", default=[])
    generate_parser.add_argument("--mcp-image", required=True)
    generate_parser.add_argument("--mcp-image-mirror", action="append", default=[])
    generate_parser.add_argument("--out", required=True, type=Path)
    generate_parser.set_defaults(function=generate)

    verify_parser = commands.add_parser("verify")
    verify_parser.add_argument("--manifest", required=True, type=Path)
    verify_parser.add_argument("--assets-dir", type=Path)
    verify_parser.set_defaults(function=verify)
    return root


def main() -> None:
    args = parser().parse_args()
    args.function(args)


if __name__ == "__main__":
    main()
