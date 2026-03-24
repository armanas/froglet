#!/bin/sh
set -eu

cmd="${1:-}"

ensure_dir() {
  path="$1"
  mkdir -p "$path"
  chown -R froglet:froglet "$path"
  chmod 700 "$path"
}

umask 077

case "$cmd" in
  froglet-discovery)
    db_path="${FROGLET_DISCOVERY_DB_PATH:-/data/discovery.db}"
    ensure_dir "$(dirname "$db_path")"
    ;;
  froglet-provider|froglet-runtime|froglet-operator)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    ensure_dir "$data_dir"
    ;;
esac

exec gosu froglet "$@"
