//! `?q=` on the paged lists (`/api/runs/workflows`, `/api/runs/agents`,
//! `/api/sessions`, `/api/projects/:id/{runs,sessions}`), and the registry
//! fixtures the web's twin registries are checked against.
//!
//! Regenerate the fixtures after a deliberate registry change with
//! `BLESS=1 cargo test -p rupu-cp --test it list_query::`.

// Throwaway in-process mock-server client, not rupu's egress.
#![allow(clippy::disallowed_methods)]

use rupu_cp::query::{agent_runs, sessions, workflow_runs, Fields};
use serde_json::Value;

fn fixture_matches(list: &str, fields: Fields) {
    let path = format!(
        "{}/tests/fixtures/list_query/{list}.fields.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let got = serde_json::to_value(fields).unwrap();
    if std::env::var("BLESS").is_ok() {
        let mut text = String::from("[\n");
        let rows: Vec<String> = got
            .as_array()
            .unwrap()
            .iter()
            .map(|f| format!("  {}", serde_json::to_string(f).unwrap()))
            .collect();
        text.push_str(&rows.join(",\n"));
        text.push_str("\n]\n");
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let want: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("fixture missing: run with BLESS=1"),
    )
    .unwrap();
    assert_eq!(got, want, "{list}: the registry and {path} differ");
}

#[test]
fn workflow_runs_registry_matches_its_fixture() {
    fixture_matches("workflow_runs", workflow_runs::FIELDS);
}

#[test]
fn agent_runs_registry_matches_its_fixture() {
    fixture_matches("agent_runs", agent_runs::FIELDS);
}

#[test]
fn sessions_registry_matches_its_fixture() {
    fixture_matches("sessions", sessions::FIELDS);
}

/// A time field matches by time, and only a time field does; an ordered
/// field compares on its ladder.
#[test]
fn every_registry_pairs_kinds_with_their_match_modes() {
    use rupu_query::{FieldKind, MatchMode};
    for fields in [workflow_runs::FIELDS, agent_runs::FIELDS, sessions::FIELDS] {
        for f in fields {
            let time = matches!(f.def.kind, FieldKind::Time);
            let by_time = matches!(f.mode, MatchMode::Since | MatchMode::Until);
            assert_eq!(time, by_time, "{}", f.def.key);
            let ordered = f.def.kind.is_ordered();
            assert_eq!(ordered, f.mode == MatchMode::Ordered, "{}", f.def.key);
        }
    }
}
