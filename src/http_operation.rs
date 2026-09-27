//! One bounded JSON operation over the existing Wasm host. The generated module
//! embeds the operation; callers supply input only. No new Kernel artifact.
use crate::{
    config::WasmHttpPolicy,
    wasm_http::{self, HttpFetchRequest},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, time::Instant};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HttpOperation {
    pub url: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_profile: Option<String>,
    pub input_schema: Value,
    pub output_schema: Value,
    #[serde(default)]
    pub fixed_body: Map<String, Value>,
    pub timeout_ms: u64,
    pub max_request_bytes: usize,
    pub max_response_bytes: usize,
}

impl HttpOperation {
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(self)
            .map_err(|_| "invalid operation")?
            .len()
            > 64 * 1024
        {
            return Err("HTTP operation definition exceeds 64KiB".into());
        }
        let url = reqwest::Url::parse(&self.url).map_err(|_| "invalid HTTP operation URL")?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
        {
            return Err(
                "HTTP operations require a fixed HTTPS URL without credentials, query or fragment"
                    .into(),
            );
        }
        if !matches!(self.method.as_str(), "POST" | "GET") {
            return Err("HTTP operations support GET or POST".into());
        }
        if self.method == "GET" && !self.fixed_body.is_empty() {
            return Err("GET operations cannot have a body".into());
        }
        if !(1..=30_000).contains(&self.timeout_ms)
            || !(1..=512 * 1024).contains(&self.max_request_bytes)
            || !(1..=crate::sandbox::WASM_MAX_OUTPUT_BYTES).contains(&self.max_response_bytes)
        {
            return Err(
                "operation requires timeout <=30000ms, request <=512KiB and response <=128KiB"
                    .into(),
            );
        }
        if self.auth_profile.as_ref().is_some_and(|s| {
            s.is_empty()
                || s.len() > 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        }) {
            return Err("invalid operator auth profile name".into());
        }
        if self.input_schema["type"] != "object"
            || self.input_schema["additionalProperties"] != false
        {
            return Err(
                "operation input_schema must be an object with additionalProperties:false".into(),
            );
        }
        if self
            .fixed_body
            .keys()
            .any(|key| self.input_schema["properties"].get(key).is_some())
        {
            return Err("fixed upstream fields cannot also be caller-controlled properties".into());
        }
        compile_schema(&self.input_schema)?;
        compile_schema(&self.output_schema)?;
        Ok(())
    }
    pub fn hash(&self) -> Result<String, String> {
        crate::canonical_json::to_vec(self)
            .map(crate::crypto::sha256_hex)
            .map_err(|e| e.to_string())
    }
    pub fn capabilities(&self) -> Result<Vec<String>, String> {
        Ok(vec![format!("net.http.operation.{}", self.hash()?)])
    }
    pub fn validate_input(&self, input: &Value) -> Result<(), String> {
        let schema = compile_schema(&self.input_schema)?;
        if !schema.is_valid(input) {
            return Err("HTTP operation input does not match the approved schema".into());
        }
        if self.method == "GET" && input.as_object().is_none_or(|o| !o.is_empty()) {
            return Err("GET operation input must be an empty object".into());
        }
        let mut body = input
            .as_object()
            .ok_or("operation input must be an object")?
            .clone();
        body.extend(self.fixed_body.clone());
        if serde_json::to_vec(&body)
            .map_err(|_| "invalid input")?
            .len()
            > self.max_request_bytes
        {
            return Err("HTTP operation request exceeds the approved byte limit".into());
        }
        Ok(())
    }
}

