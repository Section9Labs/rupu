//! `?q=` on the paged lists (`/api/runs/workflows`, `/api/runs/agents`,
//! `/api/sessions`, `/api/projects/:id/{runs,sessions}`, `/api/events`), and the registry
//! fixtures the web's twin registries are checked against.
//!
//! Regenerate the fixtures after a deliberate registry change with
//! `BLESS=1 cargo test -p rupu-cp --test it list_query::`.

// Throwaway in-process mock-server client, not rupu's egress.
#![allow(clippy::disallowed_methods)]

use rupu_cp::query::{agent_runs, events, sessions, workflow_runs, Fields};
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

#[test]
fn events_registry_matches_its_fixture() {
    fixture_matches("events", events::FIELDS);
}

/// The card group each event type answers to, as the Live Events page's
/// `cardFromEvent` assigns it; `src/lib/query/lists/events.lockstep.test.ts`
/// holds the page to the same table.
#[test]
fn event_groups_match_their_fixture() {
    let path = format!(
        "{}/tests/fixtures/list_query/events.groups.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let want: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (event_type, group) in &want {
        assert_eq!(events::group_of(event_type), group, "{event_type}");
        assert!(events::GROUPS.contains(&group.as_str()), "{group}");
    }
}

/// Event rows and the queries that keep them, as `GET /api/events` holds
/// them; `src/lib/query/lists/events.lockstep.test.ts` holds the page's
/// registry, over the cards it makes of the same rows, to the same answers.
#[test]
fn event_rows_match_their_fixture() {
    let path = format!(
        "{}/tests/fixtures/list_query/events.matches.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let rows = fixture["rows"].as_array().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let q = case["q"].as_str().unwrap();
        let query = rupu_cp::query::ListQuery::parse(Some(q), events::FIELDS)
            .unwrap()
            .unwrap();
        let kept: Vec<Value> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| query.matches(r).unwrap())
            .map(|(i, _)| Value::from(i))
            .collect();
        assert_eq!(Value::Array(kept), case["keep"], "{q}");
    }
}

/// A time field matches by time, and only a time field does; an ordered
/// field compares on its ladder.
#[test]
fn every_registry_pairs_kinds_with_their_match_modes() {
    use rupu_query::{FieldKind, MatchMode};
    for fields in [
        workflow_runs::FIELDS,
        agent_runs::FIELDS,
        sessions::FIELDS,
        events::FIELDS,
    ] {
        for f in fields {
            let time = matches!(f.def.kind, FieldKind::Time);
            let by_time = matches!(f.mode, MatchMode::Since | MatchMode::Until);
            assert_eq!(time, by_time, "{}", f.def.key);
            let ordered = f.def.kind.is_ordered();
            assert_eq!(ordered, f.mode == MatchMode::Ordered, "{}", f.def.key);
        }
    }
}

// ── endpoints ───────────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use reqwest::StatusCode;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore};
use serde_json::json;

async fn serve(state: rupu_cp::state::AppState) -> String {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn state(dir: &Path) -> rupu_cp::state::AppState {
    rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default())
}

/// A server over `dir` with one HTTP peer at `peer_url`; its host id.
async fn serve_with_peer(dir: &Path, peer_url: &str) -> (String, String) {
    let state = state(dir);
    let host = state.hosts.add_host("peer", peer_url, None).unwrap();
    (serve(state).await, host.id)
}

async fn get(url: String) -> (StatusCode, reqwest::header::HeaderMap, Value) {
    let resp = reqwest::get(&url).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    (status, headers, resp.json().await.unwrap())
}

async fn rows_of(url: String) -> Value {
    let (status, _, body) = get(url.clone()).await;
    assert_eq!(status, StatusCode::OK, "GET {url}: {body}");
    body
}

/// The values of `key` in page order (never sorted: order is under test).
fn col(rows: &Value, key: &str) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("not an array: {rows}"))
        .iter()
        .map(|r| r[key].as_str().unwrap().to_string())
        .collect()
}

fn day(d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, d, 12, 0, 0).unwrap()
}

