#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let role = match std::env::var("FROGLET_NODE_ROLE")
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "provider" => froglet::server::ServiceRole::Provider,
        // "requester" accepted as alias for "runtime"
        "runtime" | "requester" => froglet::server::ServiceRole::Runtime,
        // Default: both provider and runtime on one node
        _ => froglet::server::ServiceRole::Dual,
    };
    froglet::server::run_with_role(role).await
}
