# Operator Deploy & Verification Guide

Status: current reference for standing up or updating the hosted Froglet
environment at `ai.froglet.dev`. Closes [TODO.md Order 26](../TODO.md).

This document is the single runbook for "stand up a Froglet node, attach it
to the public domain, verify it works, and roll it back when it doesn't."
It derives from the real automation in this repo — every step cites the
script it runs.

## 1. What this deploys

A single AWS Lightsail Container Service (`froglet-node`, `us-east-1`,
`power=small`) running the `froglet-provider` role image from
`ghcr.io/armanas/froglet-provider`. Cloudflare proxies `ai.froglet.dev` in
front of it. Lightsail issues an ACM certificate for the custom hostname so
the Host-header check passes.

**Out of scope here:**
- Marketplace read API deployment (lives in `froglet-services`, tracked as
  [TODO.md Order 64](../TODO.md)).
- Lightning node setup ([Order 54](../TODO.md)).
- Stripe webhook receiver ([Order 57](../TODO.md)).
- Status page ([Order 63](../TODO.md)).

Each of those lands through its own runbook when the external credential
is in hand.

## 2. Prerequisites (one-time)

### 2.1 Accounts and credentials

- AWS account with **Lightsail + RDS + CloudWatch Logs** access via an IAM
  user. Details in the session log for the first standup; the user is
  `froglet-deploy` with the `FrogletLightsailDeploy` customer-managed
  policy plus `AmazonRDSFullAccess` and `CloudWatchLogsReadOnlyAccess`.
- Cloudflare account owning the `froglet.dev` zone (zone id
  `ff6367e195a95ebe1a1acb066f8b09a6`).
- A **$50/mo AWS billing budget** configured under
  Billing → Budgets → `froglet-monthly-cost`, so credit exhaustion
  surfaces as an email alert rather than a silent bill.

### 2.2 Secrets in macOS Keychain

No secret ever lives in a file on disk or an environment-config file.
Every helper reads from the macOS Keychain per invocation.

```bash
security add-generic-password -a froglet -s cloudflare-dns-token -w '<CF_DNS_Edit_TOKEN>' -U
security add-generic-password -a froglet -s aws-deploy-access-key  -w '<AKIA...>' -U
security add-generic-password -a froglet -s aws-deploy-secret-key  -w '<SECRET>' -U
```

To avoid leaving the secret in shell history, prefix each command with a
single leading space, or use the `read -rs` pattern (paste the secret at
the blank prompt):

```bash
 read -rs VAL && security add-generic-password -a froglet -s aws-deploy-secret-key -w "$VAL" -U && unset VAL
```

### 2.3 Local tooling

```bash
brew install awscli     # aws CLI (tested with 2.x)
# Keychain and curl are stdlib on macOS.
```

Both helper scripts validate the environment themselves and print a
specific error if anything is missing.

## 3. First deploy

Run these in order. Each command is idempotent (re-runnable) except where
noted.

```bash
# Confirm DNS auth works.
./scripts/cloudflare_dns.sh verify
./scripts/cloudflare_dns.sh zone       # should print status=active

# Confirm AWS auth works.
./scripts/deploy_aws.sh verify
./scripts/deploy_aws.sh status         # should print "no container service found" on first run

# Provision the Lightsail Container Service. BILLABLE. Prompts interactively.
./scripts/deploy_aws.sh create --power small
# Wait ~3-5 min until ./scripts/deploy_aws.sh status reports state=READY.

# Request the ACM certificate for ai.froglet.dev. This is a one-time setup step;
# keep it out of the main script so it does not re-issue on every deploy.
# Validation record is a CNAME published in Cloudflare, then AWS polls DNS.
AWS_ACCESS_KEY_ID=$(security find-generic-password -a froglet -s aws-deploy-access-key -w) \
AWS_SECRET_ACCESS_KEY=$(security find-generic-password -a froglet -s aws-deploy-secret-key -w) \
AWS_DEFAULT_REGION=us-east-1 \
  aws lightsail create-certificate --certificate-name ai-froglet-dev --domain-name ai.froglet.dev

# Fetch the validation CNAME AWS wants us to publish.
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_DEFAULT_REGION=us-east-1 \
  aws lightsail get-certificates --certificate-name ai-froglet-dev --include-certificate-details

# Publish the validation CNAME via Cloudflare. Not proxied — ACM needs raw DNS.
./scripts/cloudflare_dns.sh create CNAME '<validation-subdomain>' '<acm-validation-target>' 300 false

# Wait for cert to flip to status=ISSUED (usually <5 min with Cloudflare's DoH resolvers).

# Attach the cert as a custom public domain.
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_DEFAULT_REGION=us-east-1 \
  aws lightsail update-container-service --service-name froglet-node \
  --public-domain-names '{"ai-froglet-dev": ["ai.froglet.dev"]}'

# Deploy the image. Use a pinned tag, never :latest.
./scripts/deploy_aws.sh deploy ghcr.io/armanas/froglet-provider:v0.1.0-alpha.0

# Point the public hostname at the Lightsail service. Proxied (orange cloud).
./scripts/deploy_aws.sh endpoint    # prints the Lightsail URL to use as CNAME target
./scripts/cloudflare_dns.sh upsert CNAME ai '<lightsail-url-from-endpoint-command>' 300 true
```

