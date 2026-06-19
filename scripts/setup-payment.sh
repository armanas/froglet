#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rail="${1:-}"
shift || true

out_path=""
verify=1
lightning_mode="${FROGLET_LIGHTNING_MODE:-mock}"

usage() {
  cat <<'EOF'
Usage:
  scripts/setup-payment.sh lightning|stripe|x402 [--out PATH] [--mode mock|lnd_rest|phoenixd] [--no-verify]

Writes an env snippet for one launch payment rail and runs a verification probe.

Lightning modes:
  mock      local stub, no wallet (development)
  lnd_rest  external LND node — hold-invoice escrow (pay-on-success)
  phoenixd  self-custodial ACINQ phoenixd — prepaid (lightning.prepaid.v1),
            no escrow, dead-simple setup with automatic liquidity
EOF
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] || fail "$name is required"
}

require_http_url() {
  local name="$1"
  local value="$2"
  case "$value" in
    http://*|https://*) ;;
    *)
      fail "$name must use http:// or https://"
      ;;
  esac
}

require_stripe_secret_key() {
  local secret_key="${FROGLET_STRIPE_SECRET_KEY:-}"
  if [[ "$secret_key" == sk_test_* ]]; then
    return 0
  fi
  if [[ "$secret_key" == sk_live_* && "${FROGLET_STRIPE_LIVE_CONFIRM:-}" == "fresh" ]]; then
    return 0
  fi
  fail "FROGLET_STRIPE_SECRET_KEY must be sk_test_..., or sk_live_... with FROGLET_STRIPE_LIVE_CONFIRM=fresh"
}

validate_stripe_webhook_secret() {
  local webhook_secret="${FROGLET_STRIPE_WEBHOOK_SECRET:-}"
  [[ -z "$webhook_secret" || "$webhook_secret" == whsec_* ]] || fail "FROGLET_STRIPE_WEBHOOK_SECRET must start with whsec_ when set"
}

normalize_x402_network() {
  local network="${FROGLET_X402_NETWORK:-base}"
  network="$(printf '%s' "$network" | tr '[:upper:]' '[:lower:]')"
  case "$network" in
    base)
      printf '%s\n' "$network"
      ;;
    *)
      fail "FROGLET_X402_NETWORK must be base for the current Froglet x402 implementation"
      ;;
  esac
}

require_x402_wallet_address() {
  local wallet_address="${FROGLET_X402_WALLET_ADDRESS:-}"
  [[ "$wallet_address" =~ ^0x[0-9A-Fa-f]{40}$ ]] || fail "FROGLET_X402_WALLET_ADDRESS must be a 0x-prefixed 20-byte Base address"
}

validate_no_control_chars() {
  local label="$1"
  local value="$2"
  case "$value" in
    *[[:cntrl:]]*)
      fail "$label must not contain newline or control characters"
      ;;
  esac
}

env_line() {
  local name="$1"
  local value="$2"
  [[ "$name" =~ ^[A-Z_][A-Z0-9_]*$ ]] || fail "invalid environment variable name: $name"
  validate_no_control_chars "$name" "$value"
  printf '%s=%q' "$name" "$value"
}

snippet_lines=()

begin_snippet() {
  snippet_lines=()
}

add_env_line() {
  local name="$1"
  local value="$2"
  local line
  line="$(env_line "$name" "$value")"
  snippet_lines+=("$line")
}

add_optional_env_line() {
  local name="$1"
  local value="${!name:-}"
  if [[ -n "$value" ]]; then
    add_env_line "$name" "$value"
  fi
}

write_current_snippet() {
  write_snippet "${snippet_lines[@]}"
  snippet_lines=()
}

write_snippet() {
  validate_no_control_chars "output path" "$out_path"
  mkdir -p "$(dirname "$out_path")"
  : >"$out_path"
  chmod 0600 "$out_path"
  local line
  for line in "$@"; do
    if [[ -n "$line" ]]; then
      printf '%s\n' "$line" >>"$out_path"
    fi
  done
  chmod 0600 "$out_path"
}

