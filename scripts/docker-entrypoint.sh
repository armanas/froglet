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
  froglet)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    ensure_dir "$data_dir"
    ;;
  marketplace)
    db_path="${FROGLET_MARKETPLACE_DB_PATH:-/data/marketplace.db}"
    db_dir="$(dirname "$db_path")"
    ensure_dir "$db_dir"
    if [ -e "$db_path" ]; then
      chown froglet:froglet "$db_path"
      chmod 600 "$db_path"
    fi
    ;;
esac

exec gosu froglet "$@"
