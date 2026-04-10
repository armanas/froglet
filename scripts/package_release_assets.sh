#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version=""
platform=""
arch=""
out_dir=""

usage() {
  cat <<'EOF'
Usage: scripts/package_release_assets.sh --version <tag> --platform <linux|darwin> --arch <x86_64|arm64> --out-dir <dir>
EOF
}

while [[ $# -gt 0 ]]; do
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

mkdir -p "$out_dir"
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
  cp LICENSE "$bundle_dir/LICENSE"

  tar -czf "$out_dir/$archive_name" -C "$bundle_dir" "$binary" LICENSE
  rm -rf "$bundle_dir"
}

package_binary "froglet-node" "$repo_root/target/release/froglet-node"
package_binary "froglet-marketplace" "$repo_root/target/release/froglet-marketplace"
