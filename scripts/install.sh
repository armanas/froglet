#!/bin/sh
set -eu

REPO="${FROGLET_INSTALL_REPO:-armanas/froglet}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"
LATEST_URL="${FROGLET_INSTALL_LATEST_URL:-https://github.com/$REPO/releases/latest}"
DOWNLOAD_BASE_URL="${FROGLET_INSTALL_BASE_URL:-https://github.com/$REPO/releases/download}"
GITHUB_API_BASE_URL="${FROGLET_GITHUB_API_BASE_URL:-https://api.github.com}"
MANIFEST_NAME="release-manifest.json"
ATTESTATION_NAME="release-manifest.intoto.jsonl"
MANIFEST_SHA256_PIN="${FROGLET_RELEASE_MANIFEST_SHA256:-}"
MANIFEST_OUT="${FROGLET_RELEASE_MANIFEST_OUT:-}"
GH_ATTESTATION_MODE="${FROGLET_GH_ATTESTATION_MODE:-auto}"

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

curl_https() {
  curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 "$@"
}

download_to_file() {
  url="$1"
  output="$2"
  case "$url" in
    https://*) curl_https "$url" -o "$output" ;;
    file://*)
      [ "${FROGLET_TRUSTED_MANIFEST_PIN:-0}" = "1" ] || \
        fail "file:// install inputs require explicit FROGLET_TRUSTED_MANIFEST_PIN=1"
      curl -fsSL "$url" -o "$output"
      ;;
    *) fail "install material URL must use https://: $url" ;;
  esac
}

normalize_tag() {
  case "$1" in
    v*) printf '%s\n' "$1" ;;
    *) printf 'v%s\n' "$1" ;;
  esac
}

validate_tag() {
  printf '%s' "$1" | grep -Eq '^v[0-9A-Za-z][0-9A-Za-z.+-]*$' || \
    fail "invalid release tag: $1"
}

resolve_tag() {
  if [ -n "${VERSION:-}" ]; then
    normalize_tag "$VERSION"
    return 0
  fi

  resolved_url="$(curl_https -o /dev/null -w '%{url_effective}' "$LATEST_URL")"
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

file_sha256() {
  path="$1"
  cmd="$(checksum_cmd)"
  if [ "$cmd" = "sha256sum" ]; then
    sha256sum "$path" | awk '{print $1}'
    return 0
  fi
  shasum -a 256 "$path" | awk '{print $1}'
}

verify_file_sha256() {
  path="$1"
  expected="$2"
  actual="$(file_sha256 "$path")"
  [ "$actual" = "$expected" ] || fail "SHA-256 mismatch for $(basename "$path")"
}

manifest_value() {
  key="$1"
  manifest="$2"
  values="$(
    sed -n 's/^[[:space:]]*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$manifest"
  )"
  count="$(printf '%s\n' "$values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$count" = "1" ] || fail "release manifest must contain exactly one string field: $key"
  printf '%s\n' "$values"
}

require_sha256() {
  name="$1"
  value="$2"
  printf '%s' "$value" | grep -Eq '^[0-9a-f]{64}$' || \
    fail "$name must be a lowercase SHA-256 digest"
}

require_source_revision() {
  value="$1"
  printf '%s' "$value" | grep -Eq '^[0-9a-f]{40}([0-9a-f]{24})?$' || \
    fail "source_revision must be a 40- or 64-character lowercase hex digest"
}

require_immutable_image() {
  name="$1"
  value="$2"
  printf '%s' "$value" | grep -Eq \
    '^[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}$' || \
    fail "$name must be an immutable OCI sha256 digest reference"
}

