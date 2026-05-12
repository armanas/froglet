#!/usr/bin/env bash
# Path D end-to-end smoke: deploy a froglet-provider to Fly.io, register
# it against a Froglet marketplace, and capture evidence that the
# provider was accepted. Used to validate that a no-DNS PaaS shape works
# against the public marketplace before promoting Path D in launch
# copy.
#
# Prerequisites:
#   - `flyctl` (or `fly`) installed and `fly auth login` already done
#   - `curl`, `jq` available
#
# Usage:
#   scripts/fly_provider_smoke.sh \
#     [--marketplace-url URL] \
#     [--image REF] \
#     [--region REGION] \
#     [--keep] \
#     [--app-prefix PREFIX]
#
# Defaults:
#   --marketplace-url https://marketplace.froglet.dev
#   --image           ghcr.io/armanas/froglet-provider:latest
#   --region          iad
#   --app-prefix      froglet-pathd-smoke
#
# When `--keep` is not passed, the Fly app is destroyed on exit (trap),
# so the smoke costs nothing beyond the seconds it ran. With `--keep`,
# the app stays up for manual inspection — destroy it later with
# `fly apps destroy <app>`.

set -euo pipefail

marketplace_url="https://marketplace.froglet.dev"
image="ghcr.io/armanas/froglet-provider:latest"
region="iad"
keep=0
app_prefix="froglet-pathd-smoke"

usage() {
  sed -n '2,32p' "$0" | sed 's/^# \{0,1\}//'
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --marketplace-url) marketplace_url="$2"; shift 2 ;;
    --image)           image="$2";           shift 2 ;;
    --region)          region="$2";          shift 2 ;;
    --keep)            keep=1;               shift   ;;
    --app-prefix)      app_prefix="$2";      shift 2 ;;
    -h|--help)         usage ;;
    *) echo "unknown arg: $1" >&2; usage ;;
  esac
done

# Prefer `fly` (modern) but fall back to `flyctl` (older installs).
if command -v fly >/dev/null 2>&1; then
  fly_cmd=fly
elif command -v flyctl >/dev/null 2>&1; then
  fly_cmd=flyctl
else
  echo "fly_provider_smoke: install flyctl first (https://fly.io/docs/hands-on/install-flyctl/)" >&2
  exit 1
fi

for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "fly_provider_smoke: $tool is required" >&2
    exit 1
  }
done

if ! "$fly_cmd" auth whoami >/dev/null 2>&1; then
  echo "fly_provider_smoke: run '$fly_cmd auth login' first" >&2
  exit 1
fi

# Deterministic-enough app name with a timestamp so re-runs don't
# collide. Fly app names must be 1-30 chars, lowercase + digits +
# hyphens, no trailing hyphen.
suffix=$(date -u +%Y%m%d%H%M%S)
app_name="${app_prefix}-${suffix}"
app_name="${app_name:0:30}"
app_name="${app_name%-}"
public_url="https://${app_name}.fly.dev"

evidence_dir="_tmp/fly_provider_smoke/${suffix}"
mkdir -p "$evidence_dir"

cleanup() {
  local rc=$?
  if [[ $keep -eq 0 ]]; then
    echo "fly_provider_smoke: destroying ${app_name}"
    "$fly_cmd" apps destroy "$app_name" --yes >/dev/null 2>&1 || true
  else
    echo "fly_provider_smoke: --keep set; ${app_name} left running"
    echo "  destroy later with: ${fly_cmd} apps destroy ${app_name} --yes"
  fi
  exit $rc
}
trap cleanup EXIT

cd "$(mktemp -d)"

cat > fly.toml <<EOF
app = "${app_name}"
primary_region = "${region}"

[build]
  image = "${image}"

[env]
  FROGLET_NODE_ROLE = "provider"
  FROGLET_DATA_DIR = "/data"
  FROGLET_IDENTITY_AUTO_GENERATE = "true"
  FROGLET_LISTEN_ADDR = "0.0.0.0:8080"
  FROGLET_PUBLIC_BASE_URL = "${public_url}"
  FROGLET_PAYMENT_BACKEND = "none"

[http_service]
  internal_port = 8080
  force_https = true
  auto_stop_machines = false
  auto_start_machines = true
  min_machines_running = 1

[[mounts]]
  source = "froglet_data"
  destination = "/data"
EOF

echo "fly_provider_smoke: app=${app_name}  url=${public_url}  region=${region}"
echo "fly_provider_smoke: image=${image}"
echo "fly_provider_smoke: marketplace=${marketplace_url}"
echo "fly_provider_smoke: evidence=${evidence_dir}"

"$fly_cmd" apps create "$app_name" --machines >/dev/null
"$fly_cmd" volumes create froglet_data --region "$region" --size 1 --app "$app_name" --yes >/dev/null

