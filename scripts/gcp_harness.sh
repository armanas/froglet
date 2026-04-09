#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

: "${FROGLET_GCP_PROJECT:?FROGLET_GCP_PROJECT is required}"
: "${FROGLET_GCP_ZONE:=europe-west6-b}"
: "${FROGLET_GCP_NETWORK:=froglet-harness}"
: "${FROGLET_GCP_SUBNET:=froglet-harness-subnet}"
: "${FROGLET_GCP_REMOTE_USER:=$(whoami)}"
: "${FROGLET_GCP_BUILD_ROLE:=froglet-marketplace}"
: "${FROGLET_GCP_MARKETPLACE_MACHINE_TYPE:=e2-standard-4}"
: "${FROGLET_GCP_PROVIDER_FREE_MACHINE_TYPE:=e2-standard-2}"
: "${FROGLET_GCP_PROVIDER_PAID_MACHINE_TYPE:=e2-standard-4}"
: "${FROGLET_GCP_SETTLEMENT_MACHINE_TYPE:=e2-standard-4}"
: "${FROGLET_GCP_IMAGE_FAMILY:=debian-12}"
: "${FROGLET_GCP_IMAGE_PROJECT:=debian-cloud}"
: "${FROGLET_GCP_HARNESS_STATE_ROOT:=$repo_root/_tmp/gcp-harness}"
: "${FROGLET_GCP_OPENAI_SECRET:=openclaw-api-key}"

ROLES=(
  froglet-marketplace
  froglet-provider-free
  froglet-provider-paid
  froglet-settlement-lab
)

latest_run_file="$FROGLET_GCP_HARNESS_STATE_ROOT/latest-run"

# For continuation subcommands (deploy, seed, run-matrix, run-agentic, collect),
# read the run ID from latest-run if FROGLET_GCP_HARNESS_RUN_ID is not set.
# provision and destroy always generate a fresh run ID by default.
_harness_subcommand="${1:-}"
if [[ -z "${FROGLET_GCP_HARNESS_RUN_ID:-}" && -f "$latest_run_file" ]]; then
  case "$_harness_subcommand" in
    deploy|seed|run-matrix|run-agentic|collect)
      run_id_default="$(cat "$latest_run_file")"
      ;;
    *)
      run_id_default="$(date -u +%Y%m%d%H%M%S)"
      ;;
  esac
else
  run_id_default="$(date -u +%Y%m%d%H%M%S)"
fi
: "${FROGLET_GCP_HARNESS_RUN_ID:=$run_id_default}"
: "${FROGLET_GCP_HARNESS_STATE_DIR:=$FROGLET_GCP_HARNESS_STATE_ROOT/$FROGLET_GCP_HARNESS_RUN_ID}"

STATE_DIR="$FROGLET_GCP_HARNESS_STATE_DIR"
INVENTORY_PATH="$STATE_DIR/inventory.json"
SCENARIO_PATH="$STATE_DIR/scenario.json"
SEED_FREE_PATH="$STATE_DIR/seed-provider-free.json"
SEED_PAID_PATH="$STATE_DIR/seed-provider-paid.json"
TOKENS_DIR="$STATE_DIR/tokens"
RESULTS_DIR="$STATE_DIR/results"
REMOTE_ROOT="/home/$FROGLET_GCP_REMOTE_USER/froglet-harness"
DEPLOY_MARKER="$STATE_DIR/deploy.ok"

mkdir -p "$STATE_DIR" "$STATE_DIR/meta" "$STATE_DIR/pki" "$STATE_DIR/tmp" "$TOKENS_DIR" "$RESULTS_DIR"
printf '%s\n' "$FROGLET_GCP_HARNESS_RUN_ID" >"$latest_run_file"

gcp_ssh_flags() {
  GCP_SSH_FLAGS=(--project="$FROGLET_GCP_PROJECT" --zone="$FROGLET_GCP_ZONE" --quiet)
  if [[ -n "${FROGLET_GCP_SSH_KEY_FILE:-}" ]]; then
    GCP_SSH_FLAGS+=(--ssh-key-file="$FROGLET_GCP_SSH_KEY_FILE")
  fi
}

gcp_ssh() {
  local role="$1"
  shift
  gcp_ssh_flags
  gcloud compute ssh "$role" "${GCP_SSH_FLAGS[@]}" --command="bash -lc $(printf '%q' "$*")"
}

gcp_scp_to() {
  local role="$1"
  local local_path="$2"
  local remote_path="$3"
  gcp_ssh_flags
  gcloud compute scp --recurse "${GCP_SSH_FLAGS[@]}" "$local_path" "$role:$remote_path"
}

gcp_scp_from() {
  local role="$1"
  local remote_path="$2"
  local local_path="$3"
  gcp_ssh_flags
  gcloud compute scp --recurse "${GCP_SSH_FLAGS[@]}" "$role:$remote_path" "$local_path"
}

resolve_openclaw_api_key() {
  if [[ -n "${OPENCLAW_API_KEY:-}" ]]; then
    printf '%s' "$OPENCLAW_API_KEY"
    return 0
  fi
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    printf '%s' "$OPENAI_API_KEY"
    return 0
  fi
  gcloud secrets versions access latest \
    --project="$FROGLET_GCP_PROJECT" \
    --secret="$FROGLET_GCP_OPENAI_SECRET"
}

role_meta_path() {
  echo "$STATE_DIR/meta/$1.json"
}

role_remote_root() {
  echo "$REMOTE_ROOT"
}

role_data_root() {
  local role="$1"
  echo "$REMOTE_ROOT/data/$role"
}

