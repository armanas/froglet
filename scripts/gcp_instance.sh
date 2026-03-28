#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# GCP Compute Engine instance lifecycle manager for Froglet testing.
#
# This script is sourced by test_suite.sh (run_gcp_rig) — it is NOT
# executed directly.  All tests run ON the VM via SSH so that compose
# services are reachable on loopback and auth tokens are available
# locally inside the VM.
#
# Required env:
#   FROGLET_GCP_PROJECT   – GCP project ID
#
# Optional env:
#   FROGLET_GCP_ZONE           – default us-central1-a
#   FROGLET_GCP_MACHINE_TYPE   – default e2-standard-4
#   FROGLET_GCP_IMAGE_FAMILY   – default debian-12
#   FROGLET_GCP_IMAGE_PROJECT  – default debian-cloud
#   FROGLET_GCP_SSH_KEY_FILE   – override SSH key for gcloud compute ssh
# ---------------------------------------------------------------------------
set -euo pipefail

: "${FROGLET_GCP_PROJECT:?FROGLET_GCP_PROJECT is required}"
: "${FROGLET_GCP_ZONE:=us-central1-a}"
: "${FROGLET_GCP_MACHINE_TYPE:=e2-standard-4}"
: "${FROGLET_GCP_IMAGE_FAMILY:=debian-12}"
: "${FROGLET_GCP_IMAGE_PROJECT:=debian-cloud}"

_GCP_INSTANCE_NAME=""
_GCP_CLEANUP_REGISTERED=0
_GCP_REMOTE_USER=""
_GCP_REMOTE_HOME=""
_GCP_REMOTE_ROOT=""
_GCP_ARTIFACT_DIR=""

# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------
_gcp_ssh_flags() {
  GCP_SSH_FLAGS=(--project="$FROGLET_GCP_PROJECT" --zone="$FROGLET_GCP_ZONE" --quiet)
  if [[ -n "${FROGLET_GCP_SSH_KEY_FILE:-}" ]]; then
    GCP_SSH_FLAGS+=(--ssh-key-file="$FROGLET_GCP_SSH_KEY_FILE")
  fi
}

_gcp_remote_root() {
  echo "$_GCP_REMOTE_ROOT"
}

_gcp_remote_ssh() {
  _gcp_ssh_flags
  gcloud compute ssh "$_GCP_INSTANCE_NAME" \
    "${GCP_SSH_FLAGS[@]}" \
    --command="bash -lc $(printf '%q' "$1")"
}

