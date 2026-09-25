use std::{net::TcpListener, process::Command};

#[test]
fn native_preflight_checks_relay_backend_only_when_needed() {
    let provider = TcpListener::bind("127.0.0.1:0").unwrap();
    let runtime = TcpListener::bind("127.0.0.1:0").unwrap();
    let backend = TcpListener::bind("127.0.0.1:0").unwrap();
    let provider_url = format!("http://{}", provider.local_addr().unwrap());
    let runtime_url = format!("http://{}", runtime.local_addr().unwrap());
    let backend_addr = backend.local_addr().unwrap().to_string();
    drop(provider);
    drop(runtime);

    let run = |relay_url: &str| {
        Command::new(env!("CARGO_BIN_EXE_froglet-node"))
            .args(["doctor", "--preflight", "--json"])
            .env("FROGLET_PROVIDER_URL", &provider_url)
            .env("FROGLET_RUNTIME_URL", &runtime_url)
            .env("FROGLET_NETWORK_MODE", "clearnet")
            .env("FROGLET_RELAY_URL", relay_url)
            .env("FROGLET_TOR_BACKEND_LISTEN_ADDR", &backend_addr)
            .output()
            .unwrap()
    };

    let without_relay = run("");
    assert!(
        without_relay.status.success(),
        "{}",
        String::from_utf8_lossy(&without_relay.stdout)
    );
    let with_relay = run("wss://relay.froglet.dev/v1/tunnel");
    assert!(!with_relay.status.success());
    let report: serde_json::Value = serde_json::from_slice(&with_relay.stdout).unwrap();
    assert_eq!(report["code"], "port_unavailable");
    assert!(report["error"].as_str().unwrap().contains(&backend_addr));
}
