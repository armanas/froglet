# Froglet Pre-Release Hardening Plan

Code review revealed 9 issues ranging from "project doesn't compile" to API inconsistencies. These must be fixed before release, especially for automated clients (clawbot). The fixes below are ordered by dependency: compilation first, then critical security, then hardening.

---

## Fix 1: Cargo.toml — Missing Dependencies (CRITICAL)

**Why:** Project fails to compile. `ConcurrencyLimitLayer`, `TimeoutLayer`, and `Lazy` are all dead code — the safety nets that limit concurrency and enforce timeouts don't exist in the built binary.

**File:** `Cargo.toml`

- **Line 12:** Change `tower` features from `["util"]` to `["util", "limit", "timeout"]`
- **After line 48:** Add `once_cell = "1"`

**Verify:** `cargo check` succeeds with no errors.

---

## Fix 2: Marketplace Timestamp Freshness (CRITICAL)

**Why:** Captured signed register/heartbeat requests are replayable forever. An attacker who records a valid signed request can keep a stale node appearing active indefinitely without possessing the private key.

**File:** `src/marketplace_server.rs`

- Add constant `MAX_REQUEST_AGE_SECS: i64 = 120` near top of file (after line 20)
- Add helper function:
  ```rust
  fn request_is_stale(request_timestamp: i64, now: i64) -> bool {
      (now - request_timestamp).abs() > MAX_REQUEST_AGE_SECS
  }
  ```
- **Register handler (after line 99**, after `let now = current_unix_timestamp();`**):** Insert staleness check before `let conn = state.db.lock().await;`:
  ```rust
  if request_is_stale(payload.timestamp, now) {
      return bad_request("request timestamp is too old or too far in the future");
  }
  ```
- **Heartbeat handler (after line 147**, after `let now = current_unix_timestamp();`**):** Insert identical staleness check before `let conn = state.db.lock().await;`

---

## Fix 3: Event Signature — Sign Full Canonical Event (CRITICAL)

**Why:** Only `event.content` is signed at `api.rs:291`. Fields `kind`, `tags`, `created_at`, and `id` can be spoofed by anyone who observes a valid event — they can resubmit the same signature with different metadata.

**File:** `src/api.rs`

- **After line 144** (after `NodeEventEnvelope` struct closing brace): Add impl block:
  ```rust
  impl NodeEventEnvelope {
      pub fn canonical_signing_bytes(&self) -> Vec<u8> {
          serde_json::json!([
              self.id,
              self.pubkey,
              self.created_at,
              self.kind,
              self.tags,
              self.content
          ])
          .to_string()
          .into_bytes()
      }
  }
  ```
- **Line 291:** Change:
  ```rust
  // Before:
  if !crypto::verify_signature(&event.pubkey, &event.sig, &event.content)
  // After:
  if !crypto::verify_message(&event.pubkey, &event.sig, &event.canonical_signing_bytes())
  ```

**Breaking change:** Existing clients must update to sign the canonical `[id, pubkey, created_at, kind, tags, content]` JSON array instead of just `content`. Document in README.

---

## Fix 4: Identity Seed TOCTOU

**Why:** `identity.rs:70-74` writes the seed file with `fs::write` (world-readable by default) then calls `set_mode(path, 0o600)` after. There's a brief window where the private key is exposed.

**File:** `src/identity.rs`

- **Replace `persist_signing_key` function (lines 70-74):**
  ```rust
  fn persist_signing_key(path: &Path, signing_key: &SigningKey) -> Result<(), String> {
      let seed_hex = hex::encode(signing_key.to_bytes());

      #[cfg(unix)]
      {
          use std::os::unix::fs::OpenOptionsExt;
          use std::io::Write;
          let mut file = std::fs::OpenOptions::new()
              .write(true)
              .create_new(true)
              .mode(0o600)
              .open(path)
              .map_err(|e| format!("Failed to create identity seed file {}: {e}", path.display()))?;
          file.write_all(seed_hex.as_bytes())
              .map_err(|e| format!("Failed to write identity seed {}: {e}", path.display()))?;
      }

      #[cfg(not(unix))]
      {
          fs::write(path, seed_hex)
              .map_err(|e| format!("Failed to write node identity seed {}: {e}", path.display()))?;
      }

      Ok(())
  }
  ```

Key: `create_new(true)` prevents overwriting an existing seed, and `.mode(0o600)` sets permissions atomically at file creation time.

---

## Fix 5: Backoff Bug