Between each `deploy_aws.sh deploy` call, the service takes ~60-180 seconds
to reach `state=RUNNING` and `deployment state=ACTIVE`. Use
`./scripts/deploy_aws.sh status` to watch.

## 4. Verification after deploy

```bash
# Strict-local plus hosted cells. Exits nonzero if any check fails.
FROGLET_DOCS_URL=https://froglet.dev \
FROGLET_HOSTED_PROVIDER_URL=https://ai.froglet.dev \
FROGLET_HOSTED_RUNTIME_URL=https://ai.froglet.dev \
  ./scripts/release_gate.sh --hosted --strict
```

Per-check semantics (see [scripts/hosted_smoke.sh](../scripts/hosted_smoke.sh)):

| Check | What it verifies |
| --- | --- |
| docs url | HTTP 200, `text/html`, body contains "Froglet" (catches parked-page regressions) |
| hosted provider health | `/health` returns `{"status":"ok","service":"froglet"}` |
| capabilities | `/v1/node/capabilities` returns JSON with `api_version=="v1"`, non-empty `identity.node_id`, non-empty `version` |
| identity | `/v1/node/identity` returns JSON with `node_id` and `public_key` of length ≥32 |
| openapi | `/v1/openapi.yaml` starts with `openapi:` |

If any check FAILs, the release-gate step `hosted` returns nonzero and the
summary table at the end highlights the failing row.

## 5. Routine update (new image tag)

```bash
# Pick the tag you want live.
TAG=v0.1.0-alpha.1

# Sanity-check the image actually exists in GHCR.
curl -sI "https://ghcr.io/v2/armanas/froglet-provider/manifests/$TAG" \
  | head -1   # should print: HTTP/2 200

# Deploy.
./scripts/deploy_aws.sh deploy "ghcr.io/armanas/froglet-provider:$TAG"

# Wait for deployment state=ACTIVE.
./scripts/deploy_aws.sh status

# Verify public path still healthy.
FROGLET_HOSTED_PROVIDER_URL=https://ai.froglet.dev \
FROGLET_HOSTED_RUNTIME_URL=https://ai.froglet.dev \
  ./scripts/release_gate.sh --hosted --strict
```

Lightsail keeps the previous deployment alive until the new one passes its
health check. A failed new deployment does not take the service down; it
stays in state=`DEPLOYING` with the old version still serving.

## 6. Rollback

Lightsail indexes every deployment by version number. List them:

```bash
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... AWS_DEFAULT_REGION=us-east-1 \
  aws lightsail get-container-service-deployments --service-name froglet-node
```

Rolling back means re-submitting the last known-good deployment spec. The
`deploy_aws.sh deploy` subcommand always submits the canonical template
from `ops/lightsail/froglet-node.template.json` — so rollback is just
"deploy the previous image tag":

```bash
./scripts/deploy_aws.sh deploy ghcr.io/armanas/froglet-provider:v0.1.0-alpha.0   # revert to the last-green tag
./scripts/deploy_aws.sh status
```

When you cannot reach a green state via image-tag rollback (config drift,
cert invalidation, Cloudflare mis-routing), the contingency is the
`destroy` subcommand — tears the service down entirely:

```bash
./scripts/deploy_aws.sh destroy   # prompts for service name to confirm
```

Re-provision per §3 once you know what changed. The DNS CNAME survives the
destroy + recreate; only the Lightsail URL changes, which `upsert` handles.

## 7. Routine observability

```bash
./scripts/deploy_aws.sh logs    # last 10 min of container logs from CloudWatch
./scripts/deploy_aws.sh endpoint  # prints the live Lightsail URL + recommended CF CNAME
```

Full log retention lives in AWS CloudWatch Logs under the
`/aws/lightsail/containers/froglet-node` log group. Adjust retention via
`aws logs put-retention-policy` when storage cost starts mattering.

## 8. What's intentionally not automated here

- **Cert creation** is one-shot per hostname; the deploy script does not
  re-issue it on every run. Renewal is automatic once issued.
- **DNS CNAME** lives in Cloudflare, not in the Lightsail spec. We
  deliberately keep DNS and compute separately revertible.
- **Multi-region** — explicitly single-region (us-east-1) until there is
  measured latency evidence that a second region is justified.
- **Auto-scaling** — Lightsail Container Service's `scale` parameter is
  static at 1. Vertical scaling is a `power` change via
  `create-container-service` re-spec, not per-deploy.

## 9. Related runbooks

- [docs/RELEASE.md](RELEASE.md) — cutting release tags and publishing
  images (what populates `ghcr.io/armanas/froglet-*` in the first place).
- [docs/SUBDOMAIN_PLAN.md](SUBDOMAIN_PLAN.md) — canonical DNS record
  inventory and the live zone state.
- [docs/RUNTIME.md](RUNTIME.md) — what the node process does at runtime
  (useful when debugging a deploy that starts but misbehaves).
