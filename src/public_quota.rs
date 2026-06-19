use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct PublicQuotaConfig {
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
}

impl IdentityQuota {
    pub fn new(max_per_window: u32, window: Duration) -> Self {
        Self {
            max_per_window: max_per_window.max(1),
            window: window.max(Duration::from_secs(1)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check_and_increment(&self, identity: &str) -> QuotaDecision {
        self.check_and_increment_at(identity, Instant::now())
    }

    fn check_and_increment_at(&self, identity: &str, now: Instant) -> QuotaDecision {
        let key = normalize_identity_key(identity);
        let mut buckets = self.lock();
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
