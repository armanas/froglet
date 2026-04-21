//! Echo builtin service — returns input unchanged.
//!
//! This is one of the seed services published by `ai.froglet.dev` at launch.
//! It has no external dependencies, no side effects, and zero price. Its job
//! is to prove the discover → quote → deal → execute → receipt loop against a
//! live hosted provider.

use crate::execution::BuiltinServiceHandler;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct EchoHandler;

impl BuiltinServiceHandler for EchoHandler {
    fn execute<'a>(
        &'a self,
        input: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send + 'a>> {
        Box::pin(async move { Ok(input) })
    }
}