print_common_footer() {
  printf 'Wrote payment env snippet to %s\n' "$out_path"
  printf 'Load it with: set -a; . %q; set +a\n' "$out_path"
}

probe_lightning_mock() {
  printf 'Verification: lightning mock mode is configured locally; no wallet probe is required.\n'
}

probe_lightning_lnd_rest() {
  local rest_url="${FROGLET_LIGHTNING_REST_URL:-}"
  local macaroon_path="${FROGLET_LIGHTNING_MACAROON_PATH:-}"
  local tls_cert_path="${FROGLET_LIGHTNING_TLS_CERT_PATH:-}"
  local macaroon_hex
  need_cmd curl
  need_cmd od
  [[ -f "$macaroon_path" ]] || fail "macaroon file not found: $macaroon_path"
  if [[ -n "$tls_cert_path" ]]; then
    [[ -f "$tls_cert_path" ]] || fail "TLS cert file not found: $tls_cert_path"
  fi
  macaroon_hex="$(od -An -vtx1 "$macaroon_path" | tr -d ' \n')"
  if [[ -n "$tls_cert_path" ]]; then
    curl --fail --silent --show-error \
      --cacert "$tls_cert_path" \
      -H "Grpc-Metadata-macaroon: $macaroon_hex" \
      "$rest_url/v1/getinfo" >/dev/null
  else
    curl --fail --silent --show-error \
      -H "Grpc-Metadata-macaroon: $macaroon_hex" \
      "$rest_url/v1/getinfo" >/dev/null
  fi
  printf 'Verification: LND REST endpoint responded to /v1/getinfo.\n'
}

probe_lightning_phoenixd() {
  local url="${FROGLET_LIGHTNING_PHOENIXD_URL:-}"
  local password="${FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD:-}"
  local response
  need_cmd curl
  need_cmd python3
  # phoenixd uses HTTP Basic auth with an empty username.
  response="$(
    curl --fail --silent --show-error \
      -u ":$password" \
      "$url/getinfo"
  )" || fail "phoenixd /getinfo probe failed"
  printf '%s' "$response" | python3 -c '
import json
import sys

payload = json.load(sys.stdin)
if not payload.get("nodeId"):
    raise SystemExit("phoenixd /getinfo response missing nodeId")
' || fail "phoenixd /getinfo response missing nodeId"
  printf 'Verification: phoenixd /getinfo responded with a nodeId.\n'
}

probe_stripe() {
  local secret_key="${FROGLET_STRIPE_SECRET_KEY:-}"
  local api_version="${FROGLET_STRIPE_API_VERSION:-2026-04-22.preview}"
  local response
  local livemode
  need_cmd curl
  need_cmd python3
  response="$(
    curl --fail --silent --show-error \
      -H "Authorization: Bearer $secret_key" \
      -H "Stripe-Version: $api_version" \
      "https://api.stripe.com/v1/account"
  )" || fail "Stripe /v1/account probe failed"
  livemode="$(
    printf '%s' "$response" | python3 -c '
import json
import sys

payload = json.load(sys.stdin)
if payload.get("object") != "account":
    sys.stdout.write("not-account")
    raise SystemExit(0)
value = payload.get("livemode")
if value is True:
    sys.stdout.write("true")
elif value is False:
    sys.stdout.write("false")
else:
    sys.stdout.write("missing")
'
  )" || fail "failed to parse Stripe /v1/account response"
  case "$livemode" in
    false)
      printf 'Verification: Stripe account access authenticated and livemode=false on /v1/account.\n'
      ;;
    missing)
      printf 'Verification: Stripe account access authenticated with an sk_test_ key; /v1/account omitted livemode.\n'
      ;;
    true)
      if [[ "$secret_key" == sk_live_* && "${FROGLET_STRIPE_LIVE_CONFIRM:-}" == "fresh" ]]; then
        printf 'Verification: Stripe live account access authenticated and livemode=true on /v1/account.\n'
      else
        fail "Stripe /v1/account reported livemode=true; set FROGLET_STRIPE_LIVE_CONFIRM=fresh before configuring a live key"
      fi
      ;;
    not-account)
      fail "Stripe /v1/account response was not an account object"
      ;;
    *)
      fail "Stripe /v1/account response did not include livemode=false"
      ;;
  esac
}

