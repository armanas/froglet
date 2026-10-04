#!/usr/bin/env bash
# Deploy an isolated, free-only Fly candidate and check public reachability and
# anonymous refusal. This does not qualify computation, marketplace admission,
# image provenance, or production readiness. No marketplace registration occurs.
set -euo pipefail
umask 077

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image=""
expected_version=""
organization=""
region=fra
keep=0
prepare_only=0
app_prefix=froglet-check
max_deals=0
max_quotes=0
max_runtime_ms=0
access_mode=private
evidence_dir=""
app_created=0
failure_reason=""

usage() {
  cat <<'EOF'
Usage: scripts/fly_provider_smoke.sh --image ghcr.io/OWNER/froglet-provider@sha256:DIGEST
       --expected-version VERSION --org ORGANIZATION [--region REGION] [--keep] [--prepare-only]
       [--app-prefix PREFIX] [--evidence-dir DIRECTORY]
       [--access-mode private|invite]
       [--max-deals N] [--max-quotes N] [--max-runtime-ms N]

The image must be an exact digest and VERSION must match node capabilities.
The operator must separately verify release/image provenance before deployment.
Default region: fra. Default cumulative allowances: zero (no new execution).
Explicit candidate ceilings: deals/quotes <= 1000; runtime <= 5000000 ms.
Creates one shared-CPU/1 GiB Machine and a 3 GiB persistent data volume.
Private access mode requires the owner credential for new work; owner-control
routes share the public listener and require bearer authentication. They are
not isolated by a separate network listener. No runtime port is published.
Explicit invite mode permits separately issued bounded invitations. This
helper does not issue credentials or test authenticated invitation execution.

--prepare-only writes the configuration without calling Fly or curl and does
not require --org. Actual deployment requires an explicit billing organization.
Without --keep, only an app successfully created by this run is destroyed.
With --keep, the app and volume remain billable, including after a failed check.
Cloud limits are not a spending cap. No paid rails or public registration.
EOF
}
fail() { failure_reason="$*"; printf 'fly_provider_smoke: %s\n' "$failure_reason" >&2; exit 1; }
need_value() { [[ $# -ge 2 && -n "$2" ]] || fail "missing value for $1"; }
while [[ $# -gt 0 ]]; do
  case "$1" in
    --image) need_value "$@"; image="$2"; shift 2 ;;
    --expected-version) need_value "$@"; expected_version="$2"; shift 2 ;;
    --org) need_value "$@"; organization="$2"; shift 2 ;;
    --region) need_value "$@"; region="$2"; shift 2 ;;
    --app-prefix) need_value "$@"; app_prefix="$2"; shift 2 ;;
    --evidence-dir) need_value "$@"; evidence_dir="$2"; shift 2 ;;
    --max-deals) need_value "$@"; max_deals="$2"; shift 2 ;;
    --max-quotes) need_value "$@"; max_quotes="$2"; shift 2 ;;
    --max-runtime-ms) need_value "$@"; max_runtime_ms="$2"; shift 2 ;;
    --access-mode) need_value "$@"; access_mode="$2"; shift 2 ;;
    --keep) keep=1; shift ;;
    --prepare-only) prepare_only=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) fail "unknown argument: $1" ;;
  esac
done
[[ "$image" =~ ^ghcr\.io/[a-z0-9._-]+/froglet-provider@sha256:[0-9a-f]{64}$ ]] \
  || fail "--image must be an exact ghcr.io/OWNER/froglet-provider@sha256:<64 lowercase hex> reference"
