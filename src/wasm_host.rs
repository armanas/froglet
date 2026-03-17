use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use reqwest::redirect::Policy as RedirectPolicy;
use serde::Deserialize;

use crate::{
    canonical_json,
    config::{WasmHttpPolicy, WasmPolicy},
    wasm_db::{self, DbQueryRequest},
    wasm_http::{self, HttpFetchRequest},
};

#[derive(Clone)]
pub struct WasmHostEnvironment {
    pub policy: WasmPolicy,
    pub http_client: Option<reqwest::Client>,
}

impl WasmHostEnvironment {
    pub fn from_policy(policy: WasmPolicy) -> Result<Self, String> {
        let http_client = policy.http.as_ref().map(build_http_client).transpose()?;
        Ok(Self {
            policy,
            http_client,
        })
    }

    pub fn advertised_capabilities(&self) -> Vec<String> {
        self.policy.advertised_capabilities()
    }
}

#[derive(Clone)]
pub struct WasmExecutionContext {
    environment: Arc<WasmHostEnvironment>,
    granted_capabilities: Vec<String>,
    http_calls_used: u32,
    db_queries_used: u32,
    execution_deadline: Option<Instant>,
}

impl WasmExecutionContext {
    pub fn new(
        environment: Arc<WasmHostEnvironment>,
        granted_capabilities: Vec<String>,
        execution_deadline: Option<Instant>,
    ) -> Self {
        Self {
            environment,
            granted_capabilities,
            http_calls_used: 0,
            db_queries_used: 0,
            execution_deadline,
        }
    }

    pub fn dispatch_json(&mut self, request_bytes: &[u8]) -> Result<Vec<u8>, String> {
        let request: HostCallRequest = serde_json::from_slice(request_bytes)
            .map_err(|error| format!("invalid host call JSON: {error}"))?;

        let response =
            match request {
                HostCallRequest::HttpFetch { request } => {
                    let http_policy =
                        self.environment.policy.http.as_ref().ok_or_else(|| {
                            "http.fetch is not enabled on this provider".to_string()
                        })?;
                    let http_client = self
                        .environment
                        .http_client
                        .as_ref()
                        .ok_or_else(|| "http.fetch client is not initialized".to_string())?;
                    wasm_http::fetch(
                        http_policy,
                        http_client,
                        &self.granted_capabilities,
                        &mut self.http_calls_used,
                        request,
                        self.execution_deadline,
                    )?
                }
                HostCallRequest::DbQuery { request } => {
                    let sqlite_policy =
                        self.environment.policy.sqlite.as_ref().ok_or_else(|| {
                            "db.query is not enabled on this provider".to_string()
                        })?;
                    wasm_db::query(
                        sqlite_policy,
                        &self.granted_capabilities,
                        &mut self.db_queries_used,
                        request,
                        self.execution_deadline,
                    )?
                }
            };

        canonical_json::to_vec(&response).map_err(|error| error.to_string())
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op")]
enum HostCallRequest {
    #[serde(rename = "http.fetch")]
    HttpFetch { request: HttpFetchRequest },
    #[serde(rename = "db.query")]
    DbQuery { request: DbQueryRequest },
}

fn build_http_client(policy: &WasmHttpPolicy) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .redirect(RedirectPolicy::limited(policy.max_redirects))
        .build()
        .map_err(|error| format!("failed to build async http client: {error}"))
}
