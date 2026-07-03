# Relay Ingress (v1 contract)

Status: contract specification — the relay **service** implementation lives in
the closed-source marketplace services workspace; the **node-side tunnel
client** lands in this repo behind a config flag. This document is the
interface both sides build against.

Relay ingress gives a froglet provider a public HTTPS address without DNS
setup, TLS certificates, port forwarding, or NAT traversal: the node dials
**out** to the relay and holds a persistent tunnel; the relay terminates TLS
for `https://<label>.relay.froglet.dev` and forwards requests down the tunnel.
This is the enterprise-friendly reachability path (outbound-only, works behind
corporate firewalls) and complements — not replaces — clearnet self-hosting
and Tor hidden services.

**Layering:** relay ingress is a transport adapter. It does not touch the
kernel: signed artifacts simply advertise the relay URL as their
`provider_url`, exactly as clearnet or onion URLs are advertised today.
`provider_resolution` already classifies `https://*.relay.froglet.dev` as
public clearnet HTTPS; no consumer-side changes are required.

## 1. Addressing

Each provider gets a stable hostname derived from its identity key:

```
label     = lowercase RFC 4648 base32, no padding, of the 32-byte provider pubkey
hostname  = <label>.relay.froglet.dev          (label is 52 chars, fits the 63-char DNS limit)
public_url = https://<label>.relay.froglet.dev
```

The label is deterministic and reversible: anyone can recompute the expected
hostname from a `provider_id` and vice versa, so a relay URL self-certifies
which identity it claims to front (the tunnel auth in § 3 proves the claim).

## 2. Transport

- v1 tunnel transport is **WebSocket over TLS (WSS)** to
  `wss://relay.froglet.dev/v1/tunnel`. WSS traverses corporate proxies and is
  trivially implementable on both sides; QUIC/HTTP-3 is an explicit future
  optimization, negotiated via the `capabilities` field (§ 3), not a v1
  requirement.
- One tunnel connection per provider. Reconnect with exponential backoff plus
  jitter (initial 1s, cap 60s). The relay treats a new authenticated tunnel
  for the same identity as a replacement and closes the old one.
- Heartbeat: WebSocket ping/pong every 30s from the relay; three missed pongs
  evict the tunnel and the hostname stops resolving to a backend (returns 503
  with a JSON body `{"error": "provider_offline"}`).

## 3. Authentication (challenge–response, provider identity key)

```
client → relay   {"type": "hello", "provider_id": "<64-hex pubkey>",
                  "capabilities": ["frame.v1"]}
relay  → client  {"type": "challenge", "challenge": "<32-byte hex nonce>"}
client → relay   {"type": "auth", "signature": "<hex Schnorr signature>"}
relay  → client  {"type": "ready", "public_url": "https://<label>.relay.froglet.dev",
                  "heartbeat_secs": 30, "max_body_bytes": 10485760}
```

The signature is over the domain-separated message
`"froglet-relay-auth/v1" || challenge_bytes || pubkey_bytes` using the node's
identity key (the same key and Schnorr scheme as `froglet-node sign-message`).
Challenges are single-use and expire after 60s. Registration is permissionless
— possession of the key is the only requirement — with relay-side quotas
(§ 6).

## 4. Request framing (`frame.v1`)

HTTP requests arriving at the public hostname are forwarded as JSON text
frames; responses return the same `id`:

```
relay  → client  {"id": "<opaque>", "type": "request", "method": "GET",
                  "path": "/v1/feed", "query": "limit=10",
                  "headers": {"accept": "application/json"},
                  "body_b64": ""}
client → relay   {"id": "<opaque>", "type": "response", "status": 200,
                  "headers": {"content-type": "application/json"},
                  "body_b64": "<base64>"}
```

- Bodies are capped at `max_body_bytes` (10 MiB v1, matching the daemon's
  HTTP body limits); oversized requests get a relay-generated 413.
- Header forwarding is allowlist-based in both directions (content-type,
  accept, authorization, content-length, plus `x-froglet-*`). The relay adds
  `x-forwarded-for` and `x-froglet-relay: v1`.
- Per-request timeout: 60s from frame dispatch to response frame; the relay
  answers 504 on expiry and discards late responses by `id`.
- Streaming responses and WebSocket pass-through are out of scope for
  `frame.v1`; they are future capability strings.

## 5. Node-side behavior (this repo)

- Config (naming follows existing `FROGLET_TOR_*` patterns):
  - `FROGLET_RELAY_URL` — relay endpoint, e.g. `wss://relay.froglet.dev/v1/tunnel`;
    unset disables the tunnel.
  - `FROGLET_RELAY_ENABLED` — explicit on/off independent of URL presence
    (bootstrap sets both).
- The tunnel client forwards decoded requests to the local provider listener
  (same loopback backend the Tor hidden service uses).
- On `ready`, the node records the relay `public_url` in `TransportStatus`
  alongside `clearnet_url` / `tor_onion_url`; registration and the publish
  engine pick it up through the existing `/v1/node/capabilities` surface.
- On eviction or disconnect, the node clears the relay URL from
  `TransportStatus` and reconnects per § 2.

## 6. Trust boundary and limits

- **The relay terminates TLS and sees request/response plaintext.** It is
  marketplace-operated infrastructure in the same trust class as the
  marketplace index. Kernel signatures on artifacts, offers, deals, and
  receipts protect *integrity* end-to-end regardless; payload
  *confidentiality* from the relay operator is not provided in v1. Providers
  handling sensitive payloads should use clearnet self-hosting (their own
  TLS) or negotiate end-to-end encryption at the application layer; an
  e2e-encrypted tunnel mode is a candidate v2 capability.
- Relay-side quotas are operator policy (per-identity connection rate,
  request rate, bandwidth); quota errors surface as HTTP 429 with a JSON body
  including a `retry_after_secs` hint.
- The relay never originates requests down the tunnel other than forwarding
  public traffic; the node must still treat every forwarded request as
  untrusted public input.

## 7. Open questions (settle before service build)

1. Hostname reuse after long offline periods — reserve labels indefinitely
   (they are identity-derived, so yes by default) vs. quota-expire mappings.
2. Whether the relay should verify the fronted node serves a signed
   `/v1/feed` matching the tunnel identity before routing (cheap
   anti-confusion check, adds a startup probe).
3. Multi-relay federation and `relay.<region>.froglet.dev` naming.
4. Frame encoding: JSON is v1 for debuggability; CBOR is a candidate
   `frame.v2` if profiling shows overhead.
