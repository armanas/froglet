#!/usr/bin/env bash
# Thin wrapper over the AWS v4 Lightsail Containers API for the froglet-node
# hosted environment. Matches the shape of scripts/cloudflare_dns.sh —
# credentials live in the macOS Keychain, never in environment files or shell
# history, and the script resolves them per invocation.
#
# The deployment spec is templated in ops/lightsail/froglet-node.template.json.
# The script substitutes the image tag and submits the deployment to Lightsail.
#
# Subcommands:
#   verify                        — confirm AWS auth works (sts get-caller-identity)
#   status                        — print the current container service state + endpoint
#   create [--power <size>]       — provision the container service (BILLABLE)
#                                   default power=small ($20/mo). Other powers:
#                                   nano, micro, small, medium, large, xlarge.
#   deploy <image-ref>            — push a new deployment using the template spec
#   logs [--follow]               — tail logs from the running container
#   endpoint                      — print the generated CNAME target for Cloudflare
#   destroy                       — tear down the container service (confirms first)
#
# Environment overrides:
#   FROGLET_AWS_REGION            — default us-east-1
#   FROGLET_AWS_SERVICE_NAME      — default froglet-node
#   FROGLET_AWS_TOKEN_ACCOUNT     — default froglet
#   FROGLET_AWS_ACCESS_KEY_SVC    — default aws-deploy-access-key
#   FROGLET_AWS_SECRET_KEY_SVC    — default aws-deploy-secret-key

set -euo pipefail

REGION="${FROGLET_AWS_REGION:-us-east-1}"
SERVICE_NAME="${FROGLET_AWS_SERVICE_NAME:-froglet-node}"
TOKEN_ACCOUNT="${FROGLET_AWS_TOKEN_ACCOUNT:-froglet}"
ACCESS_KEY_SVC="${FROGLET_AWS_ACCESS_KEY_SVC:-aws-deploy-access-key}"
SECRET_KEY_SVC="${FROGLET_AWS_SECRET_KEY_SVC:-aws-deploy-secret-key}"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMPLATE="${REPO_ROOT}/ops/lightsail/froglet-node.template.json"

die() { echo "deploy_aws: $*" >&2; exit 1; }

read_key() {
  local svc="$1"
  security find-generic-password -a "$TOKEN_ACCOUNT" -s "$svc" -w 2>/dev/null \
    || die "keychain entry not found (account=$TOKEN_ACCOUNT service=$svc)"
}

# Exports AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_DEFAULT_REGION for a
# single aws CLI call, then unsets them. No disk footprint, no env-var leak
# beyond this function's lexical scope.
awscli() {
  local ak sk
  ak="$(read_key "$ACCESS_KEY_SVC")"
  sk="$(read_key "$SECRET_KEY_SVC")"
  AWS_ACCESS_KEY_ID="$ak" \
  AWS_SECRET_ACCESS_KEY="$sk" \
  AWS_DEFAULT_REGION="$REGION" \
    aws "$@"
}

require_aws_cli() {
  command -v aws >/dev/null 2>&1 \
    || die "aws CLI not installed. Install: brew install awscli"
}

cmd_verify() {
  require_aws_cli
  awscli sts get-caller-identity --output json | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
print(f"account: {d['Account']}")
print(f"arn:     {d['Arn']}")
PY
)"
  printf 'region:  %s\n' "$REGION"
}

cmd_status() {
  require_aws_cli
  awscli lightsail get-container-services \
    --query "containerServices[?containerServiceName=='${SERVICE_NAME}']" \
    --output json \
    | python3 -c "$(cat <<'PY'
import json, sys
svcs = json.load(sys.stdin)
if not svcs:
    print("no container service found")
    print(f"run 'scripts/deploy_aws.sh create' to provision")
    sys.exit(0)
s = svcs[0]
print(f"name:         {s['containerServiceName']}")
print(f"state:        {s['state']}")
print(f"power:        {s['power']}")
print(f"scale:        {s['scale']}")
print(f"url:          {s.get('url') or '(not assigned yet)'}")
if s.get('publicDomainNames'):
    print(f"public names: {s['publicDomainNames']}")
cdep = s.get('currentDeployment') or {}
if cdep:
    print(f"current deployment version: {cdep.get('version')}")
    print(f"current deployment state:   {cdep.get('state')}")
    for name, c in (cdep.get('containers') or {}).items():
        print(f"  container '{name}':")
        print(f"    image: {c.get('image')}")
        print(f"    ports: {c.get('ports')}")
PY
)"
}

cmd_create() {
  require_aws_cli
  local power="small"
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --power) power="$2"; shift 2 ;;
      *) die "unknown arg to create: $1" ;;
    esac
  done

  echo "deploy_aws: about to provision Lightsail Container Service:"
  echo "  name:   ${SERVICE_NAME}"
  echo "  region: ${REGION}"
  echo "  power:  ${power}"
  echo "  scale:  1"
  echo
  echo "This starts monthly billing. Approximate cost:"
  case "$power" in
    nano)   echo "  ~\$7/mo  (512 MiB, 0.25 vCPU)"   ;;
    micro)  echo "  ~\$10/mo (1 GiB, 0.25 vCPU)"     ;;
    small)  echo "  ~\$20/mo (2 GiB, 0.5 vCPU)"      ;;
    medium) echo "  ~\$40/mo (4 GiB, 1 vCPU)"        ;;
    large)  echo "  ~\$80/mo (8 GiB, 2 vCPU)"        ;;
    xlarge) echo "  ~\$160/mo (16 GiB, 4 vCPU)"      ;;
    *) die "unknown power: $power" ;;
  esac
  echo
  echo "AWS credits will absorb this if active; otherwise it's real money."
  read -rp "Proceed? [y/N] " answer
  [[ "$answer" == "y" || "$answer" == "Y" ]] || die "cancelled"

  awscli lightsail create-container-service \
    --service-name "$SERVICE_NAME" \
    --power "$power" \
    --scale 1 \
    --output json \
    | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