fn record(id: &str, workflow: &str, status: RunStatus, at: DateTime<Utc>) -> RunRecord {
    RunRecord {
        customer: Some(None),
        id: id.into(),
        workflow_name: workflow.into(),
        status,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: "ws_q".into(),
        workspace_path: PathBuf::from("/tmp/proj"),
        transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: at,
        finished_at: Some(at),
        error_message: None,
        awaiting: Vec::new(),
        awaiting_step_id: None,
        approval_prompt: None,
        awaiting_since: None,
        expires_at: None,
        issue_ref: None,
        issue: None,
        parent_run_id: None,
        backend_id: None,
        worker_id: None,
        artifact_manifest_path: None,
        runner_pid: None,
        source_wake_id: None,
        active_step_id: None,
        active_step_kind: None,
        active_step_agent: None,
        active_step_transcript_path: None,
        resume_requested_at: None,
        resume_claimed_at: None,
        resume_claimed_by: None,
        resume_mode: None,
        resume_gate_id: None,
        resume_approver: None,
        resume_rerequested_at: None,
        reject_cleanup_pending: None,
        engagement_profiles: Vec::new(),
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        gate_decisions: Vec::new(),
        system_prompt_suffix: None,
        codename: None,
        cause: None,
    }
}

/// Six workflow runs, one a day: `alpha` on days 1–3, `beta` on days 4–6;
/// days 2, 4 and 6 failed, the rest completed.
fn seed_runs(global: &Path) -> RunStore {
    let store = RunStore::new(global.join("runs"));
    for d in 1..=6u32 {
        let workflow = if d <= 3 { "alpha" } else { "beta" };
        let status = if d % 2 == 0 {
            RunStatus::Failed
        } else {
            RunStatus::Completed
        };
        store
            .create(
                record(&format!("run_{d}"), workflow, status, day(d)),
                "name: x\n",
            )
            .unwrap();
    }
    store
}

#[tokio::test]
async fn workflow_runs_filter_by_q_before_paging() {
    let tmp = tempfile::tempdir().unwrap();
    seed_runs(tmp.path());
    let base = serve(state(tmp.path())).await;
    for host in ["&host=local", ""] {
        let url = |extra: &str| format!("{base}/api/runs/workflows?q=status:failed{host}{extra}");
        assert_eq!(
            col(&rows_of(url("")).await, "id"),
            ["run_6", "run_4", "run_2"]
        );
        assert_eq!(
            col(&rows_of(url("&offset=1&limit=1")).await, "id"),
            ["run_4"],
            "page 2 of the filtered list{host}"
        );
        assert_eq!(
            col(&rows_of(url("&offset=2&limit=5")).await, "id"),
            ["run_2"]
        );
    }
}

#[tokio::test]
async fn workflow_runs_q_ands_with_lifecycle_and_reads_time_and_text() {
    let tmp = tempfile::tempdir().unwrap();
    seed_runs(tmp.path());
    let base = serve(state(tmp.path())).await;
    let ids = |q: &'static str, extra: &'static str| {
        let base = base.clone();
        async move {
            col(
                &rows_of(format!("{base}/api/runs/workflows?host=local&q={q}{extra}")).await,
                "id",
            )
        }
    };
    assert_eq!(
        ids("workflow:beta", "&lifecycle=failed").await,
        ["run_6", "run_4"]
    );
    assert_eq!(
        ids("workflow:beta", "&lifecycle=completed").await,
        ["run_5"]
    );
    assert_eq!(
        ids("since:2026-10-02%20until:2026-10-03", "").await,
        ["run_3", "run_2"]
    );
    assert_eq!(ids("-status:failed%20ALPHA", "").await, ["run_3", "run_1"]);
    assert_eq!(ids("id:run_5", "").await, ["run_5"]);
    assert_eq!(
        ids("since:3652425d", "").await,
        ["run_6", "run_5", "run_4", "run_3", "run_2", "run_1"]
    );
    assert!(ids("until:3652425d", "").await.is_empty());
}

