#!/usr/bin/env bash
set -euo pipefail

docs_url="${FROGLET_DOCS_URL:-}"
provider_url="${FROGLET_HOSTED_PROVIDER_URL:-}"
runtime_url="${FROGLET_HOSTED_RUNTIME_URL:-}"
strict="${FROGLET_HOSTED_SMOKE_STRICT:-0}"

pending=0
failed=0

usage() {
  cat <<'EOF'
Usage: scripts/hosted_smoke.sh

Environment:
  FROGLET_DOCS_URL                  Published docs URL to smoke
  FROGLET_HOSTED_PROVIDER_URL       Hosted provider base URL
  FROGLET_HOSTED_RUNTIME_URL        Hosted runtime base URL
  FROGLET_HOSTED_SMOKE_STRICT=1     Exit nonzero when any check is still pending
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

report_pass() {
  printf '[pass] %s\n' "$1"
}

report_pending() {
  pending=1
  printf '[pending] %s\n' "$1"
}

report_fail() {
  failed=1
  printf '[fail] %s\n' "$1" >&2
}

check_url() {
  local label="$1"
  local url="$2"

  if curl --fail --silent --show-error --location "$url" >/dev/null; then
    report_pass "$label: $url"
  else
    report_fail "$label: $url"
  fi
}

if [[ -n "$docs_url" ]]; then
  check_url "docs url" "$docs_url"
else
  report_pending "docs url smoke is waiting for FROGLET_DOCS_URL"
fi

if [[ -n "$provider_url" ]]; then
  check_url "hosted provider health" "${provider_url%/}/health"
else
  report_pending "hosted provider smoke is waiting for FROGLET_HOSTED_PROVIDER_URL"
fi

if [[ -n "$runtime_url" ]]; then
  check_url "hosted runtime health" "${runtime_url%/}/health"
else
  report_pending "hosted runtime smoke is waiting for FROGLET_HOSTED_RUNTIME_URL"
fi

report_pending "live MCP smoke remains blocked on Claude auth and hosted project config (Order: 11)"

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

if [[ "$pending" -ne 0 && "$strict" == "1" ]]; then
  exit 2
fi

exit 0
