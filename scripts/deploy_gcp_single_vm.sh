#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
remote_root="${FROGLET_GCP_REMOTE_ROOT:-~/froglet}"

usage() {
  cat <<'EOF'
Usage:
  scripts/deploy_gcp_single_vm.sh create|deploy|status|destroy

Required environment:
  FROGLET_GCP_PROJECT   GCP project id

Optional environment:
  FROGLET_GCP_INSTANCE_NAME   default: froglet-selfhost
  FROGLET_GCP_ZONE            default: us-central1-a
  FROGLET_GCP_MACHINE_TYPE    default: e2-standard-4
  FROGLET_GCP_IMAGE_FAMILY    default: debian-12
  FROGLET_GCP_IMAGE_PROJECT   default: debian-cloud
  FROGLET_GCP_BOOT_DISK_SIZE  default: 50GB
  FROGLET_GCP_REMOTE_USER     default: current local user
EOF
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

instance_name="${FROGLET_GCP_INSTANCE_NAME:-froglet-selfhost}"
zone="${FROGLET_GCP_ZONE:-us-central1-a}"
machine_type="${FROGLET_GCP_MACHINE_TYPE:-e2-standard-4}"
image_family="${FROGLET_GCP_IMAGE_FAMILY:-debian-12}"
image_project="${FROGLET_GCP_IMAGE_PROJECT:-debian-cloud}"
boot_disk_size="${FROGLET_GCP_BOOT_DISK_SIZE:-50GB}"
remote_user="${FROGLET_GCP_REMOTE_USER:-$(whoami)}"

need_gcp() {
  need_cmd gcloud
  [[ -n "${FROGLET_GCP_PROJECT:-}" ]] || fail "FROGLET_GCP_PROJECT is required"
}

gcp_ssh_flags() {
  GCP_SSH_FLAGS=(--project="$FROGLET_GCP_PROJECT" --zone="$zone" --quiet)
  if [[ -n "${FROGLET_GCP_SSH_KEY_FILE:-}" ]]; then
    GCP_SSH_FLAGS+=(--ssh-key-file="$FROGLET_GCP_SSH_KEY_FILE")
  fi
}

instance_exists() {
  gcloud compute instances describe "$instance_name" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$zone" >/dev/null 2>&1
}

startup_script() {
  cat <<EOF
#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

retry_cmd() {
  local attempt=1
  local max_attempts=30
  while true; do
    if "$@"; then
      return 0
    fi
    if [[ "\$attempt" -ge "\$max_attempts" ]]; then
      return 1
    fi
    attempt=\$((attempt + 1))
    sleep 10
  done
}

retry_cmd apt-get -o DPkg::Lock::Timeout=60 update -qq
retry_cmd apt-get -o DPkg::Lock::Timeout=60 install -y -qq ca-certificates curl gnupg lsb-release tar

install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/\$(. /etc/os-release; echo "\$ID")/gpg \
  | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
chmod a+r /etc/apt/keyrings/docker.gpg

printf '%s\n' \
  "deb [arch=\$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/\$(. /etc/os-release; echo "\$ID") \$(. /etc/os-release; echo "\$VERSION_CODENAME") stable" \
  >/etc/apt/sources.list.d/docker.list

retry_cmd apt-get -o DPkg::Lock::Timeout=60 update -qq
retry_cmd apt-get -o DPkg::Lock::Timeout=60 install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
systemctl enable --now docker
usermod -aG docker "$remote_user" || true
EOF
}

ssh_cmd() {
  gcp_ssh_flags
  gcloud compute ssh "$instance_name" "${GCP_SSH_FLAGS[@]}" \
    --command="bash -lc $(printf '%q' "$1")"
}

create_instance() {
  need_gcp
  if instance_exists; then
    printf 'Instance %s already exists in %s.\n' "$instance_name" "$zone"
    return 0
  fi

  printf 'Creating %s in %s...\n' "$instance_name" "$zone"
  gcloud compute instances create "$instance_name" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$zone" \
    --machine-type="$machine_type" \
    --image-family="$image_family" \
    --image-project="$image_project" \
    --boot-disk-size="$boot_disk_size" \
    --boot-disk-type=pd-ssd \
    --scopes=cloud-platform \
    --metadata=startup-script="$(startup_script)" \
    --quiet
  wait_ready
}

wait_ready() {
  local deadline=$((SECONDS + 600))
  printf 'Waiting for SSH and Docker...\n'
  while [[ $SECONDS -lt $deadline ]]; do
    if ssh_cmd "docker info >/dev/null 2>&1"; then
      printf 'Instance %s is ready.\n' "$instance_name"
      return 0
    fi
    sleep 10
  done
  fail "timed out waiting for $instance_name to become ready"
}

sync_repo() {
  local sync_cmd
  sync_cmd="set -euo pipefail; mkdir -p $remote_root; find $remote_root -mindepth 1 -maxdepth 1 -exec rm -rf {} +; tar -xzf - -C $remote_root"
  printf 'Syncing local checkout to %s:%s...\n' "$instance_name" "$remote_root"
  COPYFILE_DISABLE=1 COPY_EXTENDED_ATTRIBUTES_DISABLE=1 tar -C "$repo_root" \
    --exclude='.git' \
    --exclude='target' \
    --exclude='node_modules' \
    --exclude='docs-site/node_modules' \
    --exclude='docs-site/dist' \
    --exclude='data' \
    --exclude='_tmp' \
    --exclude='.venv' \
    --exclude='coverage' \
    --exclude='*.db' \
    --exclude='*.db-shm' \
    --exclude='*.db-wal' \
    -czf - . | {
      gcp_ssh_flags
      gcloud compute ssh "$instance_name" "${GCP_SSH_FLAGS[@]}" \
        --command="bash -lc $(printf '%q' "$sync_cmd")"
    }
}

deploy_stack() {
  need_gcp
  instance_exists || fail "instance $instance_name does not exist; run create first"
  wait_ready
  sync_repo
  printf 'Starting Froglet stack on %s...\n' "$instance_name"
  ssh_cmd "
    set -euo pipefail
    cd $remote_root
    docker compose up --build -d --wait
    curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null
    curl --fail --silent --show-error http://127.0.0.1:8081/health >/dev/null
  "
  printf 'Deployed Froglet to %s.\n' "$instance_name"
}

status_instance() {
  need_gcp
  instance_exists || fail "instance $instance_name does not exist"
  gcloud compute instances describe "$instance_name" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$zone" \
    --format='value(name,status,networkInterfaces[0].accessConfigs[0].natIP)'
  ssh_cmd "
    set -euo pipefail
    if [ -d $remote_root ]; then
      cd $remote_root
      docker compose ps
    fi
  "
}

destroy_instance() {
  need_gcp
  if ! instance_exists; then
    printf 'Instance %s does not exist.\n' "$instance_name"
    return 0
  fi
  printf 'Destroying %s...\n' "$instance_name"
  gcloud compute instances delete "$instance_name" \
    --project="$FROGLET_GCP_PROJECT" \
    --zone="$zone" \
    --quiet
}

case "${1:-}" in
  create)
    create_instance
    ;;
  deploy)
    deploy_stack
    ;;
  status)
    status_instance
    ;;
  destroy)
    destroy_instance
    ;;
  -h|--help|"")
    usage
    ;;
  *)
    fail "unknown subcommand: $1"
    ;;
esac
