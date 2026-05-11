use std::io::Read;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // First positional arg (when present) selects a non-server subcommand.
    // Anything else runs the long-running server. This keeps the binary's
    // primary use case as the default and lets operators reach the
    // node-key utilities without a separate CLI binary.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("sign-message") => return run_sign_message(),
        Some("print-identity") => return run_print_identity(),
        Some("help" | "--help" | "-h") => {
            print_help();
            return Ok(());
        }
        _ => {}
    }

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

/// Read a message from stdin and emit a hex BIP340 Schnorr signature over
/// the message using the node identity. Used by providers who need to
/// produce signed claims (e.g. marketplace provider-domain claims under
/// `<slug>.providers.<suffix>`) without standing up a separate signing
/// tool. Loads the same identity the server would, from the
/// `FROGLET_DATA_DIR`-rooted seed file.
///
/// Stderr gets a one-line `provider_id: <pubkey>` so a caller can see
/// which identity signed the message; stdout is just the signature hex
/// so the output is pipe-friendly.
fn run_sign_message() -> Result<(), Box<dyn std::error::Error>> {
    let identity = load_identity_for_cli()?;
    let mut message = Vec::new();
    std::io::stdin().read_to_end(&mut message)?;
    eprintln!("provider_id: {}", identity.node_id());
    println!("{}", identity.sign_message_hex(&message));
    Ok(())
}

/// Print the node's provider_id (uncompressed-secp256k1 pubkey hex) so a
/// provider can copy it into marketplace claims without parsing the seed
/// file by hand.
fn run_print_identity() -> Result<(), Box<dyn std::error::Error>> {
    let identity = load_identity_for_cli()?;
    println!("{}", identity.node_id());
    Ok(())
}

fn load_identity_for_cli() -> Result<froglet::identity::NodeIdentity, Box<dyn std::error::Error>> {
    let config = froglet::config::NodeConfig::from_env()
        .map_err(|e| format!("failed to load node config: {e}"))?;
    let identity = froglet::identity::NodeIdentity::load_or_create(&config)
        .map_err(|e| format!("failed to load node identity: {e}"))?;
    Ok(identity)
}

fn print_help() {
    println!(
        "froglet-node — Froglet provider/runtime/dual server\n\
         \n\
         Usage:\n  \
           froglet-node                          run the server (FROGLET_NODE_ROLE selects role)\n  \
           froglet-node sign-message             read a message from stdin and emit a hex Schnorr signature\n  \
           froglet-node print-identity           print the node's provider_id (pubkey hex)\n  \
           froglet-node help                     show this message\n"
    );
}
