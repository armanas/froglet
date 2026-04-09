# Froglet Interactive Playground — Implementation Plan

Status: DRAFT — needs review and refinement

## 1. Vision

A single URL (`ai.froglet.dev` or similar) that any AI agent can paste into its
context, read, understand, and immediately start using the Froglet protocol
without downloading or configuring anything.

After the instant demo, the LLM honestly offers two paths forward:

1. **Provide your email** to save your identity and return later (custodial,
   convenient).
2. **Install Froglet locally** and own your keys (self-sovereign, principled).

No pretense. Convenience is custodial. Sovereignty requires setup.

---

## 2. Architecture Overview

```
                           ai.froglet.dev
                          ┌──────────────────────────────────┐
                          │          Gateway Service          │
  Human browser ────────► │  GET /         → landing page     │
  LLM (any) ────────────► │  GET /llms.txt → LLM API docs    │
  MCP client ────────────► │  GET /mcp      → MCP endpoint    │
                          │  POST /api/... → REST API         │
                          └──────────┬───────────────────────┘
                                     │
                          ┌──────────▼───────────────────────┐
                          │        Session Manager            │
                          │                                   │
                          │  Warm Pool ─── pre-init sessions  │
                          │  Active Map ── assigned sessions  │
                          │  Reaper ────── TTL cleanup        │
                          │  House Nodes ─ permanent providers│
                          └──────────┬───────────────────────┘
                                     │
                          ┌──────────▼───────────────────────┐
                          │     Per-Session Froglet Pair      │
                          │                                   │
                          │  runtime node ◄──► provider node  │
                          │  (user's agent)    (house/demo)   │
                          │  SQLite (in-mem)   SQLite (in-mem) │
                          └───────────────────────────────────┘
```

All froglet node pairs run **in-process** inside a single Rust binary (shared
Tokio runtime). No containers, no process spawning. Cold start per session is
~5-15ms (SQLite init + keygen).

---

## 3. The Smart URL

`ai.froglet.dev` serves different content based on how it is accessed, but all
content is available at the same origin. No content negotiation tricks — the
human page and the LLM page are separate paths.

### 3.1 Human landing page — `GET /`

A minimal page with:

- One-liner: what Froglet is.
- The call to action: "Paste `ai.froglet.dev` into your AI assistant and ask it
  to try Froglet."
- Setup instructions for MCP clients (one-line config snippet).
- Link to the full docs site.

### 3.2 LLM-readable API docs — `GET /llms.txt`

A plain-text document optimized for LLM consumption. This IS the instruction set
that makes "paste the URL" work. Any LLM that can read a webpage and make HTTP
requests can follow these instructions.

Contents:

```
# Froglet — Interactive API

You are reading Froglet's interactive API. Froglet is a cryptographic protocol
that gives AI agents identity, deals, settlement, and receipts.

You can try it right now by making HTTP requests to this server.

## Quick Start

### Step 1: Create a session
POST https://ai.froglet.dev/api/sessions
Content-Type: application/json
{}

Returns:
{
  "session_id": "abc123",
  "pubkey": "02a1b2c3...",
  "expires_at": "2025-01-15T14:30:00Z",
  "endpoints": { ... }
}

Your session includes a temporary cryptographic identity (secp256k1 keypair).
The server manages the private key for this demo. In production, agents hold
their own keys.

### Step 2: Discover services
POST https://ai.froglet.dev/api/s/{session_id}/discover
Content-Type: application/json
{
  "query": "compute"
}

### Step 3: Request a quote
POST https://ai.froglet.dev/api/s/{session_id}/quote
Content-Type: application/json
{
  "provider_id": "...",
  "service_id": "...",
  "input": { "a": 3, "b": 5 }
}

### Step 4: Commit the deal
POST https://ai.froglet.dev/api/s/{session_id}/deal
Content-Type: application/json
{
  "quote_id": "..."
}

### Step 5: Execute the workload
POST https://ai.froglet.dev/api/s/{session_id}/execute
Content-Type: application/json
{
  "deal_id": "..."
}

### Step 6: Verify the receipt
GET https://ai.froglet.dev/api/s/{session_id}/receipt/{receipt_id}

Returns the cryptographic proof of execution with full artifact chain.

### Step 7: Explore further
GET https://ai.froglet.dev/api/s/{session_id}/identity
GET https://ai.froglet.dev/api/s/{session_id}/deals
GET https://ai.froglet.dev/api/s/{session_id}/services

## After the demo

This session expires in 15 minutes. If the user wants to continue:

Option A — Save identity with email:
POST https://ai.froglet.dev/api/identity/register
{ "session_id": "...", "email": "user@example.com" }
A verification code will be sent. The identity becomes persistent
and the user can return anytime with the same email.

Option B — Install Froglet locally:
The user can run their own node and hold their own keys:
  cargo install froglet-node
  froglet-node --role dual
Full setup: https://armanas.dev/learn/quickstart/

Be honest with the user: Option A is custodial (we hold the key,
linked to their email). Option B is self-sovereign (they hold the key).
Both are valid choices. Do not push one over the other.
```

