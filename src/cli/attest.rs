//! `froglet-node attest-dns-record <zone>` — produce the signed
//! `_froglet.<zone>` TXT record for DNS identity attestation.
//!
//! This is the subject side of Flow 1 (steps 1–2) in
//! `docs/IDENTITY_ATTESTATION.md`: sign the canonical bind statement with
//! the node identity key and print the exact TXT record to publish. The
//! marketplace attestation service (Order 81, not in this repo) verifies
//! the record and issues the signed `identity_attestation/v1` credential.
//!
//! The signed bytes come from
//! `froglet_protocol::protocol::identity_attestation::dns_bind_statement_bytes`,
//! the same function the issuance service verifies with — the JCS shape is
//! a cross-service contract.

use super::{CliError, pop_flag};
use crate::protocol::{dns_bind_statement_bytes, format_dns_txt_record};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AttestDnsRecordReport {
    pub zone: String,
    /// DNS name to create the TXT record at.
    pub record_name: String,
    /// The exact TXT record value.
    pub record_value: String,
    pub provider_id: String,
    pub ts: String,
    pub next_step: String,
}

pub async fn run(mut args: Vec<String>) -> Result<(), CliError> {
    let json_mode = pop_flag(&mut args, "--json");
    if args.len() != 1 || args[0].starts_with("--") {
        return Err(CliError::BadArgs(
            "usage: froglet-node attest-dns-record <zone> [--json]\n  zone is the bare DNS zone \
             you control, e.g. example.com"
                .to_string(),
        ));
    }
    let zone = args[0].trim().trim_end_matches('.').to_ascii_lowercase();

    let config = crate::config::NodeConfig::from_env()
        .map_err(|error| CliError::Other(format!("failed to load node config: {error}")))?;
    let identity = crate::identity::NodeIdentity::load_or_create(&config)
        .map_err(|error| CliError::Other(format!("failed to load node identity: {error}")))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| CliError::Other(format!("system clock before epoch: {error}")))?
        .as_secs();
    let ts = unix_to_rfc3339(now);

    let message =
        dns_bind_statement_bytes(&zone, identity.node_id(), &ts).map_err(CliError::BadArgs)?;
    let signature = identity.sign_message_hex(&message);

    let report = AttestDnsRecordReport {
        record_name: format!("_froglet.{zone}"),
        record_value: format_dns_txt_record(identity.node_id(), &signature, &ts),
        provider_id: identity.node_id().to_string(),
        zone,
        ts,
        next_step: "Publish the TXT record, then request issuance from the marketplace \
                    attestation service. The bind timestamp has a 10-minute freshness window — \
                    re-run this command if issuance is requested later."
            .to_string(),
    };

    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("report serializes")
        );
    } else {
        println!("Create this DNS TXT record (TTL 300 is a reasonable default):");
        println!();
        println!(
            "  {}. 300 IN TXT \"{}\"",
            report.record_name, report.record_value
        );
        println!();
        println!("provider_id: {}", report.provider_id);
        println!("signed_at:   {}", report.ts);
        println!();
        println!("{}", report.next_step);
    }
    Ok(())
}

/// Unix seconds → RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`). Hand-rolled
/// (Howard Hinnant's civil-from-days) to keep the crate free of a datetime
/// dependency; the repo has no other RFC 3339 producer.
fn unix_to_rfc3339(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs_of_day = unix_secs % 86_400;
    let (hour, minute, second) = (
        secs_of_day / 3_600,
        (secs_of_day % 3_600) / 60,
        secs_of_day % 60,
    );

    // civil_from_days: days since 1970-01-01 → (y, m, d).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_to_rfc3339_matches_known_timestamps() {
        assert_eq!(unix_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(unix_to_rfc3339(951_782_400), "2000-02-29T00:00:00Z"); // leap day
        assert_eq!(unix_to_rfc3339(1_751_500_800), "2025-07-03T00:00:00Z");
        assert_eq!(unix_to_rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
        assert_eq!(unix_to_rfc3339(1_767_225_600), "2026-01-01T00:00:00Z");
    }
}