#[tokio::test]
async fn a_bad_q_is_a_400_naming_its_token_on_every_list() {
    let tmp = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path(), "ws_q", &tmp.path().join("proj"));
    let base = serve(state(tmp.path())).await;
    for path in [
        "/api/runs/workflows",
        "/api/runs/agents",
        "/api/sessions",
        "/api/projects/ws_q/runs",
        "/api/projects/ws_q/sessions",
        "/api/events",
    ] {
        let (status, _, body) = get(format!("{base}{path}?q=since:soon%20sevrity:high")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["token"], 0, "{path}: {body}");
        assert_eq!(body["code"], "bad_value", "{path}: {body}");
        assert_eq!(
            (body["start"].clone(), body["end"].clone()),
            (json!(0), json!(10))
        );
        let (_, _, body) = get(format!("{base}{path}?q=sevrity:high")).await;
        assert_eq!(body["code"], "unknown_key", "{path}: {body}");
        assert_eq!(
            body["error"],
            "unknown key `sevrity` (quote the text to search for it)"
        );
        for key in ["since", "until"] {
            let (status, _, body) = get(format!("{base}{path}?q=alpha%20{key}:100000000d")).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {key}: {body}");
            assert_eq!(body["code"], "bad_value", "{path} {key}: {body}");
            assert_eq!(body["token"], 1, "{path} {key}: {body}");
            assert_eq!(
                body["error"],
                "`100000000d` is too long a span (at most 10000 years)"
            );
        }
    }
}

#[tokio::test]
async fn is_archived_lists_the_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let store = seed_runs(tmp.path());
    store.archive("run_2").unwrap();
    store.archive("run_5").unwrap();
    let base = serve(state(tmp.path())).await;
    let rows = rows_of(format!("{base}/api/runs/workflows?q=is:archived")).await;
    assert_eq!(col(&rows, "id"), ["run_5", "run_2"]);
    assert_eq!(rows[0]["archived"], true);
    assert_eq!(rows[0]["host_id"], "local");
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?q=is:archived%20status:failed"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["run_2"]);
    let rows = rows_of(format!("{base}/api/runs/workflows?host=local")).await;
    assert_eq!(col(&rows, "id"), ["run_6", "run_4", "run_3", "run_1"]);
    let rows = rows_of(format!("{base}/api/runs/workflows?q=-is:archived")).await;
    assert_eq!(col(&rows, "id"), ["run_6", "run_4", "run_3", "run_1"]);
}

#[tokio::test]
async fn facets_count_the_scope_before_q() {
    let tmp = tempfile::tempdir().unwrap();
    seed_runs(tmp.path());
    let base = serve(state(tmp.path())).await;
    let body = rows_of(format!(
        "{base}/api/runs/workflows?host=local&q=workflow:beta&facets=1&lifecycle=failed"
    ))
    .await;
    assert_eq!(col(&body["rows"], "id"), ["run_6", "run_4"]);
    assert_eq!(body["facets"]["status"], json!({"failed": 3}));
    assert_eq!(body["facets"]["workflow"], json!({"alpha": 1, "beta": 2}));
    assert_eq!(body["facets"]["host"], json!({"local": 3}));
    assert_eq!(body["facets_partial"], false);
    // Without `facets=1` the body stays an array.
    assert!(rows_of(format!(
        "{base}/api/runs/workflows?host=local&q=workflow:beta"
    ))
    .await
    .is_array());
}

fn remote_run(id: &str, status: &str, at: &str, customer: Option<Option<&str>>) -> Value {
    let mut v = json!({
        "id": id,
        "workflow_name": "remote-wf",
        "status": status,
        "started_at": at,
        "finished_at": null,
        "trigger": "manual",
        "usage": {"input_tokens": 0, "output_tokens": 0, "cached_tokens": 0,
                  "cache_write_tokens": 0, "total_tokens": 0, "cost_usd": null,
                  "priced": true, "runs": 1, "partial": false},
        "turns": 0,
        "duration_ms": null,
        "codename": "cobalt-harbor",
        "codename_derived": false,
    });
    if let Some(c) = customer {
        v["customer"] = json!(c);
        v["customer_derived"] = json!(false);
    }
    v
}

