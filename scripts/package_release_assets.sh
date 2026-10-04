#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version=""
platform=""
arch=""
out_dir=""
binary_path=""
cargo_target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"

usage() {
  cat <<'EOF'
Usage: scripts/package_release_assets.sh --version <tag> --platform <linux|darwin> --arch <x86_64|arm64> --out-dir <dir> [--binary <path>]

Packages a prebuilt executable, checking its basic OS/architecture header.
This check does not establish source provenance or runtime compatibility.
Default binary: ${CARGO_TARGET_DIR:-target}/release/froglet-node.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version|--platform|--arch|--out-dir|--binary)
      if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
        echo "package_release_assets: $1 requires a value" >&2
        exit 1
      fi
      ;;
  esac
  case "$1" in
    --version)
      version="$2"
      shift 2
      ;;
    --platform)
      platform="$2"
      shift 2
      ;;
    --arch)
      arch="$2"
      shift 2
      ;;
    --out-dir)
      out_dir="$2"
      shift 2
      ;;
    --binary)
      binary_path="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

[[ -n "$version" && -n "$platform" && -n "$arch" && -n "$out_dir" ]] || {
  usage >&2
  exit 1
}

if [[ "$version" != v* ]]; then
  version="v$version"
fi

[[ "$version" =~ ^v[0-9A-Za-z][0-9A-Za-z.+-]*$ ]] || {
  echo "package_release_assets: invalid release version" >&2
  exit 1
}
case "$platform:$arch" in
  linux:x86_64|linux:arm64|darwin:arm64) ;;
  *) echo "package_release_assets: unsupported packaging target" >&2; exit 1 ;;
esac

mkdir -p "$out_dir"
cp scripts/agent-bootstrap.sh "$out_dir/agent-bootstrap.sh"
chmod 0755 "$out_dir/agent-bootstrap.sh"
stage_dir="$(mktemp -d "${TMPDIR:-/tmp}/froglet-release-assets.XXXXXX")"
cleanup() {
  rm -rf "$stage_dir"
}
trap cleanup EXIT

package_binary() {
  local binary="$1"
  local src="$2"
  local bundle_dir="$stage_dir/$binary"
  local archive_name="${binary}-${version}-${platform}-${arch}.tar.gz"

  [[ -x "$src" ]] || {
    echo "missing executable binary at $src" >&2
    exit 1
  }

  mkdir -p "$bundle_dir"
  cp "$src" "$bundle_dir/$binary"
  # Inspect the bytes being archived, rather than trusting the requested label.
  python3 - "$bundle_dir/$binary" "$platform" "$arch" <<'PY'
import struct
import sys

path, platform, arch = sys.argv[1:]
with open(path, "rb") as source:
    header = source.read(64)
valid = False
if platform == "darwin" and len(header) >= 32 and header[:4] == b"\xcf\xfa\xed\xfe":
    cpu, _, kind = struct.unpack_from("<III", header, 4)
    valid = cpu == {"arm64": 0x0100000c, "x86_64": 0x01000007}[arch] and kind == 2
elif platform == "linux" and len(header) >= 64 and header[:7] == b"\x7fELF\x02\x01\x01":
    kind, machine = struct.unpack_from("<HH", header, 16)
    valid = kind in (2, 3) and machine == {"arm64": 183, "x86_64": 62}[arch]
if not valid:
    raise SystemExit("package_release_assets: binary header does not match requested OS/architecture")
PY
  cp LICENSE "$bundle_dir/LICENSE"

  # macOS tar otherwise emits AppleDouble ._ members which its own listings
  # can hide. BSD/GNU tar both support these flags; USTAR needs no PAX/xattrs.
  COPYFILE_DISABLE=1 tar --no-xattrs --format=ustar \
    -czf "$out_dir/$archive_name" -C "$bundle_dir" "$binary" LICENSE
  rm -rf "$bundle_dir"
}

package_binary "froglet-node" "${binary_path:-$cargo_target_dir/release/froglet-node}"
