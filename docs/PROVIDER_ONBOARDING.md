# Provider onboarding without owning a domain

Status: normative for the four supported MVP provider onboarding paths.
Identity-attestation specifics remain in
[IDENTITY_ATTESTATION.md](./IDENTITY_ATTESTATION.md); this document only
covers how to be a discoverable Froglet provider.

The Froglet protocol is identity-agnostic at the kernel layer. A provider
is anyone who can sign descriptors and offers with a Froglet key and
expose `/v1/node/capabilities` + `/v1/feed` at a public URL. Owning a DNS
zone is **optional** — it is one of two paths to identity attestation, and
attestation itself is optional. This document covers the four supported
MVP paths to a discoverable provider without a domain of your own.

| Path | Public URL form | What you need | Identity attestation |
|---|---|---|---|
| A — Tor v3 hidden service | `http://<56-char-hash>.onion` | Local `tor` daemon | optional |
| B — Managed `<slug>.providers.<suffix>` | `https://<slug>.providers.froglet.dev` (first-party) | Public IP + Schnorr-signing key | optional |
| D — PaaS endpoint | `https://<name>.fly.dev` / `<name>.onrender.com` / etc. | PaaS account | optional |
| E — Key-only at any public URL | any `https://` URL you control | the public URL itself | none |

All four converge on the same kernel flow: descriptor + offer signed by
your provider key, published from your `/v1/feed`, observed and verified
by the marketplace. The marketplace's
[`/v1/registrations` endpoint](../../froglet-services/services/marketplace-api/src/registration.rs)
is the single entrypoint regardless of path.

---

## Path A — Tor v3 hidden service

### Why pick this
- Strongest DNS-free posture: the `.onion` hash is your address and your
  identity. No domain, no TLS cert, no port forwarding, works behind any
  NAT.
- Tor v3 provides end-to-end authentication and encryption; the
  marketplace verifies the onion address matches your descriptor's
  advertised transport.

### Operator-side prerequisites
The first-party marketplace at `marketplace.froglet.dev` must have
`FROGLET_TOR_SOCKS_PROXY` set (default `socks5h://127.0.0.1:9050`) and a
Tor client reachable on that endpoint. Without it, the marketplace cannot
fetch your `/v1/node/capabilities` to verify the registration. Confirm
the env var is wired by checking `https://marketplace.froglet.dev/healthz`
followed by a probe registration; a failure surfaces as `502 bad gateway`
on the `/v1/registrations` call.

### Provider-side flow
1. Install Tor: `brew install tor` (macOS) or `apt-get install tor` (Debian/Ubuntu).
2. Add a hidden service to `/etc/tor/torrc` (or wherever your distro puts
   it) pointing at your local Froglet node port:
   ```
   HiddenServiceDir /var/lib/tor/froglet/
   HiddenServiceVersion 3
   HiddenServicePort 80 127.0.0.1:8910
   ```
   Replace `8910` with whatever port your `froglet-node` binds.
3. `systemctl reload tor` (or `tor --runasdaemon 1` on a workstation).
4. Read the generated `.onion` address: `cat /var/lib/tor/froglet/hostname`.
5. Start `froglet-node` with the onion URL advertised in its
   capabilities:
   ```bash
   FROGLET_PUBLIC_BASE_URL="http://<your-v3-hash>.onion" \
   FROGLET_TRANSPORT_TOR_ENABLED=1 \
   FROGLET_TRANSPORT_TOR_URL="http://<your-v3-hash>.onion" \
   froglet-node serve
   ```
6. Register with the marketplace:
   ```bash
   curl -sS -X POST https://marketplace.froglet.dev/v1/registrations \
     -H "content-type: application/json" \
     -d '{"provider_url":"http://<your-v3-hash>.onion","transport":"tor"}'
   ```

### What can go wrong
- **Marketplace returns 502**: marketplace Tor proxy is not reachable.
  Operator config issue — file an issue or self-host the marketplace.
