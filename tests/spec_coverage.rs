/// Enforces the docs/SPEC.md rule that every normative statement maps to at
/// least one executable test.
///
/// SPEC.md marks normative statements with bracketed anchors built from
/// `SPEC-`, an area code, and a number. A test claims one by naming it after
/// the word "covers:" in a comment. This test fails if any anchor in the spec
/// has no claiming test, or if a test claims an anchor the spec does not
/// define — so prose and code cannot drift apart silently.
///
/// (This file deliberately contains no literal anchor: the scanner reads its
/// own source too, and an example would register as a phantom claim.)
use std::{collections::BTreeSet, fs, path::PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn spec_anchors() -> BTreeSet<String> {
    let spec = fs::read_to_string(repo_root().join("docs/SPEC.md")).expect("read docs/SPEC.md");
    let mut anchors = BTreeSet::new();
    let mut rest = spec.as_str();
    while let Some(start) = rest.find("[SPEC-") {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find(']') {
            let anchor = &rest[..end];
            // Anchors are [SPEC-<AREA>-<N>]; ignore markdown links that merely
            // start with the same prefix.
            if anchor
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
            {
                anchors.insert(anchor.to_string());
            }
        }
    }
    anchors
}

fn claimed_anchors() -> BTreeSet<String> {
    let mut claimed = BTreeSet::new();
    let mut roots = vec![
        repo_root().join("tests"),
        repo_root().join("froglet-protocol/src"),
        repo_root().join("froglet-verify/src"),
        repo_root().join("froglet-verify/tests"),
    ];

    while let Some(path) = roots.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                roots.push(entry_path);
            } else if entry_path.extension().is_some_and(|ext| ext == "rs") {
                let Ok(source) = fs::read_to_string(&entry_path) else {
                    continue;
                };
                for line in source.lines() {
                    let Some(marker) = line.find("covers:") else {
                        continue;
                    };
                    for token in line[marker + "covers:".len()..].split([',', ' ']) {
                        let token = token.trim();
                        if token.starts_with("SPEC-") {
                            claimed.insert(token.to_string());
                        }
                    }
                }
            }
        }
    }
    claimed
}

#[test]
fn every_normative_spec_statement_has_a_test() {
    let anchors = spec_anchors();
    assert!(
        !anchors.is_empty(),
        "docs/SPEC.md defines no [SPEC-XX-N] anchors; the coverage gate would be vacuous"
    );
    let claimed = claimed_anchors();

    let uncovered: Vec<_> = anchors.difference(&claimed).cloned().collect();
    assert!(
        uncovered.is_empty(),
        "docs/SPEC.md statements with no test claiming them via a `// covers: <anchor>` comment: {uncovered:?}"
    );
}

#[test]
fn no_test_claims_an_anchor_the_spec_does_not_define() {
    let anchors = spec_anchors();
    let claimed = claimed_anchors();

    let orphaned: Vec<_> = claimed.difference(&anchors).cloned().collect();
    assert!(
        orphaned.is_empty(),
        "tests claim anchors that docs/SPEC.md no longer defines (stale after a spec edit?): {orphaned:?}"
    );
}