#[tokio::test]
async fn a_remote_hosts_runs_are_filtered_on_the_coordinator() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs/workflows");
        then.status(200).json_body(json!([
            remote_run("rr_3", "failed", "2026-10-09T00:00:00Z", Some(None)),
            remote_run(
                "rr_2",
                "completed",
                "2026-10-08T00:00:00Z",
                Some(Some("acme"))
            ),
            remote_run("rr_1", "failed", "2026-10-07T00:00:00Z", Some(Some("acme"))),
        ]));
    });
    let tmp = tempfile::tempdir().unwrap();
    seed_runs(tmp.path());
    let (base, peer) = serve_with_peer(tmp.path(), &mock.base_url()).await;

    let rows = rows_of(format!(
        "{base}/api/runs/workflows?host={peer}&q=status:failed"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["rr_3", "rr_1"]);
    assert_eq!(rows[0]["host_id"], peer);
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?host={peer}&q=status:failed&offset=1&limit=1"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["rr_1"]);
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?host={peer}&q=customer:acme%20status:failed"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["rr_1"]);
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?host={peer}&q=customer:none"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["rr_3"]);
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?q=host:{peer}%20-status:failed"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["rr_2"]);

    // The fan-out merges both hosts' matches newest-first, then pages.
    let rows = rows_of(format!("{base}/api/runs/workflows?q=status:failed&limit=3")).await;
    assert_eq!(col(&rows, "id"), ["rr_3", "rr_1", "run_6"]);
    let rows = rows_of(format!(
        "{base}/api/runs/workflows?q=status:failed&offset=3"
    ))
    .await;
    assert_eq!(col(&rows, "id"), ["run_4", "run_2"]);

    // A peer's archive is not listed: 501 for it alone, named on the fan-out.
    let (status, _, body) = get(format!(
        "{base}/api/runs/workflows?host={peer}&q=is:archived"
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    let (status, headers, _) = get(format!("{base}/api/runs/workflows?q=is:archived")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get("x-rupu-hosts-unfilterable").unwrap(),
        &format!("{peer}:is")
    );
}

#[tokio::test]
async fn a_peer_that_cannot_report_a_queried_field_is_left_out_and_named() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs/workflows");
        then.status(200).json_body(json!([remote_run(
            "old_1",
            "failed",
            "2026-10-09T00:00:00Z",
            None
        )]));
    });
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs/agents");
        then.status(200).json_body(json!([{
            "run_id": "ra_1", "source": "standalone", "started_at": "2026-10-01T00:00:00Z",
        }]));
    });
    mock.mock(|when, then| {
        when.method("GET").path("/api/sessions");
        then.status(200).json_body(json!([{
            "session_id": "rs_1", "updated_at": "2026-10-01T00:00:00Z",
        }]));
    });
    let tmp = tempfile::tempdir().unwrap();
    let (base, peer) = serve_with_peer(tmp.path(), &mock.base_url()).await;
    for path in ["/api/runs/workflows", "/api/runs/agents", "/api/sessions"] {
        // A query that never names `customer` still filters the old peer.
        let rows = rows_of(format!("{base}{path}?host={peer}&q=-customer_less")).await;
        assert_eq!(rows.as_array().unwrap().len(), 1, "{path}");
        let (status, _, body) = get(format!("{base}{path}?host={peer}&q=customer:acme")).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{path}: {body}");
        assert_eq!(
            body["error"],
            format!("host {peer} can't report a customer for every run")
        );
        let (status, headers, body) = get(format!("{base}{path}?q=customer:acme")).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert_eq!(
            headers.get("x-rupu-hosts-without-customer").unwrap(),
            peer.as_str(),
            "{path}"
        );
        assert!(body.as_array().unwrap().is_empty(), "{path}: {body}");
    }
}

// ── agent runs ──────────────────────────────────────────────────────────