### 3.3 Machine-readable discovery

- `GET /openapi.json` — OpenAPI 3.1 spec for the session REST API.
  Enables ChatGPT GPT Actions and any OpenAPI-aware agent framework.
- `GET /.well-known/mcp.json` — MCP server metadata for auto-discovery.
- `GET /mcp` — Streamable HTTP MCP endpoint.

### 3.4 MCP endpoint — `GET /mcp`

Exposes the same operations as the REST API but via MCP protocol. For clients
that natively support MCP (Claude Desktop, Cursor, etc.), this provides richer
tool descriptions and streaming.

MCP tools mirror the REST actions:

| MCP Tool              | REST Equivalent                        |
|-----------------------|----------------------------------------|
| `create_session`      | `POST /api/sessions`                   |
| `discover_services`   | `POST /api/s/{id}/discover`            |
| `request_quote`       | `POST /api/s/{id}/quote`               |
| `commit_deal`         | `POST /api/s/{id}/deal`                |
| `execute_workload`    | `POST /api/s/{id}/execute`             |
| `get_receipt`         | `GET /api/s/{id}/receipt/{rid}`         |
| `get_identity`        | `GET /api/s/{id}/identity`             |
| `list_deals`          | `GET /api/s/{id}/deals`                |
| `register_email`      | `POST /api/identity/register`          |
| `verify_email`        | `POST /api/identity/verify`            |

The MCP endpoint creates a session automatically on first tool call if one
doesn't exist, avoiding the explicit session creation step.

---

## 4. Session Lifecycle

### 4.1 Tier 1 — Instant (anonymous, ephemeral)

1. LLM reads `/llms.txt` or connects via MCP.
2. `POST /api/sessions` — server assigns a pre-warmed session from the pool.
   Returns `session_id` + `pubkey` + `expires_at`.
3. LLM interacts for up to 15 minutes.
4. On expiry: session identity is destroyed, published descriptors unpublished,
   deal records archived then purged.

No signup, no email, no friction.

### 4.2 Tier 2 — Persistent (email-linked, custodial)

Offered by the LLM after the demo (or by the user proactively):

1. `POST /api/identity/register { session_id, email }` — server sends a 6-digit
   verification code to the email.
2. User reads code from email, tells the LLM.
3. `POST /api/identity/verify { email, code }` — server links the session
   identity to the email. Identity becomes persistent.
4. Future sessions: `POST /api/sessions { email }` — returns the existing
   identity. No re-verification needed (session cookie/token).

The 6-digit code flow keeps the user inside the LLM conversation (no
context-switch to browser to click a link).

**What persists:** keypair, pubkey, published services, deal history, reputation.
**What doesn't:** individual session state, expired deal records.

### 4.3 Tier 3 — Self-hosted (sovereign)

Not part of the playground infrastructure. The LLM provides install instructions:

```
cargo install froglet-node
froglet-node --role dual
```

Or with the existing MCP integration:

