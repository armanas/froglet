use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct PublicQuotaConfig {
    pub requests_per_window: u32,
    pub hosted_trial_deals_per_identity: u32,
    pub hosted_trial_sessions_per_identity: u32,
    pub event_publishes_per_identity: u32,
    pub quotes_per_identity: u32,
    pub confidential_sessions_per_identity: u32,
    pub trust_forward_public_quota_headers: bool,
    pub hosted_trial_window_secs: u64,
    pub public_write_window_secs: u64,
}

impl Default for PublicQuotaConfig {
    fn default() -> Self {
        Self {
            requests_per_window: 6_000,
            hosted_trial_deals_per_identity: 10,
            hosted_trial_sessions_per_identity: 20,
            event_publishes_per_identity: 60,
            quotes_per_identity: 60,
            confidential_sessions_per_identity: 20,
            trust_forward_public_quota_headers: false,
            hosted_trial_window_secs: 900,
            public_write_window_secs: 900,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaDecision {
    Allowed { remaining: u32 },
    Rejected { retry_after_secs: u64 },
}

#[derive(Debug, Clone)]
struct QuotaBucket {
    window_started: Instant,
    count: u32,
}

pub struct IdentityQuota {
    max_per_window: u32,
    window: Duration,
    buckets: Mutex<HashMap<String, QuotaBucket>>,
    blocked: std::sync::atomic::AtomicU64,
    allowed: std::sync::atomic::AtomicU64,
}

// Bound attacker-controlled identity storage as well as request frequency.
const MAX_IDENTITY_BUCKETS: usize = 10_000;

impl IdentityQuota {
    pub fn new(max_per_window: u32, window: Duration) -> Self {
        Self {
            max_per_window: max_per_window.max(1),
            window: window.max(Duration::from_secs(1)),
            buckets: Mutex::new(HashMap::new()),
            blocked: std::sync::atomic::AtomicU64::new(0),
            allowed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn check_and_increment(&self, identity: &str) -> QuotaDecision {
        let decision = self.check_and_increment_at(identity, Instant::now());
        match decision {
            QuotaDecision::Allowed { .. } => &self.allowed,
            QuotaDecision::Rejected { .. } => &self.blocked,
        }
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        decision
    }

    pub fn counters(&self) -> serde_json::Value {
        serde_json::json!({"allowed":self.allowed.load(std::sync::atomic::Ordering::Relaxed), "blocked":self.blocked.load(std::sync::atomic::Ordering::Relaxed), "scope":"current_process"})
    }

    fn check_and_increment_at(&self, identity: &str, now: Instant) -> QuotaDecision {
        let key = normalize_identity_key(identity);
        let mut buckets = self.lock();
        if !buckets.contains_key(&key) && buckets.len() >= MAX_IDENTITY_BUCKETS {
            buckets.retain(|_, bucket| now.duration_since(bucket.window_started) < self.window);
            if buckets.len() >= MAX_IDENTITY_BUCKETS {
                return QuotaDecision::Rejected {
                    retry_after_secs: self.window.as_secs().max(1),
                };
            }
        }
        let bucket = buckets.entry(key).or_insert(QuotaBucket {
            window_started: now,
            count: 0,
        });

        if now.duration_since(bucket.window_started) >= self.window {
            bucket.window_started = now;
            bucket.count = 0;
        }

        if bucket.count >= self.max_per_window {
            let retry_after_secs = self
                .window
                .saturating_sub(now.duration_since(bucket.window_started))
                .as_secs()
                .max(1);
            return QuotaDecision::Rejected { retry_after_secs };
        }

        bucket.count = bucket.count.saturating_add(1);
        QuotaDecision::Allowed {
            remaining: self.max_per_window.saturating_sub(bucket.count),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, QuotaBucket>> {
        match self.buckets.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn normalize_identity_key(identity: &str) -> String {
    let normalized = identity.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        "anonymous".to_string()
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_is_tracked_per_identity() {
        let quota = IdentityQuota::new(1, Duration::from_secs(60));
        let now = Instant::now();

        assert_eq!(
            quota.check_and_increment_at("identity-a", now),
            QuotaDecision::Allowed { remaining: 0 }
        );
        assert_eq!(
            quota.check_and_increment_at("identity-b", now),
            QuotaDecision::Allowed { remaining: 0 }
        );
        assert!(matches!(
            quota.check_and_increment_at("identity-a", now),
            QuotaDecision::Rejected { .. }
        ));
    }

    #[test]
    fn identity_rotation_is_bounded_and_expired_buckets_are_reclaimed() {
        let quota = IdentityQuota::new(2, Duration::from_secs(60));
        let now = Instant::now();
        for index in 0..MAX_IDENTITY_BUCKETS {
            assert!(matches!(
                quota.check_and_increment_at(&index.to_string(), now),
                QuotaDecision::Allowed { .. }
            ));
        }
        assert!(matches!(
            quota.check_and_increment_at("new-identity", now),
            QuotaDecision::Rejected { .. }
        ));
        assert_eq!(quota.lock().len(), MAX_IDENTITY_BUCKETS);
        assert!(matches!(
            quota.check_and_increment_at("0", now),
            QuotaDecision::Allowed { .. }
        ));
        assert!(matches!(
            quota.check_and_increment_at("new-identity", now + Duration::from_secs(61)),
            QuotaDecision::Allowed { .. }
        ));
        assert_eq!(quota.lock().len(), 1);
    }

    #[test]
    fn quota_window_resets() {
        let quota = IdentityQuota::new(1, Duration::from_secs(60));
        let now = Instant::now();
        let later = now + Duration::from_secs(61);

        assert_eq!(
            quota.check_and_increment_at("identity", now),
            QuotaDecision::Allowed { remaining: 0 }
        );
        assert_eq!(
            quota.check_and_increment_at("identity", later),
            QuotaDecision::Allowed { remaining: 0 }
        );
    }

    #[test]
    fn identity_keys_are_normalized() {
        let quota = IdentityQuota::new(1, Duration::from_secs(60));
        let now = Instant::now();

        assert_eq!(
            quota.check_and_increment_at(" Identity ", now),
            QuotaDecision::Allowed { remaining: 0 }
        );
        assert!(matches!(
            quota.check_and_increment_at("identity", now),
            QuotaDecision::Rejected { .. }
        ));
    }
}
