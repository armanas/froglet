//! A `froglet.wasm.run_json.v1` service for requester-supplied, illustrative term mappings.
//!
//! Input: `{"mappings":[{"source":"term","target":"concept"}],"observed_terms":["term"]}`.
//! Strings use exact, case-sensitive identity, without normalization. The result reports mapping conflicts and
//! missing observed terms; it makes no claims about scientific truth or the correctness of a target concept.
//! See the `adder` sample for the allocation and packed pointer/length ABI.

use serde::de::{value::MapAccessDeserializer, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

/// Reserve `len` bytes for the host to write the request into.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buffer = Vec::<u8>::with_capacity(len as u32 as usize);
    let pointer = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    pointer as i32
}

/// Read request bytes and return response JSON as `(pointer << 32) | length`.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn run(pointer: i32, len: i32) -> i64 {
    let request =
        unsafe { std::slice::from_raw_parts(pointer as u32 as *const u8, len as u32 as usize) };
    let bytes = respond(request).into_bytes();
    let packed = ((bytes.as_ptr() as u32 as i64) << 32) | bytes.len() as i64;
    std::mem::forget(bytes);
    packed
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    mappings: Vec<Object<Mapping>>,
    observed_terms: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    source: String,
    target: String,
}

// A derived serde struct also accepts a positional JSON array. This wrapper accepts only objects while retaining
// derived field validation, including rejection of duplicate, unknown, and missing fields.
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> Result<Self::Value, M::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

#[derive(Serialize)]
struct Conflict {
    source: String,
    targets: Vec<String>,
}

#[derive(Serialize)]
struct Response {
    mappings_consistent: bool,
    conflicts: Vec<Conflict>,
    unmapped_terms: Vec<String>,
    mapped_terms: usize,
}

const INVALID_INPUT: &str = "{\"error\":\"expected mappings as objects with nonempty source and target strings, and observed_terms as nonempty strings\"}";

/// Evaluate the JSON request with the same behavior used by the Wasm ABI.
pub fn respond(request: &[u8]) -> String {
    let Ok(Object(request)) = serde_json::from_slice::<Object<Request>>(request) else {
        return INVALID_INPUT.to_owned();
    };
    if request
        .mappings
        .iter()
        .any(|Object(row)| row.source.is_empty() || row.target.is_empty())
        || request.observed_terms.iter().any(String::is_empty)
    {
        return INVALID_INPUT.to_owned();
    }

    let mut mappings = BTreeMap::<String, BTreeSet<String>>::new();
    for Object(row) in request.mappings {
        mappings.entry(row.source).or_default().insert(row.target);
    }
    let observed = request.observed_terms.into_iter().collect::<BTreeSet<_>>();
    let conflicts = mappings
        .iter()
        .filter(|(_, targets)| targets.len() > 1)
        .map(|(source, targets)| Conflict {
            source: source.clone(),
            targets: targets.iter().cloned().collect(),
        })
        .collect::<Vec<_>>();
    let unmapped_terms = observed
        .iter()
        .filter(|term| !mappings.contains_key(*term))
        .cloned()
        .collect();
    let mapped_terms = observed
        .iter()
        .filter(|term| {
            mappings
                .get(*term)
                .is_some_and(|targets| targets.len() == 1)
        })
        .count();

    serde_json::to_string(&Response {
        mappings_consistent: conflicts.is_empty(),
        conflicts,
        unmapped_terms,
        mapped_terms,
    })
    .expect("response contains only JSON-serializable strings, booleans, and integers")
}

#[cfg(test)]
mod tests {
    use super::{respond, INVALID_INPUT};