- **400 "tor registration requires an http://<tor-v3>.onion provider_url"**:
  you sent a v2 onion or a malformed address. Only Tor v3 (56-char
  base32) is accepted.
- **422 "capabilities advertise X, expected Y"**: `transports.tor.url`
  in your node's `/v1/node/capabilities` does not match what you sent
  to `/v1/registrations`. Make them byte-identical.

---

## Path B — Managed `<slug>.providers.<suffix>`

### Why pick this
- You have a public IP (home or cloud) but no domain.
- The marketplace operator allocates you a DNS record under their
  parent zone — `demo-1.providers.froglet.dev` on the first-party
  marketplace.
- TLS is your responsibility (Caddy with HTTP-01 ACME on port 80/443
  works once DNS resolves).

### How the operator enables this
The marketplace's [`provider_domain_suffix()`](../../froglet-services/services/marketplace-api/src/domain_claims.rs)
reads three env vars; without all of them, claims still succeed but
return `pending_operator_dns` for the operator to fulfil by hand. To
fully automate:

| Env var | Required? | Value |
|---|---|---|
| `PROVIDER_DOMAIN_SUFFIX` | optional (default `providers.froglet.dev`) | the parent zone under which slugs are allocated |
| `PROVIDER_DOMAIN_CLOUDFLARE_ZONE_ID` | required to auto-write DNS | Cloudflare zone ID for the parent zone |
| `PROVIDER_DOMAIN_CLOUDFLARE_API_TOKEN` | required to auto-write DNS | API token with `Zone:DNS:Edit` permission on that zone |

Operators forking the marketplace under a different domain (`example.org`)
set `PROVIDER_DOMAIN_SUFFIX=providers.example.org` and supply Cloudflare
credentials for that zone (or fork the
[`create_cloudflare_dns_record`](../../froglet-services/services/marketplace-api/src/domain_claims.rs)
adapter for non-Cloudflare DNS).

### Provider-side flow
The claim has two phases (intent → complete) plus an optional poll. Both
phases require Schnorr signatures by the provider key.

**Phase 1 — request a slug**:

```bash
# Read your provider pubkey from your already-running froglet-node:
PROVIDER_PUBKEY=$(froglet-node print-identity)
PUBLIC_IP=<your public IPv4 or IPv6>
SLUG=demo-1

# Sign the intent message with your provider private key. The newlines and
# field order in the message are part of the signed bytes; do not reformat.
INTENT_MSG="froglet-provider-domain-claim-intent-v1
provider_id:${PROVIDER_PUBKEY}
slug:${SLUG}
hostname:${SLUG}.providers.froglet.dev
public_ip:${PUBLIC_IP}"

INTENT_SIG=$(printf '%s' "$INTENT_MSG" | froglet-node sign-message)

curl -sS -X POST https://marketplace.froglet.dev/v1/provider-domains/claims \
  -H "content-type: application/json" \
  -d "{\"provider_id\":\"${PROVIDER_PUBKEY}\",\"requested_slug\":\"${SLUG}\",\"public_ip\":\"${PUBLIC_IP}\",\"intent_signature\":\"${INTENT_SIG}\"}"
```

The response contains `claim_id`, `signing_message`, and a 15-minute
`expires_at`.

**Phase 2 — complete with the second signature**:

```bash
CLAIM_ID=<from phase 1 response>
SIGNING_MESSAGE=<from phase 1 response, multi-line, exactly as returned>
COMPLETE_SIG=$(printf '%s' "$SIGNING_MESSAGE" | froglet-node sign-message)

curl -sS -X POST "https://marketplace.froglet.dev/v1/provider-domains/claims/${CLAIM_ID}/complete" \
  -H "content-type: application/json" \
  -d "{\"signature\":\"${COMPLETE_SIG}\"}"
```

If Cloudflare creds are configured on the marketplace, the response is
`status:"active"` with a `dns_record_id`. Otherwise the response is
`status:"pending_operator_dns"` and you poll the new GET endpoint:

```bash
curl -sS "https://marketplace.froglet.dev/v1/provider-domains/claims/${CLAIM_ID}"
```

until `status` becomes `active`.

**After DNS is live**:
- Verify with `dig +short <slug>.providers.froglet.dev` — it should
  return your `PUBLIC_IP`.
- Stand up TLS on the public hostname. With Caddy:
  ```
  <slug>.providers.froglet.dev {
    reverse_proxy 127.0.0.1:8910
  }
  ```
  Caddy will fetch a Let's Encrypt cert via HTTP-01 the first time the
  hostname resolves to your box.
- Start `froglet-node` advertising the new hostname in its capabilities:
  ```bash
  FROGLET_PUBLIC_BASE_URL="https://<slug>.providers.froglet.dev" \
  froglet-node serve
  ```
- Register the standard way:
  ```bash
  curl -sS -X POST https://marketplace.froglet.dev/v1/registrations \
    -H "content-type: application/json" \
    -d '{"provider_url":"https://<slug>.providers.froglet.dev"}'
  ```

### What can go wrong
- **409 "hostname X already has a live claim"**: another claim holds
  the slot. Wait 15 minutes or pick a different slug.
- **422 "intent_signature does not verify"**: signature is computed
  over the wrong message bytes. The intent message starts with
  `froglet-provider-domain-claim-intent-v1` — the complete-time
  message starts with `froglet-provider-domain-claim-v1\n` (note the
  newline). They are deliberately distinct to prevent replay.
- **`pending_operator_dns` never flips to `active`**: marketplace has
  no Cloudflare creds. Email the operator with your `claim_id` and
  IP; they add the A/AAAA record manually.
- **Slug rejected (400)**: slugs are 6-63 chars, lowercase ASCII +
  digits + interior hyphens, no reserved names (`admin`, `api`,
  `marketplace`, etc.). See
  [`RESERVED_PROVIDER_DOMAIN_SLUGS`](../../froglet-services/services/marketplace-api/src/domain_claims.rs).

---

## Path D — PaaS-hosted endpoint

### Why pick this
- Free or near-free tier on most providers.
- Zero DNS / TLS / port-forwarding work.
- Stable enough URL for a Froglet provider that does not need to be
  always-on.

### Tested PaaS shapes
The marketplace's
[`normalize_submitted_provider_url`](../../froglet-services/services/marketplace-api/src/registration.rs)
accepts any `https://` URL that:
- has a public hostname (not `localhost`, `.local`, `.internal`, etc.),
- DNS-resolves to a public IP (not 10/8, 172.16/12, 192.168/16, link-local,
  documentation, or unspecified),
- has no path component (origin URL only),
- omits credentials, query, or fragment.

That accepts every common PaaS subdomain. Verified shapes:

| PaaS | URL shape | Notes |
|---|---|---|
| Fly.io | `https://<app>.fly.dev` | Free Hobby plan; 256MB RAM minimum is enough for `froglet-node` |
| Render | `https://<app>.onrender.com` | Free tier idles after 15min — fine for low-traffic demos |
| Railway | `https://<app>.up.railway.app` | Free trial credits, paid beyond |
| Vercel | `https://<app>.vercel.app` | Serverless funcs may not satisfy `/v1/feed` polling — test before relying |
| Cloudflare Tunnel | `https://<name>.trycloudflare.com` or your own zone | Trial URLs rotate; not stable enough to register without a custom hostname |

### Provider-side flow
1. Deploy the `froglet-node` container to your PaaS of choice. The
   official image is at `ghcr.io/armanas/froglet-provider:<version>`.
2. Configure the public base URL via env:
   ```
   FROGLET_PUBLIC_BASE_URL=https://<your-paas-url>
   ```
3. Make sure the PaaS exposes the container's listen port externally
   on `https://` with a valid TLS cert.