# Use --ha=false so we deploy a single machine (smoke test, not HA).
"$fly_cmd" deploy --remote-only --ha=false --app "$app_name" 2>&1 | tee "${OLDPWD}/${evidence_dir}/deploy.log"

# Wait up to 5 minutes for /v1/node/capabilities to come up.
deadline=$(( $(date -u +%s) + 300 ))
while [[ $(date -u +%s) -lt $deadline ]]; do
  if curl -fsS --max-time 5 "${public_url}/v1/node/capabilities" -o "${OLDPWD}/${evidence_dir}/capabilities.json" 2>/dev/null; then
    break
  fi
  sleep 5
done
if [[ ! -s "${OLDPWD}/${evidence_dir}/capabilities.json" ]]; then
  echo "fly_provider_smoke: provider did not become reachable at ${public_url}/v1/node/capabilities within 5 min" >&2
  exit 1
fi

provider_id=$(jq -r '.identity.node_id' "${OLDPWD}/${evidence_dir}/capabilities.json")
advertised=$(jq -r '.transports.clearnet.url' "${OLDPWD}/${evidence_dir}/capabilities.json")
echo "fly_provider_smoke: provider_id=${provider_id}"
echo "fly_provider_smoke: advertised=${advertised}"

if [[ "$advertised" != "${public_url}" ]]; then
  echo "fly_provider_smoke: advertised URL mismatch (got '${advertised}', expected '${public_url}')" >&2
  exit 1
fi

# Wait for /v1/feed to have a descriptor and at least one offer before
# registering — the marketplace's registration path requires both to
# verify provider identity end-to-end.
deadline=$(( $(date -u +%s) + 60 ))
while [[ $(date -u +%s) -lt $deadline ]]; do
  curl -fsS --max-time 5 "${public_url}/v1/feed?limit=100" -o "${OLDPWD}/${evidence_dir}/feed.json" || true
  if [[ -s "${OLDPWD}/${evidence_dir}/feed.json" ]]; then
    n=$(jq '[.artifacts[] | select(.kind == "descriptor")] | length' "${OLDPWD}/${evidence_dir}/feed.json")
    o=$(jq '[.artifacts[] | select(.kind == "offer")] | length' "${OLDPWD}/${evidence_dir}/feed.json")
    if [[ "$n" -ge 1 && "$o" -ge 1 ]]; then
      break
    fi
  fi
  sleep 3
done

echo "fly_provider_smoke: posting registration to ${marketplace_url}/v1/registrations"
curl -fsS -X POST "${marketplace_url}/v1/registrations" \
  -H "content-type: application/json" \
  -d "{\"provider_url\":\"${public_url}\"}" \
  -o "${OLDPWD}/${evidence_dir}/register.json"

reg_status=$(jq -r '.status' "${OLDPWD}/${evidence_dir}/register.json")
reg_provider=$(jq -r '.provider_id' "${OLDPWD}/${evidence_dir}/register.json")
reg_offers=$(jq -r '.offers_seen' "${OLDPWD}/${evidence_dir}/register.json")

if [[ "$reg_status" != "active" ]]; then
  echo "fly_provider_smoke: registration status was '${reg_status}', expected 'active'" >&2
  cat "${OLDPWD}/${evidence_dir}/register.json" >&2
  exit 1
fi
if [[ "$reg_provider" != "$provider_id" ]]; then
  echo "fly_provider_smoke: registered provider_id '${reg_provider}' != advertised '${provider_id}'" >&2
  exit 1
fi

# Marketplace indexer is eventually consistent; poll for the provider
# to show up in the public /v1/providers list. Allow up to 60s.
deadline=$(( $(date -u +%s) + 60 ))
listed=0
while [[ $(date -u +%s) -lt $deadline ]]; do
  if curl -fsS --max-time 5 "${marketplace_url}/v1/providers/${provider_id}" -o "${OLDPWD}/${evidence_dir}/provider_detail.json" 2>/dev/null; then
    listed=1
    break
  fi
  sleep 3
done

cat <<EOF

fly_provider_smoke: SUCCESS

  provider_id   ${provider_id}
  provider_url  ${public_url}
  offers_seen   ${reg_offers}
  indexer_seen  $([[ $listed -eq 1 ]] && echo yes || echo "not yet (eventually consistent)")
  evidence      ${OLDPWD}/${evidence_dir}

Path D is verified end-to-end. Files in evidence dir:
  capabilities.json   /v1/node/capabilities response from the Fly provider
  feed.json           /v1/feed (descriptor + offer)
  register.json       marketplace registration response
  provider_detail.json marketplace /v1/providers/<id> response (if indexed)
  deploy.log          fly deploy output
EOF