/// A standalone `rupu run` of `agent` on `model`, started `at`.
fn seed_agent_run(global: &Path, id: &str, agent: &str, model: &str, at: DateTime<Utc>) {
    let dir = global.join("transcripts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.meta.json")),
        json!({ "run_id": id, "trigger_source": "run_cli" }).to_string(),
    )
    .unwrap();
    let start = rupu_transcript::Event::RunStart {
        run_id: id.into(),
        workspace_id: "ws_q".into(),
        agent: agent.into(),
        provider: "anthropic".into(),
        model: model.into(),
        started_at: at,
        mode: rupu_transcript::RunMode::Ask,
        schema: None,
        system_prompt: None,
        codename: None,
        customer: Some(None),
        launch_skills: Vec::new(),
        launch_preload_skills: Vec::new(),
        launch_args: Vec::new(),
    };
    let mut line = serde_json::to_vec(&start).unwrap();
    line.push(b'\n');
    std::fs::write(dir.join(format!("{id}.jsonl")), line).unwrap();
}

#[tokio::test]
async fn agent_runs_filter_by_q_before_paging_on_every_path() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs/agents");
        then.status(200).json_body(json!([
            {"run_id": "ra_2", "source": "standalone", "agent": "reviewer",
             "model": "claude-opus", "started_at": "2026-10-09T00:00:00Z", "customer": null},
            {"run_id": "ra_1", "source": "standalone", "agent": "triage",
             "model": "claude-opus", "started_at": "2026-10-08T00:00:00Z", "customer": null},
        ]));
    });
    let tmp = tempfile::tempdir().unwrap();
    for d in 1..=4u32 {
        let agent = if d % 2 == 0 { "reviewer" } else { "triage" };
        seed_agent_run(
            tmp.path(),
            &format!("run_a{d}"),
            agent,
            "claude-sonnet",
            day(d),
        );
    }
    let (base, peer) = serve_with_peer(tmp.path(), &mock.base_url()).await;
    let ids = |rows: Value| col(&rows, "run_id");

    let url = format!("{base}/api/runs/agents?host=local&q=agent:review");
    assert_eq!(ids(rows_of(url.clone()).await), ["run_a4", "run_a2"]);
    assert_eq!(
        ids(rows_of(format!("{url}&offset=1&limit=1")).await),
        ["run_a2"]
    );
    let url = format!("{base}/api/runs/agents?host={peer}&q=agent:review");
    assert_eq!(ids(rows_of(url).await), ["ra_2"]);
    let url = format!("{base}/api/runs/agents?q=agent:review%20-model:sonnet");
    assert_eq!(ids(rows_of(url).await), ["ra_2"]);
    let url = format!("{base}/api/runs/agents?q=agent:review&offset=1&limit=2");
    assert_eq!(ids(rows_of(url).await), ["run_a4", "run_a2"]);
    let url = format!("{base}/api/runs/agents?q=since:2026-10-04%20source:standalone");
    assert_eq!(ids(rows_of(url).await), ["ra_2", "ra_1", "run_a4"]);

    let body = rows_of(format!("{base}/api/runs/agents?q=agent:review&facets=1")).await;
    assert_eq!(body["facets"]["agent"], json!({"reviewer": 3, "triage": 3}));
    let mut hosts = serde_json::Map::new();
    hosts.insert("local".into(), json!(4));
    hosts.insert(peer.clone(), json!(2));
    assert_eq!(body["facets"]["host"], Value::Object(hosts));
}

// ── sessions ────────────────────────────────────────────────────────────

fn seed_session(global: &Path, dir: &str, id: &str, agent: &str, at: DateTime<Utc>) {
    let path = global.join(dir).join(id);
    std::fs::create_dir_all(&path).unwrap();
    let v = json!({
        "session_id": id,
        "agent_name": agent,
        "workspace_id": "ws_q",
        "created_at": at.to_rfc3339(),
        "updated_at": at.to_rfc3339(),
        "status": "idle",
        "customer": null,
    });
    std::fs::write(path.join("session.json"), v.to_string()).unwrap();
}

