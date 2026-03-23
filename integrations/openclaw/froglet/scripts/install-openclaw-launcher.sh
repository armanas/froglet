#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LAUNCHER_PATH="${SCRIPT_DIR}/openclaw-launcher.mjs"
TARGET_DIR="${1:-$HOME/bin}"
TARGET_PATH="${TARGET_DIR}/openclaw"

mkdir -p "${TARGET_DIR}"
ln -sf "${LAUNCHER_PATH}" "${TARGET_PATH}"

cat <<EOF
Installed Froglet managed-host OpenClaw launcher:
  ${TARGET_PATH}

Optional managed env file:
  \$HOME/.config/froglet/openclaw.env

If the upstream OpenClaw binary is not at /usr/bin/openclaw or /usr/local/bin/openclaw,
set:
  export FROGLET_OPENCLAW_UPSTREAM_BIN=/absolute/path/to/openclaw

Behavior:
  openclaw           -> local Froglet chat loop
  openclaw ...args   -> forwards to the upstream OpenClaw CLI
EOF