role_machine_type() {
  case "$1" in
    froglet-marketplace) echo "$FROGLET_GCP_MARKETPLACE_MACHINE_TYPE" ;;
    froglet-provider-free) echo "$FROGLET_GCP_PROVIDER_FREE_MACHINE_TYPE" ;;
    froglet-provider-paid) echo "$FROGLET_GCP_PROVIDER_PAID_MACHINE_TYPE" ;;
    froglet-settlement-lab) echo "$FROGLET_GCP_SETTLEMENT_MACHINE_TYPE" ;;
    *) echo "unknown role $1" >&2; exit 1 ;;
  esac
}

role_has_stack() {
  case "$1" in
    froglet-marketplace|froglet-provider-free|froglet-provider-paid) return 0 ;;
    *) return 1 ;;
  esac
}

role_has_provider_service() {
  case "$1" in
    froglet-provider-free|froglet-provider-paid) return 0 ;;
    *) return 1 ;;
  esac
}

role_has_runtime_service() {
  case "$1" in
    froglet-marketplace|froglet-provider-free|froglet-provider-paid) return 0 ;;
    *) return 1 ;;
  esac
}

role_has_public_edge() {
  case "$1" in
    froglet-marketplace|froglet-provider-free|froglet-provider-paid) return 0 ;;
    *) return 1 ;;
  esac
}

role_public_target_port() {
  echo 8080
}

inventory_get() {
  local path_expr="$1"
  python3 - "$INVENTORY_PATH" "$path_expr" <<'PY'
import json, sys
inventory = json.load(open(sys.argv[1], "r", encoding="utf-8"))
value = inventory
for part in sys.argv[2].split("."):
    value = value[part]
if isinstance(value, (dict, list)):
    print(json.dumps(value))
else:
    print(value)
PY
}

require_commands() {
  local missing=0
  for command in gcloud cargo openssl python3 node curl; do
    if ! command -v "$command" >/dev/null 2>&1; then
      echo "missing required command: $command" >&2
      missing=1
    fi
  done
  if [[ "$missing" -ne 0 ]]; then
    exit 1
  fi
}

startup_script() {
  cat <<EOF
#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl git openssl sqlite3 python3 python3-pip python3-venv nodejs npm docker.io build-essential pkg-config libssl-dev clang cmake
systemctl enable --now docker
usermod -aG docker ${FROGLET_GCP_REMOTE_USER} || true
EOF
}

create_instance() {
  local role="$1"
  local machine_type
  machine_type="$(role_machine_type "$role")"

  gcloud compute instances delete "$role" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --quiet 2>/dev/null || true

  gcloud compute instances create "$role" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --network="$FROGLET_GCP_NETWORK" \
    --subnet="$FROGLET_GCP_SUBNET" \
    --machine-type="$machine_type" \
    --image-family="$FROGLET_GCP_IMAGE_FAMILY" \
    --image-project="$FROGLET_GCP_IMAGE_PROJECT" \
    --boot-disk-size=50GB \
    --boot-disk-type=pd-ssd \
    --tags="froglet-harness,$role" \
    --scopes=cloud-platform \
    --metadata=startup-script="$(startup_script)" \
    --quiet
}

wait_for_ssh() {
  local role="$1"
  local deadline=$((SECONDS + 900))
  while [[ $SECONDS -lt $deadline ]]; do
    if gcp_ssh "$role" "echo ready >/dev/null" >/dev/null 2>&1; then
      return 0
    fi
    sleep 5
  done
  echo "timed out waiting for SSH on $role" >&2
  return 1
}

write_role_meta() {
  local role="$1"
  local nat_ip internal_ip
  nat_ip="$(gcloud compute instances describe "$role" --project="$FROGLET_GCP_PROJECT" --zone="$FROGLET_GCP_ZONE" --format='get(networkInterfaces[0].accessConfigs[0].natIP)')"
  internal_ip="$(gcloud compute instances describe "$role" --project="$FROGLET_GCP_PROJECT" --zone="$FROGLET_GCP_ZONE" --format='get(networkInterfaces[0].networkIP)')"
  local remote_root data_root
  remote_root="$(role_remote_root)"
  data_root="$(role_data_root "$role")"

  cat >"$(role_meta_path "$role")" <<EOF
{
  "instance": "$role",
  "role": "$role",
  "zone": "$FROGLET_GCP_ZONE",
  "internal_ip": "$internal_ip",
  "nat_ip": "$nat_ip",
  "remote_root": "$remote_root",
  "data_root": "$data_root",
  "provider_local_url": "http://127.0.0.1:8080",
  "runtime_url": "http://127.0.0.1:8081",
  "token_paths": {
    "provider_control": "$data_root/runtime/froglet-control.token",
    "consumer_control": "$data_root/runtime/consumerctl.token",
    "runtime_auth": "$data_root/runtime/auth.token"
  }
}
EOF
}

generate_ca() {
  local ca_key="$STATE_DIR/pki/ca.key"
  local ca_pem="$STATE_DIR/pki/ca.pem"
  local ca_cfg="$STATE_DIR/tmp/ca-openssl.cnf"
  if [[ -f "$ca_key" && -f "$ca_pem" ]]; then
    return 0
  fi
  cat >"$ca_cfg" <<'EOF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3_ca

[dn]
CN = froglet-gcp-harness-ca

[v3_ca]
basicConstraints = critical,CA:TRUE,pathlen:1
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer
EOF
  openssl genrsa -out "$ca_key" 4096 >/dev/null 2>&1
  openssl req -x509 -new -nodes -key "$ca_key" -sha256 -days 7 \
    -config "$ca_cfg" \
    -extensions v3_ca \
    -out "$ca_pem" >/dev/null 2>&1
}

