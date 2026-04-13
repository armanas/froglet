# GCP Single VM

Public self-host wrapper for a single Froglet VM on GCP.

## Purpose

This wrapper gives the public repo one stable self-host interface:

- `create`
- `deploy`
- `status`
- `destroy`

It is intentionally small. It creates one Compute Engine VM, replaces the
remote checkout with the current local tree, and starts the default
`docker compose` stack on that VM.

## Required environment

- `FROGLET_GCP_PROJECT`

## Optional environment

- `FROGLET_GCP_INSTANCE_NAME` (default: `froglet-selfhost`)
- `FROGLET_GCP_ZONE` (default: `us-central1-a`)
- `FROGLET_GCP_MACHINE_TYPE` (default: `e2-standard-4`)
- `FROGLET_GCP_IMAGE_FAMILY` (default: `debian-12`)
- `FROGLET_GCP_IMAGE_PROJECT` (default: `debian-cloud`)
- `FROGLET_GCP_BOOT_DISK_SIZE` (default: `50GB`)
- `FROGLET_GCP_REMOTE_USER` (default: current local user)

## Usage

```bash
FROGLET_GCP_PROJECT=your-project \
  ./scripts/deploy_gcp_single_vm.sh create

FROGLET_GCP_PROJECT=your-project \
  ./scripts/deploy_gcp_single_vm.sh deploy

FROGLET_GCP_PROJECT=your-project \
  ./scripts/deploy_gcp_single_vm.sh status

FROGLET_GCP_PROJECT=your-project \
  ./scripts/deploy_gcp_single_vm.sh destroy
```

## What `deploy` does

1. waits for SSH and Docker
2. replaces the remote repo checkout with the current local tree
3. runs `docker compose up --build -d --wait`
4. checks the provider and runtime health endpoints

This wrapper is the supported launch target for self-hosted GCP. Broader cloud
or hosted-account automation stays outside the current public repo scope.