_gcp_compose_env_assignments() {
  local assignments=()
  local forwarded=(
    FROGLET_PRICE_EXEC_WASM
    FROGLET_PAYMENT_BACKEND
    FROGLET_LIGHTNING_MODE
    FROGLET_ALLOW_DISCOVERY_DISABLED
  )
  local var
  for var in "${forwarded[@]}"; do
    if [[ -n "${!var:-}" ]]; then
      assignments+=("$var=$(printf '%q' "${!var}")")
    fi
  done
  if ((${#assignments[@]})); then
    printf '%s ' "${assignments[@]}"
  fi
}

gcp_bootstrap_vm() {
  gcp_run_cmd '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive

    wait_for_bootstrap() {
      local attempt=1
      local max_attempts=60
      while \
        sudo -n systemctl is-active --quiet google-startup-scripts.service 2>/dev/null \
        || sudo -n systemctl is-active --quiet apt-daily.service 2>/dev/null \
        || sudo -n systemctl is-active --quiet apt-daily-upgrade.service 2>/dev/null \
        || pgrep -x apt >/dev/null 2>&1 \
        || pgrep -x apt-get >/dev/null 2>&1 \
        || pgrep -x dpkg >/dev/null 2>&1; do
        if [[ "$attempt" -ge "$max_attempts" ]]; then
          echo "timed out waiting for bootstrap/apt locks to clear" >&2
          return 1
        fi
        attempt=$((attempt + 1))
        sleep 5
      done
    }

    if command -v docker >/dev/null 2>&1 \
      && sudo -n docker info >/dev/null 2>&1 \
      && sudo -n docker compose version >/dev/null 2>&1; then
      exit 0
    fi

    retry_cmd() {
      local attempt=1
      local max_attempts=30
      while true; do
        wait_for_bootstrap
        if "$@"; then
          return 0
        fi
        if [[ "$attempt" -ge "$max_attempts" ]]; then
          return 1
        fi
        attempt=$((attempt + 1))
        sleep 10
      done
    }

    retry_cmd sudo -n apt-get -o DPkg::Lock::Timeout=60 update -qq
    retry_cmd sudo -n apt-get -o DPkg::Lock::Timeout=60 install -y -qq ca-certificates curl gnupg lsb-release python3 python3-pip python3-venv nodejs npm

    sudo -n install -d -m 0755 /etc/apt/keyrings
    if [[ ! -f /etc/apt/keyrings/docker.gpg ]]; then
      curl -fsSL https://download.docker.com/linux/$(. /etc/os-release; echo "$ID")/gpg \
        | sudo -n gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    fi
    sudo -n chmod a+r /etc/apt/keyrings/docker.gpg

    docker_repo=""
    docker_repo="deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/$(. /etc/os-release; echo "$ID") $(. /etc/os-release; echo "$VERSION_CODENAME") stable"
    if [[ ! -f /etc/apt/sources.list.d/docker.list ]] || ! grep -Fqx "$docker_repo" /etc/apt/sources.list.d/docker.list; then
      printf "%s\n" "$docker_repo" | sudo -n tee /etc/apt/sources.list.d/docker.list >/dev/null
    fi

    retry_cmd sudo -n apt-get -o DPkg::Lock::Timeout=60 update -qq
    retry_cmd sudo -n apt-get -o DPkg::Lock::Timeout=60 install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
    sudo -n systemctl enable --now docker
    sudo -n docker info >/dev/null
    sudo -n docker compose version >/dev/null
  '
}

gcp_collect_logs() {
  if [[ -z "$_GCP_INSTANCE_NAME" ]]; then
    return 0
  fi

  local remote_root
  remote_root="$(_gcp_remote_root)"

  echo "Collecting GCP compose logs ..."
  mkdir -p "$_GCP_ARTIFACT_DIR"
  _gcp_remote_ssh "
    set -euo pipefail
    echo '== command -v docker =='
    command -v docker || true
    echo '== startup-script log =='
    sudo -n journalctl -u google-startup-scripts.service --no-pager -n 200 || true
    if command -v docker >/dev/null 2>&1; then
      cd $(printf '%q' "$remote_root")
      echo '== docker compose ps =='
      sudo -n docker compose ps || true
      echo '== docker compose logs =='
      sudo -n docker compose logs --no-color --tail=200 discovery provider operator runtime || true
    fi
  " >"$_GCP_ARTIFACT_DIR/compose.log" 2>&1 || true
  echo "GCP logs written to $_GCP_ARTIFACT_DIR/compose.log"
}

gcp_cleanup() {
  local status="$1"
  if [[ "$status" -ne 0 ]]; then
    gcp_collect_logs
  fi
  gcp_destroy_instance
  return "$status"
}

# ---------------------------------------------------------------------------
# gcp_create_instance – provision an ephemeral VM with Docker
# ---------------------------------------------------------------------------
gcp_create_instance() {
  _GCP_INSTANCE_NAME="froglet-test-$(date +%s)-$$"
  _GCP_REMOTE_USER="${FROGLET_GCP_USER:-$(whoami)}"
  _GCP_REMOTE_HOME="/home/$_GCP_REMOTE_USER"
  _GCP_REMOTE_ROOT="$_GCP_REMOTE_HOME/froglet"
  _GCP_ARTIFACT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/froglet-gcp-${_GCP_INSTANCE_NAME}-XXXXXX")"

  if [[ "$_GCP_CLEANUP_REGISTERED" == "0" ]]; then
    trap 'status=$?; trap - EXIT; gcp_cleanup "$status"; exit "$status"' EXIT
    _GCP_CLEANUP_REGISTERED=1
  fi

  echo "Creating GCP instance $_GCP_INSTANCE_NAME ..."
  local startup_script
startup_script="$(cat <<'EOF'
#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

wait_for_bootstrap() {
  local attempt=1
  local max_attempts=60
  while \
    systemctl is-active --quiet apt-daily.service 2>/dev/null \
    || systemctl is-active --quiet apt-daily-upgrade.service 2>/dev/null \
    || pgrep -x apt >/dev/null 2>&1 \
    || pgrep -x apt-get >/dev/null 2>&1 \
    || pgrep -x dpkg >/dev/null 2>&1; do
    if [[ "$attempt" -ge "$max_attempts" ]]; then
      echo "timed out waiting for apt locks to clear" >&2
      return 1
    fi
    attempt=$((attempt + 1))
    sleep 5
  done
}

retry_cmd() {
  local attempt=1
  local max_attempts=30
  while true; do
    wait_for_bootstrap
    if "$@"; then
      return 0
    fi
    if [[ "$attempt" -ge "$max_attempts" ]]; then
      return 1
    fi
    attempt=$((attempt + 1))
    sleep 10
  done
}

retry_cmd apt-get -o DPkg::Lock::Timeout=60 update -qq
retry_cmd apt-get -o DPkg::Lock::Timeout=60 install -y -qq ca-certificates curl gnupg lsb-release python3 python3-pip python3-venv nodejs npm

install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/$(. /etc/os-release; echo "$ID")/gpg \
  | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
chmod a+r /etc/apt/keyrings/docker.gpg

echo \
  "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/$(. /etc/os-release; echo "$ID") $(. /etc/os-release; echo "$VERSION_CODENAME") stable" \
  >/etc/apt/sources.list.d/docker.list

retry_cmd apt-get -o DPkg::Lock::Timeout=60 update -qq
retry_cmd apt-get -o DPkg::Lock::Timeout=60 install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
systemctl enable --now docker
EOF
)"
  gcloud compute instances create "$_GCP_INSTANCE_NAME" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --machine-type="$FROGLET_GCP_MACHINE_TYPE" \
    --image-family="$FROGLET_GCP_IMAGE_FAMILY" \
    --image-project="$FROGLET_GCP_IMAGE_PROJECT" \
    --boot-disk-size=50GB \
    --boot-disk-type=pd-ssd \
    --scopes=cloud-platform \
    --metadata=startup-script="$startup_script" \
    --quiet

  echo "Instance $_GCP_INSTANCE_NAME created."
}

# ---------------------------------------------------------------------------
# gcp_wait_ready – poll until Docker is usable on the VM
# ---------------------------------------------------------------------------
gcp_wait_ready() {
  local deadline=$((SECONDS + 600))
  echo "Waiting for VM SSH to become reachable ..."

  while [[ $SECONDS -lt $deadline ]]; do
    if gcp_run_cmd "set -euo pipefail; echo ssh-ready >/dev/null"; then
      echo "SSH is ready."
      break
    fi
    sleep 5
  done

  if [[ $SECONDS -ge $deadline ]]; then
    echo "ERROR: VM SSH did not become ready within 10 minutes." >&2
    return 1
  fi

  echo "Ensuring Docker bootstrap on VM ..."
  gcp_bootstrap_vm
  echo "VM is ready."
}

# ---------------------------------------------------------------------------
# gcp_deploy_stack – rsync local worktree to VM, build & start compose
# ---------------------------------------------------------------------------
gcp_deploy_stack() {
  local repo_root
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  local remote_root
  remote_root="$(_gcp_remote_root)"
  local compose_env_assignments
  compose_env_assignments="$(_gcp_compose_env_assignments)"
  local remote_extract_cmd
  remote_extract_cmd="set -euo pipefail; mkdir -p $(printf '%q' "$remote_root") && tar -xzf - -C $(printf '%q' "$remote_root")"

  echo "Preparing remote workspace ..."
  gcp_run_cmd "set -euo pipefail; mkdir -p $(printf '%q' "$remote_root/data")"

  echo "Syncing local worktree to VM ..."
  COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 tar -C "$repo_root" \
    --exclude='.git' \
    --exclude='target' \
    --exclude='node_modules' \
    --exclude='data' \
    --exclude='_tmp' \
    --exclude='coverage' \
    --exclude='*.db' \
    --exclude='*.db-shm' \
    --exclude='*.db-wal' \
    --exclude='node.db*' \
    -czf - . \
    | {
        _gcp_ssh_flags
        gcloud compute ssh "$_GCP_INSTANCE_NAME" \
          "${GCP_SSH_FLAGS[@]}" \
          --command="bash -lc $(printf '%q' "$remote_extract_cmd")"
      }

  echo "Building and starting compose stack on VM ..."
  gcp_run_cmd "
    set -euo pipefail
    cd $(printf '%q' "$remote_root")
    sudo -n env ${compose_env_assignments} docker compose up --build -d --wait
  "

  echo "Waiting for health endpoints ..."
  gcp_run_cmd '
    set -euo pipefail
    for port in 9090 8080 8081 9191; do
      for i in $(seq 1 30); do
        if curl -fsS "http://127.0.0.1:$port/health" >/dev/null; then
          echo "  port $port healthy"
          break
        fi
        if [ "$i" -eq 30 ]; then
          echo "ERROR: port $port did not become healthy" >&2
          exit 1
        fi
        sleep 2
      done
    done
  '

  echo "Compose stack deployed and healthy on VM."
}

# ---------------------------------------------------------------------------
# gcp_run_cmd – execute a command on the VM via SSH
# ---------------------------------------------------------------------------
gcp_run_cmd() {
  _gcp_remote_ssh "$1"
}

# ---------------------------------------------------------------------------
# gcp_run_test_on_vm – run a test_suite.sh category on the VM
# ---------------------------------------------------------------------------
gcp_run_test_on_vm() {
  local categories="$1"
  local remote_root
  remote_root="$(_gcp_remote_root)"
  local runtime_token_dir="$remote_root/data/runtime"
  local provider_token_copy="$remote_root/.froglet-provider-control.token"
  local consumer_token_copy="$remote_root/.froglet-consumer-control.token"
  local env_exports=(
    "export FROGLET_TEST_REMOTE_STACK=1"
    "export FROGLET_TEST_PROVIDER_URL=http://127.0.0.1:8080"
    "export FROGLET_TEST_RUNTIME_URL=http://127.0.0.1:8081"
    "export FROGLET_TEST_DISCOVERY_URL=http://127.0.0.1:9090"
    "export FROGLET_TEST_OPERATOR_URL=http://127.0.0.1:9191"
    "export FROGLET_TEST_DATA_ROOT=$(printf '%q' "$remote_root/data")"
    "export FROGLET_DATA_ROOT=$(printf '%q' "$remote_root/data")"
    "export FROGLET_AUTH_TOKEN_PATH=$(printf '%q' "$provider_token_copy")"
    "export FROGLET_TEST_PROVIDER_CONTROL_AUTH_TOKEN_PATH=$(printf '%q' "$provider_token_copy")"
    "export FROGLET_TEST_CONSUMER_CONTROL_AUTH_TOKEN_PATH=$(printf '%q' "$consumer_token_copy")"
    "export FROGLET_BASE_URL=http://127.0.0.1:9191"
    "export FROGLET_PROVIDER_URL=http://127.0.0.1:8080"
  )

  # Forward API keys if set locally
  if [[ -n "${OPENCLAW_API_KEY:-}" ]]; then
    env_exports+=("export OPENCLAW_API_KEY=$(printf '%q' "$OPENCLAW_API_KEY")")
  fi
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    env_exports+=("export OPENAI_API_KEY=$(printf '%q' "$OPENAI_API_KEY")")
  fi
  if [[ -n "${FROGLET_ACCEPTANCE_TESTS:-}" ]]; then
    env_exports+=("export FROGLET_ACCEPTANCE_TESTS=$(printf '%q' "$FROGLET_ACCEPTANCE_TESTS")")
  fi

  echo "Running test categories on VM: $categories"
  gcp_run_cmd "
    set -euo pipefail
    $(printf '%s; ' "${env_exports[@]}")
    cd $(printf '%q' "$remote_root")
    sudo -n cat $(printf '%q' "$runtime_token_dir/froglet-control.token") > $(printf '%q' "$provider_token_copy")
    sudo -n cat $(printf '%q' "$runtime_token_dir/consumerctl.token") > $(printf '%q' "$consumer_token_copy")
    chmod 0600 $(printf '%q' "$provider_token_copy") $(printf '%q' "$consumer_token_copy")
    python3 -m venv .venv
    . .venv/bin/activate
    python3 -m pip install --upgrade pip
    python3 -m pip install -r python/requirements.txt
    npm ci --prefix integrations/mcp/froglet
    ./scripts/test_suite.sh $categories
  "
}

# ---------------------------------------------------------------------------
# gcp_get_ip – return the external IP of the running VM
# ---------------------------------------------------------------------------
gcp_get_ip() {
  gcloud compute instances describe "$_GCP_INSTANCE_NAME" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --format='get(networkInterfaces[0].accessConfigs[0].natIP)' \
    --quiet
}

# ---------------------------------------------------------------------------
# gcp_destroy_instance – delete the VM (idempotent, safe to call repeatedly)
# ---------------------------------------------------------------------------
gcp_destroy_instance() {
  if [[ -z "$_GCP_INSTANCE_NAME" ]]; then
    return 0
  fi

  echo "Destroying GCP instance $_GCP_INSTANCE_NAME ..."
  gcloud compute instances delete "$_GCP_INSTANCE_NAME" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --quiet 2>/dev/null || true

  echo "Instance $_GCP_INSTANCE_NAME destroyed."
  _GCP_INSTANCE_NAME=""
}