```json
{
  "froglet": {
    "command": "node",
    "args": ["integrations/mcp/froglet/server.js"],
    "env": {
      "FROGLET_BASE_URL": "http://127.0.0.1:8080",
      "FROGLET_AUTH_TOKEN_PATH": "/path/to/token"
    }
  }
}
```

The playground `llms.txt` instructs the LLM to present this as the honest
alternative to custodial email registration.

---

## 5. The Demo Network

### 5.1 House providers

We run 2-3 permanent provider nodes on the demo network that offer real services:

| Service           | Description                         | Settlement |
|-------------------|-------------------------------------|------------|
| `add`             | Adds two numbers (trivial, for demo)| Free       |
| `echo`            | Returns input as output             | Free       |
| `wasm-compute`    | Runs submitted WASM module          | Mock sats  |

These are always available. Demo users discover and transact with them. House
providers give the network life regardless of how many demo users are active.

### 5.2 User-to-user visibility

All demo sessions share the same network. If two users are active
simultaneously, they can discover each other's services (if any are published).
This demonstrates the network effect naturally.

### 5.3 Settlement in demo

No real Lightning. Options:

- **Mock settlement**: server simulates invoice/preimage flow instantly. Deals
  show realistic settlement artifacts but no real sats move.
- **Regtest Lightning**: actual LND on regtest with auto-funded wallets. More
  authentic but adds infrastructure complexity.

Recommendation: **mock settlement for launch**. Add regtest later if there's
demand.

### 5.4 Network isolation

The demo network is fully isolated from any future production network. Different
Nostr relays (or no Nostr — direct HTTP discovery only), different marketplace
instance, no cross-pollination. Demo identities and artifacts never leak into
production.

---

## 6. Identity Model

### 6.1 Ephemeral identity (tier 1)

- Server generates secp256k1 keypair per session.
- Private key held in server memory only (never persisted to disk).
- Public key returned to the LLM.
- On session expiry: keypair zeroed from memory. Gone forever.

### 6.2 Email-linked identity (tier 2)

- Same keypair, but on email verification it's persisted to an encrypted
  identity store (SQLite with at-rest encryption or similar).
- Keyed by email hash (we store email for verification but the lookup key is
  `SHA256(email)`).
- Exportable: `GET /api/identity/export { email, code }` returns the 32-byte
  seed. User can import it into a local froglet-node. One-way migration from
  custodial to sovereign.

### 6.3 Honesty contract

The `llms.txt` and every identity-related API response include clear language:

> "This identity is managed by the Froglet demo server. The private key is held
> on our infrastructure. For self-sovereign identity where you control the keys,
> install Froglet locally."

We don't pretend custodial is sovereign. We don't hide the trade-off.

---

## 7. Anti-Abuse

### 7.1 Rate limiting

| Limit                          | Value           |
|--------------------------------|-----------------|
| Sessions per IP per hour       | 3               |
| Concurrent sessions per IP     | 1               |
| API calls per session          | 200             |
| Session duration               | 15 min (tier 1) |
| Deals per session              | 10              |
| WASM fuel per execution        | 10M             |

### 7.2 Global caps

| Cap                            | Value           |
|--------------------------------|-----------------|
| Total concurrent sessions      | 50              |
| Warm pool size                 | 10              |
| Email registrations per day    | 100             |
| Email verifications per email  | 3 attempts      |

If the session pool is exhausted, return `503` with `Retry-After` header and a
message the LLM can relay to the user.

### 7.3 Email as abuse gate

Tier 2 (persistent) requires email verification. This naturally limits abuse for
persistent identities. One identity per email. Disposable email domains can be
blocked if needed (use a blocklist).

### 7.4 No proof-of-work for launch

PoW adds friction for LLMs and marginal benefit when IP rate limiting + session
caps are in place. Reconsider if abuse materializes.

---

## 8. Session Manager Implementation

### 8.1 Core data structures

