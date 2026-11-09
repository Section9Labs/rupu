use super::*;
use crate::api::workflow_observation::{capture, Window};
use std::{fs, path::PathBuf};
const ROOT: &str = "run_flow";
const WS: &str = "ws_fixture";
const WORKER: &str = "run_worker";
fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let state = AppState::new(
        tmp.path().canonicalize().unwrap(),
        rupu_config::PricingConfig::default(),
    );
    let record:rupu_orchestrator::runs::RunRecord=serde_json::from_value(json!({"id":ROOT,"workflow_name":"flow","status":"completed","inputs":{},"workspace_id":WS,"workspace_path":"/fixture","transcript_dir":"/fixture/transcripts","started_at":"2026-11-09T00:00:00Z"})).unwrap();
    state
        .run_store
        .create(record, "name: flow\nsteps: []\n")
        .unwrap();
    let path = state.run_store.root.join(ROOT);
    (tmp, state, path)
}
fn lines(values: &[Value]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|v| {
            let mut b = serde_json::to_vec(v).unwrap();
            b.push(b'\n');
            b
        })
        .collect()
}
fn dispatch(run: &str) -> Value {
    json!({"type":"dispatch_started","run_id":run,"sub_run_id":"sub_leaf","kind":"subagent","agent":"leaf","transcript_path":"/fixture/leaf.jsonl","codename":"flow>worker>leaf","provider":"openai","model":"local-flow-leaf"})
}
fn complete(run: &str) -> Value {
    json!({"type":"dispatch_completed","run_id":run,"sub_run_id":"sub_leaf","kind":"subagent","success":true,"tokens_in":1,"tokens_out":1})
}
fn result() -> Value {
    json!({"step_id":"branches","run_id":"","transcript_path":"","output":"done","success":true,"skipped":false,"rendered_prompt":"review","kind":"for_each","items":[{"index":0,"item":"alpha","sub_id":"a","rendered_prompt":"review","run_id":WORKER,"transcript_path":"/fixture/worker.jsonl","output":"done","success":true}],"findings":[],"finished_at":"2026-11-09T00:00:01Z"})
}
fn capture_events(state: &AppState, enriched: bool) -> (Value, Value) {
    let c = capture::capture_mode(state, ROOT, WS, Window::Prefix, enriched).unwrap();
    let p = c.page("events", None, None, 50).unwrap();
    (c.header, p)
}
#[test]
fn authentic_nested_dispatch_pair_keeps_emitter_and_captured_item_provenance() {
    let (_tmp, s, p) = fixture();
    fs::write(p.join("step_results.jsonl"), lines(&[result()])).unwrap();
    fs::write(
        p.join("events.jsonl"),
        lines(&[dispatch(WORKER), complete(WORKER)]),
    )
    .unwrap();
    let (h, page) = capture_events(&s, true);
    assert_eq!(h["event_emitters"], true);
    assert_eq!(page["event_emitters"], true);
    assert_eq!(h["sections"]["events"]["malformed_rows"], 0);
    assert_eq!(h["sections"]["events"]["nested_emitter_rows"], 2);
    for row in page["records"].as_array().unwrap() {
        assert_eq!(row["state"], "available");
        assert_eq!(row["value"]["run_id"], WORKER);
        assert_eq!(row["emitter"]["scope"], "captured_workflow_agent");
        assert_eq!(
            row["emitter"]["reference_record_ids"],
            json!(["result:0/items:0"])
        );
        assert!(row.get("event_type").is_none());
    }
}
#[test]
fn legacy_default_retains_old_rows_and_omits_all_enrichment_fields() {
    let (_tmp, s, p) = fixture();
    fs::write(p.join("step_results.jsonl"), lines(&[result()])).unwrap();
    fs::write(p.join("events.jsonl"), lines(&[dispatch(WORKER)])).unwrap();
    let (h, page) = capture_events(&s, false);
    assert!(h.get("event_emitters").is_none());
    assert!(page.get("event_emitters").is_none());
    assert!(h["sections"]["events"].get("nested_emitter_rows").is_none());
    assert_eq!(h["sections"]["events"]["malformed_rows"], 1);
    assert_eq!(page["records"][0]["state"], "malformed");
    assert!(page["records"][0].get("emitter").is_none());
}
#[test]
fn root_event_rows_are_unchanged_in_enriched_mode() {
    let (_tmp, s, p) = fixture();
    fs::write(
        p.join("events.jsonl"),
        lines(&[
            dispatch(ROOT),
            json!({"type":"step_started","run_id":ROOT,"step_id":"s","kind":"linear","agent":"a"}),
        ]),
    )
    .unwrap();
    let (_, old) = capture_events(&s, false);
    let (_, new) = capture_events(&s, true);
    assert_eq!(old["records"], new["records"]);
}
#[test]
fn unknown_future_and_foreign_events_are_unverified_not_malformed() {
    let (_tmp, s, p) = fixture();
    fs::write(
        p.join("events.jsonl"),
        lines(&[
            dispatch("run_foreign"),
            json!({"type":"future_event","run_id":ROOT,"private_payload":"not echoed"}),
        ]),
    )
    .unwrap();
    let (h, page) = capture_events(&s, true);
    assert_eq!(h["sections"]["events"]["malformed_rows"], 0);
    assert_eq!(h["sections"]["events"]["unverified_emitter_rows"], 2);
    for row in page["records"].as_array().unwrap() {
        assert_eq!(row["state"], "unavailable");
        assert!(row["value"].is_null());
        assert_eq!(row["reason"], "event_emitter_unverified");
        assert_eq!(row["emitter"]["reference_record_ids"], json!([]));
    }
    assert_eq!(page["records"][1]["event_type"], "future_event");
    assert!(!serde_json::to_string(&page).unwrap().contains("not echoed"));
}
#[test]
fn latest_window_without_reference_does_not_invent_membership_or_scan_prefix() {
    let (_tmp, s, p) = fixture();
    let mut bytes = vec![b'x'; input::MAX_FILE + 32];
    bytes.push(b'\n');
    bytes.extend(lines(&[dispatch(WORKER)]));
    fs::write(p.join("events.jsonl"), bytes).unwrap();
    let c = capture::capture_mode(&s, ROOT, WS, Window::Latest, true).unwrap();
    let page = c.latest_filtered_page("events", None, 1).unwrap();
    assert_eq!(page["records"][0]["state"], "unavailable");
    assert_eq!(page["event_emitters"], true);
    assert!(c.header["sections"]["events"]["head_truncated"]
        .as_bool()
        .unwrap());
    assert_eq!(c.header["sections"]["events"]["unverified_emitter_rows"], 1);
}
#[test]
fn bad_json_and_invalid_known_event_remain_malformed() {
    let (_tmp, s, p) = fixture();
    let mut bytes = b"not-json\n".to_vec();
    bytes.extend(lines(&[json!({"type":"dispatch_completed","run_id":ROOT})]));
    fs::write(p.join("events.jsonl"), bytes).unwrap();
    let (h, page) = capture_events(&s, true);
    assert_eq!(h["sections"]["events"]["malformed_rows"], 2);
    assert_eq!(h["sections"]["events"]["unverified_emitter_rows"], 0);
    assert!(page["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["state"] == "malformed"));
}
#[test]
fn event_self_reference_cannot_qualify_nested_dispatch() {
    let (_tmp, s, p) = fixture();
    let agent = json!({"type":"agent_started","run_id":ROOT,"step_id":"s","agent":"a","provider":"p","model":"m","agent_run_id":WORKER,"transcript_path":"/fixture/worker.jsonl"});
    fs::write(p.join("events.jsonl"), lines(&[agent, dispatch(WORKER)])).unwrap();
    let (_, page) = capture_events(&s, true);
    assert_eq!(page["records"][0]["state"], "available");
    assert_eq!(page["records"][1]["state"], "unavailable");
}
#[test]
fn captured_reference_limit_is_explicit_and_foreign_non_dispatch_is_not_qualified() {
    let mut index = Index::default();
    for i in 0..50 {
        index.add(WORKER, &format!("attempt:{i}"));
    }
    let Classified::Nested(v) = index.classify(&dispatch(WORKER), ROOT) else {
        panic!("nested")
    };
    assert_eq!(v["reference_record_ids"].as_array().unwrap().len(), 32);
    assert_eq!(v["references_truncated"], true);
    assert!(matches!(
        index.classify(&json!({"type":"run_paused","run_id":WORKER}), ROOT),
        Classified::Unverified(_)
    ));
}
#[test]
fn unavailable_reference_rows_do_not_enter_index() {
    let (_tmp, _s, path) = fixture();
    let input = input::Input::open(&path).unwrap();
    let sections = BTreeMap::from([(
        "attempts".into(),
        vec![serde_json::to_vec(
            &json!({"record_id":"attempt:0","state":"malformed","value":{"agent_run_id":WORKER}}),
        )
        .unwrap()],
    )]);
    let index = Index::build(&sections, &input, 0).unwrap();
    assert!(matches!(
        index.classify(&dispatch(WORKER), ROOT),
        Classified::Unverified(_)
    ));
}
#[test]
fn diagnostics_never_truncate_identity_or_echo_oversized_event_type() {
    let index = Index::default();
    let value = json!({"type":"x".repeat(129),"run_id":"x".repeat(129)});
    let Classified::Unverified(v) = index.classify(&value, ROOT) else {
        panic!("unverified")
    };
    assert!(v["run_id"].is_null());
    assert!(diagnostic_type(&value).is_none());
}
#[tokio::test]
async fn capture_cache_separates_legacy_and_enriched_requests() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (_tmp, s, p) = fixture();
    fs::write(p.join("events.jsonl"), lines(&[dispatch(WORKER)])).unwrap();
    let mut snapshots = Vec::new();
    for flag in ["", "&include_event_emitters=true", ""] {
        let response = super::super::routes()
            .with_state(s.clone())
            .oneshot(
                Request::builder()
                    .uri(format!("/api/runs/{ROOT}/observation?ws_id={WS}{flag}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value["event_emitters"],
            if flag.is_empty() {
                Value::Null
            } else {
                json!(true)
            }
        );
        snapshots.push(value["snapshot"].clone());
    }
    assert_ne!(snapshots[0], snapshots[1]);
    assert_eq!(snapshots[0], snapshots[2]);
}
