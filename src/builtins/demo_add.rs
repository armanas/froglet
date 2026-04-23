//! Add builtin service — sums two signed 64-bit integers.
//!
//! Input shape:  `{ "a": <i64>, "b": <i64> }`
//! Output shape: `{ "sum": <i64> }`
//!
//! Integer overflow returns an error so callers see exact arithmetic, never
//! a silent wrap. `i64` was chosen over `u64` so negative inputs are valid
//! without operand-sign surprises; this matches the standard JSON Number
//! integer range Froglet callers use elsewhere.

use crate::execution::BuiltinServiceHandler;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Deserialize)]
struct AddInput {
    a: i64,
    b: i64,
}

#[derive(Debug, Serialize)]
struct AddOutput {
    sum: i64,
}

pub struct AddHandler;

impl BuiltinServiceHandler for AddHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move {
            let req: AddInput = serde_json::from_value(input)
                .map_err(|e| format!("invalid demo.add input: {e}"))?;
            let sum = req
                .a
                .checked_add(req.b)
                .ok_or_else(|| "demo.add overflow: a + b exceeds i64 range".to_string())?;
            serde_json::to_value(AddOutput { sum }).map_err(|e| format!("demo.add serialize: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn demo_add_sums_positives() {
        let handler = AddHandler;
        let out = handler.execute(json!({ "a": 3, "b": 5 })).await.unwrap();
        assert_eq!(out, json!({ "sum": 8 }));
    }

    #[tokio::test]
    async fn demo_add_handles_negatives() {
        let handler = AddHandler;
        let out = handler.execute(json!({ "a": -7, "b": 10 })).await.unwrap();
        assert_eq!(out, json!({ "sum": 3 }));
    }

    #[tokio::test]
    async fn demo_add_rejects_missing_operands() {
        let handler = AddHandler;
        let err = handler
            .execute(json!({ "a": 1 }))
            .await
            .expect_err("missing b must error");
        assert!(err.contains("invalid demo.add input"), "got: {err}");
    }

    #[tokio::test]
    async fn demo_add_rejects_overflow() {
        let handler = AddHandler;
        let err = handler
            .execute(json!({ "a": i64::MAX, "b": 1 }))
            .await
            .expect_err("overflow must error");
        assert!(err.contains("overflow"), "got: {err}");
    }
}
