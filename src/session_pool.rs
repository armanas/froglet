//! Short-lived session-token pool for the public `try.froglet.dev` surface.
//!
//! # Design decisions (per SYSTEM_DESIGN.md §8 and the plan answers)
//!
//! - Session tokens are **authentication-only**. The node always signs
//!   artifacts with its own `state.identity`. Each session slot is just a
//!   bearer token with a 15-minute TTL and a slot id for logging and
//!   rate-limiting. There is no per-session signing key.
//! - Pool is fixed-size. When all slots are assigned and unexpired,
//!   `POST /api/sessions` returns 503.
//! - Expired slots are freed lazily on the next `assign()` call. A tokio
//!   reaper is optional — it only improves observability.
//! - Token shape: 32 random bytes, hex-encoded (64 chars). Compared in
//!   constant time via `subtle::ConstantTimeEq` at validation.
//! - Disabled by default (`FROGLET_SESSION_POOL_ENABLED=1` opts in). A
//!   standard self-host never exposes `/api/sessions`.

use rand::RngCore;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;

/// Opaque session identifier returned from `assign()`.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub token: String,
    pub slot_id: usize,
    pub expires_at: Instant,
    pub ttl: Duration,
}

#[derive(Debug, Clone)]
struct SessionSlot {
    token: String,
    expires_at: Instant,
}

/// Fixed-size pool of short-lived session tokens.
pub struct SessionPool {
    slots: Mutex<Vec<Option<SessionSlot>>>,
    ttl: Duration,
}

impl SessionPool {
    /// Create a new pool with `size` slots, each issued with `ttl` duration.
    pub fn new(size: usize, ttl: Duration) -> Self {
        let mut slots = Vec::with_capacity(size);
        slots.resize_with(size, || None);
        Self {
            slots: Mutex::new(slots),
            ttl,
        }
    }

    /// Pool capacity (constant across the process lifetime).
    pub fn size(&self) -> usize {
        self.lock().len()
    }

    /// Configured TTL for every slot.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// Assign a new token to an available slot. Returns `None` if the pool is
    /// full with live assignments (caller should return HTTP 503). Expired
    /// assignments are reaped lazily before looking for a free slot.
    pub fn assign(&self) -> Option<SessionInfo> {
        let now = Instant::now();
        let mut slots = self.lock();
        self.reap_expired_locked(&mut slots, now);

        let slot_id = slots.iter().position(|s| s.is_none())?;
        let token = new_random_token();
        let expires_at = now + self.ttl;
        slots[slot_id] = Some(SessionSlot {
            token: token.clone(),
            expires_at,
        });
        Some(SessionInfo {
            token,
            slot_id,
            expires_at,
            ttl: self.ttl,
        })
    }

    /// Check whether a bearer token corresponds to a live pool slot. Uses
    /// constant-time comparison; a match returns the slot id, a miss returns
    /// `None`. Expired slots return `None` (the stale mapping is NOT cleared
    /// here — wait for the next `assign()` or `reap_expired()`).
    pub fn validate(&self, token: &str) -> Option<usize> {
        let now = Instant::now();
        let slots = self.lock();
        for (slot_id, slot) in slots.iter().enumerate() {
            let Some(entry) = slot else { continue };
            if entry.expires_at < now {
                continue;
            }
            if token.as_bytes().ct_eq(entry.token.as_bytes()).unwrap_u8() == 1 {
                return Some(slot_id);
            }
        }
        None
    }

    /// Clear any slots whose TTL has elapsed. Returns the count reaped.
    pub fn reap_expired(&self) -> usize {
        let now = Instant::now();
        let mut slots = self.lock();
        self.reap_expired_locked(&mut slots, now)
    }

    fn reap_expired_locked(
        &self,
        slots: &mut MutexGuard<'_, Vec<Option<SessionSlot>>>,
        now: Instant,
    ) -> usize {
        let mut count = 0;
        for slot in slots.iter_mut() {
            if let Some(entry) = slot
                && entry.expires_at < now
            {
                *slot = None;
                count += 1;
            }
        }
        count
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Option<SessionSlot>>> {
        // Mutex-poison recovery: if a previous panic left the lock poisoned,
        // we take the inner value anyway. The pool has no invariant that a
        // panicked caller could have violated mid-mutation — worst case an
        // in-flight assignment is partially written, which the next caller
        // will overwrite with a fresh one.
        match self.slots.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

fn new_random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_hands_out_up_to_size_tokens() {
        let pool = SessionPool::new(3, Duration::from_secs(60));
        let t1 = pool.assign().expect("first assign");
        let t2 = pool.assign().expect("second");
        let t3 = pool.assign().expect("third");
        assert_eq!(
            [t1.slot_id, t2.slot_id, t3.slot_id].iter().sum::<usize>(),
            3,
            "three distinct slot ids must be handed out"
        );
        assert_ne!(t1.token, t2.token);
        assert_ne!(t2.token, t3.token);
    }

    #[test]
    fn assign_returns_none_when_pool_saturated() {
        let pool = SessionPool::new(2, Duration::from_secs(60));
        let _t1 = pool.assign().unwrap();
        let _t2 = pool.assign().unwrap();
        assert!(pool.assign().is_none(), "pool should be exhausted");
    }

    #[test]
    fn validate_returns_slot_id_for_live_token() {
        let pool = SessionPool::new(2, Duration::from_secs(60));
        let info = pool.assign().unwrap();
        assert_eq!(pool.validate(&info.token), Some(info.slot_id));
    }

    #[test]
    fn validate_returns_none_for_unknown_token() {
        let pool = SessionPool::new(2, Duration::from_secs(60));
        let _ = pool.assign().unwrap();
        assert_eq!(pool.validate("not-a-real-token"), None);
    }

    #[test]
    fn validate_rejects_expired_token() {
        // Near-zero TTL makes "expired" an immediate state transition.
        let pool = SessionPool::new(2, Duration::from_millis(1));
        let info = pool.assign().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(pool.validate(&info.token), None);
    }

    #[test]
    fn assign_reuses_slot_after_expiry_via_lazy_reap() {
        let pool = SessionPool::new(1, Duration::from_millis(1));
        let first = pool.assign().unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let second = pool
            .assign()
            .expect("expired slot should be reclaimed on next assign");
        assert_eq!(
            first.slot_id, second.slot_id,
            "the recycled slot id must be the same (pool of N fixed slots)"
        );
        assert_ne!(
            first.token, second.token,
            "the new token must differ from the expired one"
        );
    }

    #[test]
    fn reap_expired_counts_swept_slots() {
        let pool = SessionPool::new(3, Duration::from_millis(1));
        let _ = pool.assign();
        let _ = pool.assign();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(pool.reap_expired(), 2);
        assert_eq!(pool.reap_expired(), 0, "idempotent after sweep");
    }

    #[test]
    fn validate_is_case_sensitive() {
        let pool = SessionPool::new(1, Duration::from_secs(60));
        let info = pool.assign().unwrap();
        let upper = info.token.to_ascii_uppercase();
        if upper != info.token {
            assert_eq!(pool.validate(&upper), None);
        }
    }
}
