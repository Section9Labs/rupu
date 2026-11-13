use super::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

struct Fixture {
    home: TempDir,
    dir: std::path::PathBuf,
    transcript: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("sessions/ses_context");
        fs::create_dir_all(&dir).unwrap();
        let transcript = home.path().join("lead.jsonl");
        let record = json!({"version":1,"session_id":"ses_context","agent_name":"lead","provider_name":"provider","model":"model","agent_system_prompt":"private prompt","max_turns":10,"permission_mode":"readonly","no_stream":true,"workspace_id":"ws_context","workspace_path":home.path(),"transcripts_dir":home.path(),"created_at":"2026-11-13T10:00:00Z","updated_at":"2026-11-13T10:01:00Z","status":"idle","last_run_id":"run_lead","last_transcript_path":transcript,"runs":[{"run_id":"run_lead","prompt":"private user message","transcript_path":transcript,"started_at":"2026-11-13T10:00:00Z"}],"context_window_tokens":1000,"max_tokens":300,"total_tokens_in":999999});
        fs::write(dir.join("session.json"), record.to_string()).unwrap();
        let f = Self {
            home,
            dir,
            transcript,
        };
        f.events(vec![Self::start("run_lead"), Self::usage(450, None)]);
        f
    }
    fn start(id: &str) -> Value {
        json!({"type":"run_start","data":{"run_id":id,"workspace_id":"ws_context","agent":"lead","provider":"provider","model":"model","started_at":"2026-11-13T10:00:00Z","mode":"readonly"}})
    }
    fn usage(n: u32, purpose: Option<&str>) -> Value {
        json!({"type":"usage","data":{"provider":"provider","model":"model","input_tokens":n,"output_tokens":10,"cached_tokens":40,"purpose":purpose}})
    }
    fn events(&self, events: Vec<Value>) {
        fs::write(
            &self.transcript,
            events
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
        )
        .unwrap();
    }
    fn edit(&self, apply: impl FnOnce(&mut Value)) {
        let p = self.dir.join("session.json");
        let mut v: Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        apply(&mut v);
        fs::write(p, v.to_string()).unwrap();
    }
    fn get(&self) -> ContextSnapshot {
        read_snapshot(self.home.path(), "ses_context").unwrap()
    }
}

