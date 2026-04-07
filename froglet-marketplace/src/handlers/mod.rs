pub mod provider;
pub mod receipts;
pub mod register;
pub mod search;

use serde::Serialize;

pub(crate) fn default_page_limit() -> i64 {
    20
}

#[derive(Debug, Serialize)]
pub(crate) struct TrustSummary {
    pub total: i64,
    pub succeeded: i64,
    pub failed: i64,
}