```rust
struct SessionManager {
    warm_pool: Vec<DemoSession>,
    active: HashMap<String, DemoSession>,
    house_providers: Vec<FrogletNode>,
    identity_store: IdentityStore,
    config: SessionConfig,
}

struct DemoSession {
    session_id: String,
    runtime_node: FrogletNode,     // user's "agent" node
    signing_key: SigningKey,       // secp256k1 private key
    pubkey: VerifyingKey,
    email: Option<String>,         // set after tier 2 registration
    created_at: Instant,
    expires_at: Instant,
    request_count: AtomicU32,
    deal_count: AtomicU32,
}

struct IdentityStore {
    db: rusqlite::Connection,      // persistent SQLite
    // email_hash → encrypted seed
}
```

### 8.2 Pool management

- On startup: pre-warm `pool_size` sessions (default 10).
- On session assignment: move from warm pool to active map, start TTL timer.
- Background replenisher: keeps warm pool at target size.
- Reaper task: runs every 60s, destroys expired sessions, unpublishes their
  artifacts from the demo network.

### 8.3 In-process node hosting

Each `FrogletNode` is NOT a separate process. It's an in-memory instance sharing
the Tokio runtime. The session manager creates node instances with:

- In-memory SQLite (no disk I/O).
- Loopback-only listeners (or no listener — direct function calls).
- Shared reference to house provider nodes for discovery.
- Mock settlement backend.

This requires refactoring froglet-node to support **library mode** (callable as
a Rust library, not just as a standalone binary). The existing code is structured
around `main()` → start server. We need to extract the core into a reusable
struct.

### 8.4 Library mode refactor

Current: `froglet-node/src/main.rs` → starts Tokio runtime, binds ports, runs
forever.

Needed: extract `FrogletNode` struct that can be instantiated programmatically:

```rust
let node = FrogletNode::builder()
    .role(Role::Runtime)
    .database(Database::InMemory)
    .settlement(Settlement::Mock)
    .discovery(Discovery::Direct(house_providers.clone()))
    .build()
    .await?;

// Use node directly, no HTTP listener needed
let quote = node.request_quote(&provider_id, &workload).await?;
```

This is the single biggest refactoring task in the plan. Estimated at 1-2 weeks
depending on how tightly coupled the current code is to the HTTP layer.

---

## 9. Gateway Service

A Rust/Axum HTTP server that handles all external traffic.

### 9.1 Routes

```
# Human
GET  /                              → HTML landing page
GET  /setup                         → MCP/integration setup guide

# LLM discovery
GET  /llms.txt                      → LLM-optimized API instructions
GET  /openapi.json                  → OpenAPI 3.1 spec
GET  /.well-known/mcp.json          → MCP server metadata

# MCP
GET  /mcp                           → MCP Streamable HTTP endpoint

# Session REST API
POST /api/sessions                  → Create session
GET  /api/s/:id                     → Session status
POST /api/s/:id/discover            → Discover services
POST /api/s/:id/quote               → Request quote
POST /api/s/:id/deal                → Commit deal
POST /api/s/:id/execute             → Execute workload
GET  /api/s/:id/receipt/:rid        → Get receipt
GET  /api/s/:id/identity            → Get session identity info
GET  /api/s/:id/deals               → List session deals
GET  /api/s/:id/services            → List available services
GET  /api/s/:id/artifacts/:hash     → Get artifact by hash

# Identity persistence
POST /api/identity/register         → Start email registration
POST /api/identity/verify           → Verify email code
POST /api/sessions/resume           → Resume session with email
GET  /api/identity/export           → Export seed (authenticated)

# Health
GET  /health                        → Health check
GET  /metrics                       → Prometheus metrics (optional)
```

### 9.2 API design principles

- Every response includes `session.expires_in` so the LLM knows the time budget.
- Error responses include `llm_hint` field with plain-English guidance:
  ```json
  {
    "error": "session_expired",
    "llm_hint": "This session has expired. Create a new one with POST /api/sessions, or if the user provided an email earlier, resume with POST /api/sessions/resume { email: '...' }"
  }
  ```
- All artifact responses include the full signed artifact JSON so the LLM can
  inspect cryptographic structure.
- Deal lifecycle responses include a `next_step` field guiding the LLM through
  the protocol.

