# Identity Attestation

Status: specification for the optional identity-attestation layer on the
Froglet marketplace. Implementation tracked as [TODO.md Order 81](../TODO.md).
This document is **normative for the attestation credential shape and the two
attestation flows** (DNS and OAuth/OIDC). It is **not** normative for the
Froglet kernel, which remains identity-agnostic.

## Why this exists

Froglet identities are cryptographic (a signing key). That is sufficient for
the protocol's purposes but insufficient for two adjacent concerns:

1. **Counterparty discovery** — a requester who needs to buy compute from a
   known organization or person wants to filter for providers who have proven
   a real-world link (a domain, a GitHub org, etc.), not just "some pubkey."
2. **Sybil resistance for the arbiter service** — the claims-court design in
   [TODO.md Order 80](../TODO.md) needs an eligibility gate for high-value
   adjudication, and a verified identity attestation is the lever that raises
   the cost of running many adjudicator identities.

Neither concern justifies making attestations mandatory. They are **always
optional**, **always user-initiated**, and **never block a kernel-level deal
flow**. The marketplace service indexes them; consumers filter on them;
nothing else changes.

## Scope

Two attestation kinds ship in the first implementation. Both were explicitly
chosen; stronger identity forms (W3C Verifiable Credentials from regulated
issuers, proof-of-personhood systems like World ID / BrightID) are out of
scope for this doc.

| Kind | Proves | Typical use |
| --- | --- | --- |
| `dns` | The subject controls a DNS zone | Orgs with a brand domain |
| `oauth` | The subject controls an account on a specific OAuth provider (GitHub is the first supported; pattern extends to Google / GitLab / Gitea / Microsoft without protocol changes) | Individual developers, hobbyists |

## Credential shape

An `IdentityAttestation` is a **signed artifact** using the same envelope as
every other Froglet signed artifact. The inner payload:

```json
{
  "schema_version": "froglet/v1",
  "artifact_type": "identity_attestation/v1",
  "subject_pubkey": "<hex-encoded secp256k1 public key of the Froglet identity>",
  "attestation_kind": "dns" | "oauth",
  "attestation_claim": {
    "dns_zone": "example.com"            // for kind=dns
    // OR
    "oauth_provider": "github",          // for kind=oauth
    "oauth_subject": "armanas"            // stable OAuth subject id (not display name)
  },
  "issued_at": "<RFC3339 UTC timestamp>",
  "expires_at": "<RFC3339 UTC timestamp, issued_at + 180 days>",
  "issuer": "<hex-encoded pubkey of the marketplace attestation service>",
  "evidence_ref": {
    "kind": "dns_txt" | "url",
    "locator": "_froglet.example.com"    // for kind=dns
    // OR
    "locator": "https://gist.github.com/armanas/abc123" // for kind=oauth
  }
}
```

The outer envelope is signed by the **marketplace attestation service**, not
by the subject. This matters: the subject's signature proves control of the
Froglet key; the marketplace's signature on the attestation proves that the
marketplace observed the subject's bind statement at the attested URL and
verified the chain at `issued_at`. Consumers verify both.

## Flow 1: DNS attestation

### Preconditions
- The subject already has a Froglet identity key (node identity or any other
  Froglet signing key).
- The subject controls a DNS zone (e.g. `example.com`).

### Steps

1. **Subject signs a bind statement** using their Froglet private key:
   ```
   froglet-identity-bind/v1
   dns:example.com
   <subject_pubkey_hex>
   <current-RFC3339-UTC-timestamp>
   ```
   Output: a hex-encoded signature over the canonical JCS encoding of that
   statement as a JSON object.

2. **Subject publishes a TXT record** at `_froglet.example.com`:
   ```
   _froglet.example.com. 300 IN TXT "v=froglet1; pubkey=<hex>; sig=<hex>; ts=<rfc3339>"
   ```
   TTL is the subject's choice; 300 seconds is a reasonable default.

3. **Subject calls `marketplace.attest_dns`** with the zone name and the
   Froglet pubkey.

4. **The marketplace attestation service**:
   - Resolves the TXT record using **DNS-over-HTTPS** (Cloudflare
     `1.1.1.1` or Google `8.8.8.8` — explicitly not the operator-local
     resolver, which may be compromised and is exactly the DNS-rebind vector
     already discussed in the Order-70 IP-pinning work).
   - Parses the record, verifies the timestamp is within a 10-minute window
     of `now` (replay protection), and verifies the signature against the
     claimed pubkey over the canonical bind statement.
   - If everything checks out, issues and signs the `IdentityAttestation`
     credential, stores it in the attestation index, returns it to the caller.

### Re-verification

A DNS attestation expires 180 days after issuance. A background job
re-resolves every 30 days; a failed re-resolution (record removed, zone
transferred, signature no longer valid) invalidates the attestation
immediately. The credential's `expires_at` is treated as a hard ceiling;
verifiers MUST reject expired attestations regardless of cache state.

### What this proves, what it does not

- **Proves:** the subject controlled the DNS zone at `issued_at` and held the
  corresponding Froglet private key at the same moment.