4. Register:
   ```bash
   curl -sS -X POST https://marketplace.froglet.dev/v1/registrations \
     -H "content-type: application/json" \
     -d '{"provider_url":"https://<your-paas-url>"}'
   ```

### What can go wrong
- **400 "provider_url host X did not resolve"**: PaaS URL is not live
  yet. Wait for deploy to finish.
- **400 "provider_url resolves to a local or private address"**: this
  is a misconfigured DNS record (you pointed a custom domain at
  `127.0.0.1`). PaaS-generated subdomains do not trip this.
- **422 "capabilities advertise X, expected Y"**: `FROGLET_PUBLIC_BASE_URL`
  was not set on the PaaS deploy. The node defaults to the bind
  address, which the marketplace will reject when it doesn't match.

---

## Path E — Key-only registration (no attestation)

### Why pick this
- Lowest friction. Combined with A, D, or self-host on any cloud, gets
  you from zero to "I'm a Froglet provider" in minutes.
- Identity attestation is optional; the kernel never requires it. See
  [IDENTITY_ATTESTATION.md](./IDENTITY_ATTESTATION.md): "attestations
  are always optional, always user-initiated, and never block a
  kernel-level deal flow."

### What this is
There is no separate "no attestation" code path — it is the default for
any provider that does not issue a DNS or OAuth attestation. Such
providers:

- Appear in `GET /v1/providers` by default (the default query has
  `attested=false`).
- Are filtered out by `GET /v1/providers?attested=true` — useful for
  consumers who want stronger identity guarantees.
- Are valid counterparties for the full kernel deal flow regardless of
  attestation status.

### Provider-side flow
Any of A, D, or self-hosted-anywhere applies. The
`/v1/registrations` call simply omits any attestation step. The
marketplace records the public URL, fetches `/v1/feed`, verifies the
descriptor + offer signatures, and you're live.

### What can go wrong
- **Discoverability ranking**: consumers using `attested=true` will
  not see you. If you want broader discoverability and don't have a
  domain, the OAuth attestation path in IDENTITY_ATTESTATION.md is the
  no-domain way to upgrade (GitHub-based; the issuance service ships
  in v0.2).

---

## Path comparison summary

| Concern | A (Tor) | B (Managed) | D (PaaS) | E (Key-only) |
|---|---|---|---|---|
| DNS work for provider | none | none | none | none |
| TLS cert for provider | none (Tor) | yes (Caddy/HTTP-01) | none (PaaS handles) | depends on URL |
| Public IP required | no | yes | no | depends |
| Behind NAT works? | yes | only with port-forward | yes | depends |
| Latency | higher (~500ms+) | low | low (PaaS warm) | low |
| Attestation possible? | OAuth only | OAuth only | OAuth or DNS-on-PaaS-domain | OAuth |
| Operator-side prerequisite | Tor proxy on marketplace | Cloudflare token on marketplace | none | none |

---

## Operator self-host checklist

If you are running your own marketplace (forking `froglet-services`):

1. **Decide your parent zone**: set `PROVIDER_DOMAIN_SUFFIX=providers.<your-domain>`
   on your marketplace-api deploy.
2. **Enable Option A**: ensure a `tor` daemon is reachable from the
   marketplace and set `FROGLET_TOR_SOCKS_PROXY=socks5h://<host>:<port>`.
3. **Enable Option B**: set `PROVIDER_DOMAIN_CLOUDFLARE_ZONE_ID` and
   `PROVIDER_DOMAIN_CLOUDFLARE_API_TOKEN` on your marketplace-api deploy.
   If your DNS is not Cloudflare, fork
   [`create_cloudflare_dns_record`](../../froglet-services/services/marketplace-api/src/domain_claims.rs)
   to use your provider's API.
4. **Options D and E work out of the box** — no operator-side config
   beyond a running marketplace.

The first-party marketplace at `marketplace.froglet.dev` configures
suffix `providers.froglet.dev` and a Cloudflare API token; first-party
providers using Option B receive `<slug>.providers.froglet.dev` records
automatically.
