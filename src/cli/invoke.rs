//! `froglet-node invoke <offer_hash_or_id> <json_input>` — call a
//! Froglet service through the local daemon's runtime.
//!
//! Phase 2 minimum: read input from positional arg or stdin, POST to
//! the daemon's run-compute endpoint, print the response. Full deal
//! flow (quote → deal → execute → receipt) is handled by the daemon;
//! this subcommand is just the human-facing wrapper.
//!
//! Phase 2 stub: the wire format for run-compute differs by runtime
//! and the existing MCP `invoke_service` already handles the full
//! matrix. For now, point users at the MCP / direct daemon call and
//! defer the CLI version to a follow-up.

use super::{CliError, pop_flag};

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    let _ = json_mode; // suppress unused for now

    if args.len() < 2 {
        return Err(CliError::BadArgs(
            "usage: froglet-node invoke <offer_id_or_hash> <json_input> [--json]".to_string(),
        ));
    }

    // The full implementation calls the daemon's compute endpoint. The
    // wire format for that endpoint varies by runtime + contract; the
    // MCP `invoke_service` tool already handles every variant. Phase 2
    // ships the publish loop; invoke gets a thin pointer here so the
    // help output stays honest.
    Err(CliError::Other(
        "froglet-node invoke is a Phase 2 stub. Use the MCP `invoke_service` tool, \
         or call the daemon's runtime API directly. The CLI version lands in a \
         follow-up once the wire shape is stable across runtimes."
            .to_string(),
    ))
}
