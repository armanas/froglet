#!/usr/bin/env bash
set -euo pipefail

dir=""
version=""
targets=()

usage() {
  cat <<'EOF'
Usage: scripts/verify_release_assets.sh --dir <dir> --version <tag> [--target <platform:arch> ...]
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      dir="$2"
      shift 2
      ;;
    --version)
      version="$2"
      shift 2
      ;;
    --target)
      targets+=("$2")
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

[[ -n "$dir" && -n "$version" ]] || {
  usage >&2
  exit 1
}

if [[ "$version" != v* ]]; then
  version="v$version"
fi

if [[ ${#targets[@]} -eq 0 ]]; then
  targets=("linux:x86_64" "linux:arm64" "darwin:arm64")
fi

for target in "${targets[@]}"; do
  platform="${target%%:*}"
  arch="${target##*:}"
  for binary in froglet-node froglet-marketplace; do
    asset="$dir/${binary}-${version}-${platform}-${arch}.tar.gz"
    [[ -f "$asset" ]] || {
      echo "missing release asset: $asset" >&2
      exit 1
    }
    tar -tzf "$asset" | grep -Fx "$binary" >/dev/null
    tar -tzf "$asset" | grep -Fx "LICENSE" >/dev/null
  done
done

if [[ -f "$dir/SHA256SUMS" ]]; then
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$dir" && sha256sum -c SHA256SUMS >/dev/null)
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$dir" && shasum -a 256 -c SHA256SUMS >/dev/null)
  else
    echo "missing required checksum tool: sha256sum or shasum" >&2
    exit 1
  fi
fi
