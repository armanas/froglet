//! Live integration tests for `froglet_publish_engine::publish()`.
//!
//! Every test in this file is `#[ignore]`d so `cargo test` runs them
//! only when invoked explicitly. They require infrastructure that the
//! CI box does not have:
//!
//! - A running `froglet-node` daemon at `FROGLET_DAEMON_URL`
//!   (default `http://127.0.0.1:8080`)
//! - The daemon's provider-control token reachable via
//!   `FROGLET_PROVIDER_CONTROL_TOKEN` (literal) or
//!   `FROGLET_PROVIDER_CONTROL_TOKEN_PATH` (file)
//! - For SelfHosted + Tor backends, network reachability to
//!   `marketplace.froglet.dev` (or `FROGLET_TEST_MARKETPLACE_URL`)
//!
//! Run with:
//!
//! ```bash
//! # Start daemon first:
//! froglet-node &
//!
//! # Smoke local backend (no marketplace):
//! cargo test -p froglet-publish-engine --test live_integration \
//!   live_local -- --ignored --nocapture
//!
//! # Smoke self-hosted (registers against marketplace):
//! FROGLET_TEST_SELF_URL=https://my-host.fly.dev \
//!   cargo test -p froglet-publish-engine --test live_integration \
//!     live_self_hosted -- --ignored --nocapture
//! ```
//!
//! Failures are noisy on purpose. Each test prints the daemon's
//! provider_id + the offer hash so the operator can verify on the
//! marketplace dashboard.

use froglet_protocol::manifest::ServiceManifest;
use froglet_publish_engine::{DaemonClient, PublishInput, SourceLocator, publish};
use url::Url;

fn marketplace_url() -> Url {
    let raw = std::env::var("FROGLET_TEST_MARKETPLACE_URL")
        .unwrap_or_else(|_| "https://marketplace.froglet.dev".to_string());
    Url::parse(&raw).expect("FROGLET_TEST_MARKETPLACE_URL must be a valid URL")
}

fn python_handler() -> &'static str {
    r#"def handler(event, context):
    return {"echo": event, "mounts": context.get("mounts", {})}
"#
}

fn service_manifest(name: &str, hosting: &str, hosting_url: Option<&str>) -> ServiceManifest {
    let hosting_self = hosting_url
        .map(|u| format!("\n[hosting.self]\nurl = \"{u}\""))
        .unwrap_or_default();
    let toml = format!(
        r#"schema_version = "froglet-service/v3"
service_id = "{name}"
runtime = "python"
package_kind = "inline_source"
entrypoint_kind = "handler"
entrypoint = "handler.py"
[hosting]
default = "{hosting}"{hosting_self}
[settlement]
method = "none"
"#
    );
    ServiceManifest::from_toml(&toml)
        .expect("test manifest parses")
        .0
}

#[tokio::test]
#[ignore = "requires running froglet-node daemon"]
async fn live_local_backend_publishes_offer_without_marketplace() {
    let daemon = DaemonClient::from_env().expect("DaemonClient::from_env");
    let service = service_manifest("publish-engine-live-local", "local", None);
    let input = PublishInput {
        project: None,
        service,
        source: SourceLocator::Inline(python_handler().to_string()),
        hosting_override: None,
        marketplace_url: marketplace_url(),
    };
    let output = publish(input, &daemon)
        .await
        .expect("publish should succeed");

    println!("LOCAL BACKEND RESULT:");
    println!("  provider_id:  {}", output.provider_id);
    println!("  public_url:   {}", output.public_url);
    println!("  offer_hash:   {}", output.offer_hash);
    println!("  warnings:     {:?}", output.warnings);

    assert!(!output.provider_id.is_empty());
    assert!(!output.offer_hash.is_empty());
    assert!(
        output.marketplace_offer_url.is_none(),
        "local backend must not register with marketplace; got URL: {:?}",
        output.marketplace_offer_url
    );
    assert!(
        output.warnings.iter().any(|w| matches!(
            w,
            froglet_publish_engine::PublishWarning::NotRegistered { .. }
        )),
        "expected NotRegistered warning for local backend"
    );
}

#[tokio::test]
#[ignore = "requires running daemon + FROGLET_TEST_SELF_URL pointing at a reachable provider"]
async fn live_self_hosted_backend_registers_with_marketplace() {
    let self_url = std::env::var("FROGLET_TEST_SELF_URL")
        .expect("FROGLET_TEST_SELF_URL must be set to a public https URL pointing at this daemon");

    let daemon = DaemonClient::from_env().expect("DaemonClient::from_env");
    let service = service_manifest("publish-engine-live-self", "self", Some(&self_url));
    let input = PublishInput {
        project: None,
        service,
        source: SourceLocator::Inline(python_handler().to_string()),
        hosting_override: None,
        marketplace_url: marketplace_url(),
    };
    let output = publish(input, &daemon)
        .await
        .expect("publish should succeed");

    println!("SELF-HOSTED BACKEND RESULT:");
    println!("  provider_id:           {}", output.provider_id);
    println!("  public_url:            {}", output.public_url);
    println!("  offer_hash:            {}", output.offer_hash);
    if let Some(u) = &output.marketplace_offer_url {
        println!("  marketplace_offer_url: {u}");
    }
    if let Some(u) = &output.status_url {
        println!("  status_url:            {u}");
    }

    assert!(!output.provider_id.is_empty());
    assert!(output.marketplace_offer_url.is_some());
    assert!(output.status_url.is_some());
    assert!(output.public_url.starts_with("https://"));
}

#[tokio::test]
#[ignore = "requires running daemon with FROGLET_NETWORK_MODE=tor (or dual)"]
async fn live_tor_backend_uses_daemon_onion_url() {
    let daemon = DaemonClient::from_env().expect("DaemonClient::from_env");
    let service = service_manifest("publish-engine-live-tor", "tor", None);
    let input = PublishInput {
        project: None,
        service,
        source: SourceLocator::Inline(python_handler().to_string()),
        hosting_override: None,
        marketplace_url: marketplace_url(),
    };
    let output = publish(input, &daemon)
        .await
        .expect("publish should succeed");

    println!("TOR BACKEND RESULT:");
    println!("  provider_id:  {}", output.provider_id);
    println!("  public_url:   {}", output.public_url);
    println!("  offer_hash:   {}", output.offer_hash);

    assert!(output.public_url.contains(".onion"));
    assert!(output.marketplace_offer_url.is_some());
}