#[tokio::test]
async fn sessions_filter_by_q_and_is_reads_their_scope() {
    let mock = httpmock::MockServer::start();
    // An SSH-like peer lists one scope without saying which.
    mock.mock(|when, then| {
        when.method("GET").path("/api/sessions");
        then.status(200).json_body(json!([
            {"session_id": "rs_1", "agent_name": "reviewer", "updated_at": "2026-10-09T00:00:00Z",
             "created_at": "2026-10-09T00:00:00Z", "customer": null},
        ]));
    });
    let tmp = tempfile::tempdir().unwrap();
    seed_session(tmp.path(), "sessions", "ses_1", "reviewer", day(1));
    seed_session(tmp.path(), "sessions", "ses_2", "triage", day(2));
    seed_session(tmp.path(), "sessions", "ses_3", "reviewer", day(3));
    seed_session(tmp.path(), "sessions-archive", "ses_4", "reviewer", day(4));
    seed_workspace(tmp.path(), "ws_q", &tmp.path().join("proj"));
    let (base, peer) = serve_with_peer(tmp.path(), &mock.base_url()).await;
    let ids = |rows: Value| col(&rows, "session_id");

    let url = format!("{base}/api/sessions?host=local&q=agent:reviewer");
    assert_eq!(ids(rows_of(url.clone()).await), ["ses_4", "ses_3", "ses_1"]);
    assert_eq!(
        ids(rows_of(format!("{url}&offset=1&limit=1")).await),
        ["ses_3"]
    );
    let url = format!("{base}/api/sessions?host=local&q=agent:reviewer%20is:active");
    assert_eq!(ids(rows_of(url).await), ["ses_3", "ses_1"]);
    let url = format!("{base}/api/sessions?host=local&q=is:archived");
    assert_eq!(ids(rows_of(url).await), ["ses_4"]);
    // `q` ANDs with the old `scope` param.
    let url = format!("{base}/api/sessions?host=local&scope=active&q=agent:triage");
    assert_eq!(ids(rows_of(url).await), ["ses_2"]);
    let url = format!("{base}/api/sessions?host=local&scope=archived&q=agent:triage");
    assert!(ids(rows_of(url).await).is_empty());

    let url = format!("{base}/api/sessions?host={peer}&q=is:active%20agent:review");
    let rows = rows_of(url).await;
    assert_eq!(ids(rows.clone()), ["rs_1"]);
    assert_eq!(rows[0]["scope"], "active");
    let url = format!("{base}/api/sessions?q=agent:reviewer%20is:active&limit=2");
    assert_eq!(ids(rows_of(url).await), ["rs_1", "ses_3"]);

    // The project's session list.
    let url = format!("{base}/api/projects/ws_q/sessions?q=agent:reviewer&offset=1");
    let rows = rows_of(url).await;
    assert_eq!(ids(rows.clone()), ["ses_3", "ses_1"]);
    assert!(rows[0].get("host_id").is_none());
}

#[tokio::test]
async fn project_runs_filter_by_q_before_paging() {
    let tmp = tempfile::tempdir().unwrap();
    seed_runs(tmp.path());
    seed_workspace(tmp.path(), "ws_q", &tmp.path().join("proj"));
    let base = serve(state(tmp.path())).await;
    let url = format!("{base}/api/projects/ws_q/runs?q=status:failed");
    assert_eq!(
        col(&rows_of(url.clone()).await, "id"),
        ["run_6", "run_4", "run_2"]
    );
    assert_eq!(
        col(&rows_of(format!("{url}&offset=1&limit=1")).await, "id"),
        ["run_4"]
    );
    let body = rows_of(format!("{url}&facets=1")).await;
    assert_eq!(
        body["facets"]["status"],
        json!({"completed": 3, "failed": 3})
    );
}

/// Register `<global>/workspaces/<id>.toml` pointing at a fresh project dir.
fn seed_workspace(global: &Path, id: &str, project: &Path) {
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(project).unwrap();
    std::fs::write(
        dir.join(format!("{id}.toml")),
        format!(
            "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            project.display()
        ),
    )
    .unwrap();
}