probe_x402() {
  local facilitator_url="${FROGLET_X402_FACILITATOR_URL:-https://api.cdp.coinbase.com/platform/v2/x402}"
  local status
  local body='{"payload":{}}'
  need_cmd curl
  status="$(
    curl --silent --show-error \
      --output /dev/null \
      --write-out '%{http_code}' \
      -H 'Content-Type: application/json' \
      -d "$body" \
      "$facilitator_url/verify" || true
  )"
  case "$status" in
    200|400|401|403|422)
      printf 'Verification: x402 wallet/network inputs validated locally and facilitator /verify responded with HTTP %s.\n' "$status"
      ;;
    404)
      fail "x402 facilitator /verify endpoint not found at $facilitator_url/verify"
      ;;
    000|"")
      fail "x402 facilitator probe could not reach $facilitator_url/verify"
      ;;
    *)
      fail "x402 facilitator probe failed for $facilitator_url/verify (HTTP $status)"
      ;;
  esac
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      [[ $# -ge 2 ]] || fail "--out requires a value"
      out_path="$2"
      shift 2
      ;;
    --mode)
      [[ $# -ge 2 ]] || fail "--mode requires a value"
      lightning_mode="$2"
      shift 2
      ;;
    --no-verify)
      verify=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

case "$rail" in
  lightning)
    out_path="${out_path:-$repo_root/.froglet/payment/lightning.env}"
    if [[ "$lightning_mode" == "mock" ]]; then
      begin_snippet
      add_env_line FROGLET_PAYMENT_BACKEND lightning
      add_env_line FROGLET_LIGHTNING_MODE mock
      write_current_snippet
      printf 'Required inputs:\n'
      printf '  - none for local lightning mock mode\n'
      if [[ "$verify" -eq 1 ]]; then
        probe_lightning_mock
      fi
    elif [[ "$lightning_mode" == "lnd_rest" ]]; then
      require_env FROGLET_LIGHTNING_REST_URL
      require_env FROGLET_LIGHTNING_MACAROON_PATH
      require_http_url FROGLET_LIGHTNING_REST_URL "${FROGLET_LIGHTNING_REST_URL}"
      begin_snippet
      add_env_line FROGLET_PAYMENT_BACKEND lightning
      add_env_line FROGLET_LIGHTNING_MODE lnd_rest
      add_env_line FROGLET_LIGHTNING_REST_URL "${FROGLET_LIGHTNING_REST_URL}"
      add_env_line FROGLET_LIGHTNING_MACAROON_PATH "${FROGLET_LIGHTNING_MACAROON_PATH}"
      add_env_line FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS "${FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS:-5}"
      add_optional_env_line FROGLET_LIGHTNING_TLS_CERT_PATH
      write_current_snippet
      printf 'Required inputs:\n'
      printf '  - FROGLET_LIGHTNING_REST_URL\n'
      printf '  - FROGLET_LIGHTNING_MACAROON_PATH\n'
      printf '  - FROGLET_LIGHTNING_TLS_CERT_PATH when the endpoint uses https\n'
      if [[ "$verify" -eq 1 ]]; then
        probe_lightning_lnd_rest
      fi
    elif [[ "$lightning_mode" == "phoenixd" ]]; then
      require_env FROGLET_LIGHTNING_PHOENIXD_URL
      require_env FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD
      require_http_url FROGLET_LIGHTNING_PHOENIXD_URL "${FROGLET_LIGHTNING_PHOENIXD_URL}"
      begin_snippet
      add_env_line FROGLET_PAYMENT_BACKEND lightning
      add_env_line FROGLET_LIGHTNING_MODE phoenixd
      add_env_line FROGLET_LIGHTNING_PHOENIXD_URL "${FROGLET_LIGHTNING_PHOENIXD_URL}"
      add_env_line FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD "${FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD}"
      add_env_line FROGLET_LIGHTNING_PHOENIXD_REQUEST_TIMEOUT_SECS "${FROGLET_LIGHTNING_PHOENIXD_REQUEST_TIMEOUT_SECS:-15}"
      add_optional_env_line FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM
      write_current_snippet
      printf 'Required inputs:\n'
      printf '  - FROGLET_LIGHTNING_PHOENIXD_URL (default http://127.0.0.1:9740)\n'
      printf '  - FROGLET_LIGHTNING_PHOENIXD_HTTP_PASSWORD (the http-password from ~/.phoenix/phoenix.conf)\n'
      printf '  - FROGLET_LIGHTNING_PHOENIXD_MAINNET_CONFIRM=1 for non-loopback (real-funds) URLs\n'
      printf 'Note: phoenixd is PREPAID (no escrow): the buyer pays upfront and the\n'
      printf '      receipt carries a cryptographic preimage proof. For pay-on-success\n'
      printf '      escrow, use --mode lnd_rest instead.\n'
      if [[ "$verify" -eq 1 ]]; then
        probe_lightning_phoenixd
      fi
    else
      fail "unsupported lightning mode: $lightning_mode"
    fi
    ;;
  stripe)
    out_path="${out_path:-$repo_root/.froglet/payment/stripe.env}"
    require_env FROGLET_STRIPE_SECRET_KEY
    require_stripe_secret_key
    validate_stripe_webhook_secret
    begin_snippet
    add_env_line FROGLET_PAYMENT_BACKEND stripe
    add_env_line FROGLET_STRIPE_SECRET_KEY "${FROGLET_STRIPE_SECRET_KEY}"
    add_env_line FROGLET_STRIPE_API_VERSION "${FROGLET_STRIPE_API_VERSION:-2026-04-22.preview}"
    add_optional_env_line FROGLET_STRIPE_WEBHOOK_SECRET
    write_current_snippet
    printf 'Required inputs:\n'
    printf '  - FROGLET_STRIPE_SECRET_KEY (sk_test_... by default; sk_live_... only with FROGLET_STRIPE_LIVE_CONFIRM=fresh)\n'
    printf '  - optional FROGLET_STRIPE_API_VERSION\n'
    printf '  - optional FROGLET_STRIPE_WEBHOOK_SECRET for /v1/webhooks/stripe\n'
    if [[ "$verify" -eq 1 ]]; then
      probe_stripe
    fi
    ;;
  x402)
    out_path="${out_path:-$repo_root/.froglet/payment/x402.env}"
    require_env FROGLET_X402_WALLET_ADDRESS
    require_x402_wallet_address
    x402_network="$(normalize_x402_network)"
    require_http_url \
      FROGLET_X402_FACILITATOR_URL \
      "${FROGLET_X402_FACILITATOR_URL:-https://api.cdp.coinbase.com/platform/v2/x402}"
    begin_snippet
    add_env_line FROGLET_PAYMENT_BACKEND x402
    add_env_line FROGLET_X402_WALLET_ADDRESS "${FROGLET_X402_WALLET_ADDRESS}"
    add_env_line FROGLET_X402_NETWORK "${x402_network}"
    add_env_line FROGLET_X402_FACILITATOR_URL "${FROGLET_X402_FACILITATOR_URL:-https://api.cdp.coinbase.com/platform/v2/x402}"
    write_current_snippet
    printf 'Required inputs:\n'
    printf '  - FROGLET_X402_WALLET_ADDRESS (0x-prefixed Base address)\n'
    printf '  - optional FROGLET_X402_NETWORK=base\n'
    printf '  - optional FROGLET_X402_FACILITATOR_URL\n'
    if [[ "$verify" -eq 1 ]]; then
      probe_x402
    fi
    ;;
  ""|-h|--help)
    usage
    exit 0
    ;;
  *)
    fail "unsupported payment rail: $rail"
    ;;
esac

print_common_footer