#[test]
fn context_snapshot_only_reads_lead_usage_and_never_returns_history() {
    let f = Fixture::new();
    let child = f.home.path().join("child.jsonl");
    fs::write(&child, Fixture::usage(9999, None).to_string()).unwrap();
    f.events(vec![
        Fixture::start("run_lead"),
        json!({"type":"turn_start","data":{"turn_idx":2}}),
        Fixture::usage(450, None),
        json!({"type":"tool_call","data":{"call_id":"dispatch_child","tool":"dispatch_agent","input":{"agent":"child"}}}),
        json!({"type":"tool_result","data":{"call_id":"dispatch_child","duration_ms":1,"output":json!({"run_id":"run_child","transcript_path":child}).to_string()}}),
        Fixture::usage(900, Some("compaction")),
    ]);
    let s = f.get();
    assert_eq!(s.freshness, "recorded");
    assert_eq!(s.input_tokens, Some(450));
    assert_eq!(s.transcript_turn, Some(2));
    assert_eq!(s.usage_index, Some(1));
    assert_eq!(s.context_window_tokens, Some(1000));
    assert_eq!(s.compact_threshold_tokens, Some(700));
    assert_eq!(s.output_headroom_tokens, Some(300));
    let serialized = serde_json::to_string(&s).unwrap();
    assert!(!serialized.contains("private"));
    assert!(!serialized.contains("transcript_path"));
    assert!(s.call_id.is_none() && s.observed_at.is_none());
}
#[test]
fn context_snapshot_does_not_fall_back_to_old_run() {
    let f = Fixture::new();
    f.edit(|v| {
        v["active_run_id"] = json!("run_next");
        v["active_transcript_path"] = json!(f.home.path().join("missing.jsonl"));
    });
    let s = f.get();
    assert!(s.input_tokens.is_none());
    assert_eq!(s.run_id.as_deref(), Some("run_next"));
}
#[test]
fn context_snapshot_rejects_wrong_transcript_owner_and_model() {
    let f = Fixture::new();
    f.events(vec![Fixture::start("run_child"), Fixture::usage(900, None)]);
    assert_eq!(f.get().freshness, "unavailable");
    let mut start = Fixture::start("run_lead");
    start["data"]["model"] = json!("different");
    f.events(vec![start, Fixture::usage(900, None)]);
    assert!(f.get().input_tokens.is_none());
}
#[test]
fn context_snapshot_resolved_limits_outrank_pin_and_keep_source() {
    let f = Fixture::new();
    f.edit(|v|v["model_limits"]=json!({"input":{"tokens":800,"source":{"kind":"observed"}},"output":{"tokens":100,"source":{"kind":"agent"}},"compact_at_percent":60,"output_shares_context":true}));
    let s = f.get();
    assert_eq!(s.context_window_tokens, Some(800));
    assert_eq!(s.context_window_source.as_deref(), Some("observed"));
    assert_eq!(s.compact_threshold_tokens, Some(480));
    assert_eq!(s.compact_at_percent, Some(60));
}
#[test]
fn context_snapshot_unknown_limits_do_not_invent_percentage() {
    let f = Fixture::new();
    f.edit(|v| {
        v["context_window_tokens"] = Value::Null;
        v["max_tokens"] = Value::Null;
    });
    let s = f.get();
    assert_eq!(s.input_tokens, Some(450));
    assert!(
        s.context_window_tokens.is_none()
            && s.compact_threshold_tokens.is_none()
            && s.context_window_source.is_none()
    );
}
#[test]
fn context_snapshot_compaction_invalidates_until_next_lead_call() {
    let f = Fixture::new();
    let compact = json!({"type":"compaction","data":{"seq":1,"summarized_messages":10,"backup_path":"private","messages":[]}});
    f.events(vec![
        Fixture::start("run_lead"),
        Fixture::usage(450, None),
        compact.clone(),
    ]);
    assert_eq!(f.get().freshness, "after_compaction");
    assert!(f.get().input_tokens.is_none());
    f.events(vec![
        Fixture::start("run_lead"),
        Fixture::usage(450, None),
        compact,
        Fixture::usage(120, None),
    ]);
    assert_eq!(f.get().input_tokens, Some(120));
    fs::write(f.dir.join("compaction.result.json"),json!({"id":"cmp_one","outcome":"compacted","detail":"done","finished_at":"2026-11-13T11:00:00Z"}).to_string()).unwrap();
    assert_eq!(f.get().freshness, "after_compaction");
}
#[test]
fn context_snapshot_stale_marker_is_not_cleaned_up_and_settings_invalidate() {
    let f = Fixture::new();
    let path = f.dir.join("compaction.json");
    let marker =
        json!({"id":"cmp_stale","pid":u32::MAX,"started_at":"2026-11-13T10:00:00Z"}).to_string();
    fs::write(&path, &marker).unwrap();
    assert_eq!(f.get().input_tokens, Some(450));
    assert_eq!(fs::read_to_string(&path).unwrap(), marker);
    assert!(!f.dir.join("session.lock").exists());
    fs::write(
        f.dir.join("settings.json"),
        json!({"provider":"other","model":"new","tools":null}).to_string(),
    )
    .unwrap();
    let s = f.get();
    assert_eq!(s.freshness, "model_changed");
    assert!(s.input_tokens.is_none() && s.context_window_tokens.is_none());
}
#[test]
fn context_snapshot_corrupt_transcript_does_not_reuse_older_number() {
    let f = Fixture::new();
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(&f.transcript)
        .unwrap();
    writeln!(file, "broken json").unwrap();
    assert_eq!(f.get().freshness, "unavailable");
    assert!(f.get().input_tokens.is_none());
}
#[tokio::test]
async fn context_snapshot_http_host_and_identity_are_explicit() {
    let f = Fixture::new();
    let state = AppState::new(
        f.home.path().to_path_buf(),
        rupu_config::PricingConfig::default(),
    );
    let app = routes().with_state(state);
    let request = |path: &str| {
        axum::http::Request::builder()
            .uri(path)
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let response = app
        .clone()
        .oneshot(request("/api/sessions/ses_context/context?host=local"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let data: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(data["schema_version"], 1);
    assert_eq!(data["host_id"], "local");
    assert_eq!(data["session_id"], "ses_context");
    assert_eq!(
        app.clone()
            .oneshot(request("/api/sessions/ses_context/context?host=remote"))
            .await
            .unwrap()
            .status(),
        501
    );
    f.edit(|v| v["session_id"] = json!("ses_wrong"));
    assert_eq!(
        app.oneshot(request("/api/sessions/ses_context/context"))
            .await
            .unwrap()
            .status(),
        409
    );
}

#[test]
fn context_snapshot_zero_usage_is_unreported_and_never_reuses_prior_number() {
    let f = Fixture::new();
    f.events(vec![
        Fixture::start("run_lead"),
        Fixture::usage(450, None),
        Fixture::usage(0, None),
    ]);
    let s = f.get();
    assert_eq!(s.freshness, "awaiting_usage");
    assert!(s.input_tokens.is_none());
}
#[test]
fn context_snapshot_unresolved_limits_keep_pins_and_nonshared_output_reserves_nothing() {
    let f = Fixture::new();
    f.edit(|v|{v["compact_at_percent"]=json!(50);v["model_limits"]=json!({"input":{"tokens":null,"source":{"kind":"unknown"}},"output":{"tokens":null,"source":{"kind":"unknown"}},"compact_at_percent":80,"output_shares_context":false});});
    let s = f.get();
    assert_eq!(s.context_window_tokens, Some(1000));
    assert_eq!(s.compact_at_percent, Some(50));
    assert_eq!(s.compact_threshold_tokens, Some(500));
    assert!(s.output_headroom_tokens.is_none());
}

#[test]
fn context_snapshot_rejects_wrong_agent_workspace_and_provider() {
    for field in ["agent", "workspace_id", "provider"] {
        let f = Fixture::new();
        let mut start = Fixture::start("run_lead");
        start["data"][field] = json!("other");
        f.events(vec![start, Fixture::usage(900, None)]);
        let snapshot = f.get();
        assert_eq!(snapshot.freshness, "unavailable", "{field}");
        assert!(snapshot.input_tokens.is_none());
    }
}

#[test]
fn context_snapshot_oversized_transcript_is_not_scanned() {
    let f = Fixture::new();
    fs::OpenOptions::new()
        .write(true)
        .open(&f.transcript)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let snapshot = f.get();
    assert_eq!(snapshot.freshness, "unavailable");
    assert!(snapshot.input_tokens.is_none());
}

#[test]
fn context_snapshot_live_compaction_has_identity_without_an_old_measurement() {
    let f = Fixture::new();
    let marker =
        json!({"id":"cmp_live","pid":std::process::id(),"started_at":"2026-11-13T10:00:00Z"})
            .to_string();
    fs::write(f.dir.join("compaction.json"), &marker).unwrap();
    let snapshot = f.get();
    assert_eq!(snapshot.freshness, "compacting");
    assert!(snapshot.compacting && snapshot.input_tokens.is_none());
    assert_eq!(snapshot.run_id.as_deref(), Some("run_lead"));
    assert!(!f.dir.join("session.lock").exists());
    fs::write(
        f.dir.join("settings.json"),
        json!({"provider":"provider","model":"next","tools":null}).to_string(),
    )
    .unwrap();
    let changed = f.get();
    assert!(changed.compacting && changed.context_window_tokens.is_none());
    assert_eq!(changed.freshness, "compacting");
}

#[test]
fn context_snapshot_archived_session_preserves_its_owner() {
    let f = Fixture::new();
    let archived = f.home.path().join("sessions-archive");
    fs::create_dir_all(&archived).unwrap();
    fs::rename(&f.dir, archived.join("ses_context")).unwrap();
    let snapshot = f.get();
    assert_eq!(snapshot.session_id, "ses_context");
    assert_eq!(snapshot.input_tokens, Some(450));
}
