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
  froglet-provider|froglet-runtime|froglet-discovery)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    ensure_dir "$data_dir"
    ;;
esac

exec gosu froglet "$@"
