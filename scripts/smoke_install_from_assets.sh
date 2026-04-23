#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

assets_dir=""
version=""

usage() {
  cat <<'EOF'
Usage: scripts/smoke_install_from_assets.sh --assets-dir <dir> --version <tag>
EOF
}

detect_host_target() {
  local os_name arch_name platform arch
  os_name="$(uname -s 2>/dev/null || true)"
  arch_name="$(uname -m 2>/dev/null || true)"

  case "$os_name" in
    Linux) platform="linux" ;;
    Darwin) platform="darwin" ;;
    *) echo "unsupported operating system for install smoke: $os_name" >&2; exit 1 ;;
  esac

  case "$arch_name" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="arm64" ;;
    *) echo "unsupported architecture for install smoke: $arch_name" >&2; exit 1 ;;
  esac

  printf '%s %s\n' "$platform" "$arch"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --assets-dir)
      assets_dir="$2"
      shift 2
      ;;
    --version)
      version="$2"
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

[[ -n "$assets_dir" && -n "$version" ]] || {
  usage >&2
  exit 1
}

if [[ "$version" != v* ]]; then
  version="v$version"
fi

set -- $(detect_host_target)
host_platform="$1"
host_arch="$2"
expected_asset="froglet-node-${version}-${host_platform}-${host_arch}.tar.gz"
if [[ ! -f "$assets_dir/$expected_asset" ]]; then
  echo "install smoke requires a host-compatible asset; missing $expected_asset in $assets_dir" >&2
  available_assets="$(
    find "$assets_dir" -maxdepth 1 -type f -name "froglet-node-${version}-*.tar.gz" \
      -exec basename {} \; | LC_ALL=C sort | tr '\n' ' '
  )"
  if [[ -n "$available_assets" ]]; then
    echo "available froglet-node archives: $available_assets" >&2
  fi
  exit 1
fi

work_dir="$(mktemp -d "${TMPDIR:-/tmp}/froglet-install-smoke.XXXXXX")"
install_root="$work_dir/install"
release_root="$work_dir/releases/$version"
data_dir="$work_dir/data"
log_file="$work_dir/froglet-node.log"
listen_port="$(
  python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"

cleanup() {
  if [[ -n "${node_pid:-}" ]]; then
    kill "$node_pid" 2>/dev/null || true
    wait "$node_pid" 2>/dev/null || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT

mkdir -p "$install_root" "$release_root" "$data_dir"
cp "$assets_dir"/* "$release_root/"

INSTALL_DIR="$install_root" \
VERSION="$version" \
FROGLET_INSTALL_BASE_URL="file://$work_dir/releases" \
sh "$repo_root/scripts/install.sh"

FROGLET_NODE_ROLE=provider \
FROGLET_DATA_DIR="$data_dir" \
FROGLET_IDENTITY_AUTO_GENERATE=true \
FROGLET_PAYMENT_BACKEND=none \
FROGLET_LISTEN_ADDR="127.0.0.1:$listen_port" \
FROGLET_RUNTIME_LISTEN_ADDR=127.0.0.1:0 \
FROGLET_TOR_BACKEND_LISTEN_ADDR=127.0.0.1:0 \
"$install_root/froglet-node" >"$log_file" 2>&1 &
node_pid=$!

for _ in $(seq 1 20); do
  if curl --fail --silent --show-error "http://127.0.0.1:$listen_port/health" >/dev/null; then
    exit 0
  fi
  sleep 1
done

cat "$log_file" >&2
echo "froglet-node did not become healthy during install smoke" >&2
exit 1
