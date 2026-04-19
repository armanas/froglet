# Monitoring, Alerting, and Rollback Runbook

Status: **living document**. The doc half of [TODO.md Order 17](../TODO.md)
is closeable via this runbook. The "real alert destinations + on-call
decision" half is explicitly marked as **PENDING HUMAN ACTION** below and
keeps Order 17 at 🟡 until wired.

## 1. What is monitored

The hosted environment is intentionally small. The monitoring surface
matches:

| Surface | Source | How observed |
| --- | --- | --- |
| Public endpoint reachability | Cloudflare edge → Lightsail ALB | BetterStack uptime check against `https://ai.froglet.dev/health` |
| Response shape correctness | Froglet node | `./scripts/release_gate.sh --hosted --strict` cron or post-deploy |
| Container-level logs | Lightsail → CloudWatch Logs group `/aws/lightsail/containers/froglet-node` | `./scripts/deploy_aws.sh logs` or AWS console |
| Container state | Lightsail API | `./scripts/deploy_aws.sh status` returns `state=RUNNING`, `deployment state=ACTIVE` |
| Certificate expiry | ACM-issued cert attached to Lightsail | AWS auto-renews; BetterStack cert-expiry monitor as backstop |
| AWS bill | AWS Budgets `froglet-monthly-cost` | Email at 85% + 100% of $50/mo threshold |

## 2. What can fail, and what the operator does

### 2.1 Bad deploy (new image is broken)

**Signal.** BetterStack health monitor flips to DOWN. `./scripts/deploy_aws.sh status` shows `deployment state=ACTIVATING` or `FAILED` while the previous deployment version is still running.

**Recovery.** Lightsail does NOT swap traffic until the new deployment passes its health check, so 99% of the time this self-heals — the broken tag just never goes live. If the operator wants to abandon the attempt:

```bash
./scripts/deploy_aws.sh deploy ghcr.io/armanas/froglet-provider:<last-green-tag>
```