- **Does not prove:** the subject is the legal owner of the zone (they might
  be a tenant with delegated DNS access), nor that the subject is the same
  human over time (zone ownership can transfer silently).

## Flow 2: OAuth / OIDC attestation

### Preconditions
- The subject already has a Froglet identity key.
- The marketplace service has a registered OAuth app with the target provider
  (GitHub first). Client id and secret are deployment-time config, not
  protocol-level.

### Steps

1. **Subject signs the same bind statement** as the DNS flow, but with the
   OAuth locator:
   ```
   froglet-identity-bind/v1
   oauth:github:armanas
   <subject_pubkey_hex>
   <current-RFC3339-UTC-timestamp>
   ```

2. **Subject posts the signed statement at a URL the OAuth provider can
   authoritatively attribute to them.** For GitHub, any of:
   - A public gist owned by `@armanas`.
   - A file at a known path in a repository the subject owns.
   - A tagged release body on a repository the subject owns.
   - The user's profile README at `github.com/<subject>/<subject>`.

   The posted content is the full signed statement plus a one-line preamble
   identifying it as a Froglet identity bind. Example:
   ```
   --- FROGLET IDENTITY BIND ---
   froglet-identity-bind/v1
   oauth:github:armanas
   <subject_pubkey_hex>
   <current-RFC3339-UTC-timestamp>
   --- SIGNATURE ---
   <hex-encoded signature>
   --- END ---
   ```

3. **Subject initiates `marketplace.attest_oauth`** by calling the handler
   with the URL and completing the OAuth authorization code flow against the
   marketplace's registered app. The authorization grants the marketplace a
   short-lived access token scoped to **reading the authenticated user's
   basic profile only** — not to repo write, not to long-lived refresh.

4. **The marketplace attestation service**:
   - Exchanges the authorization code for the access token.
   - Reads the authenticated user's stable OAuth subject id (`login` field
     for GitHub; the analogous stable id for other providers). Display name
     is never used because it is mutable.
   - Fetches the URL from the posted locator. Verifies the URL is owned by
     the authenticated user according to the OAuth provider's authority
     model (gist owned by user, file path inside user's repo, etc.).
   - Parses the posted bind statement, verifies the timestamp (same 10-minute
     window as DNS), verifies the signature matches the claimed pubkey.
   - If everything checks out, issues and signs the `IdentityAttestation`
     credential. Discards the OAuth access token; it is never persisted.

### Re-verification

OAuth attestations re-verify every 30 days by fetching the posted URL (no
OAuth required for re-read since the locator is public) and confirming the
bind statement is still there and still signature-valid. Deletion of the
posted statement invalidates the attestation. The 180-day expiry is still a
hard ceiling; full re-attestation with a fresh OAuth flow is required to
renew.

### What this proves, what it does not

- **Proves:** the subject controlled the OAuth account at `issued_at` and
  held the corresponding Froglet key at the same moment.
- **Does not prove:** the subject is the human named in the OAuth profile.
  OAuth accounts can be sold, transferred, or operated on behalf of others.

## Consumer verification

Any party presented with an `IdentityAttestation` credential MUST verify, in
order:

1. The outer signature over the credential is valid under the marketplace
   attestation service's published pubkey (baked into the marketplace
   service's discovery record).
2. `subject_pubkey` matches the Froglet identity the consumer is evaluating.
3. `expires_at > now`.
4. For high-assurance use cases only: re-fetch the evidence (TXT record or
   posted URL) and re-verify the subject's signature live. Most consumers
   can trust the marketplace's indexed cache; adjudicators on high-value
   disputes should re-verify live.

## How this surfaces in search and in the arbiter

- `marketplace.search` results include an `attestations` array per provider
  entry, each with `kind`, `claim`, `issued_at`, `expires_at`. Requesters can
  filter with query params (e.g. `attestation_kind=dns` or
  `attestation_dns_zone=example.com`).
- `marketplace.provider` returns the same array for a single provider.
- The arbiter service ([TODO.md Order 80](../TODO.md)) configures a
  **value threshold** above which adjudicator eligibility requires at least
  one `dns` or `oauth` attestation on the adjudicator's identity. Below the
  threshold, any staked identity is eligible.

## What is deliberately not included

- No storage of OAuth refresh tokens, ever. Attestation is one-shot.
- No cross-chain or on-chain anchoring of attestations. The marketplace
  signature is the root of trust; the marketplace service pubkey is what
  consumers trust.
- No W3C Verifiable Credentials envelope. The Froglet signed-artifact
  envelope is the format used everywhere else in the system and is used here
  for consistency. A VC wrapper is a possible later addition if interop with
  external VC ecosystems becomes a requirement.
- No proof-of-personhood. Explicitly out of scope per the Order-81 design.
- No attestation revocation by the subject without expiry. The re-verification
  loop plus the 180-day ceiling are the revocation mechanism. A subject who
  loses control of their DNS zone or OAuth account gets automatic invalidation
  within 30 days.