generate_role_cert() {
  local role="$1"
  local nat_ip
  nat_ip="$(python3 - "$(role_meta_path "$role")" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["nat_ip"])
PY
)"
  local tmp_cfg="$STATE_DIR/tmp/$role-openssl.cnf"
  cat >"$tmp_cfg" <<EOF
[req]
distinguished_name = dn
prompt = no
req_extensions = v3_req

[dn]
CN = $nat_ip

[v3_req]
basicConstraints = CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
subjectAltName = @alt_names
extendedKeyUsage = serverAuth
subjectKeyIdentifier = hash

[v3_sign]
basicConstraints = CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
subjectAltName = @alt_names
extendedKeyUsage = serverAuth
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid:always,issuer

[alt_names]
IP.1 = $nat_ip
IP.2 = 127.0.0.1
DNS.1 = localhost
DNS.2 = $role
EOF
  openssl genrsa -out "$STATE_DIR/pki/$role.key" 2048 >/dev/null 2>&1
  openssl req -new -key "$STATE_DIR/pki/$role.key" \
    -out "$STATE_DIR/pki/$role.csr" \
    -config "$tmp_cfg" >/dev/null 2>&1
  openssl x509 -req -in "$STATE_DIR/pki/$role.csr" \
    -CA "$STATE_DIR/pki/ca.pem" \
    -CAkey "$STATE_DIR/pki/ca.key" \
    -CAcreateserial \
    -out "$STATE_DIR/pki/$role.crt" \
    -days 7 -sha256 \
    -extfile "$tmp_cfg" \
    -extensions v3_sign >/dev/null 2>&1
}

build_inventory() {
  python3 - "$STATE_DIR" "$INVENTORY_PATH" "$FROGLET_GCP_PROJECT" "$FROGLET_GCP_ZONE" "$FROGLET_GCP_NETWORK" "$FROGLET_GCP_HARNESS_RUN_ID" <<'PY'
import json, pathlib, sys
state_dir = pathlib.Path(sys.argv[1])
inventory_path = pathlib.Path(sys.argv[2])
project, zone, network, run_id = sys.argv[3:7]
roles = {}
for meta_path in sorted((state_dir / "meta").glob("*.json")):
    role = json.load(open(meta_path, "r", encoding="utf-8"))
    if role["role"] not in ("froglet-settlement-lab",):
        role["provider_public_url"] = f'https://{role["nat_ip"]}'
    roles[role["role"]] = role
marketplace_url = f'https://{roles["froglet-marketplace"]["nat_ip"]}'
inventory = {
    "version": 1,
    "generated_at": __import__("datetime").datetime.utcnow().isoformat() + "Z",
    "run_id": run_id,
    "project": project,
    "zone": zone,
    "network": network,
    "ca_cert_path": str(state_dir / "pki" / "ca.pem"),
    "marketplace_url": marketplace_url,
    "roles": roles,
}
inventory_path.write_text(json.dumps(inventory, indent=2) + "\n", encoding="utf-8")
PY
}

sync_repo_to_role() {
  local role="$1"
  gcp_ssh "$role" "mkdir -p '$REMOTE_ROOT/repo' '$REMOTE_ROOT/bin' '$REMOTE_ROOT/etc' '$REMOTE_ROOT/logs' '$REMOTE_ROOT/pki' '$REMOTE_ROOT/state' '$REMOTE_ROOT/artifacts' '$(role_data_root "$role")'"
  COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 tar -C "$repo_root" \
    --no-xattrs \
    --exclude='.git' \
    --exclude='target' \
    --exclude='data' \
    --exclude='_tmp' \
    --exclude='node_modules' \
    --exclude='coverage' \
    -czf - . | {
      gcp_ssh_flags
      gcloud compute ssh "$role" "${GCP_SSH_FLAGS[@]}" \
        --command="bash -lc $(printf '%q' "mkdir -p '$REMOTE_ROOT/repo' && tar -xzf - -C '$REMOTE_ROOT/repo'")"
    }
}

copy_binaries_to_role() {
  local role="$1"
  local linux_bin_dir="$STATE_DIR/linux-bin"
  for binary in froglet-node froglet-marketplace; do
    if [[ -f "$linux_bin_dir/$binary" ]]; then
      local tmp_remote="$REMOTE_ROOT/tmp-$binary.$$"
      gcp_scp_to "$role" "$linux_bin_dir/$binary" "$tmp_remote"
      gcp_ssh "$role" "sudo install -m 0755 '$tmp_remote' '$REMOTE_ROOT/bin/$binary' && rm -f '$tmp_remote'"
    fi
  done
}

copy_pki_to_role() {
  local role="$1"
  gcp_scp_to "$role" "$STATE_DIR/pki/ca.pem" "$REMOTE_ROOT/pki/ca.pem"
  if role_has_public_edge "$role"; then
    gcp_scp_to "$role" "$STATE_DIR/pki/$role.crt" "$REMOTE_ROOT/pki/$role.crt"
    gcp_scp_to "$role" "$STATE_DIR/pki/$role.key" "$REMOTE_ROOT/pki/$role.key"
  fi
}

render_stack_env() {
  local role="$1"
  local nat_ip marketplace_url data_root
  nat_ip="$(python3 - "$(role_meta_path "$role")" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["nat_ip"])
PY
)"
  marketplace_url="https://$(inventory_get roles.froglet-marketplace.nat_ip)"
  data_root="$(role_data_root "$role")"
  cat <<EOF