---

## 10. Email Verification Flow

### 10.1 Registration

```
POST /api/identity/register
{
  "session_id": "abc123",
  "email": "user@example.com"
}

→ 200 { "status": "verification_sent", "message": "A 6-digit code was sent to user@example.com" }
```

### 10.2 Verification

```
POST /api/identity/verify
{
  "email": "user@example.com",
  "code": "482917"
}

→ 200 {
  "status": "verified",
  "pubkey": "02a1b2c3...",
  "message": "Identity saved. Use this email to resume sessions in the future."
}
```

### 10.3 Resume

```
POST /api/sessions/resume
{
  "email": "user@example.com"
}

→ 200 {
  "session_id": "def456",
  "pubkey": "02a1b2c3...",  // same as before
  "expires_at": "...",
  "deals_history": [ ... ],
  "message": "Welcome back. Your identity and history are restored."
}
```

### 10.4 Email sending

Use a transactional email service (Resend, Postmark, or AWS SES). Estimated
cost: $0 for <1000 emails/month (free tiers), ~$1/1000 emails after.

---

## 11. Implementation Phases

### Phase 1 — Foundation (weeks 1-2)

**Goal:** Froglet library mode + session manager skeleton.

- [ ] Refactor froglet-node into library mode (`FrogletNode` struct usable
      without HTTP listener).
- [ ] Implement in-memory SQLite option for ephemeral sessions.
- [ ] Implement mock settlement backend (if not already present).
- [ ] Build `SessionManager` with warm pool, active map, reaper.
- [ ] Set up house provider nodes with demo services (add, echo, wasm-compute).

### Phase 2 — Gateway + REST API (weeks 3-4)

**Goal:** The URL works. LLMs can create sessions and run deals.

- [ ] Build gateway Axum service with session REST API.
- [ ] Write `/llms.txt` content (the critical LLM-facing document).
- [ ] Write `/openapi.json` spec.
- [ ] Implement rate limiting (IP-based, per-session).
- [ ] Build HTML landing page for humans.
- [ ] Deploy to a single VPS (Hetzner CAX21 or similar).
- [ ] Set up domain + TLS (Caddy or similar).
- [ ] Test with Claude Code, ChatGPT, Cursor — verify LLMs can follow the
      instructions and complete a deal.

### Phase 3 — Identity persistence + email (weeks 5-6)

**Goal:** Users can save identity and return.

- [ ] Build identity store (encrypted SQLite).
- [ ] Implement email registration + 6-digit verification flow.
- [ ] Set up transactional email (Resend or similar).
- [ ] Implement session resume from email.
- [ ] Implement seed export endpoint.
- [ ] Update `/llms.txt` with tier 2 instructions.

### Phase 4 — MCP endpoint (week 7)

**Goal:** MCP-native clients get richer integration.

- [ ] Implement Streamable HTTP MCP transport in gateway.
- [ ] Define MCP tools mirroring REST API.
- [ ] Add `/.well-known/mcp.json` discovery.
- [ ] Test with Claude Desktop MCP config.
- [ ] Write setup instructions for MCP clients.

### Phase 5 — Polish + docs site integration (week 8)

**Goal:** Integrated into the docs site, production-ready.

- [ ] Update playground page on docs site with new "try it" UI.
- [ ] Add monitoring/alerting (uptime, session metrics, error rates).
- [ ] Write operational runbook.
- [ ] Load testing (verify 50 concurrent sessions on target hardware).
- [ ] Security review (rate limits, input validation, email verification).

---

## 12. Infrastructure

### 12.1 Server

Single VPS for launch. Froglet is Rust — a single binary handles everything.

| Option       | Specs                    | Cost    |
|--------------|--------------------------|---------|
| Hetzner CAX21| 4 ARM cores, 8GB, 80GB   | ~$7/mo  |
| Hetzner CAX31| 8 ARM cores, 16GB, 160GB | ~$14/mo |
| Fly.io       | 4 shared CPU, 8GB        | ~$20/mo |

Start with CAX21. Upgrade if demand warrants.

