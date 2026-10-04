//! The offline verifier must accept the page a node actually serves.
//!
//! `fixtures/node_feed_page.json` is the body of `GET /v1/feed?limit=50` from a
//! fresh `FROGLET_NODE_ROLE=dual` node, reformatted with whitespace only. Each
//! entry of its `artifacts` is an index entry (`cursor`, `hash`, `kind`, ...)
//! that wraps the signed artifact under `document`. The verifier used to expect
//! the artifact itself there and refused the real feed with "item is not an
//! artifact document". The node crate feeds its live `/v1/feed` output through
//! the same API (`public_feed_page_verifies_offline_with_froglet_verify`), so
//! this fixture and the real route cannot drift apart unnoticed.

use std::{
    io::Write,
    process::{Command, Stdio},
};

use froglet_verify::{DocumentStatus, documents_form_chain, extract_documents, verify_document};
use serde_json::{Value, json};

const FEED_PAGE: &str = include_str!("fixtures/node_feed_page.json");
const FEED_PAGE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/node_feed_page.json"
);
const NOT_AN_ARTIFACT: &str = "item is not an artifact document (no artifact_type)";

fn feed_page() -> Value {
    serde_json::from_str(FEED_PAGE).expect("the fixture is JSON")
}

fn entries(page: &Value) -> &Vec<Value> {
    page["artifacts"].as_array().expect("page has artifacts")
}

fn served_documents(page: &Value) -> Vec<Value> {
    entries(page)
        .iter()
        .map(|entry| entry["document"].clone())
        .collect()
}

fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

/// The page with one document altered after signing: `created_at` is part of
/// the signed bytes, so the envelope no longer verifies.
fn page_with_altered_offer() -> Value {
    let mut page = feed_page();
    let created_at = page["artifacts"][1]["document"]["created_at"]
        .as_i64()
        .expect("document created_at");
    page["artifacts"][1]["document"]["created_at"] = json!(created_at + 1);
    page
}

