#!/bin/sh
set -eu

cmd="${1:-}"

ensure_dir() {
  path="$1"
  mode="${2:-700}"
  mkdir -p "$path"
  chown -R froglet:froglet "$path"
  chmod "$mode" "$path"
}

umask 077

case "$cmd" in
  froglet-discovery)
    db_path="${FROGLET_DISCOVERY_DB_PATH:-/data/discovery.db}"
    ensure_dir "$(dirname "$db_path")"
    ;;
  froglet-provider|froglet-runtime|froglet-operator)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    data_dir_mode=700
    case "${FROGLET_HOST_READABLE_CONTROL_TOKEN:-}" in
      1|true|TRUE|yes|YES|on|ON)
        data_dir_mode=755
        ;;
    esac
    ensure_dir "$data_dir" "$data_dir_mode"
    ;;
esac

exec gosu froglet "$@"