### 12.2 Domain

Options (need to purchase):
- `ai.froglet.dev`
- `try.froglet.dev`
- `play.froglet.dev`
- `api.froglet.dev`

### 12.3 Email

Resend free tier: 3,000 emails/month, no cost. More than enough for launch.

### 12.4 TLS

Caddy with automatic Let's Encrypt. Zero cost, zero maintenance.

### 12.5 Cost summary

| Item                | Monthly cost |
|---------------------|-------------|
| VPS (Hetzner CAX21) | $7          |
| Domain              | ~$1 (annual amortized) |
| Email (Resend free) | $0          |
| TLS (Let's Encrypt) | $0          |
| **Total**           | **~$8/mo**  |

---

## 13. What the LLM Experience Looks Like

### 13.1 Claude Code or ChatGPT

```
User: "Check out ai.froglet.dev — what is it and can you try it?"

LLM: *fetches /llms.txt, reads the API instructions*

LLM: "Froglet is a cryptographic protocol for AI agent commerce. I can try it
      right now. Let me create a session and walk through a deal."

LLM: *POST /api/sessions*
LLM: "I have a temporary identity: 02a1b2c3... This session lasts 15 minutes."

LLM: *POST /api/s/{id}/discover { query: "compute" }*
LLM: "I found 2 providers. 'froglet-house-1' offers an 'add' service and a
      'wasm-compute' service."

LLM: *POST /api/s/{id}/quote { provider, service: "add", input: {a:3, b:5} }*
LLM: "Got a quote: free execution, deal ID quote_abc."

LLM: *POST /api/s/{id}/deal { quote_id: "quote_abc" }*
LLM: "Deal committed. Both parties signed."

LLM: *POST /api/s/{id}/execute { deal_id: "deal_abc" }*
LLM: "Execution complete. Result: 8."

LLM: *GET /api/s/{id}/receipt/{rid}*
LLM: "Receipt verified. The entire chain — descriptor, offer, quote, deal,
      receipt — is cryptographically signed and hash-linked."

LLM: "Your session expires in 11 minutes. You have two options:
      1. Provide your email to save this identity and return later. This is
         custodial — we hold the key, linked to your email.
      2. Install Froglet locally (cargo install froglet-node) and own your
         keys directly.
      Which would you prefer?"
```

### 13.2 MCP client (Claude Desktop)

User adds to Claude Desktop config:
```json
{ "froglet": { "url": "https://ai.froglet.dev/mcp" } }
```

Then:
```
User: "Discover what services are available on Froglet and try one."

Claude: *calls discover_services tool*
Claude: *calls request_quote tool*
Claude: *calls commit_deal tool*
Claude: *calls execute_workload tool*
Claude: *calls get_receipt tool*

Claude: "Done. I discovered a provider, negotiated a deal, executed a WASM
         workload, and verified the receipt. Would you like to save this
         identity or set up your own node?"
```

No manual session management — MCP endpoint handles it transparently.

---

## 14. Open Questions

1. **Demo network scope.** Should demo users only see house providers, or should
   they also see each other? Seeing each other is more compelling but creates
   potential for abuse (spam services, offensive descriptors).

2. **WASM in demo.** Should users be able to submit arbitrary WASM in the demo,
   or only invoke pre-built house services? Arbitrary WASM is the full Froglet
   experience but needs tight sandboxing (already exists in froglet-node).

3. **Regtest Lightning.** Mock settlement is simpler. Regtest is more authentic.
   For launch, mock is fine. Worth revisiting if demos feel too "toy."

4. **Identity export.** Exporting the seed lets users migrate from custodial to
   sovereign. Should export require re-verification (email code) as a safety
   measure?

5. **Usage analytics.** What do we want to track? Session count, deal completion
   rate, time-to-first-deal, conversion to email registration, conversion to
   local install. Respect privacy — no PII in analytics.

6. **Domain choice.** `ai.froglet.dev`, `try.froglet.dev`, or something else.
   The URL needs to be short, memorable, and clearly convey "paste me into your
   LLM."