// Keep schema compilation finite and offline. Unsupported composition/reference
// features are rejected explicitly instead of silently weakening validation.
fn compile_schema(schema: &Value) -> Result<jsonschema::Validator, String> {
    fn check(value: &Value, depth: usize) -> Result<(), String> {
        if depth > 24 {
            return Err("operation schema is too deeply nested".into());
        }
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if matches!(
                        key.as_str(),
                        "$ref"
                            | "$dynamicRef"
                            | "$recursiveRef"
                            | "pattern"
                            | "patternProperties"
                            | "allOf"
                            | "anyOf"
                            | "oneOf"
                            | "not"
                            | "if"
                            | "then"
                            | "else"
                    ) {
                        return Err(
                            "operation schemas do not support references, patterns or combinators"
                                .into(),
                        );
                    }
                    check(value, depth + 1)?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    check(item, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    if serde_json::to_vec(schema)
        .map_err(|_| "invalid schema")?
        .len()
        > 16 * 1024
    {
        return Err("operation schema exceeds 16KiB".into());
    }
    check(schema, 0)?;
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .should_ignore_unknown_formats(false)
        .build(schema)
        .map_err(|_| "invalid operation JSON Schema (draft 2020-12)".into())
}

pub fn execute(
    operation: HttpOperation,
    input: Value,
    policy: &WasmHttpPolicy,
    client: &reqwest::Client,
    grants: &[String],
    calls: &mut u32,
    deadline: Option<Instant>,
) -> Result<Value, String> {
    operation.validate()?;
    let hash = operation.hash()?;
    if !policy.operations_only
        || !policy.operation_hashes.contains(&hash)
        || !grants.contains(&format!("net.http.operation.{hash}"))
    {
        return Err("HTTP operation is not approved by this provider; configure operations_only and its exact operation_hash".into());
    }
    operation.validate_input(&input)?;
    let mut internal_grants = vec![crate::wasm::WASM_CAPABILITY_HTTP_FETCH.to_string()];
    if let Some(profile) = &operation.auth_profile {
        internal_grants.push(format!(
            "{}{}",
            crate::wasm::WASM_CAPABILITY_HTTP_FETCH_AUTH_PREFIX,
            profile
        ));
    }

    let mut body = input
        .as_object()
        .ok_or("operation input must be an object")?
        .clone();
    for (key, value) in &operation.fixed_body {
        if body.insert(key.clone(), value.clone()).is_some() {
            return Err("caller cannot override fixed upstream fields".into());
        }
    }
    let mut bounded = policy.clone();
    bounded.operations_only = false;
    bounded.max_redirects = 0;
    bounded.max_calls_per_execution = bounded.max_calls_per_execution.min(1);
    bounded.max_request_body_bytes = bounded
        .max_request_body_bytes
        .min(operation.max_request_bytes);
    bounded.max_response_body_bytes = bounded
        .max_response_body_bytes
        .min(operation.max_response_bytes);
    bounded.max_timeout_ms = bounded.max_timeout_ms.min(operation.timeout_ms);
    let response = wasm_http::fetch(
        &bounded, client, &internal_grants, calls,
        HttpFetchRequest {
            method: operation.method.clone(), url: operation.url,
            headers: BTreeMap::from([("content-type".into(), "application/json".into()), ("accept".into(), "application/json".into())]),
            body_text: if operation.method == "POST" {
                Some(serde_json::to_string(&body).map_err(|_| "invalid request body")?)
            } else { None },
            body_base64: None, timeout_ms: Some(bounded.max_timeout_ms), auth_profile: operation.auth_profile.clone(),
        }, deadline,
    ).map_err(|_| "HTTP operation failed: check endpoint, credential, provider policy and request/response deadlines".to_string())?;
    let status = response["status"].as_u64().unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(format!(
            "HTTP operation upstream returned status {status}; response body withheld"
        ));
    }
    let text = response["body_text"]
        .as_str()
        .ok_or("HTTP operation requires a JSON response")?;
    let result: Value =
        serde_json::from_str(text).map_err(|_| "HTTP operation returned invalid JSON")?;
    if let Some(profile) = operation
        .auth_profile
        .as_ref()
        .and_then(|name| policy.auth_profiles.get(name))
    {
        let value = &profile.header_value;
        let credential = value.strip_prefix("Bearer ").unwrap_or(value);
        fn contains(value: &Value, secret: &str) -> bool {
            match value {
                Value::String(s) => s.contains(secret),
                Value::Array(a) => a.iter().any(|v| contains(v, secret)),
                Value::Object(o) => o
                    .iter()
                    .any(|(k, v)| k.contains(secret) || contains(v, secret)),
                _ => false,
            }
        }
        if !credential.is_empty() && contains(&result, credential) {
            return Err("HTTP operation response contained provider credentials".into());
        }
    }
    if !compile_schema(&operation.output_schema)?.is_valid(&result) {
        return Err("HTTP operation response does not match the approved schema".into());
    }
    Ok(result)
}

/// A standard host_json Wasm module with a fixed operation. JSON input is
/// concatenated as a value, not interpreted as source, URL, headers or options.
pub fn module(operation: &HttpOperation) -> Result<Vec<u8>, String> {
    operation.validate()?;
    let prefix = format!(
        "{{\"op\":\"http.operation\",\"operation\":{},\"input\":",
        serde_json::to_string(operation).map_err(|e| e.to_string())?
    );
    let escaped = prefix
        .bytes()
        .map(|b| format!("\\{b:02x}"))
        .collect::<String>();
    let heap = (prefix.len() + 15) & !7;
    let initial_pages = heap.div_ceil(65536).max(1);
    wat::parse_str(format!(r#"(module
        (import "froglet_host" "call_json" (func $call (param i32 i32) (result i64)))
        (memory (export "memory") {initial_pages} 128)
        (data (i32.const 0) "{escaped}")
        (global $heap (mut i32) (i32.const {heap}))
        (func $alloc (export "alloc") (param $n i32) (result i32)
            (local $p i32) (local $end i32)
            global.get $heap local.tee $p local.get $n i32.add local.tee $end
            local.get $p i32.lt_u if unreachable end
            local.get $end memory.size i32.const 16 i32.shl i32.gt_u
            if local.get $end i32.const 65535 i32.add i32.const 16 i32.shr_u memory.size i32.sub memory.grow i32.const -1 i32.eq if unreachable end end
            local.get $end global.set $heap local.get $p)
        (func (export "run") (param $p i32) (param $n i32) (result i64)
            (local $out i32) (local $len i32)
            local.get $n i32.const {prefix_len} i32.add i32.const 1 i32.add local.tee $len call $alloc local.set $out
            local.get $out i32.const 0 i32.const {prefix_len} memory.copy
            local.get $out i32.const {prefix_len} i32.add local.get $p local.get $n memory.copy
            local.get $out local.get $len i32.add i32.const 1 i32.sub i32.const 125 i32.store8
            local.get $out local.get $len call $call))"#,prefix_len=prefix.len())).map_err(|e|e.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        config::{WasmHttpAuthProfile, WasmPolicy},
        sandbox::{WasmExecutionOptions, WasmSandbox},
        wasm_host::WasmHostEnvironment,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio_rustls::{TlsAcceptor, rustls};

    pub(crate) struct Fixture {
        pub(crate) url: String,
        pub(crate) client: reqwest::Client,
        pub(crate) seen: Arc<Mutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    pub(crate) async fn fixture() -> Fixture {
        crate::tls::ensure_rustls_crypto_provider();
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let client = crate::tls::reqwest_client_builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
            .build()
            .unwrap();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_pair.serialize_der().into()),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let seen = Arc::new(Mutex::new(vec![]));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let log = log.clone();
                            async move {
                                let path = request.uri().path().to_string();
                                let auth = request
                                    .headers()
                                    .get("authorization")
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .to_string();
                                let bytes = axum::body::to_bytes(
                                    axum::body::Body::new(request.into_body()),
                                    1024 * 1024,
                                )
                                .await
                                .unwrap();
                                let body: Value =
                                    serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                                log.lock()
                                    .unwrap()
                                    .push(json!({"path":path,"auth":auth,"body":body}));
                                let mut response = hyper::Response::builder()
                                    .header("content-type", "application/json");
                                let value = match path.as_str() {
                                    "/tool" => {
                                        json!({"length":body["text"].as_str().unwrap_or("").len()})
                                    }
                                    "/infer" => {
                                        json!({"answer":"fixture inference","model":body["model"],"max_tokens":body["max_tokens"]})
                                    }
                                    "/redirect" => {
                                        response =
                                            response.status(307).header("location", "/secret");
                                        json!({})
                                    }
                                    "/secret" => {
                                        json!({"credential":"fixture-secret-do-not-return"})
                                    }
                                    "/slow" => {
                                        tokio::time::sleep(Duration::from_millis(250)).await;
                                        json!({})
                                    }
                                    "/big" => json!({"payload":"x".repeat(20_000)}),
                                    "/error" => {
                                        response = response.status(500);
                                        json!({"secret":"fixture-secret-do-not-return"})
                                    }
                                    _ => Value::Null,
                                };
                                Ok::<_, std::convert::Infallible>(
                                    response
                                        .body(axum::body::Body::from(value.to_string()))
                                        .unwrap(),
                                )
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Fixture {
            url,
            client,
            seen,
            task,
        }
    }
    pub(crate) fn operation(url: String) -> HttpOperation {
        HttpOperation {
            url,
            method: "POST".into(),
            auth_profile: Some("upstream".into()),
            input_schema: json!({"type":"object","properties":{"text":{"type":"string","maxLength":32}},"required":["text"],"additionalProperties":false}),
            output_schema: json!({"type":"object"}),
            fixed_body: Map::new(),
            timeout_ms: 1000,
            max_request_bytes: 2048,
            max_response_bytes: 4096,
        }
    }
    pub(crate) fn policy(op: &HttpOperation) -> WasmHttpPolicy {
        let url = reqwest::Url::parse(&op.url).unwrap();
        WasmHttpPolicy {
            operations_only: true,
            operation_hashes: vec![op.hash().unwrap()],
            allowed_hosts: vec!["localhost".into()],
            allow_private_networks: true,
            max_calls_per_execution: 10,
            max_timeout_ms: 2000,
            max_request_body_bytes: 4096,
            max_response_body_bytes: 8192,
            max_redirects: 5,
            auth_profiles: BTreeMap::from([(
                "upstream".into(),
                WasmHttpAuthProfile {
                    scheme: "https".into(),
                    host: "localhost".into(),
                    port: url.port().unwrap(),
                    path_prefix: "/".into(),
                    header_name: "authorization".into(),
                    header_value: "Bearer fixture-secret-do-not-return".into(),
                },
            )]),
        }
    }
    fn run(
        op: HttpOperation,
        input: Value,
        policy: &WasmHttpPolicy,
        client: &reqwest::Client,
    ) -> Result<Value, String> {
        let grants = op.capabilities().unwrap();
        execute(
            op,
            input,
            policy,
            client,
            &grants,
            &mut 0,
            Some(Instant::now() + Duration::from_secs(2)),
        )
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn generated_wasm_invokes_tool_and_inference_with_fixed_terms() {
        let fixture = fixture().await;
        for path in ["/tool", "/infer"] {
            let mut op = operation(format!("{}{path}", fixture.url));
            if path == "/infer" {
                op.fixed_body = serde_json::from_value(
                    json!({"model":"approved-model","max_tokens":32,"stream":false}),
                )
                .unwrap();
            }
            let policy = policy(&op);
            let environment = Arc::new(WasmHostEnvironment {
                policy: WasmPolicy {
                    http: Some(policy.clone()),
                    sqlite: None,
                },
                http_client: Some(fixture.client.clone()),
            });
            let result = WasmSandbox::new(1)
                .unwrap()
                .execute_module_with_options(
                    &module(&op).unwrap(),
                    &json!({"text":"hello"}),
                    WasmExecutionOptions {
                        abi_version: crate::wasm::WASM_HOST_JSON_ABI_V1.into(),
                        capabilities_granted: op.capabilities().unwrap(),
                        host_environment: Some(environment),
                        max_memory_bytes: None,
                        fuel_limit: None,
                    },
                    Duration::from_secs(2),
                )
                .unwrap();
            if path == "/tool" {
                assert_eq!(result, json!({"length":5}));
            } else {
                assert_eq!(result["max_tokens"], 32);
                assert_eq!(result["model"], "approved-model");
            }
            let before = fixture.seen.lock().unwrap().len();
            assert!(
                run(
                    op.clone(),
                    json!({"text":"hi","url":"https://evil.example","max_tokens":999999}),
                    &policy,
                    &fixture.client
                )
                .is_err()
            );
            assert!(
                run(
                    op.clone(),
                    json!({"text":"x".repeat(33)}),
                    &policy,
                    &fixture.client
                )
                .is_err()
            );
            let mut altered = op;
            altered.url.push_str("/unapproved");
            assert!(
                run(altered, json!({"text":"hi"}), &policy, &fixture.client)
                    .unwrap_err()
                    .contains("not approved")
            );
            assert_eq!(fixture.seen.lock().unwrap().len(), before);
        }
        assert_eq!(fixture.seen.lock().unwrap().len(), 2);
        assert!(
            fixture
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|r| r["auth"] == "Bearer fixture-secret-do-not-return")
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn upstream_failures_limits_redirects_secrets_and_policy_fail_closed() {
        let fixture = fixture().await;
        for path in ["/redirect", "/big", "/slow", "/secret", "/error"] {
            let mut op = operation(format!("{}{path}", fixture.url));
            if path == "/slow" {
                op.timeout_ms = 30;
            }
            let error = run(
                op.clone(),
                json!({"text":"hello"}),
                &policy(&op),
                &fixture.client,
            )
            .unwrap_err();
            assert!(!error.contains("fixture-secret"));
        }
        assert_eq!(
            fixture
                .seen
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r["path"] == "/secret")
                .count(),
            1,
            "redirect was followed"
        );
        let op = operation(format!("{}/tool", fixture.url));
        let mut configured = policy(&op);
        let before = fixture.seen.lock().unwrap().len();
        configured.allow_private_networks = false;
        assert!(
            run(
                op.clone(),
                json!({"text":"hello"}),
                &configured,
                &fixture.client
            )
            .is_err()
        );
        assert_eq!(fixture.seen.lock().unwrap().len(), before);
        configured = policy(&op);
        let request = HttpFetchRequest {
            method: "POST".into(),
            url: op.url.clone(),
            headers: BTreeMap::new(),
            body_text: None,
            body_base64: None,
            timeout_ms: None,
            auth_profile: None,
        };
        assert!(
            wasm_http::fetch(
                &configured,
                &fixture.client,
                &[crate::wasm::WASM_CAPABILITY_HTTP_FETCH.into()],
                &mut 0,
                request,
                None
            )
            .unwrap_err()
            .contains("disabled")
        );
        let grants = op.capabilities().unwrap();
        let mut used = 0;
        execute(
            op.clone(),
            json!({"text":"hello"}),
            &configured,
            &fixture.client,
            &grants,
            &mut used,
            None,
        )
        .unwrap();
        assert!(
            execute(
                op,
                json!({"text":"hello"}),
                &configured,
                &fixture.client,
                &grants,
                &mut used,
                None
            )
            .is_err()
        );
    }
    #[test]
    fn schemas_endpoints_and_fixed_fields_are_checked_without_io() {
        let op = operation("https://example.com/infer".into());
        op.validate().unwrap();
        for url in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/?token=secret",
            "https://example.com/#x",
        ] {
            let mut bad = op.clone();
            bad.url = url.into();
            assert!(bad.validate().is_err());
        }
        for schema in [
            json!({"$ref":"file:///etc/passwd"}),
            json!({"$ref":"https://example.com/schema"}),
            json!({"pattern":".*"}),
            json!({"type":"wrong"}),
        ] {
            let mut bad = op.clone();
            bad.output_schema = schema;
            assert!(bad.validate().is_err());
        }
        let mut bad = op;
        bad.fixed_body.insert("text".into(), json!("fixed"));
        assert!(bad.validate().is_err());
    }
}