FROGLET_DATA_ROOT=$data_root
FROGLET_LISTEN_ADDR=127.0.0.1:8080
FROGLET_RUNTIME_LISTEN_ADDR=127.0.0.1:8081
FROGLET_RUNTIME_PROVIDER_BASE_URL=http://127.0.0.1:8080
FROGLET_PUBLIC_BASE_URL=https://$nat_ip
FROGLET_HTTP_CA_CERT_PATH=$REMOTE_ROOT/pki/ca.pem
FROGLET_MARKETPLACE_URL=$marketplace_url
EOF
  case "$role" in
    froglet-provider-paid)
      cat <<EOF
FROGLET_PRICE_EXEC_WASM=30
FROGLET_PAYMENT_BACKEND=lightning
FROGLET_LIGHTNING_MODE=mock
FROGLET_EXECUTION_TIMEOUT_SECS=120
FROGLET_LIGHTNING_SYNC_INTERVAL_MS=100
EOF
      ;;
  esac
}

marketplace_feed_sources() {
  python3 - "$INVENTORY_PATH" <<'PY'
import json, sys
inventory = json.load(open(sys.argv[1], "r", encoding="utf-8"))
urls = []
for name, role in inventory["roles"].items():
    if name.startswith("froglet-provider-"):
        urls.append(f'https://{role["nat_ip"]}')
print(",".join(urls))
PY
}

render_marketplace_env() {
  local role="$1"
  local db_password
  db_password="$(openssl rand -hex 16)"
  cat <<EOF
MARKETPLACE_DATABASE_URL=postgres://froglet:${db_password}@127.0.0.1:5432/marketplace
MARKETPLACE_FEED_SOURCES=$(marketplace_feed_sources)
MARKETPLACE_POLL_INTERVAL_SECS=30
EOF
}

install_remote_text_file() {
  local role="$1"
  local local_file="$2"
  local remote_file="$3"
  local mode="$4"
  local tmp_remote="$REMOTE_ROOT/tmp-$(basename "$remote_file").$$"
  gcp_scp_to "$role" "$local_file" "$tmp_remote"
  gcp_ssh "$role" "sudo install -m '$mode' '$tmp_remote' '$remote_file' && rm -f '$tmp_remote'"
}

install_remote_unit() {
  local role="$1"
  local service_name="$2"
  local contents="$3"
  local local_file="$STATE_DIR/tmp/$role-$service_name"
  printf '%s\n' "$contents" >"$local_file"
  install_remote_text_file "$role" "$local_file" "/etc/systemd/system/$service_name" 0644
}

provider_unit() {
  cat <<EOF
[Unit]
Description=Froglet Provider
After=network-online.target
Wants=network-online.target

[Service]
User=$FROGLET_GCP_REMOTE_USER
WorkingDirectory=$REMOTE_ROOT/repo
EnvironmentFile=$REMOTE_ROOT/etc/node.env
Environment=FROGLET_NODE_ROLE=provider
ExecStart=/bin/bash -lc 'exec "$REMOTE_ROOT/bin/froglet-node" >>"$REMOTE_ROOT/logs/provider.log" 2>&1'
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
}

runtime_unit() {
  cat <<EOF
[Unit]
Description=Froglet Runtime
After=network-online.target froglet-provider.service
Wants=network-online.target

[Service]
User=$FROGLET_GCP_REMOTE_USER
WorkingDirectory=$REMOTE_ROOT/repo
EnvironmentFile=$REMOTE_ROOT/etc/node.env
Environment=FROGLET_NODE_ROLE=runtime
ExecStart=/bin/bash -lc 'exec "$REMOTE_ROOT/bin/froglet-node" >>"$REMOTE_ROOT/logs/runtime.log" 2>&1'
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
}

marketplace_unit() {
  cat <<EOF
[Unit]
Description=Froglet Marketplace
After=network-online.target postgresql.service
Wants=network-online.target

[Service]
User=$FROGLET_GCP_REMOTE_USER
WorkingDirectory=$REMOTE_ROOT/repo
EnvironmentFile=$REMOTE_ROOT/etc/node.env
EnvironmentFile=$REMOTE_ROOT/etc/marketplace.env
ExecStart=/bin/bash -lc 'exec "$REMOTE_ROOT/bin/froglet-marketplace" >>"$REMOTE_ROOT/logs/marketplace.log" 2>&1'
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
}

public_edge_unit() {
  local role="$1"
  local target_port
  target_port="$(role_public_target_port "$role")"
  cat <<EOF
[Unit]
Description=Froglet Public TLS Edge
After=network-online.target
Wants=network-online.target

[Service]
User=$FROGLET_GCP_REMOTE_USER
WorkingDirectory=$REMOTE_ROOT/repo
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
ExecStart=/usr/bin/env node "$REMOTE_ROOT/repo/tests/e2e/gcp_harness/tls_proxy.mjs" --listen 0.0.0.0:443 --target http://127.0.0.1:$target_port --cert "$REMOTE_ROOT/pki/$role.crt" --key "$REMOTE_ROOT/pki/$role.key"
Restart=always
RestartSec=2

[Install]
WantedBy=multi-user.target
EOF
}

sudoers_snippet() {
  local role="$1"
  local commands="/usr/bin/systemctl restart froglet-runtime.service"
  if role_has_provider_service "$role"; then
    commands="$commands, /usr/bin/systemctl restart froglet-provider.service"
  fi
  if [[ "$role" == "froglet-marketplace" ]]; then
    commands="$commands, /usr/bin/systemctl restart froglet-marketplace.service"
  fi
  cat <<EOF
$FROGLET_GCP_REMOTE_USER ALL=(root) NOPASSWD: $commands
EOF
}

