#!/bin/sh
set -eu

REPO="${FROGLET_INSTALL_REPO:-armanas/froglet}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
INSTALL_MARKETPLACE="${INSTALL_MARKETPLACE:-0}"
LATEST_URL="${FROGLET_INSTALL_LATEST_URL:-https://github.com/$REPO/releases/latest}"
DOWNLOAD_BASE_URL="${FROGLET_INSTALL_BASE_URL:-https://github.com/$REPO/releases/download}"

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

normalize_tag() {
  case "$1" in
    v*) printf '%s\n' "$1" ;;
    *) printf 'v%s\n' "$1" ;;
  esac
}

resolve_tag() {
  if [ -n "${VERSION:-}" ]; then
    normalize_tag "$VERSION"
    return 0
  fi

  resolved_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "$LATEST_URL")"
  resolved_url="$(printf '%s' "$resolved_url" | sed 's:/*$::')"
  tag="${resolved_url##*/}"
  case "$tag" in
    v*) printf '%s\n' "$tag" ;;
    *) fail "could not resolve latest release tag from $LATEST_URL" ;;
  esac
}

detect_platform() {
  os_name="$(uname -s 2>/dev/null || true)"
  arch_name="$(uname -m 2>/dev/null || true)"

  case "$os_name" in
    Linux) platform="linux" ;;
    Darwin) platform="darwin" ;;
    *) fail "unsupported operating system: $os_name" ;;
  esac

  case "$arch_name" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="arm64" ;;
    *) fail "unsupported architecture: $arch_name" ;;
  esac

  if [ "$platform" = "darwin" ] && [ "$arch" != "arm64" ]; then
    fail "macOS x86_64 is not supported by the binary installer"
  fi

  printf '%s %s\n' "$platform" "$arch"
}

checksum_cmd() {
  if command -v sha256sum >/dev/null 2>&1; then
    printf 'sha256sum\n'
    return 0
  fi
  if command -v shasum >/dev/null 2>&1; then
    printf 'shasum\n'
    return 0
  fi
  fail "missing required checksum tool: sha256sum or shasum"
}

verify_checksums() {
  check_dir="$1"
  cmd="$(checksum_cmd)"
  if [ "$cmd" = "sha256sum" ]; then
    (cd "$check_dir" && sha256sum -c SHA256SUMS >/dev/null)
    return 0
  fi
  (cd "$check_dir" && shasum -a 256 -c SHA256SUMS >/dev/null)
}

install_binary() {
  src="$1"
  dst="$2"
  if command -v install >/dev/null 2>&1; then
    install -m 0755 "$src" "$dst"
    return 0
  fi
  cp "$src" "$dst"
  chmod 0755 "$dst"
}

path_hint() {
  case ":${PATH:-}:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac
  log "Add $INSTALL_DIR to PATH, for example:"
  log "  export PATH=\"$INSTALL_DIR:\$PATH\""
}

need_cmd curl
need_cmd tar
need_cmd mktemp

set -- $(detect_platform)
PLATFORM="$1"
ARCH="$2"
TAG="$(resolve_tag)"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/froglet-install.XXXXXX")"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$INSTALL_DIR"

BINARIES="froglet-node"
case "$INSTALL_MARKETPLACE" in
  1|true|TRUE|yes|YES)
    BINARIES="$BINARIES froglet-marketplace"
    ;;
esac

CHECKSUM_URL="$DOWNLOAD_BASE_URL/$TAG/SHA256SUMS"
curl -fsSL "$CHECKSUM_URL" -o "$TMP_DIR/SHA256SUMS.all"
: > "$TMP_DIR/SHA256SUMS"

for binary in $BINARIES; do
  asset_name="${binary}-${TAG}-${PLATFORM}-${ARCH}.tar.gz"
  asset_url="$DOWNLOAD_BASE_URL/$TAG/$asset_name"
  curl -fsSL "$asset_url" -o "$TMP_DIR/$asset_name"
  grep -F "  $asset_name" "$TMP_DIR/SHA256SUMS.all" >> "$TMP_DIR/SHA256SUMS" || \
    fail "missing checksum entry for $asset_name"
done

verify_checksums "$TMP_DIR"

for binary in $BINARIES; do
  asset_name="${binary}-${TAG}-${PLATFORM}-${ARCH}.tar.gz"
  tar -xzf "$TMP_DIR/$asset_name" -C "$TMP_DIR"
  [ -f "$TMP_DIR/$binary" ] || fail "archive $asset_name did not contain $binary"
  install_binary "$TMP_DIR/$binary" "$INSTALL_DIR/$binary"
  log "Installed $binary to $INSTALL_DIR/$binary"
done

path_hint