**Why:** `marketplace_client.rs:36-41` computes `delay_secs` with exponential backoff, but line 43 always sleeps using `heartbeat_interval`. The backoff is dead code.

**File:** `src/marketplace_client.rs`

- **Line 43:** Change:
  ```rust
  // Before:
  tokio::time::sleep(Duration::from_secs(heartbeat_interval)).await;
  // After:
  tokio::time::sleep(Duration::from_secs(delay_secs)).await;
  ```

---

## Fix 6: HTTP Body Size Limit

**Why:** No `DefaultBodyLimit` layer on the router. Axum's default is 2MB but there's no explicit cap. An attacker can send large payloads with ignored fields to exhaust memory before per-field checks run.

**File:** `src/api.rs`

- **Line 3:** Add `DefaultBodyLimit` to the extract import:
  ```rust
  extract::{DefaultBodyLimit, State},
  ```
- **Before `.with_state(state)` on line 191:** Add:
  ```rust
  .layer(DefaultBodyLimit::max(1_048_576)) // 1 MB
  ```

---

## Fix 7: Health Endpoint Returns JSON

**Why:** `/health` is the only endpoint returning plain text. Every other route returns JSON. An automated client checking `/health` must branch on content type.

**File:** `src/api.rs`

- **Lines 194-196:** Change:
  ```rust
  // Before:
  pub async fn health_check() -> impl IntoResponse {
      (StatusCode::OK, "🐸 Froglet is Running")
  }
  // After:
  pub async fn health_check() -> impl IntoResponse {
      (StatusCode::OK, Json(serde_json::json!({"status": "ok"})))
  }
  ```

---

## Fix 8: Database Error Sanitization

**Why:** `marketplace_server.rs:451-456` exposes raw rusqlite error messages to clients, which can include SQL fragments, table names, and constraint details.

**File:** `src/marketplace_server.rs`

- **Lines 451-456:** Change:
  ```rust
  // Before:
  fn database_error(error: rusqlite::Error) -> (StatusCode, Json<serde_json::Value>) {
      (
          StatusCode::INTERNAL_SERVER_ERROR,
          Json(serde_json::json!({ "error": format!("database error: {error}") })),
      )
  }
  // After:
  fn database_error(error: rusqlite::Error) -> (StatusCode, Json<serde_json::Value>) {
      tracing::error!("Database error: {error}");
      (
          StatusCode::INTERNAL_SERVER_ERROR,
          Json(serde_json::json!({ "error": "internal database error" })),
      )
  }
  ```

---

## Fix 9: Search Nodes — Filter Inactive by Default

**Why:** The search query returns all nodes including inactive ones, counting against the limit. A clawbot discovering nodes gets a mix of live and stale entries.

**File:** `src/marketplace_server.rs`

- **Lines 28-31** (`SearchQuery` struct): Add field:
  ```rust
  #[serde(default)]
  pub include_inactive: Option<bool>,
  ```
- **Lines 299-301** (SQL query): Branch on `include_inactive.unwrap_or(false)`:
  - Default: `WHERE status = 'active'` in the SQL
  - With `?include_inactive=true`: no WHERE filter
- **Lines 330-335** (post-filter loop): Additionally skip nodes where `effective_status` resolved to `"inactive"` when `include_inactive` is false, since a node can be `active` in the DB but stale by time.

---

## Verification Checklist

1. `cargo check` — confirms compilation succeeds (Fix 1)
2. `cargo test` — all existing unit tests still pass
3. Start marketplace, submit register with timestamp 200s in the past — expect 400 (Fix 2)
4. Start node, `curl /health` — expect `{"status":"ok"}` with JSON content-type (Fix 7)
5. Send >1MB POST body — expect 413 Payload Too Large (Fix 6)
6. `GET /v1/marketplace/search` excludes inactive nodes; `?include_inactive=true` includes them (Fix 9)
7. Fresh identity creation: verify seed file is `0o600` immediately (Fix 4)
8. Point marketplace URL at unreachable host, observe increasing sleep intervals in logs (Fix 5)
9. Trigger a DB error, confirm HTTP response says only `"internal database error"` (Fix 8)

---

## Files Modified

| File | Fixes |
|------|-------|
| `Cargo.toml` | 1 |
| `src/api.rs` | 3, 6, 7 |
| `src/marketplace_server.rs` | 2, 8, 9 |
| `src/marketplace_client.rs` | 5 |
| `src/identity.rs` | 4 |