setup_marketplace_postgres() {
  local role="$1"
  local db_password="$2"
  gcp_ssh "$role" "
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    sudo apt-get update -qq
    sudo apt-get install -y -qq postgresql
    sudo -u postgres createuser froglet 2>/dev/null || true
    sudo -u postgres psql -c \"ALTER USER froglet WITH PASSWORD '${db_password}';\" 2>/dev/null || true
    sudo systemctl enable postgresql
    sudo systemctl start postgresql
    sudo -u postgres dropdb --if-exists marketplace
    sudo -u postgres createdb -O froglet marketplace
  "
}

reset_marketplace_postgres() {
  gcp_ssh "froglet-marketplace" "
    set -euo pipefail
    sudo systemctl stop froglet-marketplace.service 2>/dev/null || true
    sudo systemctl start postgresql
    sudo -u postgres dropdb --if-exists marketplace
    sudo -u postgres createdb -O froglet marketplace
    sudo systemctl start froglet-marketplace.service
  "
  wait_role_health "froglet-marketplace"
}

configure_role() {
  local role="$1"
  sync_repo_to_role "$role"
  copy_binaries_to_role "$role"
  copy_pki_to_role "$role"
  gcp_scp_to "$role" "$INVENTORY_PATH" "$REMOTE_ROOT/state/inventory.json"

  if role_has_stack "$role"; then
    local env_file="$STATE_DIR/tmp/$role-node.env"
    render_stack_env "$role" >"$env_file"
    gcp_scp_to "$role" "$env_file" "$REMOTE_ROOT/etc/node.env"
    if role_has_provider_service "$role"; then
      install_remote_unit "$role" froglet-provider.service "$(provider_unit)"
    fi
    if role_has_runtime_service "$role"; then
      install_remote_unit "$role" froglet-runtime.service "$(runtime_unit)"
    fi
    local sudoers_file="$STATE_DIR/tmp/$role-sudoers"
    sudoers_snippet "$role" >"$sudoers_file"
    install_remote_text_file "$role" "$sudoers_file" "/etc/sudoers.d/froglet-harness" 0440
  fi

  if [[ "$role" == "froglet-marketplace" ]]; then
    local marketplace_env="$STATE_DIR/tmp/$role-marketplace.env"
    render_marketplace_env "$role" >"$marketplace_env"
    local db_password
    db_password="$(grep MARKETPLACE_DATABASE_URL "$marketplace_env" | sed 's|.*://froglet:\([^@]*\)@.*|\1|')"
    setup_marketplace_postgres "$role" "$db_password"
    gcp_scp_to "$role" "$marketplace_env" "$REMOTE_ROOT/etc/marketplace.env"
    install_remote_unit "$role" froglet-marketplace.service "$(marketplace_unit)"
  fi

  if role_has_public_edge "$role"; then
    install_remote_unit "$role" froglet-public-edge.service "$(public_edge_unit "$role")"
  fi

  gcp_ssh "$role" "sudo systemctl daemon-reload"
}

start_role_services() {
  local role="$1"
  if role_has_stack "$role"; then
    local services=""
    if role_has_provider_service "$role"; then
      services="froglet-provider.service"
    fi
    if role_has_runtime_service "$role"; then
      services="${services:+$services }froglet-runtime.service"
    fi
    if [[ "$role" == "froglet-marketplace" ]]; then
      services="${services:+$services }froglet-marketplace.service"
      gcp_ssh "$role" "sudo systemctl disable froglet-provider.service >/dev/null 2>&1 || true; sudo systemctl stop froglet-provider.service >/dev/null 2>&1 || true"
    fi
    gcp_ssh "$role" "sudo systemctl enable $services >/dev/null && sudo systemctl restart $services"
  fi
  if role_has_public_edge "$role"; then
    gcp_ssh "$role" "sudo systemctl enable froglet-public-edge.service >/dev/null && sudo systemctl restart froglet-public-edge.service"
  fi
}

wait_role_health() {
  local role="$1"
  local deadline=$((SECONDS + 300))
  if role_has_stack "$role"; then
    while [[ $SECONDS -lt $deadline ]]; do
      if gcp_ssh "$role" "curl -fsS http://127.0.0.1:8080/health >/dev/null && curl -fsS http://127.0.0.1:8081/health >/dev/null"; then
        return 0
      fi
      sleep 2
    done
  fi
  echo "timed out waiting for health on $role" >&2
  return 1
}