[[ "$expected_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.+-][A-Za-z0-9.-]+)?$ ]] \
  || fail "--expected-version must be the node's package version (without v)"
[[ "$region" =~ ^[a-z]{3}$ ]] || fail "invalid region"
[[ "$access_mode" == private || "$access_mode" == invite ]] || fail "access mode must be private or invite"
if [[ -n "$organization" ]]; then
  [[ "$organization" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ && ${#organization} -le 64 ]] \
    || fail "invalid organization slug"
fi
[[ "$prepare_only" == 1 || -n "$organization" ]] || fail "--org is required for deployment"
[[ "$app_prefix" =~ ^[a-z][a-z0-9-]*[a-z0-9]$ && ${#app_prefix} -le 15 ]] \
  || fail "app prefix must be 2-15 lowercase letters/digits/hyphens, with no trailing hyphen"
normalize_allowance() {
  local label="$1" value="$2" ceiling="$3"
  [[ "$value" =~ ^[0-9]{1,7}$ ]] || fail "$label must be a nonnegative integer <= $ceiling"
  value=$((10#$value))
  [[ "$value" -le "$ceiling" ]] || fail "$label exceeds candidate ceiling $ceiling"
  printf '%s' "$value"
}
max_deals="$(normalize_allowance --max-deals "$max_deals" 1000)"
max_quotes="$(normalize_allowance --max-quotes "$max_quotes" 1000)"
max_runtime_ms="$(normalize_allowance --max-runtime-ms "$max_runtime_ms" 5000000)"
command -v python3 >/dev/null 2>&1 || fail "python3 is required to record evidence"

suffix="$(date -u +%Y%m%d%H%M%S)"
app_name="${app_prefix}-${suffix}"
public_url="https://${app_name}.fly.dev"
evidence_dir="${evidence_dir:-$repo_root/_tmp/fly_provider_smoke/$suffix}"
[[ ! -e "$evidence_dir" ]] || fail "evidence directory already exists: $evidence_dir"
mkdir -p "$evidence_dir"
evidence_dir="$(cd "$evidence_dir" && pwd)"
config_path="$evidence_dir/fly.toml"
printf 'check\tstatus\tdetail\n' > "$evidence_dir/checks.tsv"

cat > "$config_path" <<EOF
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
  FROGLET_RUNTIME_LISTEN_ADDR = "127.0.0.1:0"
  FROGLET_RUNTIME_ALLOW_NON_LOOPBACK = "false"
  FROGLET_NETWORK_MODE = "clearnet"
  FROGLET_PAYMENT_BACKEND = "none"
  FROGLET_PROVIDER_REQUIRE_PAYMENT = "false"
  FROGLET_PROVIDER_ACCESS_MODE = "${access_mode}"
  FROGLET_PRICE_EVENTS_QUERY = "0"
  FROGLET_PRICE_EXEC_WASM = "0"
  FROGLET_REQUESTER_SPEND_BUDGET_MSAT = "0"
  FROGLET_PROVIDER_MAX_TOTAL_DEALS = "${max_deals}"
  FROGLET_PROVIDER_MAX_TOTAL_QUOTES = "${max_quotes}"
  FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS = "${max_runtime_ms}"
  FROGLET_PROVIDER_MIN_FREE_BYTES = "536870912"
  FROGLET_PROVIDER_MAX_DATABASE_BYTES = "134217728"
  FROGLET_PUBLIC_REQUEST_QUOTA = "120"
  FROGLET_PUBLIC_WRITE_QUOTA_WINDOW_SECS = "60"
  FROGLET_PROCESS_CONCURRENCY = "1"
  FROGLET_WASM_CONCURRENCY_LIMIT = "1"
  FROGLET_WASM_MODULE_CACHE_CAPACITY = "8"
  FROGLET_EXECUTION_TIMEOUT_SECS = "5"
  FROGLET_ALLOW_UNSANDBOXED_PYTHON = "false"
  FROGLET_GPU_ENABLED = "false"

[http_service]
  internal_port = 8080
  force_https = true
  auto_stop_machines = false
  auto_start_machines = true
  min_machines_running = 1

  [http_service.concurrency]
    type = "requests"
    soft_limit = 8
    hard_limit = 16

[[vm]]
  cpu_kind = "shared"
  cpus = 1
  memory = "1gb"

[[mounts]]
  source = "froglet_data"
  destination = "/data"
EOF

record_evidence() {
  python3 - "$evidence_dir" "$1" "$image" "$expected_version" "$app_name" "$public_url" "$keep" "$organization" "$failure_reason" <<'PY'
import csv, hashlib, json, sys
from pathlib import Path
root = Path(sys.argv[1])
with (root / 'checks.tsv').open(encoding='utf-8') as source:
    rows = list(csv.DictReader(source, delimiter='\t'))
result = {
    'schema': 'froglet.fly-candidate-smoke.v1',
    'status': sys.argv[2], 'requested_image': sys.argv[3],
    'expected_version': sys.argv[4], 'app': sys.argv[5], 'public_url': sys.argv[6],
    'keep_requested': sys.argv[7] == '1', 'checks': rows,
    'organization': sys.argv[8] or None,
    'failure_reason': sys.argv[9] or None,
    'config_sha256': hashlib.sha256((root / 'fly.toml').read_bytes()).hexdigest(),
    'scope': 'public reachability and anonymous refusal only',
    'not_verified': ['image/release provenance', 'authenticated effective safeguards',
        'remote computation and signed execution receipts', 'restart/recovery',
        'marketplace admission', 'production readiness'],
    'owner_controls': 'bearer authenticated on the public provider listener',
}
(root / 'qualification.json').write_text(json.dumps(result, indent=2) + '\n')
PY
}
record_evidence prepared
if [[ "$prepare_only" == 1 ]]; then
  printf 'Prepared %s; no Fly or HTTP operation occurred.\n' "$config_path"
  exit 0
fi

if command -v fly >/dev/null 2>&1; then fly_cmd=fly
elif command -v flyctl >/dev/null 2>&1; then fly_cmd=flyctl
else fail "install flyctl before deployment"; fi
for tool in curl jq; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done
cleanup() {
  local rc=$?
  trap - EXIT
  if [[ "$app_created" == 1 ]]; then
    if [[ "$keep" == 1 ]]; then
      printf 'fly_provider_smoke: retained %s; app/volume remain billable.\n' "$app_name"
      printf 'Cleanup: %s apps destroy %s --yes\n' "$fly_cmd" "$app_name"
    elif "$fly_cmd" apps destroy "$app_name" --yes > "$evidence_dir/cleanup.log" 2>&1; then
      printf 'cleanup\tpass\tcreated app destroyed\n' >> "$evidence_dir/checks.tsv"
    else
      printf 'cleanup\tfail\tcreated app could not be destroyed; inspect cleanup.log\n' >> "$evidence_dir/checks.tsv"
      printf 'fly_provider_smoke: cleanup failed for %s; resources may remain billable.\n' "$app_name" >&2
      failure_reason="created app cleanup failed; resources may remain billable"
      rc=1
    fi
  fi
  if [[ "$rc" != 0 ]]; then record_evidence failed
  else record_evidence passed; fi
  exit "$rc"
}
trap cleanup EXIT
"$fly_cmd" auth whoami > /dev/null 2>&1 || fail "Fly login is required"
"$fly_cmd" apps create "$app_name" --org "$organization" --yes > "$evidence_dir/create.log" 2>&1 \
  || fail "app creation failed; no app will be destroyed"
app_created=1
"$fly_cmd" volumes create froglet_data --region "$region" --size 3 --app "$app_name" --yes \
  > "$evidence_dir/volume.log" 2>&1
"$fly_cmd" deploy --ha=false --yes --app "$app_name" --config "$config_path" --image "$image" \
  > "$evidence_dir/deploy.log" 2>&1

request() {
  local method="$1" path="$2" output="$3"
  if [[ "$method" == POST ]]; then
    curl --disable --silent --show-error --max-time 15 --request POST \
      --header 'Content-Type: application/json' --data-binary '{}' \
      --output "$output" --write-out '%{http_code}' "$public_url$path"
  else
    curl --disable --silent --show-error --max-time 15 --request GET \
      --output "$output" --write-out '%{http_code}' "$public_url$path"
  fi
}
# Only the newly created candidate is queried. Do not follow redirects.
deadline=$((SECONDS + 300))
ready=0
while [[ $SECONDS -lt $deadline ]]; do
  if code="$(request GET /v1/node/capabilities "$evidence_dir/capabilities.json")" && [[ "$code" == 200 ]]; then
    ready=1; break
  fi
  sleep 5
done
[[ "$ready" == 1 ]] || fail "candidate capabilities did not become reachable within 5 minutes"
jq -e --arg expected "$expected_version" --arg url "$public_url" \
  '.version == $expected and (.identity.node_id | type == "string" and test("^[0-9a-f]{64}$")) and .transports.clearnet.enabled == true and .transports.clearnet.url == $url' \
  "$evidence_dir/capabilities.json" > /dev/null \
  || fail "capabilities version, identity or advertised URL mismatch"
printf 'capabilities\tpass\texpected version, identity and exact HTTPS origin\n' >> "$evidence_dir/checks.tsv"
code="$(request GET /health "$evidence_dir/health.json")"
[[ "$code" == 200 ]] && jq -e '.status == "ok" and .service == "froglet"' "$evidence_dir/health.json" > /dev/null \
  || fail "health response mismatch"
printf 'health\tpass\tFroglet HTTP 200 health response\n' >> "$evidence_dir/checks.tsv"

check_refusal() {
  local method="$1" path="$2" expected="$3" name="$4" code
  code="$(request "$method" "$path" "$evidence_dir/$name.json")"
  [[ "$code" == "$expected" ]] || fail "$method $path returned $code; expected $expected"
  if [[ "$expected" == 403 ]]; then
    jq -e '.code == "provider_access_required"' "$evidence_dir/$name.json" > /dev/null \
      || fail "$path did not report private provider admission"
  fi
  printf '%s\tpass\tanonymous %s %s refused with HTTP %s\n' "$name" "$method" "$path" "$code" \
    >> "$evidence_dir/checks.tsv"
}
check_refusal GET /v1/provider/usage 401 owner-usage
check_refusal POST /v1/provider/control 401 owner-control
check_refusal POST /v1/provider/quotes 403 anonymous-quote
check_refusal POST /v1/provider/deals 403 anonymous-deal
printf 'Candidate reachability/refusal checks passed: %s\n' "$public_url"
printf 'Evidence: %s\n' "$evidence_dir"
printf 'Computation, effective allowances, image provenance and marketplace admission still require separate qualification.\n'
