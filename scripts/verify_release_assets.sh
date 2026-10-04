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
    --dir|--version|--target)
      if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
        echo "verify_release_assets: $1 requires a value" >&2
        exit 1
      fi
      ;;
  esac
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

[[ "$version" =~ ^v[0-9A-Za-z][0-9A-Za-z.+-]*$ ]] || {
  echo "verify_release_assets: invalid release version" >&2
  exit 1
}

if [[ ${#targets[@]} -eq 0 ]]; then
  targets=("linux:x86_64" "linux:arm64" "darwin:arm64")
fi

for target in "${targets[@]}"; do
  case "$target" in
    linux:x86_64|linux:arm64|darwin:arm64) ;;
    *) echo "verify_release_assets: unsupported target" >&2; exit 1 ;;
  esac
done

[[ -f "$dir/SHA256SUMS" ]] || {
  echo "missing release checksums: $dir/SHA256SUMS" >&2
  exit 1
}

checksum_names=()
while IFS= read -r line || [[ -n "$line" ]]; do
  [[ -z "$line" ]] && continue
  if [[ ! "$line" =~ ^[0-9a-f]{64}[[:space:]]+[*]?([^[:space:]]+)$ ]]; then
    echo "invalid release checksum line" >&2
    exit 1
  fi
  name="${BASH_REMATCH[1]}"
  case "$name" in
    agent-bootstrap.sh|"froglet-node-${version}-linux-x86_64.tar.gz"|"froglet-node-${version}-linux-arm64.tar.gz"|"froglet-node-${version}-darwin-arm64.tar.gz") ;;
    *) echo "unexpected release checksum asset: $name" >&2; exit 1 ;;
  esac
  for existing in "${checksum_names[@]:-}"; do
    [[ "$existing" != "$name" ]] || {
      echo "duplicate release checksum for $name" >&2
      exit 1
    }
  done
  checksum_names+=("$name")
done <"$dir/SHA256SUMS"

require_checksum() {
  local name="$1"
  for existing in "${checksum_names[@]:-}"; do
    [[ "$existing" == "$name" ]] && return 0
  done
  echo "missing release checksum for $name" >&2
  exit 1
}

require_checksum agent-bootstrap.sh

[[ -f "$dir/agent-bootstrap.sh" ]] || {
  echo "missing release asset: $dir/agent-bootstrap.sh" >&2
  exit 1
}
bash -n "$dir/agent-bootstrap.sh"

for target in "${targets[@]}"; do
  platform="${target%%:*}"
  arch="${target##*:}"
  asset="$dir/froglet-node-${version}-${platform}-${arch}.tar.gz"
  require_checksum "froglet-node-${version}-${platform}-${arch}.tar.gz"
  [[ -f "$asset" ]] || {
    echo "missing release asset: $asset" >&2
    exit 1
  }
  # Read the archive directly. BSD tar may omit AppleDouble ._ members from
  # -t output, so a textual listing cannot establish exact payload coverage.
  python3 - "$asset" <<'PY'
import sys
import tarfile

try:
    with tarfile.open(sys.argv[1], "r:gz") as archive:
        members = archive.getmembers()
        if len(members) != 2 or {member.name for member in members} != {"froglet-node", "LICENSE"}:
            raise SystemExit("release archive must contain exactly one froglet-node and LICENSE")
        if any(not member.isfile() for member in members):
            raise SystemExit("release archive members must be regular files")
        if archive.pax_headers or any(member.pax_headers for member in members):
            raise SystemExit("release archive must not contain PAX/xattr metadata")
except (OSError, tarfile.TarError) as error:
    raise SystemExit(f"invalid release archive: {error}")
PY
done

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$dir" && sha256sum -c SHA256SUMS >/dev/null)
elif command -v shasum >/dev/null 2>&1; then
  (cd "$dir" && shasum -a 256 -c SHA256SUMS >/dev/null)
else
  echo "missing required checksum tool: sha256sum or shasum" >&2
  exit 1
fi