ensure_remote_rust_toolchain() {
  local role="$1"
  gcp_ssh "$role" "
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    sudo apt-get update -qq
    sudo apt-get install -y -qq build-essential pkg-config libssl-dev clang cmake curl
    if [[ ! -x \"\$HOME/.cargo/bin/cargo\" ]]; then
      curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal
    fi
    \"\$HOME/.cargo/bin/rustup\" default stable >/dev/null 2>&1 || true
  "
}

build_binaries() {
  local build_role="$FROGLET_GCP_BUILD_ROLE"
  local linux_bin_dir="$STATE_DIR/linux-bin"
  mkdir -p "$linux_bin_dir"
  sync_repo_to_role "$build_role"
  ensure_remote_rust_toolchain "$build_role"
  gcp_ssh "$build_role" "
    set -euo pipefail
    export PATH=\"\$HOME/.cargo/bin:\$PATH\"
    cd '$REMOTE_ROOT/repo'
    cargo build --release -p froglet --bin froglet-node
    cargo build --release -p froglet-marketplace --bin froglet-marketplace
    install -m 0755 target/release/froglet-node '$REMOTE_ROOT/bin/froglet-node'
    install -m 0755 target/release/froglet-marketplace '$REMOTE_ROOT/bin/froglet-marketplace'
  "
  for binary in froglet-node froglet-marketplace; do
    gcp_scp_from "$build_role" "$REMOTE_ROOT/bin/$binary" "$linux_bin_dir/$binary"
    chmod 0755 "$linux_bin_dir/$binary"
  done
}

copy_remote_tokens_locally() {
  mkdir -p "$TOKENS_DIR"
  gcp_scp_from "froglet-marketplace" "$REMOTE_ROOT/data/froglet-marketplace/runtime/froglet-control.token" "$TOKENS_DIR/provider.token"
  gcp_scp_from "froglet-marketplace" "$REMOTE_ROOT/data/froglet-marketplace/runtime/auth.token" "$TOKENS_DIR/runtime.token"
  gcp_scp_from "froglet-marketplace" "$REMOTE_ROOT/data/froglet-marketplace/runtime/consumerctl.token" "$TOKENS_DIR/consumer.token"
  printf 'bogus-token\n' >"$TOKENS_DIR/bogus.token"
  chmod 0600 "$TOKENS_DIR/provider.token" "$TOKENS_DIR/runtime.token" "$TOKENS_DIR/consumer.token" "$TOKENS_DIR/bogus.token"
}

open_marketplace_tunnel() {
  local tunnel_log="$STATE_DIR/marketplace-tunnel.log"
  gcp_ssh_flags
  gcloud compute ssh "froglet-marketplace" "${GCP_SSH_FLAGS[@]}" -- \
    -N \
    -L 18080:127.0.0.1:8080 \
    -L 18081:127.0.0.1:8081 \
    -o ExitOnForwardFailure=yes \
    >"$tunnel_log" 2>&1 &
  local tunnel_pid=$!
  echo "$tunnel_pid" >"$STATE_DIR/marketplace-tunnel.pid"
  sleep 5
}

close_marketplace_tunnel() {
  if [[ -f "$STATE_DIR/marketplace-tunnel.pid" ]]; then
    kill "$(cat "$STATE_DIR/marketplace-tunnel.pid")" 2>/dev/null || true
    rm -f "$STATE_DIR/marketplace-tunnel.pid"
  fi
}

ensure_local_python_venv() {
  if [[ ! -d "$STATE_DIR/.venv" ]]; then
    python3 -m venv "$STATE_DIR/.venv"
    "$STATE_DIR/.venv/bin/python" -m pip install --upgrade pip
    "$STATE_DIR/.venv/bin/python" -m pip install -r python/requirements.txt
  fi
}

copy_state_to_marketplace() {
  gcp_scp_to "froglet-marketplace" "$INVENTORY_PATH" "$REMOTE_ROOT/state/inventory.json"
  if [[ -f "$SCENARIO_PATH" ]]; then
    gcp_scp_to "froglet-marketplace" "$SCENARIO_PATH" "$REMOTE_ROOT/state/scenario.json"
  fi
}

generate_execution_suffix() {
  python3 - <<'PY'
import time
print(format(time.time_ns(), "x")[-12:])
PY
}

generate_scenario_set() {
  local execution_suffix="${1:-}"
  local args=(
    tests/e2e/gcp_harness/generate-scenarios.mjs
    --inventory "$INVENTORY_PATH"
    --seed-free "$SEED_FREE_PATH"
    --seed-paid "$SEED_PAID_PATH"
    --out "$SCENARIO_PATH"
  )
  if [[ -n "$execution_suffix" ]]; then
    args+=(--execution-suffix "$execution_suffix")
  fi
  node "${args[@]}"
  copy_state_to_marketplace
}

wait_for_marketplace_convergence() {
  local free_provider_id
  local paid_provider_id
  free_provider_id="$(python3 - "$SEED_FREE_PATH" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["provider_id"])
PY
)"
  paid_provider_id="$(python3 - "$SEED_PAID_PATH" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["provider_id"])
PY
)"
  gcp_ssh "froglet-marketplace" "
    set -euo pipefail
    export FREE_PROVIDER_ID=$(printf '%q' "$free_provider_id")
    export PAID_PROVIDER_ID=$(printf '%q' "$paid_provider_id")
    export FROGLET_TOKEN_PATH='$REMOTE_ROOT/data/froglet-marketplace/runtime/froglet-control.token'
    python3 - <<'PY'
import json
import os
import time
import urllib.error
import urllib.request

token = open(os.environ['FROGLET_TOKEN_PATH'], 'r', encoding='utf-8').read().strip()
headers = {
    'Authorization': f'Bearer {token}',
    'Accept': 'application/json',
    'Content-Type': 'application/json',
}
deadline = time.time() + 180
last_error = 'not started'

def request_json(url: str, payload: dict | None = None) -> dict:
    body = None if payload is None else json.dumps(payload).encode('utf-8')
    request = urllib.request.Request(url, data=body, headers=headers, method='POST' if body is not None else 'GET')
    with urllib.request.urlopen(request, timeout=10) as response:
        return json.load(response)

while time.time() < deadline:
    try:
        health = request_json('http://127.0.0.1:8081/health')
        if not isinstance(health, dict):
            last_error = f'runtime health returned unexpected type: {type(health)}'
            time.sleep(2)
            continue
        search = request_json('http://127.0.0.1:8081/v1/runtime/search', {'limit': 20})
        provider_ids = {entry.get('provider_id') for entry in search.get('providers', []) if entry.get('provider_id')}
        if os.environ['FREE_PROVIDER_ID'] in provider_ids and os.environ['PAID_PROVIDER_ID'] in provider_ids:
            print('ready')
            break
        last_error = f'marketplace convergence incomplete: {sorted(provider_ids)}'
    except (OSError, urllib.error.URLError, urllib.error.HTTPError, json.JSONDecodeError) as error:
        last_error = str(error)
    time.sleep(2)
else:
    raise SystemExit(f'timed out waiting for marketplace convergence: {last_error}')
PY
  "
}

