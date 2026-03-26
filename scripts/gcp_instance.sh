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

# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------
_gcp_ssh_flags() {
  local flags=(--project="$FROGLET_GCP_PROJECT" --zone="$FROGLET_GCP_ZONE" --quiet)
  if [[ -n "${FROGLET_GCP_SSH_KEY_FILE:-}" ]]; then
    flags+=(--ssh-key-file="$FROGLET_GCP_SSH_KEY_FILE")
  fi
  echo "${flags[@]}"
}

# ---------------------------------------------------------------------------
# gcp_create_instance – provision an ephemeral VM with Docker
# ---------------------------------------------------------------------------
gcp_create_instance() {
  _GCP_INSTANCE_NAME="froglet-test-$(date +%s)-$$"

  if [[ "$_GCP_CLEANUP_REGISTERED" == "0" ]]; then
    trap gcp_destroy_instance EXIT
    _GCP_CLEANUP_REGISTERED=1
  fi

  echo "Creating GCP instance $_GCP_INSTANCE_NAME ..."
  gcloud compute instances create "$_GCP_INSTANCE_NAME" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --machine-type="$FROGLET_GCP_MACHINE_TYPE" \
    --image-family="$FROGLET_GCP_IMAGE_FAMILY" \
    --image-project="$FROGLET_GCP_IMAGE_PROJECT" \
    --boot-disk-size=50GB \
    --boot-disk-type=pd-ssd \
    --scopes=cloud-platform \
    --metadata=startup-script='#!/bin/bash
set -e
apt-get update -qq
apt-get install -y -qq docker.io docker-compose-plugin rsync python3 python3-pip nodejs npm curl
systemctl enable --now docker
usermod -aG docker $(logname || echo "$(whoami)")
' \
    --quiet

  echo "Instance $_GCP_INSTANCE_NAME created."
}

# ---------------------------------------------------------------------------
# gcp_wait_ready – poll until Docker is usable on the VM
# ---------------------------------------------------------------------------
gcp_wait_ready() {
  local deadline=$((SECONDS + 600))
  echo "Waiting for VM to be ready (Docker usable) ..."

  while [[ $SECONDS -lt $deadline ]]; do
    if gcp_run_cmd "docker info >/dev/null 2>&1" 2>/dev/null; then
      echo "VM is ready."
      return 0
    fi
    sleep 10
  done

  echo "ERROR: VM did not become ready within 10 minutes." >&2
  return 1
}

# ---------------------------------------------------------------------------
# gcp_deploy_stack – rsync local worktree to VM, build & start compose
# ---------------------------------------------------------------------------
gcp_deploy_stack() {
  local repo_root
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

  echo "Syncing local worktree to VM ..."
  gcloud compute scp --recurse \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --quiet \
    --compress \
    "$repo_root/" \
    "$_GCP_INSTANCE_NAME:~/froglet/"

  echo "Building and starting compose stack on VM ..."
  gcp_run_cmd "cd ~/froglet && docker compose up --build -d --wait"

  echo "Waiting for health endpoints ..."
  gcp_run_cmd '
    for port in 9090 8080 8081 9191; do
      for i in $(seq 1 30); do
        if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
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
  gcloud compute ssh "$_GCP_INSTANCE_NAME" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$FROGLET_GCP_ZONE" \
    --quiet \
    --command="$1"
}

# ---------------------------------------------------------------------------
# gcp_run_test_on_vm – run a test_suite.sh category on the VM
# ---------------------------------------------------------------------------
gcp_run_test_on_vm() {
  local categories="$1"
  local env_exports=""

  # Forward API keys if set locally
  if [[ -n "${OPENCLAW_API_KEY:-}" ]]; then
    env_exports+="export OPENCLAW_API_KEY='$OPENCLAW_API_KEY'; "
  fi
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    env_exports+="export OPENAI_API_KEY='$OPENAI_API_KEY'; "
  fi

  echo "Running test categories on VM: $categories"
  gcp_run_cmd "${env_exports}cd ~/froglet && pip3 install -r python/requirements.txt --quiet 2>/dev/null; npm ci --prefix integrations/mcp/froglet --quiet 2>/dev/null; ./scripts/test_suite.sh $categories"
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