struct CliRun {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str], stdin: Option<&str>) -> CliRun {
    let mut child = Command::new(env!("CARGO_BIN_EXE_froglet-verify"))
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start froglet-verify");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait for froglet-verify");
    CliRun {
        code: output.status.code().expect("exit code"),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

#[test]
fn the_fixture_keeps_the_shape_the_node_serves() {
    // Guards the fixture itself: a friendlier hand-edited page would make the
    // rest of this file pass without proving anything about the real route.
    let page = feed_page();
    assert_eq!(
        sorted_keys(&page),
        [
            "active_offer_hashes",
            "applied_cursor",
            "artifacts",
            "cursor_semantics",
            "cursor_type",
            "has_more",
            "next_cursor",
            "page_size"
        ]
    );

    let kinds: Vec<&str> = entries(&page)
        .iter()
        .map(|entry| entry["kind"].as_str().expect("entry kind"))
        .collect();
    assert_eq!(kinds, ["descriptor", "offer", "offer", "offer"]);

    for entry in entries(&page) {
        assert_eq!(
            sorted_keys(entry),
            [
                "actor_id",
                "created_at",
                "cursor",
                "document",
                "hash",
                "kind",
                "payload_hash"
            ]
        );
        assert!(
            entry.get("artifact_type").is_none(),
            "an entry is not itself an artifact"
        );
        assert_eq!(entry["document"]["artifact_type"], entry["kind"]);
        assert_eq!(entry["document"]["hash"], entry["hash"]);
        assert_eq!(entry["document"]["payload_hash"], entry["payload_hash"]);
    }
}

#[test]
fn extract_documents_returns_the_document_of_every_served_entry_in_order() {
    let page = feed_page();
    let documents = extract_documents(&page).expect("the served page is accepted");
    assert_eq!(documents, served_documents(&page));
    assert_eq!(documents.len(), 4);
}

#[test]
fn every_document_on_the_served_page_verifies() {
    let documents = extract_documents(&feed_page()).expect("the served page is accepted");
    for document in &documents {
        let report = verify_document(document, None);
        assert_eq!(
            report.status,
            DocumentStatus::Verified,
            "{} {}: {:?}",
            report.artifact_type,
            report.hash,
            report.semantics
        );
        assert!(report.envelope_valid);
    }
    // A public feed carries descriptors, offers and receipts, not a whole deal,
    // so the CLI takes its per-artifact path for it.
    assert!(!documents_form_chain(&documents));
}

#[test]
fn a_bare_array_of_entries_and_a_single_entry_are_accepted_too() {
    let page = feed_page();
    let expected = served_documents(&page);

    let array = page["artifacts"].clone();
    assert_eq!(extract_documents(&array).unwrap(), expected);

    let single = page["artifacts"][0].clone();
    assert_eq!(extract_documents(&single).unwrap(), expected[..1]);
}

#[test]
fn shapes_that_already_worked_are_returned_unchanged() {
    let page = feed_page();
    let documents = served_documents(&page);

    let plain_page = json!({ "artifacts": documents });
    assert_eq!(extract_documents(&plain_page).unwrap(), documents);

    let wrapped = json!({ "artifact": documents[0] });
    assert_eq!(extract_documents(&wrapped).unwrap(), documents[..1]);

    // Entries and bare documents can sit side by side; neither is unwrapped twice.
    let mixed = json!([page["artifacts"][0], documents[1]]);
    assert_eq!(extract_documents(&mixed).unwrap(), documents[..2]);
}

#[test]
fn an_entry_without_a_signed_document_is_still_refused() {
    let refused = [
        json!({ "artifacts": [{ "cursor": 1, "hash": "aa", "document": { "no": "artifact_type" } }] }),
        json!({ "artifacts": [{ "cursor": 1, "document": "not an object" }] }),
        json!({ "artifacts": [{ "cursor": 1, "document": null }] }),
        json!({ "artifacts": [{ "cursor": 1 }] }),
        // The unwrap is exactly one level deep.
        json!({ "artifacts": [{ "document": { "document": { "artifact_type": "offer" } } }] }),
    ];
    for input in refused {
        assert_eq!(
            extract_documents(&input),
            Err(NOT_AN_ARTIFACT.to_string()),
            "{input}"
        );
    }
}

#[test]
fn a_document_altered_inside_a_served_page_is_reported_invalid() {
    // Unwrapping must not skip verification: the altered offer is caught and
    // the untouched artifacts still verify.
    let documents = extract_documents(&page_with_altered_offer()).expect("shape is still valid");
    let statuses: Vec<DocumentStatus> = documents
        .iter()
        .map(|document| verify_document(document, None).status)
        .collect();
    assert_eq!(
        statuses,
        [
            DocumentStatus::Verified,
            DocumentStatus::Invalid,
            DocumentStatus::Verified,
            DocumentStatus::Verified
        ]
    );
}

#[test]
fn cli_verifies_the_served_page_from_stdin() {
    let run = run_cli(&["-"], Some(FEED_PAGE));
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    assert_eq!(run.stderr, "");

    let lines: Vec<&str> = run.stdout.lines().collect();
    let kinds: Vec<&str> = lines[..4]
        .iter()
        .map(|line| {
            line.split_whitespace()
                .nth(2)
                .expect("artifact type column")
        })
        .collect();
    assert_eq!(kinds, ["descriptor", "offer", "offer", "offer"]);
    for line in &lines[..4] {
        assert!(line.starts_with("[ok ]"), "{line}");
        assert!(line.ends_with("envelope + semantics verified"), "{line}");
    }
    assert_eq!(lines.len(), 5, "{}", run.stdout);
    assert!(lines[4].starts_with("result: VALID"), "{}", run.stdout);
}

#[test]
fn cli_reads_the_served_page_from_a_file_with_the_same_verdict() {
    let from_file = run_cli(&[FEED_PAGE_PATH], None);
    let from_stdin = run_cli(&["-"], Some(FEED_PAGE));
    assert_eq!(
        from_file.code, 0,
        "{}{}",
        from_file.stdout, from_file.stderr
    );
    assert_eq!(from_file.stdout, from_stdin.stdout);
}

#[test]
fn cli_json_report_marks_every_served_artifact_verified() {
    let run = run_cli(&["--json", "-"], Some(FEED_PAGE));
    assert_eq!(run.code, 0, "{}{}", run.stdout, run.stderr);
    let report: Value = serde_json::from_str(&run.stdout).expect("--json prints JSON");
    assert_eq!(report["valid"], true);
    assert_eq!(report["chain_evaluated"], false);
    let statuses: Vec<&str> = report["artifacts"]
        .as_array()
        .expect("artifacts")
        .iter()
        .map(|artifact| artifact["status"].as_str().expect("status"))
        .collect();
    assert_eq!(statuses, ["verified"; 4]);
}

#[test]
fn cli_exits_one_when_a_document_in_the_page_was_altered() {
    let altered = serde_json::to_string(&page_with_altered_offer()).unwrap();
    let run = run_cli(&["-"], Some(&altered));
    assert_eq!(run.code, 1, "{}{}", run.stdout, run.stderr);
    assert_eq!(
        run.stdout
            .lines()
            .filter(|line| line.starts_with("[BAD]"))
            .count(),
        1,
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("result: INVALID"), "{}", run.stdout);
}

#[test]
fn cli_still_exits_two_when_an_entry_has_no_signed_document() {
    let input = json!({ "artifacts": [{ "cursor": 1, "hash": "aa", "document": {} }] });
    let run = run_cli(&["-"], Some(&input.to_string()));
    assert_eq!(run.code, 2, "{}{}", run.stdout, run.stderr);
    assert!(run.stderr.contains(NOT_AN_ARTIFACT), "{}", run.stderr);
    assert_eq!(run.stdout, "");
}