cmd_provision() {
  require_commands
  for role in "${ROLES[@]}"; do
    create_instance "$role"
  done
  for role in "${ROLES[@]}"; do
    wait_for_ssh "$role"
    write_role_meta "$role"
  done
  generate_ca
  for role in "${ROLES[@]}"; do
    if role_has_public_edge "$role"; then
      generate_role_cert "$role"
    fi
  done
  build_inventory
}

cmd_deploy() {
  require_commands
  [[ -f "$INVENTORY_PATH" ]] || cmd_provision
  build_binaries
  for role in "${ROLES[@]}"; do
    configure_role "$role"
  done
  for role in froglet-marketplace froglet-provider-free froglet-provider-paid; do
    start_role_services "$role"
    wait_role_health "$role"
  done
  copy_state_to_marketplace
  touch "$DEPLOY_MARKER"
}

cmd_seed() {
  [[ -f "$INVENTORY_PATH" ]] || cmd_provision
  [[ -f "$DEPLOY_MARKER" ]] || cmd_deploy
  reset_marketplace_postgres
  for role in froglet-provider-free froglet-provider-paid; do
    gcp_ssh "$role" "cd '$REMOTE_ROOT/repo' && node tests/e2e/gcp_harness/seed-node.mjs --inventory '$REMOTE_ROOT/state/inventory.json' --role '$role' --out '$REMOTE_ROOT/artifacts/seed.json'"
    if [[ "$role" == "froglet-provider-free" ]]; then
      gcp_scp_from "$role" "$REMOTE_ROOT/artifacts/seed.json" "$SEED_FREE_PATH"
    else
      gcp_scp_from "$role" "$REMOTE_ROOT/artifacts/seed.json" "$SEED_PAID_PATH"
    fi
  done
  wait_for_marketplace_convergence
  generate_scenario_set
}

cmd_run_matrix() {
  if [[ ! -f "$SEED_FREE_PATH" || ! -f "$SEED_PAID_PATH" ]]; then
    cmd_seed
  else
    reset_marketplace_postgres
  fi
  wait_for_marketplace_convergence
  generate_scenario_set "$(generate_execution_suffix)"
  copy_remote_tokens_locally
  ensure_local_python_venv
  ensure_remote_rust_toolchain "froglet-settlement-lab"
  open_marketplace_tunnel
  trap close_marketplace_tunnel RETURN
  local tool_status=0
  local protocol_status=0
  local openclaw_status=0
  local curated_status=0
  local lnd_status=0
  local api_key
  api_key="$(resolve_openclaw_api_key)"

  NODE_EXTRA_CA_CERTS="$STATE_DIR/pki/ca.pem" node tests/e2e/gcp_harness/run-matrix.mjs \
    --inventory "$INVENTORY_PATH" \
    --scenarios "$SCENARIO_PATH" \
    --provider-url "http://127.0.0.1:18080" \
    --runtime-url "http://127.0.0.1:18081" \
    --provider-token "$TOKENS_DIR/provider.token" \
    --runtime-token "$TOKENS_DIR/runtime.token" \
    --consumer-token "$TOKENS_DIR/consumer.token" \
    --bogus-token "$TOKENS_DIR/bogus.token" \
    --out "$RESULTS_DIR/tool-matrix.json" || tool_status=$?

  "$STATE_DIR/.venv/bin/python" tests/e2e/gcp_harness/protocol_matrix.py \
    --inventory "$INVENTORY_PATH" \
    --seed-free "$SEED_FREE_PATH" \
    --seed-paid "$SEED_PAID_PATH" \
    --provider-url "http://127.0.0.1:18080" \
    --runtime-url "http://127.0.0.1:18081" \
    --provider-token-path "$TOKENS_DIR/provider.token" \
    --runtime-token-path "$TOKENS_DIR/runtime.token" \
    --out "$RESULTS_DIR/protocol-matrix.json" || protocol_status=$?

  NODE_EXTRA_CA_CERTS="$STATE_DIR/pki/ca.pem" node tests/e2e/gcp_harness/run-openclaw-scripted.mjs \
    --inventory "$INVENTORY_PATH" \
    --scenarios "$SCENARIO_PATH" \
    --provider-url "http://127.0.0.1:18080" \
    --runtime-url "http://127.0.0.1:18081" \
    --provider-token "$TOKENS_DIR/consumer.token" \
    --runtime-token "$TOKENS_DIR/runtime.token" \
    --out "$RESULTS_DIR/openclaw-scripted.json" || openclaw_status=$?

  OPENCLAW_API_KEY="$api_key" NODE_EXTRA_CA_CERTS="$STATE_DIR/pki/ca.pem" node tests/e2e/gcp_harness/run-openclaw-curated.mjs \
    --inventory "$INVENTORY_PATH" \
    --scenarios "$SCENARIO_PATH" \
    --provider-url "http://127.0.0.1:18080" \
    --runtime-url "http://127.0.0.1:18081" \
    --provider-auth-token-path "$TOKENS_DIR/consumer.token" \
    --runtime-auth-token-path "$TOKENS_DIR/runtime.token" \
    --out "$RESULTS_DIR/openclaw-curated.json" || curated_status=$?

  gcp_ssh "froglet-settlement-lab" "
    set -euo pipefail
    cd '$REMOTE_ROOT/repo'
    if [[ ! -d .venv ]]; then
      python3 -m venv .venv
      . .venv/bin/activate
      python3 -m pip install --upgrade pip
      python3 -m pip install -r python/requirements.txt
    else
      . .venv/bin/activate
    fi
    FROGLET_RUN_LND_REGTEST=1 python3 -W error -m unittest python.tests.test_lnd_regtest.LndRegtestIntegrationTests -v > '$REMOTE_ROOT/artifacts/lnd-regtest.txt' 2>&1
  " || lnd_status=$?
  gcp_scp_from "froglet-settlement-lab" "$REMOTE_ROOT/artifacts/lnd-regtest.txt" "$RESULTS_DIR/lnd-regtest.txt" 2>/dev/null || true
  trap - RETURN
  close_marketplace_tunnel
  if [[ "$tool_status" -ne 0 || "$protocol_status" -ne 0 || "$openclaw_status" -ne 0 || "$curated_status" -ne 0 || "$lnd_status" -ne 0 ]]; then
    return 1
  fi
}

