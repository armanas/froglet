//! Offline verifier CLI for Froglet signed-artifact chains.
//!
//! Reads a single artifact document, an array of documents, or an
//! `{"artifacts": [...]}` feed page from a file or stdin; verifies every
//! artifact's envelope and semantics; and, when the input forms exactly one
//! chain (one descriptor, offer, quote, and deal), validates the full chain.
//! No network, no node, no clock unless `--now` is supplied.
//!
//! Exit codes: 0 = nothing invalid, 1 = at least one invalid finding,
//! 2 = usage or input error.

use std::io::Read;
use std::process::ExitCode;

use froglet_verify::{
    ChainDocumentsReport, DocumentStatus, SemanticsOutcome, documents_form_chain,
    extract_documents, validate_chain_documents, verify_document,
};

const USAGE: &str =
    "usage: froglet-verify [--now <unix-seconds>] [--chain | --no-chain] [--json] <file | ->

Verifies Froglet signed artifacts offline: envelope cryptography, per-kind
semantics, and (when the input holds one descriptor, offer, quote, and deal)
the full hash-linked chain.

input shapes: a single artifact object, a JSON array of artifacts, or an
{\"artifacts\": [...]} page; {\"artifact\": {...}} wrappers are unwrapped.

  --now <unix-seconds>  evaluate expiry against this time (default: skipped)
  --chain               require chain validation (error if input is not a chain)
  --no-chain            per-artifact verification only
  --json                emit the full JSON report instead of the summary";

struct Options {
    now: Option<i64>,
    chain: Option<bool>,
    json: bool,
    input: String,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut now = None;
    let mut chain = None;
    let mut json = false;
    let mut input = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--now" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--now requires a unix-seconds value".to_string())?;
                now = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| format!("--now value is not an integer: {value}"))?,
                );
            }
            "--chain" => chain = Some(true),
            "--no-chain" => chain = Some(false),
            "--json" => json = true,
            "--help" | "-h" => return Err(String::new()),
            other if input.is_none() => input = Some(other.to_string()),
            other => return Err(format!("unexpected argument: {other}")),
        }
        index += 1;
    }

    Ok(Options {
        now,
        chain,
        json,
        input: input.ok_or_else(|| "missing input file (or - for stdin)".to_string())?,
    })
}

fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|error| format!("failed to read stdin: {error}"))?;
        Ok(buffer)
    } else {
        std::fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))
    }
}

fn status_glyph(status: DocumentStatus) -> &'static str {
    match status {
        DocumentStatus::Verified => "ok ",
        DocumentStatus::EnvelopeOnly => "env",
        DocumentStatus::Invalid => "BAD",
    }
}

fn print_summary(report: &ChainDocumentsReport) {
    for artifact in &report.artifacts {
        let hash_prefix: String = artifact.hash.chars().take(12).collect();
        println!(
            "[{}] {:<14} {}  {}",
            status_glyph(artifact.status),
            artifact.artifact_type,
            hash_prefix,
            match &artifact.semantics {
                SemanticsOutcome::Valid => "envelope + semantics verified".to_string(),
                SemanticsOutcome::Invalid { error } => error.clone(),
                SemanticsOutcome::NotEvaluated { reason } => reason.clone(),
            }
        );
        if artifact.expired == Some(true) {
            println!("      note: expired relative to --now");
        }
    }
    for error in &report.structure_errors {
        println!("chain structure: {error}");
    }
    if let Some(chain) = &report.chain {
        for path_report in &chain.reports {
            let label = serde_json::to_value(path_report.path)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{:?}", path_report.path));
            if path_report.valid {
                println!("chain {label}: ok");
            } else {
                for issue in &path_report.issues {
                    println!("chain {label}: {} — {}", issue.code, issue.message);
                }
            }
            for warning in &path_report.warnings {
                println!(
                    "chain {label}: warning {} — {}",
                    warning.code, warning.message
                );
            }
        }
    }
    println!(
        "result: {}",
        if report.valid {
            "VALID"
        } else if report.chain_evaluated || !report.structure_errors.is_empty() {
            "INVALID"
        } else {
            // Per-artifact mode: valid=false only reflects artifact findings.
            if report
                .artifacts
                .iter()
                .any(|artifact| artifact.status == DocumentStatus::Invalid)
            {
                "INVALID"
            } else {
                "VALID (per-artifact only; no chain evaluated)"
            }
        }
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_args(&args) {
        Ok(options) => options,
        Err(message) => {
            if message.is_empty() {
                println!("{USAGE}");
                return ExitCode::from(0);
            }
            eprintln!("error: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let raw = match read_input(&options.input) {
        Ok(raw) => raw,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("error: input is not valid JSON: {error}");
            return ExitCode::from(2);
        }
    };

    let documents = match extract_documents(&value) {
        Ok(documents) => documents,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    if documents.is_empty() {
        eprintln!("error: input contains no artifact documents");
        return ExitCode::from(2);
    }

    let run_chain = options
        .chain
        .unwrap_or_else(|| documents_form_chain(&documents));

    let (report, any_invalid) = if run_chain {
        let report = validate_chain_documents(&documents, options.now);
        let any_invalid = !report.valid;
        (report, any_invalid)
    } else {
        let artifacts: Vec<_> = documents
            .iter()
            .map(|document| verify_document(document, options.now))
            .collect();
        let any_invalid = artifacts
            .iter()
            .any(|artifact| artifact.status == DocumentStatus::Invalid);
        (
            ChainDocumentsReport {
                valid: !any_invalid,
                chain_evaluated: false,
                structure_errors: Vec::new(),
                artifacts,
                chain: None,
            },
            any_invalid,
        )
    };

    if options.json {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("error: failed to serialize report: {error}");
                return ExitCode::from(2);
            }
        }
    } else {
        print_summary(&report);
    }

    if any_invalid {
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}