    #[test]
    fn distinct_targets_conflict_and_identical_rows_do_not() {
        let response = respond(
            br#"{
            "mappings":[
                {"source":"zeta","target":"Beta"},
                {"source":"alpha","target":"Only"},
                {"source":"zeta","target":"Alpha"},
                {"source":"zeta","target":"Beta"},
                {"source":"alpha","target":"Only"}
            ],
            "observed_terms":["zeta","alpha","alpha","missing","missing"]
        }"#,
        );
        assert_eq!(
            response,
            r#"{"mappings_consistent":false,"conflicts":[{"source":"zeta","targets":["Alpha","Beta"]}],"unmapped_terms":["missing"],"mapped_terms":1}"#
        );
    }

    #[test]
    fn row_and_term_order_do_not_change_the_response() {
        let first = respond(br#"{"mappings":[{"source":"b","target":"2"},{"source":"a","target":"2"},{"source":"a","target":"1"},{"source":"b","target":"1"}],"observed_terms":["z","y","a"]}"#);
        let second = respond(br#"{"observed_terms":["a","y","z","y"],"mappings":[{"target":"1","source":"b"},{"source":"a","target":"1"},{"source":"a","target":"2"},{"source":"b","target":"2"}]}"#);
        assert_eq!(first, second);
        assert_eq!(
            first,
            r#"{"mappings_consistent":false,"conflicts":[{"source":"a","targets":["1","2"]},{"source":"b","targets":["1","2"]}],"unmapped_terms":["y","z"],"mapped_terms":0}"#
        );
    }

    #[test]
    fn conflicts_include_sources_that_were_not_observed() {
        assert_eq!(respond(br#"{"mappings":[{"source":"other","target":"A"},{"source":"other","target":"B"}],"observed_terms":["missing"]}"#), r#"{"mappings_consistent":false,"conflicts":[{"source":"other","targets":["A","B"]}],"unmapped_terms":["missing"],"mapped_terms":0}"#);
    }

    #[test]
    fn unmapped_terms_do_not_make_unambiguous_mappings_inconsistent() {
        assert_eq!(respond(br#"{"mappings":[{"source":"known","target":"concept"}],"observed_terms":["known","missing"]}"#), r#"{"mappings_consistent":true,"conflicts":[],"unmapped_terms":["missing"],"mapped_terms":1}"#);
    }

    #[test]
    fn exact_string_identity_and_json_escapes_are_preserved() {
        assert_eq!(respond(br#"{"mappings":[{"source":"Term","target":"concept"},{"source":"quote\"\n\u03b1","target":"escaped\\target"}],"observed_terms":["Term","term"," Term ","quote\"\n\u03b1","quote\"\n\u03b1"]}"#), r#"{"mappings_consistent":true,"conflicts":[],"unmapped_terms":[" Term ","term"],"mapped_terms":2}"#);
    }

    #[test]
    fn empty_arrays_are_valid() {
        assert_eq!(
            respond(br#"{"mappings":[],"observed_terms":[]}"#),
            r#"{"mappings_consistent":true,"conflicts":[],"unmapped_terms":[],"mapped_terms":0}"#
        );
    }

    #[test]
    fn malformed_or_unsupported_inputs_return_the_same_error() {
        for input in [
            &b"not JSON"[..],
            &b"\xff"[..],
            br#"{"mappings":[],"observed_terms":[]} trailing"#,
            br#"{"mappings":[],"observed_terms":[],"extra":true}"#,
            br#"{"mappings":[],"mappings":[],"observed_terms":[]}"#,
            br#"{"mappings":[],"observed_terms":[] ,"observed_terms":[]}"#,
            br#"{"mappings":[]}"#,
            br#"{"mappings":{},"observed_terms":[]}"#,
            br#"{"mappings":[],"observed_terms":[1]}"#,
            br#"{"mappings":[],"observed_terms":[""]}"#,
            br#"{"mappings":[{"source":"","target":"A"}],"observed_terms":[]}"#,
            br#"{"mappings":[{"source":"a","target":""}],"observed_terms":[]}"#,
            br#"{"mappings":[{"source":"a","source":"b","target":"A"}],"observed_terms":[]}"#,
            br#"{"mappings":[{"source":"a","target":"A","target":"B"}],"observed_terms":[]}"#,
            br#"{"mappings":[{"source":"a","target":"A","extra":true}],"observed_terms":[]}"#,
            br#"{"mappings":[{"source":"a"}],"observed_terms":[]}"#,
            br#"{"mappings":[["a","A"]],"observed_terms":[]}"#,
            br#"[[{"source":"a","target":"A"}],["a"]]"#,
            br#"null"#,
        ] {
            assert_eq!(respond(input), INVALID_INPUT, "input: {input:?}");
        }
    }
}
