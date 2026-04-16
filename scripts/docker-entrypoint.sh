#!/bin/bash
set -euo pipefail

cmd="${1:-}"

ALLOWED_COMMANDS="froglet-node"

ensure_dir() {
  path="$1"
  mode="${2:-700}"
  mkdir -p "$path"
  chown -R froglet:froglet "$path"
  chmod "$mode" "$path"
}

umask 077

case "$cmd" in
  froglet-node)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    data_dir_mode=700
    case "${FROGLET_HOST_READABLE_CONTROL_TOKEN:-}" in
      1|true|TRUE|yes|YES|on|ON)
        data_dir_mode=750
        ;;
    esac
    ensure_dir "$data_dir" "$data_dir_mode"
    ;;
  *)
    echo "docker-entrypoint: unknown command: $cmd" >&2
    echo "allowed: $ALLOWED_COMMANDS" >&2
    exit 1
    ;;
esac

exec gosu froglet "$@"