s = d.get('containerService') or {}
print(f"provisioning started: {s.get('containerServiceName')}")
print(f"state: {s.get('state')}")
print(f"Run 'scripts/deploy_aws.sh status' in ~5 min; state will flip to READY.")
PY
)"
}

cmd_deploy() {
  require_aws_cli
  [ $# -ge 1 ] || die "usage: deploy <image-ref> (e.g. ghcr.io/armanas/froglet-provider:v0.1.0-alpha.1)"
  local image="$1"
  [ -f "$TEMPLATE" ] || die "template missing: $TEMPLATE"

  local rendered
  rendered="$(python3 - "$TEMPLATE" "$image" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    tpl = f.read()
rendered = tpl.replace("{{IMAGE}}", sys.argv[2])
# Sanity-check we produced valid JSON and drop the top-level serviceName
# (Lightsail takes it as a separate --service-name arg).
obj = json.loads(rendered)
obj.pop("serviceName", None)
print(json.dumps(obj))
PY
)"
  local containers public_endpoint
  containers="$(python3 -c "$(cat <<'PY'
import json, sys
d = json.loads(sys.argv[1])
print(json.dumps(d.get("containers", {})))
PY
)" "$rendered")"
  public_endpoint="$(python3 -c "$(cat <<'PY'
import json, sys
d = json.loads(sys.argv[1])
print(json.dumps(d.get("publicEndpoint", {})))
PY
)" "$rendered")"

  echo "deploy_aws: deploying image ${image} to ${SERVICE_NAME}"
  awscli lightsail create-container-service-deployment \
    --service-name "$SERVICE_NAME" \
    --containers "$containers" \
    --public-endpoint "$public_endpoint" \
    --output json \
    | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
s = d.get('containerService') or {}
cdep = s.get('currentDeployment') or s.get('nextDeployment') or {}
print(f"deployment version: {cdep.get('version')}")
print(f"deployment state:   {cdep.get('state')}")
print("Run 'scripts/deploy_aws.sh logs' to follow.")
PY
)"
}

cmd_logs() {
  require_aws_cli
  local follow=0
  if [[ "${1:-}" == "--follow" ]]; then follow=1; fi

  local start_time end_time
  end_time=$(date -u +%s)
  start_time=$((end_time - 600))

  awscli lightsail get-container-log \
    --service-name "$SERVICE_NAME" \
    --container-name froglet \
    --start-time "$start_time" \
    --end-time "$end_time" \
    --output json \
    | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
for entry in d.get("logEvents", []):
    print(f"{entry.get('createdAt')}  {entry.get('message')}")
PY
)"

  if [[ $follow -eq 1 ]]; then
    echo "(--follow is not natively supported by Lightsail; re-run periodically)"
  fi
}

cmd_endpoint() {
  require_aws_cli
  awscli lightsail get-container-services \
    --service-name "$SERVICE_NAME" \
    --output json \
    | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
svcs = d.get("containerServices") or []
if not svcs:
    print("no service")
    sys.exit(1)
s = svcs[0]
url = s.get("url")
if not url:
    print("no public URL yet (service not READY?)")
    sys.exit(1)
# Lightsail URL is https://<hash>.<region>.cs.amazonlightsail.com/
# Strip protocol and trailing slash for the Cloudflare CNAME target.
target = url.replace("https://", "").replace("http://", "").rstrip("/")
print(f"lightsail url:       {url}")
print(f"cloudflare CNAME:    ai.froglet.dev -> {target}")
print(f"  type: CNAME")
print(f"  proxied: true (recommended — Cloudflare fronts TLS)")
PY
)"
}

cmd_destroy() {
  require_aws_cli
  echo "deploy_aws: about to DELETE Lightsail container service ${SERVICE_NAME}"
  echo "This is destructive. Cannot be undone."
  read -rp "Type the service name to confirm: " confirm
  [[ "$confirm" == "$SERVICE_NAME" ]] || die "confirmation mismatch, aborted"

  awscli lightsail delete-container-service \
    --service-name "$SERVICE_NAME" \
    --output json \
    && echo "deleted"
}

help_text() {
  sed -n '3,27p' "$0" | sed 's/^# \{0,1\}//'
}

cmd="${1:-}"
if [ -z "$cmd" ]; then help_text; exit 1; fi
shift || true

case "$cmd" in
  verify)   cmd_verify "$@" ;;
  status)   cmd_status "$@" ;;
  create)   cmd_create "$@" ;;
  deploy)   cmd_deploy "$@" ;;
  logs)     cmd_logs "$@" ;;
  endpoint) cmd_endpoint "$@" ;;
  destroy)  cmd_destroy "$@" ;;
  -h|--help|help) help_text ;;
  *) die "unknown subcommand: $cmd" ;;
esac
