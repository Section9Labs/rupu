//! The Rust half of the finding-query lockstep. The web's
//! `src/lib/findingQuery/grammar.lockstep.test.ts` runs the same fixtures, so
//! the two parsers cannot drift apart.

use rupu_coverage::{parse_query, FIELDS};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/finding_query/");
    serde_json::from_str(&std::fs::read_to_string(format!("{path}{name}")).unwrap()).unwrap()
}

#[test]
fn every_case_parses_or_fails_exactly_as_the_fixture_says() {
    for case in fixture("cases.json").as_array().unwrap() {
        let q = case["q"].as_str().unwrap();
        match (parse_query(q), case.get("terms"), case.get("error")) {
            (Ok(parsed), Some(terms), None) => {
                assert_eq!(
                    serde_json::to_value(&parsed).unwrap()["terms"],
                    *terms,
                    "q = {q:?}"
                );
            }
            (Err(e), None, Some(want)) => {
                assert_eq!(
                    e.token as u64,
                    want["token"].as_u64().unwrap(),
                    "q = {q:?}: {e}"
                );
                assert_eq!(
                    serde_json::to_value(e.code).unwrap(),
                    want["code"],
                    "q = {q:?}: {e}"
                );
            }
            (got, _, _) => panic!("q = {q:?}: fixture and parser disagree: {got:?}"),
        }
    }
}

#[test]
fn the_registry_matches_the_fixture() {
    assert_eq!(
        serde_json::to_value(FIELDS).unwrap(),
        fixture("fields.json")
    );
}

#[test]
fn errors_carry_char_offsets_of_their_token() {
    let e = parse_query("é tag>=x").unwrap_err();
    assert_eq!((e.token, e.start, e.end), (1, 2, 8));
}
