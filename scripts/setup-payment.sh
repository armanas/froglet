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
  scripts/setup-payment.sh lightning|stripe|x402 [--out PATH] [--mode mock|lnd_rest] [--no-verify]

Writes an env snippet for one launch payment rail and runs a verification probe.
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

require_stripe_test_secret_key() {
  local secret_key="${FROGLET_STRIPE_SECRET_KEY:-}"
  [[ "$secret_key" == sk_test_* ]] || fail "FROGLET_STRIPE_SECRET_KEY must be a Stripe test secret key (sk_test_...)"
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

write_snippet() {
  mkdir -p "$(dirname "$out_path")"
  printf '%s\n' "$@" >"$out_path"
}

print_common_footer() {
  printf 'Wrote payment env snippet to %s\n' "$out_path"
  printf 'Load it with: set -a; . %s; set +a\n' "$out_path"
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

probe_stripe() {
  local secret_key="${FROGLET_STRIPE_SECRET_KEY:-}"
  local api_version="${FROGLET_STRIPE_API_VERSION:-2026-03-04.preview}"
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
    true)
      fail "Stripe /v1/account reported livemode=true; public local setup requires a test secret key"
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
      write_snippet \
        "FROGLET_PAYMENT_BACKEND=lightning" \
        "FROGLET_LIGHTNING_MODE=mock"
      printf 'Required inputs:\n'
      printf '  - none for local lightning mock mode\n'
      if [[ "$verify" -eq 1 ]]; then
        probe_lightning_mock
      fi
    elif [[ "$lightning_mode" == "lnd_rest" ]]; then
      require_env FROGLET_LIGHTNING_REST_URL
      require_env FROGLET_LIGHTNING_MACAROON_PATH
      require_http_url FROGLET_LIGHTNING_REST_URL "${FROGLET_LIGHTNING_REST_URL}"
      write_snippet \
        "FROGLET_PAYMENT_BACKEND=lightning" \
        "FROGLET_LIGHTNING_MODE=lnd_rest" \
        "FROGLET_LIGHTNING_REST_URL=${FROGLET_LIGHTNING_REST_URL}" \
        "FROGLET_LIGHTNING_MACAROON_PATH=${FROGLET_LIGHTNING_MACAROON_PATH}" \
        "FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS=${FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS:-5}" \
        "${FROGLET_LIGHTNING_TLS_CERT_PATH:+FROGLET_LIGHTNING_TLS_CERT_PATH=${FROGLET_LIGHTNING_TLS_CERT_PATH}}"
      printf 'Required inputs:\n'
      printf '  - FROGLET_LIGHTNING_REST_URL\n'
      printf '  - FROGLET_LIGHTNING_MACAROON_PATH\n'
      printf '  - FROGLET_LIGHTNING_TLS_CERT_PATH when the endpoint uses https\n'
      if [[ "$verify" -eq 1 ]]; then
        probe_lightning_lnd_rest
      fi
    else
      fail "unsupported lightning mode: $lightning_mode"
    fi
    ;;
  stripe)
    out_path="${out_path:-$repo_root/.froglet/payment/stripe.env}"
    require_env FROGLET_STRIPE_SECRET_KEY
    require_stripe_test_secret_key
    write_snippet \
      "FROGLET_PAYMENT_BACKEND=stripe" \
      "FROGLET_STRIPE_SECRET_KEY=${FROGLET_STRIPE_SECRET_KEY}" \
      "FROGLET_STRIPE_API_VERSION=${FROGLET_STRIPE_API_VERSION:-2026-03-04.preview}"
    printf 'Required inputs:\n'
    printf '  - FROGLET_STRIPE_SECRET_KEY (Stripe test-mode secret key)\n'
    printf '  - optional FROGLET_STRIPE_API_VERSION\n'
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
    write_snippet \
      "FROGLET_PAYMENT_BACKEND=x402" \
      "FROGLET_X402_WALLET_ADDRESS=${FROGLET_X402_WALLET_ADDRESS}" \
      "FROGLET_X402_NETWORK=${x402_network}" \
      "FROGLET_X402_FACILITATOR_URL=${FROGLET_X402_FACILITATOR_URL:-https://api.cdp.coinbase.com/platform/v2/x402}"
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