cmd_run_agentic() {
  if [[ ! -f "$SEED_FREE_PATH" || ! -f "$SEED_PAID_PATH" ]]; then
    cmd_seed
  fi
  wait_for_marketplace_convergence
  generate_scenario_set "$(generate_execution_suffix)"
  local api_key
  api_key="$(resolve_openclaw_api_key)"
  gcp_ssh "froglet-marketplace" "
    set -euo pipefail
    cd '$REMOTE_ROOT/repo'
    export OPENCLAW_API_KEY=$(printf '%q' "$api_key")
    export NODE_EXTRA_CA_CERTS='$REMOTE_ROOT/pki/ca.pem'
    node tests/e2e/gcp_harness/run-agentic.mjs \
      --inventory '$REMOTE_ROOT/state/inventory.json' \
      --scenarios '$REMOTE_ROOT/state/scenario.json' \
      --provider-url 'http://127.0.0.1:8080' \
      --runtime-url 'http://127.0.0.1:8081' \
      --provider-auth-token-path '$REMOTE_ROOT/data/froglet-marketplace/runtime/froglet-control.token' \
      --runtime-auth-token-path '$REMOTE_ROOT/data/froglet-marketplace/runtime/auth.token' \
      --out '$REMOTE_ROOT/artifacts/agentic-results.json'
  "
  gcp_scp_from "froglet-marketplace" "$REMOTE_ROOT/artifacts/agentic-results.json" "$RESULTS_DIR/agentic-results.json"
}

cmd_collect() {
  mkdir -p "$STATE_DIR/collected"
  for role in "${ROLES[@]}"; do
    mkdir -p "$STATE_DIR/collected/$role"
    gcp_scp_from "$role" "$REMOTE_ROOT/logs" "$STATE_DIR/collected/$role/logs" 2>/dev/null || true
    gcp_scp_from "$role" "$REMOTE_ROOT/artifacts" "$STATE_DIR/collected/$role/artifacts" 2>/dev/null || true
  done
  python3 - "$STATE_DIR" <<'PY'
import json, pathlib, sys
state_dir = pathlib.Path(sys.argv[1])
results_dir = state_dir / "results"
tool = json.loads((results_dir / "tool-matrix.json").read_text()) if (results_dir / "tool-matrix.json").exists() else None
protocol = json.loads((results_dir / "protocol-matrix.json").read_text()) if (results_dir / "protocol-matrix.json").exists() else None
openclaw = json.loads((results_dir / "openclaw-scripted.json").read_text()) if (results_dir / "openclaw-scripted.json").exists() else None
curated = json.loads((results_dir / "openclaw-curated.json").read_text()) if (results_dir / "openclaw-curated.json").exists() else None
agentic = json.loads((results_dir / "agentic-results.json").read_text()) if (results_dir / "agentic-results.json").exists() else None
lines = [
    "# Froglet GCP Harness Report",
    "",
    f"- Run ID: {state_dir.name}",
]
if tool:
    lines.append(f"- Tool matrix: {tool['passed']}/{tool['total']} passed")
if protocol:
    lines.append(f"- Protocol checks: {', '.join(sorted(protocol['checks'].keys()))}")
if openclaw:
    lines.append(f"- OpenClaw scripted: {openclaw['passed']}/{openclaw['total']} passed")
if curated:
    lines.append(f"- OpenClaw curated: {curated['passed']}/{curated['total']} passed")
if agentic:
    severe = [a for a in agentic["exploratory"]["anomalies"] if a["severity"] in {"critical", "high"}]
    lines.append(f"- Agentic critical/high anomalies: {len(severe)}")
(state_dir / "SUMMARY.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
}

cmd_destroy() {
  close_marketplace_tunnel
  for role in "${ROLES[@]}"; do
    gcloud compute instances delete "$role" \
      --project="$FROGLET_GCP_PROJECT" \
      --zone="$FROGLET_GCP_ZONE" \
      --quiet 2>/dev/null || true
  done
}

subcommand="${1:-}"
if [[ -z "$subcommand" ]]; then
  echo "usage: $0 {provision|deploy|seed|run-matrix|run-agentic|collect|destroy}" >&2
  exit 1
fi
shift || true

case "$subcommand" in
  provision) cmd_provision "$@" ;;
  deploy) cmd_deploy "$@" ;;
  seed) cmd_seed "$@" ;;
  run-matrix) cmd_run_matrix "$@" ;;
  run-agentic) cmd_run_agentic "$@" ;;
  collect) cmd_collect "$@" ;;
  destroy) cmd_destroy "$@" ;;
  *)
    echo "unknown subcommand: $subcommand" >&2
    exit 1
    ;;
esac