immutable_release_manifest_sha256() {
  release_json="$1"
  release_tag="$2"

  immutable_values="$(
    sed -n 's/^[[:space:]][[:space:]]"immutable"[[:space:]]*:[[:space:]]*\([a-z][a-z]*\)[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$release_json"
  )"
  immutable_count="$(printf '%s\n' "$immutable_values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$immutable_count" = "1" ] || \
    fail "GitHub release metadata must contain exactly one top-level immutable field"
  [ "$immutable_values" = "true" ] || \
    fail "GitHub release $release_tag is mutable; refusing it as an install trust root"

  tag_values="$(
    sed -n 's/^[[:space:]][[:space:]]"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)"[,]\{0,1\}[[:space:]]*$/\1/p' \
      "$release_json"
  )"
  tag_count="$(printf '%s\n' "$tag_values" | sed '/^$/d' | wc -l | tr -d ' ')"
  [ "$tag_count" = "1" ] || \
    fail "GitHub release metadata must contain exactly one top-level tag_name field"
  [ "$tag_values" = "$release_tag" ] || \
    fail "GitHub release metadata tag does not match requested release $release_tag"

  # GitHub's release response renders each asset as an object in the top-level
  # `assets` array. Track complete four-space asset objects so `name` and
  # `digest` must belong to the same asset; nested uploader fields are more
  # deeply indented and cannot be confused for either value.
  asset_matches="$(
    awk -v wanted="$MANIFEST_NAME" '
      /^    \{[[:space:]]*$/ {
        in_asset = 1
        name = ""
        digest = ""
        next
      }
      in_asset && /^      "name"[[:space:]]*:/ {
        value = $0
        sub(/^      "name"[[:space:]]*:[[:space:]]*"/, "", value)
        sub(/"[,]?[[:space:]]*$/, "", value)
        name = value
        next
      }
      in_asset && /^      "digest"[[:space:]]*:/ {
        value = $0
        sub(/^      "digest"[[:space:]]*:[[:space:]]*"/, "", value)
        sub(/"[,]?[[:space:]]*$/, "", value)
        digest = value
        next
      }
      in_asset && /^    \}[,]?[[:space:]]*$/ {
        if (name == wanted) {
          print "asset|" digest
        }
        in_asset = 0
      }
    ' "$release_json"
  )"
  asset_count="$(printf '%s\n' "$asset_matches" | sed -n '/^asset|/p' | wc -l | tr -d ' ')"
  [ "$asset_count" = "1" ] || \
    fail "GitHub immutable release must contain exactly one $MANIFEST_NAME asset; found $asset_count"
  manifest_digest="$(printf '%s\n' "$asset_matches" | sed -n 's/^asset|//p')"
  case "$manifest_digest" in
    sha256:*) manifest_sha256="${manifest_digest#sha256:}" ;;
    *) fail "GitHub $MANIFEST_NAME asset digest must use sha256" ;;
  esac
  require_sha256 "GitHub $MANIFEST_NAME asset digest" "$manifest_sha256"
  printf '%s\n' "$manifest_sha256"
}

verify_manifest_trust() {
  manifest="$1"
  bundle="$2"
  release_json="$3"

  if [ -n "$MANIFEST_SHA256_PIN" ]; then
    [ "${FROGLET_TRUSTED_MANIFEST_PIN:-0}" = "1" ] || \
      fail "FROGLET_RELEASE_MANIFEST_SHA256 requires explicit FROGLET_TRUSTED_MANIFEST_PIN=1"
    require_sha256 FROGLET_RELEASE_MANIFEST_SHA256 "$MANIFEST_SHA256_PIN"
    verify_file_sha256 "$manifest" "$MANIFEST_SHA256_PIN"
    log "Verified release manifest against the caller-supplied SHA-256 trust pin"
    return 0
  fi

  api_url="$GITHUB_API_BASE_URL/repos/$REPO/releases/tags/$TAG"
  curl_https \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2026-03-10' \
    "$api_url" -o "$release_json"
  api_manifest_sha256="$(immutable_release_manifest_sha256 "$release_json" "$TAG")"
  verify_file_sha256 "$manifest" "$api_manifest_sha256"
  log "Verified release manifest against GitHub's immutable release asset digest"

  # The immutable release digest is sufficient for a dependency-free install.
  # `auto` additionally binds the manifest to the pinned workflow when gh is
  # present; `required` makes that stronger provenance check a hard gate; `off`
  # is useful for deliberately dependency-minimal hosts and deterministic
  # clean-host validation.
  case "$GH_ATTESTATION_MODE" in
    auto|required|off) ;;
    *) fail "FROGLET_GH_ATTESTATION_MODE must be auto, required, or off" ;;
  esac
  if [ "$GH_ATTESTATION_MODE" = "off" ]; then
    log "Skipped optional GitHub artifact-attestation verification by configuration"
    return 0
  fi
  if ! command -v gh >/dev/null 2>&1; then
    [ "$GH_ATTESTATION_MODE" != "required" ] || \
      fail "GitHub artifact-attestation verification was required but gh is unavailable"
    log "GitHub CLI not found; skipped optional artifact-attestation verification"
    return 0
  fi
  claimed_source_revision="$(manifest_value source_revision "$manifest")"
  require_source_revision "$claimed_source_revision"
  download_to_file "$DOWNLOAD_BASE_URL/$TAG/$ATTESTATION_NAME" "$bundle"
  if ! GH_FORCE_TTY=never gh attestation verify "$manifest" \
    --bundle "$bundle" \
    --repo "$REPO" \
    --signer-workflow "$REPO/.github/workflows/release.yml" \
    --source-ref "refs/tags/$TAG" \
    --source-digest "$claimed_source_revision" \
    --deny-self-hosted-runners >/dev/null; then
    fail "release manifest attestation verification failed"
  fi
  log "Verified release manifest through its GitHub artifact attestation"
}