See [OPERATOR_DEPLOY.md §6](OPERATOR_DEPLOY.md#6-rollback) for the full rollback tree.

### 2.2 Container crash loop on a previously-healthy deploy

**Signal.** BetterStack DOWN AND `./scripts/deploy_aws.sh status` shows the service cycling. `./scripts/deploy_aws.sh logs` will show the error.

**Recovery.**
1. Identify cause from logs (usually: a runtime config change or an expired Keychain secret that got baked into env vars).
2. If cause is config, update `ops/lightsail/froglet-node.template.json`, commit, and re-deploy.
3. If cause is secret rotation mid-deploy, follow [ROTATION.md](ROTATION.md).

### 2.3 Cloudflare→Lightsail HTTPS break (404 "No Such Service")

**Signal.** `curl https://ai.froglet.dev/health` returns HTTP 404 with `server: cloudflare`, but `curl <lightsail-url>/health` direct-to-origin returns 200.

**Root cause.** Lightsail ACM custom-domain attachment was removed or cert lapsed. Its Host-header check rejects `ai.froglet.dev` and returns 404.

**Recovery.** Re-check cert status and custom-domain attachment. Re-attach via the `update-container-service --public-domain-names` call in [OPERATOR_DEPLOY.md §3](OPERATOR_DEPLOY.md#3-first-deploy).

### 2.4 DNS-level breakage (CNAME dropped, nameservers flipped)

**Signal.** `dig NS froglet.dev +short` no longer returns Cloudflare nameservers, OR `dig CNAME ai.froglet.dev +short` is empty.

**Recovery.** Re-provision via `./scripts/cloudflare_dns.sh upsert CNAME ai <lightsail-url> 300 true`. For full nameserver failures, escalate to the registrar (Namecheap).

### 2.5 Credit exhaustion → real money starts flowing

**Signal.** AWS Budgets email at 85% of $50/mo threshold.

**Recovery.** This is a business-continuity question, not a technical one. Either:
- Apply more AWS credits (AWS Activate, direct codes).
- Accept the bill (post-credit, the expected Lightsail + RDS + Voltage floor is ~$65/mo per [docs/COMPUTE_PROVIDERS.md](#) *[doc not yet written; lives in session plan]*).
- Destroy the service (`./scripts/deploy_aws.sh destroy`) if there's a reason to stop.

### 2.6 Cloudflare account compromise

**Signal.** Unexpected DNS records in the zone, or `cloudflare_dns.sh list` returns records you did not create.

**Recovery.** Revoke the compromised token immediately in Cloudflare dashboard. Rotate per [ROTATION.md](ROTATION.md). Inventory the zone records and delete anything out-of-plan.

## 3. Deployment history

Lightsail preserves every deployment with a monotonic version number.

```bash
AWS_ACCESS_KEY_ID=$(security find-generic-password -a froglet -s aws-deploy-access-key -w) \
AWS_SECRET_ACCESS_KEY=$(security find-generic-password -a froglet -s aws-deploy-secret-key -w) \
AWS_DEFAULT_REGION=us-east-1 \
  aws lightsail get-container-service-deployments --service-name froglet-node
```

Each entry shows the image tag, state, and timestamps. That's the forensic
record for "what was running at 03:17 UTC when the alert fired."

## 4. Uptime checks — what to configure

**PENDING HUMAN ACTION** ([TODO.md Order 63](../TODO.md) tracks this as the
status page lane). The runbook below is the intended target once BetterStack is signed up.

Minimum check set:

1. `https://ai.froglet.dev/health` — HTTP 200, body contains `"status":"ok"`, 1-minute interval.
2. `https://froglet.dev/` — HTTP 200, body contains `Froglet`, 5-minute interval (once docs hosting lands).
3. `https://marketplace.froglet.dev/health` — same envelope as node, once the marketplace read API lands ([TODO.md Order 64](../TODO.md)).
4. ACM cert expiry monitors on both hostnames (BetterStack supports this natively).

## 5. Alert routing — PENDING HUMAN ACTION

This is the half of Order 17 that LLM automation cannot close. Specific open decisions:

- **Where do alerts go?** Options: email, SMS, Slack, Discord, Telegram, PagerDuty.
- **Who is on-call?** Solo operator today (you). If that changes, there must be a rotation.
- **What severity tiers?** Recommendation: two tiers only — P1 (public path down) and P2 (non-path signal like budget warning).
- **What's the escalation path** if the primary contact doesn't ack a P1 within 15 minutes?

Until these decisions are made, the doc structure here is complete but the
wires aren't connected. Order 17 stays 🟡.

Recommended minimum for solo operator at launch:

- Email to primary address for P1 and P2.
- BetterStack free tier does the uptime check; email is the only destination.
- No on-call rotation; the operator is the operator.
- No escalation; if the operator misses the alert, the service is down longer. This is explicitly acceptable for an alpha.

## 6. Cross-references

- [docs/OPERATOR_DEPLOY.md](OPERATOR_DEPLOY.md) — deploy and rollback mechanics.
- [docs/ROTATION.md](ROTATION.md) — secret rotation when a credential is suspected compromised.
- [docs/SECURITY_PASS.md §3.3](SECURITY_PASS.md) — the top-risk table tied to this surface.
- [scripts/hosted_smoke.sh](../scripts/hosted_smoke.sh) — the live-content assertions run by the release gate.

## 7. Closure criteria for Order 17

- [x] Runbook exists and covers logs, uptime checks, alert routing, deployment history, rollback (this doc).
- [ ] **PENDING HUMAN ACTION:** real alert destination configured in BetterStack (or chosen alternative).
- [ ] **PENDING HUMAN ACTION:** on-call path documented (single operator is an explicit decision; document it here).
- [ ] One simulated failure exercised end-to-end: deploy a deliberately broken image, watch the alert fire, roll back, watch the alert clear.

Until the three unchecked items above are done, Order 17 stays 🟡.
