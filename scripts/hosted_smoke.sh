#!/usr/bin/env bash
# Hosted-URL smoke checks. Each check validates reachability AND response
# shape — an HTTP 200 that returns an HTML 404 from a parked domain, or a
# health endpoint that returns the wrong service, must fail this script.
#
# Called by scripts/release_gate.sh as the `hosted` step. Can also be run
# directly once public URLs exist.
#
# Closes [TODO.md Order 16](../TODO.md). See
# [docs/PAYMENT_MATRIX.md](../docs/PAYMENT_MATRIX.md) for the related
# payment-surface observability contract.
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
                                    (checks HTTP 200, text/html, body contains "Froglet")
  FROGLET_HOSTED_PROVIDER_URL       Hosted provider base URL
                                    (checks /health shape, /v1/node/capabilities shape,
                                     /v1/node/identity shape, /v1/openapi.yaml prefix)
  FROGLET_HOSTED_RUNTIME_URL        Hosted runtime base URL
                                    (checks /health shape)
  FROGLET_HOSTED_SMOKE_STRICT=1     Exit nonzero (2) when any check is pending
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

# fetch <url> <body_out_path> → prints '<http_code> <content_type>' to stdout,
# returns 0 on any HTTP response, returns 1 on network-level failure.
fetch() {
  local url="$1" body_out="$2"
  curl --silent --location \
       --max-time 15 \
       --output "$body_out" \
       --write-out '%{http_code} %{content_type}' \
       "$url" 2>/dev/null
}

check_docs_url() {
  local url="$1"
  local body_file; body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  local meta code content_type
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "docs url: network error reaching $url"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  content_type="${meta#* }"

  if [[ "$code" != "200" ]]; then
    report_fail "docs url: HTTP $code from $url"
    rm -f "$body_file"; return
  fi
  if [[ "$content_type" != text/html* ]]; then
    report_fail "docs url: unexpected content-type '$content_type' (expected text/html) from $url"
    rm -f "$body_file"; return
  fi
  if ! grep -qi "froglet" "$body_file"; then
    report_fail "docs url: body at $url does not mention 'Froglet' (is this a parked page?)"
    rm -f "$body_file"; return
  fi
  rm -f "$body_file"
  report_pass "docs url: $url"
}

check_health_shape() {
  local label="$1" url="$2"
  local body_file; body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  local meta code
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "$label: network error reaching $url"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  if [[ "$code" != "200" ]]; then
    report_fail "$label: HTTP $code from $url"
    rm -f "$body_file"; return
  fi
  if ! python3 -c "$(cat <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as f:
        d = json.load(f)
except Exception as e:
    print(f"not JSON: {e}", file=sys.stderr); sys.exit(1)
if d.get("status") != "ok":
    print(f"status != 'ok' (got {d.get('status')!r})", file=sys.stderr); sys.exit(1)
if d.get("service") != "froglet":
    print(f"service != 'froglet' (got {d.get('service')!r})", file=sys.stderr); sys.exit(1)
PY
)" "$body_file"; then
    report_fail "$label: body shape wrong from $url"
    rm -f "$body_file"; return
  fi
  rm -f "$body_file"
  report_pass "$label: $url"
}

check_capabilities_shape() {
  local url="$1"
  local body_file; body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  local meta code
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "capabilities: network error reaching $url"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  if [[ "$code" != "200" ]]; then
    report_fail "capabilities: HTTP $code from $url"
    rm -f "$body_file"; return
  fi
  if ! python3 -c "$(cat <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as f:
        d = json.load(f)
except Exception as e:
    print(f"not JSON: {e}", file=sys.stderr); sys.exit(1)
if d.get("api_version") != "v1":
    print(f"api_version != 'v1' (got {d.get('api_version')!r})", file=sys.stderr); sys.exit(1)
ident = d.get("identity") or {}
if not isinstance(ident, dict) or not ident.get("node_id"):
    print("identity.node_id missing or empty", file=sys.stderr); sys.exit(1)
if not d.get("version"):
    print("version field empty", file=sys.stderr); sys.exit(1)
PY
)" "$body_file"; then
    report_fail "capabilities: body shape wrong from $url"
    rm -f "$body_file"; return
  fi
  rm -f "$body_file"
  report_pass "capabilities: $url"
}

check_identity_shape() {
  local url="$1"
  local body_file; body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  local meta code
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "identity: network error reaching $url"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  if [[ "$code" != "200" ]]; then
    report_fail "identity: HTTP $code from $url"
    rm -f "$body_file"; return
  fi
  if ! python3 -c "$(cat <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as f:
        d = json.load(f)
except Exception as e:
    print(f"not JSON: {e}", file=sys.stderr); sys.exit(1)
node_id = d.get("node_id") or ""
pk = d.get("public_key") or ""
if not node_id:
    print("node_id missing or empty", file=sys.stderr); sys.exit(1)
if not pk or len(pk) < 32:
    print(f"public_key missing or too short (len={len(pk)})", file=sys.stderr); sys.exit(1)
PY
)" "$body_file"; then
    report_fail "identity: body shape wrong from $url"
    rm -f "$body_file"; return
  fi
  rm -f "$body_file"
  report_pass "identity: $url"
}

check_openapi_shape() {
  local url="$1"
  local body_file; body_file="$(mktemp -t froglet-hosted-smoke.XXXXXX)"
  local meta code
  if ! meta="$(fetch "$url" "$body_file")"; then
    report_fail "openapi: network error reaching $url"
    rm -f "$body_file"; return
  fi
  code="${meta%% *}"
  if [[ "$code" != "200" ]]; then
    report_fail "openapi: HTTP $code from $url"
    rm -f "$body_file"; return
  fi
  if ! head -5 "$body_file" | grep -qE '^openapi:\s*[0-9]'; then
    report_fail "openapi: body does not start with 'openapi:' at $url"
    rm -f "$body_file"; return
  fi
  rm -f "$body_file"
  report_pass "openapi: $url"
}

# ── Docs ──────────────────────────────────────────────────────────────────
if [[ -n "$docs_url" ]]; then
  check_docs_url "$docs_url"
else
  report_pending "docs url smoke is waiting for FROGLET_DOCS_URL"
fi

# ── Hosted provider (public router) ───────────────────────────────────────
if [[ -n "$provider_url" ]]; then
  base="${provider_url%/}"
  check_health_shape "hosted provider health" "${base}/health"
  check_capabilities_shape "${base}/v1/node/capabilities"
  check_identity_shape "${base}/v1/node/identity"
  check_openapi_shape "${base}/v1/openapi.yaml"
else
  report_pending "hosted provider smoke is waiting for FROGLET_HOSTED_PROVIDER_URL"
fi

# ── Hosted runtime ────────────────────────────────────────────────────────
if [[ -n "$runtime_url" ]]; then
  base="${runtime_url%/}"
  check_health_shape "hosted runtime health" "${base}/health"
else
  report_pending "hosted runtime smoke is waiting for FROGLET_HOSTED_RUNTIME_URL"
fi

# ── Live MCP smoke (tracked as TODO Order 11) ─────────────────────────────
report_pending "live MCP smoke remains blocked on Claude auth and hosted project config (TODO Order 11)"

if [[ "$failed" -ne 0 ]]; then
  exit 1
fi

if [[ "$pending" -ne 0 && "$strict" == "1" ]]; then
  exit 2
fi

exit 0