validate_manifest() {
  manifest="$1"

  schema="$(manifest_value schema "$manifest")"
  manifest_release="$(manifest_value release "$manifest")"
  manifest_repository="$(manifest_value source_repository "$manifest")"
  source_revision="$(manifest_value source_revision "$manifest")"
  source_ref="$(manifest_value source_ref "$manifest")"
  signer_workflow="$(manifest_value attestation_signer_workflow "$manifest")"
  bootstrap_asset="$(manifest_value agent_bootstrap_asset "$manifest")"
  bootstrap_sha256="$(manifest_value agent_bootstrap_sha256 "$manifest")"

  [ "$schema" = "froglet.release-bundle.v1" ] || fail "unsupported release manifest schema: $schema"
  [ "$manifest_release" = "$TAG" ] || fail "release manifest tag does not match requested release $TAG"
  [ "$manifest_repository" = "$REPO" ] || fail "release manifest repository does not match $REPO"
  require_source_revision "$source_revision"
  [ "$source_ref" = "refs/tags/$TAG" ] || fail "release manifest source_ref does not match $TAG"
  [ "$signer_workflow" = "$REPO/.github/workflows/release.yml" ] || \
    fail "release manifest signer workflow does not match the trusted release workflow"
  [ "$bootstrap_asset" = "agent-bootstrap.sh" ] || \
    fail "release manifest agent_bootstrap_asset must be agent-bootstrap.sh"
  require_sha256 agent_bootstrap_sha256 "$bootstrap_sha256"

  require_immutable_image image_provider "$(manifest_value image_provider "$manifest")"
  require_immutable_image image_runtime "$(manifest_value image_runtime "$manifest")"
  require_immutable_image image_dual "$(manifest_value image_dual "$manifest")"
  require_immutable_image image_mcp "$(manifest_value image_mcp "$manifest")"

  asset_key="binary_froglet_node_${PLATFORM}_${ARCH}_asset"
  digest_key="binary_froglet_node_${PLATFORM}_${ARCH}_sha256"
  ASSET_NAME="$(manifest_value "$asset_key" "$manifest")"
  ASSET_SHA256="$(manifest_value "$digest_key" "$manifest")"
  expected_asset="froglet-node-${TAG}-${PLATFORM}-${ARCH}.tar.gz"
  [ "$ASSET_NAME" = "$expected_asset" ] || \
    fail "release manifest asset does not match requested platform: $ASSET_NAME"
  require_sha256 "$digest_key" "$ASSET_SHA256"
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
validate_tag "$TAG"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/froglet-install.XXXXXX")"
cleanup() {
  rm -rf "$TMP_DIR"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$INSTALL_DIR"

MANIFEST_PATH="$TMP_DIR/$MANIFEST_NAME"
ATTESTATION_PATH="$TMP_DIR/$ATTESTATION_NAME"
RELEASE_METADATA_PATH="$TMP_DIR/github-release.json"
download_to_file "$DOWNLOAD_BASE_URL/$TAG/$MANIFEST_NAME" "$MANIFEST_PATH"
verify_manifest_trust "$MANIFEST_PATH" "$ATTESTATION_PATH" "$RELEASE_METADATA_PATH"
validate_manifest "$MANIFEST_PATH"

asset_url="$DOWNLOAD_BASE_URL/$TAG/$ASSET_NAME"
download_to_file "$asset_url" "$TMP_DIR/$ASSET_NAME"
verify_file_sha256 "$TMP_DIR/$ASSET_NAME" "$ASSET_SHA256"

tar -xzf "$TMP_DIR/$ASSET_NAME" -C "$TMP_DIR"
[ -f "$TMP_DIR/froglet-node" ] || fail "archive $ASSET_NAME did not contain froglet-node"
install_binary "$TMP_DIR/froglet-node" "$INSTALL_DIR/froglet-node"
log "Installed froglet-node to $INSTALL_DIR/froglet-node"

if [ -n "$MANIFEST_OUT" ]; then
  mkdir -p "$(dirname "$MANIFEST_OUT")"
  cp "$MANIFEST_PATH" "$MANIFEST_OUT"
  log "Wrote verified release manifest to $MANIFEST_OUT"
fi

path_hint
