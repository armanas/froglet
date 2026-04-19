#!/usr/bin/env bash
# Local Linux equivalent of the CI "Strict Checks" job. Runs inside the
# rust:1.91-bookworm container so macOS dev machines exercise the same
# target_os, kernel, and toolchain CI uses. Catches unused-import-under-
# linux-cfg, case-sensitivity, Linux-only clippy lints, and general drift
# that the macOS release-gate silently skips.
#
# Covers:
#   cargo fmt --check
#   cargo test --all-targets (with RUSTFLAGS=-D warnings)
#   cargo clippy --all-targets -- -D warnings
#
# Explicitly does NOT cover:
#   - Privileged sandbox tests (CAP_SYS_ADMIN; those stay #[ignore])
#   - Docker-Compose smoke (needs docker-in-docker)
#   - Node/Python side of strict_checks (runs fine on macOS)
#
# Usage:
#   ./scripts/ci_linux.sh           # full run
#   ./scripts/ci_linux.sh fmt       # cargo fmt only
#   ./scripts/ci_linux.sh test      # cargo test only
#   ./scripts/ci_linux.sh clippy    # cargo clippy only
#
# First run downloads rust:1.91-bookworm (~1 GB) and compiles everything
# cold; subsequent runs share the cached volume and finish in ~60s.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE_VOLUME="${FROGLET_CI_LINUX_CACHE:-froglet-ci-linux-cache}"
RUST_IMAGE="${FROGLET_CI_LINUX_IMAGE:-rust:1.91-bookworm}"

die() { echo "ci_linux: $*" >&2; exit 1; }

command -v docker >/dev/null 2>&1 || die "docker is required"

mode="${1:-all}"

case "$mode" in
  fmt)    inner_cmd='cargo fmt --all --check' ;;
  test)   inner_cmd='rustup component add clippy >/dev/null 2>&1 || true; CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets' ;;
  clippy) inner_cmd='rustup component add clippy >/dev/null 2>&1 && cargo clippy --all-targets -- -D warnings' ;;
  all)    inner_cmd='
    set -euo pipefail
    echo "[ci_linux] cargo fmt --check"
    cargo fmt --all --check
    echo "[ci_linux] rustup component add clippy"
    rustup component add clippy >/dev/null 2>&1
    echo "[ci_linux] cargo test --all-targets (warnings denied)"
    CARGO_INCREMENTAL=0 RUSTFLAGS="-D warnings" cargo test --all-targets
    echo "[ci_linux] cargo clippy --all-targets (warnings denied)"
    cargo clippy --all-targets -- -D warnings
  ' ;;
  *) die "unknown mode: $mode (allowed: all|fmt|test|clippy)" ;;
esac

# Ensure cache volume exists (shared target/ + cargo registry across runs).
docker volume inspect "$CACHE_VOLUME" >/dev/null 2>&1 \
  || docker volume create "$CACHE_VOLUME" >/dev/null

echo "[ci_linux] mode=${mode}  image=${RUST_IMAGE}  cache=${CACHE_VOLUME}"

docker run --rm \
  --platform linux/amd64 \
  -v "${REPO_ROOT}:/workspace" \
  -v "${CACHE_VOLUME}:/cache" \
  -e CARGO_HOME=/cache/cargo \
  -e CARGO_TARGET_DIR=/cache/target \
  -w /workspace \
  "$RUST_IMAGE" \
  bash -c "$inner_cmd"

echo "[ci_linux] mode=${mode} OK"
