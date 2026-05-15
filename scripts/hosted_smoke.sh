#!/usr/bin/env bash
# Hosted-surface smoke checks for the v0.2 topology.
#
# Hits the two public services this repo's launch claim depends on:
#
#   marketplace.froglet.dev — read plane (providers, offers, stats) and
#                             registration write plane (/v1/registrations)
#   arbiter.froglet.dev     — Froglet-provider surface (/v1/feed)
#
# Each check validates HTTP shape AND a minimum body assertion — an HTTP
# 200 that returns an HTML 404 from a parked domain, or a JSON endpoint
# that returns nothing recognisable, must FAIL this script.
#
# Exit codes:
#   0  all checks passed
#   1  one or more checks failed
#   2  invocation error (missing curl, bad arg)
#
# Usage:
#   scripts/hosted_smoke.sh
#
# Environment (all optional, defaults match production):
#   FROGLET_MARKETPLACE_URL   default: https://marketplace.froglet.dev
#   FROGLET_ARBITER_URL       default: https://arbiter.froglet.dev
#   FROGLET_SMOKE_TIMEOUT     default: 15 (seconds per request)
set -uo pipefail

marketplace_url="${FROGLET_MARKETPLACE_URL:-https://marketplace.froglet.dev}"
arbiter_url="${FROGLET_ARBITER_URL:-https://arbiter.froglet.dev}"
timeout="${FROGLET_SMOKE_TIMEOUT:-15}"

if ! command -v curl >/dev/null 2>&1; then
  echo "scripts/hosted_smoke.sh: curl is required" >&2
  exit 2
fi

failed=0
passed=0

# Pretty status output: green ✓ on TTY, plain [pass] otherwise.
if [[ -t 1 ]]; then
  ok="\033[32m✓\033[0m"
  ng="\033[31m✗\033[0m"
else
  ok="[pass]"
  ng="[fail]"
fi

report_pass() {
  passed=$((passed + 1))
  printf '%b %s\n' "$ok" "$1"
}
report_fail() {
  failed=$((failed + 1))
  printf '%b %s\n' "$ng" "$1" >&2
}

# fetch <url> <body_out>
# Prints '<http_code> <content_type>' on stdout; non-zero exit on network
# error (DNS, TLS, connection refused).
fetch() {
  local url="$1" body_out="$2"
  curl --silent --location \
       --max-time "$timeout" \
       --output "$body_out" \
       --write-out '%{http_code} %{content_type}' \
       "$url" 2>/dev/null
}

# check <label> <url> <expected_http_code> <body_must_contain>
# body_must_contain may be empty — in which case only HTTP code is checked.
check() {
  local label="$1" url="$2" want_code="$3" body_contains="$4"
  local body_file meta code content_type
  body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "${label}: network error reaching ${url}"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  content_type="${meta#* }"

  if [[ "$code" != "$want_code" ]]; then
    report_fail "${label}: HTTP ${code} (want ${want_code}) from ${url} [content-type: ${content_type}]"
    rm -f "$body_file"; return
  fi

  if [[ -n "$body_contains" ]]; then
    if ! grep -q -- "$body_contains" "$body_file"; then
      report_fail "${label}: HTTP ${code} but body missing expected substring '${body_contains}' (got: $(head -c 200 "$body_file"))"
      rm -f "$body_file"; return
    fi
  fi

  report_pass "${label}: HTTP ${code} ${content_type}"
  rm -f "$body_file"
}

echo "Hosted smoke against:"
echo "  marketplace: ${marketplace_url}"
echo "  arbiter:     ${arbiter_url}"
echo

# ── marketplace.froglet.dev ───────────────────────────────────────────

# Health probe — bare-minimum reachability. Body must be JSON-ish; we
# check for "status" to confirm we got the real /healthz handler and
# not a Cloudflare error page or a parked domain.
check \
  "marketplace /healthz" \
  "${marketplace_url}/healthz" \
  200 \
  "status"

# Provider listing — confirms the read plane is wired to Postgres and
# the JSON response shape is non-empty. The API wraps lists in
# {"items": [...]} and items carry "provider_id"; we check the wrapper
# key, which is always present even when the list is empty.
check \
  "marketplace /v1/providers" \
  "${marketplace_url}/v1/providers?limit=1" \
  200 \
  "items"

# Offers listing — same wrapper shape, different table behind it.
check \
  "marketplace /v1/offers" \
  "${marketplace_url}/v1/offers?limit=1" \
  200 \
  "items"

# Stats — confirms the aggregate queries run.
check \
  "marketplace /v1/stats" \
  "${marketplace_url}/v1/stats" \
  200 \
  ""

# ── arbiter.froglet.dev ───────────────────────────────────────────────
#
# As of v0.2 the arbiter is a real Froglet provider (commit 40c2224),
# not a legacy axum service. There is no /healthz route; /v1/feed is
# the canonical Froglet protocol endpoint and serves as the health
# proxy.

check \
  "arbiter /v1/feed" \
  "${arbiter_url}/v1/feed" \
  200 \
  ""

# ── Summary ───────────────────────────────────────────────────────────

echo
total=$((passed + failed))
echo "Checks: ${passed}/${total} passed"
if [[ $failed -ne 0 ]]; then
  echo "Hosted smoke FAILED — see [fail] lines above." >&2
  exit 1
fi
echo "Hosted smoke PASSED."
exit 0
