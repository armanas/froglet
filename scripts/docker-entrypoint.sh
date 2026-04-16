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

host_readable=0
case "${FROGLET_HOST_READABLE_CONTROL_TOKEN:-}" in
  1|true|TRUE|yes|YES|on|ON)
    host_readable=1
    ;;
esac

case "$cmd" in
  froglet-node)
    data_dir="${FROGLET_DATA_DIR:-/data}"
    if [ "$host_readable" = "1" ]; then
      data_dir_mode=750
    else
      data_dir_mode=700
      # Warn loudly when the MCP / OpenClaw / agent use case is likely.
      # Agent integrations read the control token from the host-mounted
      # volume; without this opt-in the file is 0600 root-only and the
      # agent fails to start with a confusing "permission denied". Emit a
      # single clear hint instead of making the user guess.
      cat >&2 <<'EOF'
docker-entrypoint: FROGLET_HOST_READABLE_CONTROL_TOKEN is not set.
  If you are using the Froglet MCP / OpenClaw agent integration from the
  host, the control token file under ./data/runtime/ must be readable by
  your host user. Set:

      export FROGLET_HOST_READABLE_CONTROL_TOKEN=true

  and re-run `docker compose up`. If Froglet is only accessed from inside
  the container (pure in-cluster use), you can ignore this.

EOF
    fi
    ensure_dir "$data_dir" "$data_dir_mode"
    ;;
  *)
    echo "docker-entrypoint: unknown command: $cmd" >&2
    echo "allowed: $ALLOWED_COMMANDS" >&2
    exit 1
    ;;
esac

exec gosu froglet "$@"
