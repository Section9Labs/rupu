//! `?customer=` on the CP's lists and aggregates (customers Plan 2A,
//! Task 7): runs, workflow runs, agent runs, sessions, findings, projects,
//! usage and the dashboard. Local rows are filtered by attribution (recorded
//! customer, else the workspace's current assignment) BEFORE they are paged;
//! remote rows are filtered on the coordinator, and a peer whose rows carry
//! no `customer` key is too old to say — 501 on a single-host request, and
//! skipped + named in `X-Rupu-Hosts-Without-Customer` on a fan-out.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore, StepKind, StepResultRecord};
use rupu_workspace::{CustomerStore, NewCustomer};
use serde_json::{json, Value};

const MODEL: &str = "claude-filter-test";

// ── harness ─────────────────────────────────────────────────────────────

async fn spawn(dir: &Path) -> String {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    serve(state).await
}

async fn spawn_with_remote(dir: &Path, mock_base_url: &str) -> (String, String) {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let host = state
        .hosts
        .add_host("old-peer", mock_base_url, None)
        .expect("add_host");
    let id = host.id.clone();
    (serve(state).await, id)
}

async fn serve(state: rupu_cp::state::AppState) -> String {
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

async fn get_json(url: String) -> Value {
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {url}");
    resp.json().await.unwrap()
}

fn ids(rows: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = rows
        .as_array()
        .unwrap_or_else(|| panic!("not an array: {rows}"))
        .iter()
        .map(|r| r[key].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

fn by_id<'a>(rows: &'a Value, key: &str, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r[key] == id)
        .unwrap_or_else(|| panic!("no row {id} in {rows}"))
}

// ── seeding ─────────────────────────────────────────────────────────────

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

fn create_customer(global: &Path, slug: &str) {
    CustomerStore::new(global)
        .create(
            slug,
            &NewCustomer {
                name: slug.to_uppercase(),
                ..Default::default()
            },
        )
        .unwrap();
}

fn assign(global: &Path, slug: &str, ws: &str) {
    CustomerStore::new(global)
        .assign(slug, rupu_workspace::ProjectRef::Id(ws))
        .unwrap();
}

/// `Some(slug)` → a recorded slug; `None` → a LEGACY record (no key). A
/// recorded "no customer" (`null`) goes through the `*_rec` helpers with
/// `Some(None)`.
fn legacy_or_slug(customer: Option<&str>) -> rupu_transcript::RecordedField {
    customer.map(|c| Some(c.to_string()))
}

/// A transcript with one `Usage` event: 1M input tokens of `anthropic/MODEL`.
fn write_transcript(
    path: &Path,
    run_id: &str,
    ws: &str,
    customer: Option<&str>,
    started_at: DateTime<Utc>,
) {
    write_transcript_rec(path, run_id, ws, legacy_or_slug(customer), started_at);
}

fn write_transcript_rec(
    path: &Path,
    run_id: &str,
    ws: &str,
    customer: rupu_transcript::RecordedField,
    started_at: DateTime<Utc>,
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let start = rupu_transcript::Event::RunStart {
        run_id: run_id.into(),
        workspace_id: ws.into(),
        agent: "reviewer".into(),
        provider: "anthropic".into(),
        model: MODEL.into(),
        started_at,
        mode: rupu_transcript::RunMode::Ask,
        schema: None,
        system_prompt: None,
        codename: None,
        customer,
    };
    let usage = rupu_transcript::Event::Usage {
        provider: "anthropic".into(),
        model: MODEL.into(),
        served_model: None,
        input_tokens: 1_000_000,
        output_tokens: 0,
        cached_tokens: 0,
        cache_write_tokens: 0,
        purpose: None,
    };
    let mut buf = Vec::new();
    for ev in [&start, &usage] {
        buf.extend(serde_json::to_vec(ev).unwrap());
        buf.push(b'\n');
    }
    std::fs::write(path, &buf).unwrap();
}

fn record(
    id: &str,
    ws: &str,
    customer: rupu_transcript::RecordedField,
    started_at: DateTime<Utc>,
) -> RunRecord {
    RunRecord {
        customer,
        id: id.into(),
        workflow_name: "wf".into(),
        status: RunStatus::Completed,
        inputs: BTreeMap::new(),
        event: None,
        workspace_id: ws.into(),
        workspace_path: PathBuf::from("/tmp/proj"),
        transcript_dir: PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at,
        finished_at: Some(started_at),
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
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        gate_decisions: Vec::new(),
        codename: None,
        cause: None,
    }
}

/// A completed workflow run with one step whose transcript carries usage.
fn seed_run(global: &Path, id: &str, ws: &str, customer: Option<&str>, at: DateTime<Utc>) {
    seed_run_rec(global, id, ws, legacy_or_slug(customer), at);
}

fn seed_run_rec(
    global: &Path,
    id: &str,
    ws: &str,
    customer: rupu_transcript::RecordedField,
    at: DateTime<Utc>,
) {
    let store = RunStore::new(global.join("runs"));
    store
        .create(record(id, ws, customer.clone(), at), "name: wf\n")
        .unwrap();
    let transcript_path = global.join("tx").join(format!("{id}.jsonl"));
    write_transcript_rec(&transcript_path, id, ws, customer, at);
    store
        .append_step_result(
            id,
            &StepResultRecord {
                run_outcome: None,
                step_id: "s1".into(),
                run_id: id.into(),
                transcript_path,
                output: String::new(),
                success: true,
                skipped: false,
                rendered_prompt: String::new(),
                kind: StepKind::Linear,
                items: vec![],
                findings: vec![],
                iterations: 0,
                resolved: true,
                finished_at: Utc::now(),
                loop_iteration: None,
                host: None,
                codename: None,
                cause: None,
                error: None,
            },
        )
        .unwrap();
}

/// A standalone `rupu run`: `<global>/transcripts/<id>.{meta.json,jsonl}`.
fn seed_agent_run(global: &Path, id: &str, ws: &str, customer: Option<&str>, at: DateTime<Utc>) {
    seed_agent_run_rec(global, id, ws, legacy_or_slug(customer), at);
}

fn seed_agent_run_rec(
    global: &Path,
    id: &str,
    ws: &str,
    customer: rupu_transcript::RecordedField,
    at: DateTime<Utc>,
) {
    let dir = global.join("transcripts");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{id}.meta.json")),
        json!({ "run_id": id, "trigger_source": "run_cli" }).to_string(),
    )
    .unwrap();
    write_transcript_rec(&dir.join(format!("{id}.jsonl")), id, ws, customer, at);
}

fn seed_session(global: &Path, id: &str, ws: &str, customer: Option<&str>, at: DateTime<Utc>) {
    let dir = global.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let mut v = json!({
        "session_id": id,
        "agent_name": "reviewer",
        "workspace_id": ws,
        "created_at": at.to_rfc3339(),
        "updated_at": at.to_rfc3339(),
        "status": "idle",
    });
    if let Some(c) = customer {
        v["customer"] = json!(c);
    }
    std::fs::write(dir.join("session.json"), v.to_string()).unwrap();
}

/// One finding in a project's coverage ledger.
fn seed_finding(project: &Path, id: &str) {
    let dir = project.join(".rupu").join("coverage").join("t1");
    std::fs::create_dir_all(&dir).unwrap();
    let finding = format!(
        "{{\"id\":\"{id}\",\"file_path\":\"src/a.rs\",\"line_range\":[1,5],\
\"scope\":\"line\",\"summary\":\"thing\",\"severity\":\"high\",\
\"concern_id\":\"ssrf\",\
\"evidence\":{{\"rationale\":\"because\"}},\
\"declared_by\":{{\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\"}},\
\"declared_at\":\"2026-06-19T00:00:00Z\"}}\n"
    );
    std::fs::write(dir.join("findings.jsonl"), finding).unwrap();
}

/// Two customers + unassigned work:
/// - `ws_acme` assigned to acme, `ws_globex` to globex, `ws_none` to nobody;
/// - `r_acme` recorded acme (in `ws_none`: the RECORDED slug wins),
///   `r_acme_legacy` recorded nothing in `ws_acme` (derived acme),
///   `r_globex` recorded globex, `r_none` nothing in `ws_none`.
struct Fleet {
    _tmp: tempfile::TempDir,
    _proj: tempfile::TempDir,
    global: PathBuf,
}

fn seed_fleet() -> Fleet {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path().to_path_buf();
    for ws in ["ws_acme", "ws_globex", "ws_none"] {
        seed_workspace(&global, ws, &proj.path().join(ws));
    }
    create_customer(&global, "acme");
    create_customer(&global, "globex");
    assign(&global, "acme", "ws_acme");
    assign(&global, "globex", "ws_globex");

    let now = Utc::now();
    seed_run(
        &global,
        "r_acme",
        "ws_none",
        Some("acme"),
        now - Duration::hours(1),
    );
    seed_run(
        &global,
        "r_acme_legacy",
        "ws_acme",
        None,
        now - Duration::hours(2),
    );
    seed_run(
        &global,
        "r_globex",
        "ws_globex",
        Some("globex"),
        now - Duration::hours(3),
    );
    seed_run(&global, "r_none", "ws_none", None, now - Duration::hours(4));

    seed_agent_run(
        &global,
        "a_acme",
        "ws_none",
        Some("acme"),
        now - Duration::hours(1),
    );
    seed_agent_run(
        &global,
        "a_acme_legacy",
        "ws_acme",
        None,
        now - Duration::hours(2),
    );
    seed_agent_run(&global, "a_none", "ws_none", None, now - Duration::hours(3));

    seed_session(
        &global,
        "s_acme",
        "ws_none",
        Some("acme"),
        now - Duration::hours(1),
    );
    seed_session(
        &global,
        "s_acme_legacy",
        "ws_acme",
        None,
        now - Duration::hours(2),
    );
    seed_session(
        &global,
        "s_globex",
        "ws_globex",
        Some("globex"),
        now - Duration::hours(3),
    );

    seed_finding(&proj.path().join("ws_acme"), "f_acme");
    seed_finding(&proj.path().join("ws_globex"), "f_globex");

    Fleet {
        _tmp: tmp,
        _proj: proj,
        global,
    }
}

// ── 1. runs ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn runs_filter_by_recorded_or_derived_customer() {
    let f = seed_fleet();
    let base = spawn(&f.global).await;

    for path in [
        "/api/runs?host=local",
        "/api/runs",
        "/api/runs/workflows?host=local",
    ] {
        let sep = if path.contains('?') { '&' } else { '?' };
        let rows = get_json(format!("{base}{path}{sep}customer=acme")).await;
        assert_eq!(ids(&rows, "id"), ["r_acme", "r_acme_legacy"], "{path}");
        let rec = by_id(&rows, "id", "r_acme");
        assert_eq!(rec["customer"], "acme");
        assert_eq!(rec["customer_derived"], false);
        let legacy = by_id(&rows, "id", "r_acme_legacy");
        assert_eq!(legacy["customer"], "acme");
        assert_eq!(legacy["customer_derived"], true);

        let rows = get_json(format!("{base}{path}{sep}customer=none")).await;
        assert_eq!(ids(&rows, "id"), ["r_none"], "{path}");
        assert!(rows[0].as_object().unwrap().contains_key("customer"));
        assert!(rows[0]["customer"].is_null());
        assert_eq!(rows[0]["customer_derived"], false);
    }

    // No filter: every run, each row carrying its attribution.
    let rows = get_json(format!("{base}/api/runs?host=local")).await;
    assert_eq!(rows.as_array().unwrap().len(), 4);
    assert_eq!(by_id(&rows, "id", "r_globex")["customer"], "globex");
    assert_eq!(
        by_id(&rows, "id", "r_acme_legacy")["customer_derived"],
        true
    );
}

// ── 2. filter before paging ─────────────────────────────────────────────

#[tokio::test]
async fn the_filter_applies_before_pagination() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_x", &proj.path().join("x"));
    create_customer(global, "acme");
    let now = Utc::now();
    for i in 0..30 {
        // The oldest three (i = 27..30) are Acme's.
        let customer = (i >= 27).then_some("acme");
        seed_run(
            global,
            &format!("run_{i:02}"),
            "ws_x",
            customer,
            now - Duration::minutes(i),
        );
    }
    let base = spawn(global).await;
    for path in [
        "/api/runs?host=local",
        "/api/runs/workflows?host=local",
        "/api/runs",
    ] {
        let sep = if path.contains('?') { '&' } else { '?' };
        let rows = get_json(format!("{base}{path}{sep}customer=acme&limit=10&offset=0")).await;
        assert_eq!(ids(&rows, "id"), ["run_27", "run_28", "run_29"], "{path}");
    }
}

// ── 3. the other lists and aggregates ───────────────────────────────────

#[tokio::test]
async fn agent_runs_and_sessions_filter_by_customer() {
    let f = seed_fleet();
    let base = spawn(&f.global).await;

    for path in ["/api/runs/agents?host=local&", "/api/runs/agents?"] {
        let rows = get_json(format!("{base}{path}customer=acme")).await;
        assert_eq!(ids(&rows, "run_id"), ["a_acme", "a_acme_legacy"], "{path}");
        assert_eq!(by_id(&rows, "run_id", "a_acme")["customer_derived"], false);
        let legacy = by_id(&rows, "run_id", "a_acme_legacy");
        assert_eq!(legacy["customer"], "acme");
        assert_eq!(legacy["customer_derived"], true);
        let rows = get_json(format!("{base}{path}customer=none")).await;
        assert_eq!(ids(&rows, "run_id"), ["a_none"], "{path}");
    }

    for path in ["/api/sessions?host=local&", "/api/sessions?"] {
        let rows = get_json(format!("{base}{path}customer=acme")).await;
        assert_eq!(
            ids(&rows, "session_id"),
            ["s_acme", "s_acme_legacy"],
            "{path}"
        );
        let legacy = by_id(&rows, "session_id", "s_acme_legacy");
        assert_eq!(legacy["customer"], "acme");
        assert_eq!(legacy["customer_derived"], true);
        let rows = get_json(format!("{base}{path}customer=globex")).await;
        assert_eq!(ids(&rows, "session_id"), ["s_globex"], "{path}");
    }
}

#[tokio::test]
async fn projects_and_findings_filter_by_current_assignment() {
    let f = seed_fleet();
    let base = spawn(&f.global).await;

    let rows = get_json(format!("{base}/api/projects?customer=acme")).await;
    assert_eq!(ids(&rows, "ws_id"), ["ws_acme"]);
    let rows = get_json(format!("{base}/api/projects?customer=none")).await;
    assert_eq!(ids(&rows, "ws_id"), ["ws_none"]);

    let body = get_json(format!("{base}/api/findings?customer=acme")).await;
    assert_eq!(ids(&body["findings"], "id"), ["f_acme"]);
    assert_eq!(body["findings"][0]["customer"], "acme");
    assert_eq!(body["summary"]["total"], 1);
    // Unfiltered rows carry the customer too (null when none).
    let body = get_json(format!("{base}/api/findings")).await;
    assert_eq!(body["summary"]["total"], 2);
    assert_eq!(
        by_id(&body["findings"], "id", "f_globex")["customer"],
        "globex"
    );
}

#[tokio::test]
async fn usage_counts_only_the_customers_spend() {
    let f = seed_fleet();
    let base = spawn(&f.global).await;

    // Acme: two workflow runs + two standalone agent runs, 1M input each.
    let body = get_json(format!("{base}/api/usage?customer=acme")).await;
    assert_eq!(body["summary"]["input_tokens"], 4_000_000, "{body}");
    let body = get_json(format!("{base}/api/usage?customer=globex")).await;
    assert_eq!(body["summary"]["input_tokens"], 1_000_000, "{body}");
    let body = get_json(format!("{base}/api/usage?customer=none")).await;
    assert_eq!(body["summary"]["input_tokens"], 2_000_000, "{body}");
    let body = get_json(format!("{base}/api/usage")).await;
    assert_eq!(body["summary"]["input_tokens"], 7_000_000, "{body}");

    let rows = get_json(format!("{base}/api/usage/runs?customer=acme")).await;
    assert_eq!(
        ids(&rows, "run_id"),
        ["a_acme", "a_acme_legacy", "r_acme", "r_acme_legacy"]
    );

    let buckets = get_json(format!("{base}/api/usage/timeline?customer=globex")).await;
    let total: u64 = buckets
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["rows"].as_array().unwrap().iter())
        .map(|r| r["input_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 1_000_000);

    let resp = reqwest::get(format!("{base}/api/usage/outliers?customer=acme"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn usage_prices_each_source_with_its_customers_pricing() {
    let f = seed_fleet();
    // Global prices MODEL at $1/Mtok input; Acme's layer at $5.
    let toml = |p: f64| {
        format!("[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {p}\noutput_per_mtok = 1.0\n")
    };
    std::fs::write(f.global.join("config.toml"), toml(1.0)).unwrap();
    let acme_cfg = CustomerStore::new(&f.global).config_path("acme");
    std::fs::write(acme_cfg, toml(5.0)).unwrap();
    let base = spawn(&f.global).await;

    let body = get_json(format!("{base}/api/usage?customer=acme")).await;
    let cost = body["summary"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 20.0).abs() < 1e-9, "4M at $5 = $20, got {cost}");
    // Unfiltered: Acme at $5 (4M) + globex and none at $1 (3M).
    let body = get_json(format!("{base}/api/usage")).await;
    let cost = body["summary"]["cost_usd"].as_f64().unwrap();
    assert!((cost - 23.0).abs() < 1e-9, "got {cost}");
}

#[tokio::test]
async fn dashboard_counts_only_the_customers_runs() {
    let f = seed_fleet();
    let base = spawn(&f.global).await;

    let manual = |body: &Value| -> u64 {
        body["throughput_buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["manual"].as_u64().unwrap())
            .sum()
    };
    let body = get_json(format!("{base}/api/dashboard?range=all&customer=acme")).await;
    assert_eq!(manual(&body), 2, "{body}");
    assert_eq!(body["findings_open"], 1);
    let body = get_json(format!(
        "{base}/api/dashboard?range=all&host=local&customer=none"
    ))
    .await;
    assert_eq!(manual(&body), 1, "{body}");
    assert_eq!(body["findings_open"], 0);
    let body = get_json(format!("{base}/api/dashboard?range=all")).await;
    assert_eq!(manual(&body), 4, "{body}");
    assert_eq!(body["findings_open"], 2);
}

// ── 4. bad slug ─────────────────────────────────────────────────────────

#[tokio::test]
async fn a_bad_slug_is_a_400_everywhere() {
    let tmp = tempfile::tempdir().unwrap();
    let base = spawn(tmp.path()).await;
    for path in [
        "/api/runs",
        "/api/runs/workflows",
        "/api/runs/agents",
        "/api/sessions",
        "/api/findings",
        "/api/projects",
        "/api/usage",
        "/api/usage/timeline",
        "/api/usage/runs",
        "/api/usage/outliers",
        "/api/dashboard",
    ] {
        let resp = reqwest::get(format!("{base}{path}?customer=Bad%20Slug"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{path}");
    }
}

// ── 5. remote hosts ─────────────────────────────────────────────────────

fn remote_run(id: &str, customer: Option<Option<&str>>) -> Value {
    let mut v = json!({
        "id": id,
        "workflow_name": "wf",
        "status": "completed",
        "started_at": "2026-10-01T00:00:00Z",
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
async fn an_old_peer_cannot_be_filtered_by_customer() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs");
        then.status(200)
            .json_body(json!([remote_run("rr_1", None), remote_run("rr_2", None)]));
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
    let (base, host) = spawn_with_remote(tmp.path(), &mock.base_url()).await;

    // Single host: 501, naming the host.
    let resp = reqwest::get(format!("{base}/api/runs?host={host}&customer=acme"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"],
        format!("host {host} can't report a customer for every run")
    );

    // Without a filter the old peer still lists.
    let rows = get_json(format!("{base}/api/runs?host={host}")).await;
    assert_eq!(rows.as_array().unwrap().len(), 2);

    // Fan-out: the old peer is skipped and named in the header.
    let resp = reqwest::get(format!("{base}/api/runs?customer=acme"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("x-rupu-hosts-without-customer")
            .map(|v| v.to_str().unwrap().to_string()),
        Some(host.clone())
    );
    let rows: Value = resp.json().await.unwrap();
    assert!(rows.as_array().unwrap().is_empty(), "{rows}");

    // The agent-run and session lists follow the same rule.
    for path in ["/api/runs/agents", "/api/sessions"] {
        let resp = reqwest::get(format!("{base}{path}?host={host}&customer=acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
        let resp = reqwest::get(format!("{base}{path}?customer=acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        assert_eq!(
            resp.headers()
                .get("x-rupu-hosts-without-customer")
                .and_then(|v| v.to_str().ok()),
            Some(host.as_str()),
            "{path}"
        );
    }

    // Aggregates are not filtered remotely: 501.
    for path in ["/api/usage", "/api/dashboard"] {
        let resp = reqwest::get(format!("{base}{path}?host={host}&customer=acme"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED, "{path}");
    }
}

/// A peer whose listing could not attribute one row (its CLI omitted that
/// row's customer keys) can't be filtered either — never counted as "none".
#[tokio::test]
async fn a_peer_row_without_a_customer_makes_the_host_unfilterable() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs");
        then.status(200).json_body(json!([
            remote_run("rr_acme", Some(Some("acme"))),
            remote_run("rr_unattributed", None),
        ]));
    });
    let tmp = tempfile::tempdir().unwrap();
    let (base, host) = spawn_with_remote(tmp.path(), &mock.base_url()).await;
    for customer in ["acme", "none"] {
        let resp = reqwest::get(format!("{base}/api/runs?host={host}&customer={customer}"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED, "{customer}");
        let body: Value = resp.json().await.unwrap();
        assert_eq!(
            body["error"],
            format!("host {host} can't report a customer for every run")
        );
    }
}

#[tokio::test]
async fn a_new_peers_rows_are_filtered_on_the_coordinator() {
    let mock = httpmock::MockServer::start();
    mock.mock(|when, then| {
        when.method("GET").path("/api/runs");
        then.status(200).json_body(json!([
            remote_run("rr_acme", Some(Some("acme"))),
            remote_run("rr_none", Some(None)),
            remote_run("rr_globex", Some(Some("globex"))),
        ]));
    });
    let tmp = tempfile::tempdir().unwrap();
    let (base, host) = spawn_with_remote(tmp.path(), &mock.base_url()).await;

    let rows = get_json(format!("{base}/api/runs?host={host}&customer=acme")).await;
    assert_eq!(ids(&rows, "id"), ["rr_acme"]);
    let rows = get_json(format!("{base}/api/runs?host={host}&customer=none")).await;
    assert_eq!(ids(&rows, "id"), ["rr_none"]);

    let resp = reqwest::get(format!("{base}/api/runs?customer=globex"))
        .await
        .unwrap();
    assert!(resp
        .headers()
        .get("x-rupu-hosts-without-customer")
        .is_none());
    let rows: Value = resp.json().await.unwrap();
    assert_eq!(ids(&rows, "id"), ["rr_globex"]);
}

// ── fail closed ─────────────────────────────────────────────────────────

/// Under a customer filter, and for every count / rollup / price, an
/// assignment that cannot be read fails the request (500 naming the
/// workspace) — it never reads as "no customer".
#[tokio::test]
async fn an_unreadable_assignment_fails_the_request() {
    let f = seed_fleet();
    // Replace ws_acme's sidecar with a directory: present, but unreadable.
    let sidecar = f.global.join("workspaces").join("ws_acme.customer");
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::create_dir_all(&sidecar).unwrap();
    let base = spawn(&f.global).await;
    for path in [
        "/api/runs?host=local&customer=none",
        "/api/runs?customer=acme",
        "/api/runs/workflows?customer=none",
        "/api/runs/agents?host=local&customer=none",
        "/api/sessions?host=local&customer=none",
        "/api/findings?customer=none",
        "/api/usage/runs?customer=none",
        "/api/dashboard?host=local&customer=none",
        // Counts, rollups and prices fail closed unfiltered too. (`/api/usage`
        // marks the local host offline with the reason instead — its
        // per-host contract.)
        "/api/customers",
        "/api/usage/runs",
        "/api/projects",
    ] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR, "{path}");
        let body: Value = resp.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("ws_acme"),
            "{path}: {body}"
        );
    }
}

/// An UNFILTERED run / agent-run / session list does not fail on one
/// unreadable assignment: the rows whose customer can't be known are listed
/// WITHOUT the `customer` / `customer_derived` keys (never as "no
/// customer"); every other row keeps them.
#[tokio::test]
async fn an_unreadable_assignment_degrades_an_unfiltered_list() {
    let f = seed_fleet();
    let sidecar = f.global.join("workspaces").join("ws_acme.customer");
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::create_dir_all(&sidecar).unwrap();
    let base = spawn(&f.global).await;
    let has_keys = |row: &Value| {
        let o = row.as_object().unwrap();
        o.contains_key("customer") || o.contains_key("customer_derived")
    };
    for (path, key, broken, ok) in [
        ("/api/runs?host=local", "id", "r_acme_legacy", "r_acme"),
        ("/api/runs", "id", "r_acme_legacy", "r_acme"),
        ("/api/runs/workflows", "id", "r_acme_legacy", "r_acme"),
        (
            "/api/runs/agents?host=local",
            "run_id",
            "a_acme_legacy",
            "a_acme",
        ),
        ("/api/runs/agents", "run_id", "a_acme_legacy", "a_acme"),
        (
            "/api/sessions?host=local",
            "session_id",
            "s_acme_legacy",
            "s_acme",
        ),
        ("/api/sessions", "session_id", "s_acme_legacy", "s_acme"),
    ] {
        let resp = reqwest::get(format!("{base}{path}")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{path}");
        let rows: Value = resp.json().await.unwrap();
        assert!(!has_keys(by_id(&rows, key, broken)), "{path}: {rows}");
        // Priced at the global rates — and saying so (R3).
        assert_eq!(
            by_id(&rows, key, broken)["usage"]["pricing_error"],
            "customer unknown; priced at global rates",
            "{path}: {rows}"
        );
        assert_eq!(by_id(&rows, key, ok)["customer"], "acme", "{path}");
        assert!(
            by_id(&rows, key, ok)["usage"]
                .get("pricing_error")
                .is_none(),
            "{path}"
        );
    }
    // The session detail degrades the same way (R2).
    let detail = get_json(format!("{base}/api/sessions/s_acme_legacy")).await;
    assert!(!has_keys(&detail), "{detail}");
    let detail = get_json(format!("{base}/api/sessions/s_acme")).await;
    assert_eq!(detail["customer"], "acme");
}

/// `GET /api/runs/:id` and `/graph` carry the run's ATTRIBUTION on `run`, as
/// the list rows do: a recorded slug (not derived), a recorded none (`null`),
/// a legacy run in an assigned project (derived), and — for an unreadable
/// assignment — neither key (never "no customer").
#[tokio::test]
async fn a_runs_detail_and_graph_carry_its_attribution() {
    let f = seed_fleet();
    // A snapshot the graph can parse (the shared seed's is a bare name).
    let store = RunStore::new(f.global.join("runs"));
    for id in ["r_acme", "r_acme_legacy", "r_globex", "r_none"] {
        std::fs::write(
            store.workflow_snapshot_path(id),
            "name: wf\nsteps:\n  - id: s1\n    agent: reviewer\n    prompt: hi\n",
        )
        .unwrap();
    }
    let base = spawn(&f.global).await;
    for (id, customer, derived) in [
        ("r_acme", json!("acme"), false),
        ("r_globex", json!("globex"), false),
        ("r_acme_legacy", json!("acme"), true),
        ("r_none", Value::Null, false),
    ] {
        let detail = get_json(format!("{base}/api/runs/{id}")).await;
        let graph = get_json(format!("{base}/api/runs/{id}/graph")).await;
        for (what, v) in [("detail", &detail), ("graph", &graph)] {
            let run = v["run"].as_object().unwrap();
            assert_eq!(run["customer"], customer, "{id} {what}");
            assert_eq!(run["customer_derived"], derived, "{id} {what}");
        }
    }

    // An unreadable assignment: the legacy run says nothing (both keys left
    // out); a run that recorded its customer is unaffected.
    let sidecar = f.global.join("workspaces").join("ws_acme.customer");
    std::fs::remove_file(&sidecar).unwrap();
    std::fs::create_dir_all(&sidecar).unwrap();
    for path in ["/api/runs/r_acme_legacy", "/api/runs/r_acme_legacy/graph"] {
        let v = get_json(format!("{base}{path}")).await;
        let run = v["run"].as_object().unwrap();
        assert!(!run.contains_key("customer"), "{path}: {v}");
        assert!(!run.contains_key("customer_derived"), "{path}: {v}");
    }
    let v = get_json(format!("{base}/api/runs/r_acme")).await;
    assert_eq!(v["run"]["customer"], "acme");
}

// ── residual fixes ──────────────────────────────────────────────────────

/// `GET /api/sessions/:id` carries `customer` / `customer_derived`, attributed
/// exactly as the list does: recorded slug, recorded none (stays none in an
/// assigned project), legacy (derived from the current assignment).
#[tokio::test]
async fn session_detail_carries_its_customer() {
    let f = seed_fleet();
    let at = Utc::now() - Duration::hours(5);
    let dir = f.global.join("sessions").join("s_none_recorded");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("session.json"),
        json!({
            "session_id": "s_none_recorded",
            "agent_name": "reviewer",
            "workspace_id": "ws_acme",
            "customer": null,
            "created_at": at.to_rfc3339(),
            "updated_at": at.to_rfc3339(),
            "status": "idle",
        })
        .to_string(),
    )
    .unwrap();
    let base = spawn(&f.global).await;
    let keys = |v: &Value| (v["customer"].clone(), v["customer_derived"].clone());
    let detail = get_json(format!("{base}/api/sessions/s_acme")).await;
    assert_eq!(keys(&detail), (json!("acme"), json!(false)));
    let detail = get_json(format!("{base}/api/sessions/s_none_recorded")).await;
    assert!(
        detail.as_object().unwrap().contains_key("customer"),
        "{detail}"
    );
    assert_eq!(keys(&detail), (json!(null), json!(false)));
    let detail = get_json(format!("{base}/api/sessions/s_acme_legacy")).await;
    assert_eq!(keys(&detail), (json!("acme"), json!(true)));
}

/// A mirrored worker run (its `run.json` carries `worker_id`) is never
/// attributed through the coordinator's assignments: one that recorded a
/// customer counts as that; a LEGACY one — even in a project the coordinator
/// has assigned — is not counted under any customer nor under "none": it is
/// left out of filters and rollups with its worker host named, and listed
/// unfiltered without customer keys (priced at the global rates, flagged).
#[tokio::test]
async fn mirrored_legacy_runs_are_never_attributed_through_the_coordinator() {
    let f = seed_fleet();
    let store = RunStore::new(f.global.join("runs"));
    let at = Utc::now() - Duration::minutes(30);
    for (id, customer) in [
        ("m_legacy", None),
        ("m_acme", Some(Some("acme".to_string()))),
    ] {
        let mut rec = record(id, "ws_acme", customer.clone(), at);
        rec.worker_id = Some("node_1".into());
        store.create(rec, "name: wf\n").unwrap();
    }
    let base = spawn(&f.global).await;
    let header = |resp: &reqwest::Response| {
        resp.headers()
            .get("x-rupu-hosts-without-customer")
            .map(|v| v.to_str().unwrap().to_string())
    };

    for path in [
        "/api/runs?host=local&",
        "/api/runs?",
        "/api/runs/workflows?host=local&",
    ] {
        for filter in ["none", "acme"] {
            let resp = reqwest::get(format!("{base}{path}customer={filter}"))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{path}{filter}");
            assert_eq!(header(&resp).as_deref(), Some("node_1"), "{path}{filter}");
            let rows: Value = resp.json().await.unwrap();
            let got = ids(&rows, "id");
            assert!(
                !got.contains(&"m_legacy".to_string()),
                "{path}{filter}: {got:?}"
            );
            assert_eq!(
                got.contains(&"m_acme".to_string()),
                filter == "acme",
                "{path}{filter}"
            );
        }
    }
    // Unfiltered: listed, keyless, priced unknown.
    let rows = get_json(format!("{base}/api/runs?host=local")).await;
    let legacy = by_id(&rows, "id", "m_legacy");
    assert!(
        !legacy.as_object().unwrap().contains_key("customer"),
        "{legacy}"
    );
    assert_eq!(
        legacy["usage"]["pricing_error"],
        "customer unknown; priced at global rates"
    );
    assert_eq!(by_id(&rows, "id", "m_acme")["customer"], "acme");

    // Rollups: acme counts its own recorded runs + the mirrored acme one.
    let rows = get_json(format!("{base}/api/customers")).await;
    let acme = by_id(&rows, "slug", "acme");
    assert_eq!(acme["rollup"]["run_count"], 3, "{acme}");
    assert_eq!(acme["rollup"]["hosts_without_customer"], json!(["node_1"]));
    // Usage and dashboard under a filter name the host, never count it.
    let usage = get_json(format!("{base}/api/usage?customer=none")).await;
    assert_eq!(
        usage["hosts_without_customer"],
        json!(["node_1"]),
        "{usage}"
    );
    assert_eq!(usage["summary"]["input_tokens"], 2_000_000, "{usage}");
    let resp = reqwest::get(format!("{base}/api/usage/runs?customer=none"))
        .await
        .unwrap();
    assert_eq!(header(&resp).as_deref(), Some("node_1"));
    let dash = get_json(format!("{base}/api/dashboard?host=local&customer=none")).await;
    assert_eq!(dash["hosts_without_customer"], json!(["node_1"]), "{dash}");
}

/// A run's detail, live usage and graph cost what its list row costs: all
/// priced at its attributed customer's pricing (spec §1).
#[tokio::test]
async fn a_runs_detail_costs_what_its_row_costs() {
    let f = seed_fleet();
    let toml = |p: f64| {
        format!("[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {p}\noutput_per_mtok = 1.0\n")
    };
    std::fs::write(f.global.join("config.toml"), toml(1.0)).unwrap();
    std::fs::write(CustomerStore::new(&f.global).config_path("acme"), toml(5.0)).unwrap();
    let base = spawn(&f.global).await;
    let rows = get_json(format!("{base}/api/runs?host=local")).await;
    for (id, want) in [("r_acme", 5.0), ("r_acme_legacy", 5.0), ("r_none", 1.0)] {
        let row_cost = by_id(&rows, "id", id)["usage"]["cost_usd"]
            .as_f64()
            .unwrap();
        assert!((row_cost - want).abs() < 1e-9, "{id} row: {row_cost}");
        let detail = get_json(format!("{base}/api/runs/{id}")).await;
        assert_eq!(
            detail["usage"]["cost_usd"].as_f64().unwrap(),
            row_cost,
            "{id} detail"
        );
        let live = get_json(format!("{base}/api/runs/{id}/usage")).await;
        assert_eq!(
            live["summary"]["cost_usd"].as_f64().unwrap(),
            row_cost,
            "{id} usage"
        );
    }
}

/// The session LIST prices each turn at that turn's own attribution, not
/// the session's latest customer: a turn that recorded none at the global
/// $1, a turn that recorded acme at Acme's $5 — $6, the same the rollups and
/// the session detail show.
#[tokio::test]
async fn the_session_list_prices_each_turn_at_its_own_customer() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_p", &proj.path().join("ws_p"));
    create_customer(global, "acme");
    assign(global, "acme", "ws_p");
    let toml = |p: f64| {
        format!("[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {p}\noutput_per_mtok = 1.0\n")
    };
    std::fs::write(global.join("config.toml"), toml(1.0)).unwrap();
    std::fs::write(CustomerStore::new(global).config_path("acme"), toml(5.0)).unwrap();
    let at = Utc::now() - Duration::hours(2);
    let tdir = global.join("sess_tx");
    let mut runs = Vec::new();
    for (id, field) in [("t1", Some(None)), ("t2", Some(Some("acme".to_string())))] {
        let path = tdir.join(format!("{id}.jsonl"));
        write_transcript_rec(&path, id, "ws_p", field, at);
        runs.push(
            json!({"run_id": id, "transcript_path": path.to_string_lossy(),
                         "started_at": at.to_rfc3339(), "status": "ok"}),
        );
    }
    let sdir = global.join("sessions").join("s_1");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("session.json"),
        json!({
            "session_id": "s_1", "agent_name": "reviewer", "provider_name": "anthropic",
            "model": MODEL, "workspace_id": "ws_p", "customer": "acme",
            "created_at": at.to_rfc3339(), "updated_at": at.to_rfc3339(),
            "status": "idle", "runs": runs,
        })
        .to_string(),
    )
    .unwrap();
    let base = spawn(global).await;
    let rows = get_json(format!("{base}/api/sessions?host=local")).await;
    let cost = by_id(&rows, "session_id", "s_1")["usage"]["cost_usd"]
        .as_f64()
        .unwrap();
    assert!((cost - 6.0).abs() < 1e-9, "$1 + $5, got {cost}");
    let detail = get_json(format!("{base}/api/sessions/s_1")).await;
    assert_eq!(detail["usage"]["cost_usd"].as_f64().unwrap(), cost);
}

// ── tri-state attribution ───────────────────────────────────────────────

/// A run that RECORDED no customer (`"customer": null`) stays "none" after
/// its project is assigned: not derived, not in the new customer's filter or
/// rollup, and included in `?customer=none`. A legacy run (no key) in the
/// same project still derives the assignment.
#[tokio::test]
async fn a_recorded_none_stays_none_after_the_project_is_assigned() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_p", &proj.path().join("ws_p"));
    create_customer(global, "acme");
    let at = Utc::now() - Duration::hours(1);
    // Launched while the project was unassigned: recorded none.
    seed_run_rec(global, "r_none", "ws_p", Some(None), at);
    seed_agent_run_rec(global, "a_none", "ws_p", Some(None), at);
    // A run that predates customers (no key).
    seed_run(global, "r_legacy", "ws_p", None, at - Duration::hours(1));
    seed_agent_run(global, "a_legacy", "ws_p", None, at - Duration::hours(1));
    // Now the project is assigned.
    assign(global, "acme", "ws_p");
    let base = spawn(global).await;

    for (list, key, none, legacy) in [
        ("/api/runs?host=local&", "id", "r_none", "r_legacy"),
        ("/api/runs?", "id", "r_none", "r_legacy"),
        (
            "/api/runs/agents?host=local&",
            "run_id",
            "a_none",
            "a_legacy",
        ),
    ] {
        let rows = get_json(format!("{base}{list}")).await;
        let row = by_id(&rows, key, none);
        assert!(row["customer"].is_null(), "{list}: {row}");
        assert_eq!(row["customer_derived"], false, "{list}");
        let row = by_id(&rows, key, legacy);
        assert_eq!(row["customer"], "acme", "{list}");
        assert_eq!(row["customer_derived"], true, "{list}");

        let rows = get_json(format!("{base}{list}customer=acme")).await;
        assert_eq!(ids(&rows, key), [legacy], "{list}");
        let rows = get_json(format!("{base}{list}customer=none")).await;
        assert_eq!(ids(&rows, key), [none], "{list}");
    }

    // Acme's rollup counts only the legacy (derived) work.
    let rows = get_json(format!("{base}/api/customers")).await;
    let acme = by_id(&rows, "slug", "acme");
    assert_eq!(acme["rollup"]["run_count"], 1, "{acme}");
    // Usage: acme's spend is the legacy run + the legacy agent run only.
    let usage = get_json(format!("{base}/api/usage?customer=acme")).await;
    assert_eq!(usage["summary"]["input_tokens"], 2_000_000, "{usage}");
    let usage = get_json(format!("{base}/api/usage?customer=none")).await;
    assert_eq!(usage["summary"]["input_tokens"], 2_000_000, "{usage}");
}

/// Session turns are attributed per turn: turn 1 ran while the project was
/// unassigned (its transcript recorded `null`), turn 2 after it was assigned
/// to Acme (recorded `acme`; the session record now says acme). Turn 1 stays
/// none — the session's latest customer never re-attributes it — and turn 2
/// is Acme.
#[tokio::test]
async fn a_session_turn_that_recorded_none_is_not_reattributed() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_p", &proj.path().join("ws_p"));
    create_customer(global, "acme");
    assign(global, "acme", "ws_p");
    let at = Utc::now() - Duration::hours(2);
    let tdir = global.join("transcripts");
    let turns = [
        ("turn_1", Some(None), at),
        (
            "turn_2",
            Some(Some("acme".to_string())),
            at + Duration::hours(1),
        ),
    ];
    let mut runs = Vec::new();
    for (id, field, t) in &turns {
        let path = tdir.join(format!("{id}.jsonl"));
        write_transcript_rec(&path, id, "ws_p", field.clone(), *t);
        std::fs::write(
            tdir.join(format!("{id}.meta.json")),
            json!({"run_id": id, "session_id": "s_1", "trigger_source": "session_turn"})
                .to_string(),
        )
        .unwrap();
        runs.push(json!({
            "run_id": id,
            "transcript_path": path.to_string_lossy(),
            "started_at": t.to_rfc3339(),
            "status": "ok",
        }));
    }
    let sdir = global.join("sessions").join("s_1");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("session.json"),
        json!({
            "session_id": "s_1",
            "agent_name": "reviewer",
            "workspace_id": "ws_p",
            "customer": "acme",
            "created_at": at.to_rfc3339(),
            "updated_at": at.to_rfc3339(),
            "status": "idle",
            "runs": runs,
        })
        .to_string(),
    )
    .unwrap();
    let base = spawn(global).await;

    let rows = get_json(format!("{base}/api/runs/agents?host=local")).await;
    let t1 = by_id(&rows, "run_id", "turn_1");
    assert!(t1["customer"].is_null(), "{t1}");
    assert_eq!(t1["customer_derived"], false);
    let t2 = by_id(&rows, "run_id", "turn_2");
    assert_eq!(t2["customer"], "acme");
    assert_eq!(t2["customer_derived"], false);
    let rows = get_json(format!("{base}/api/runs/agents?host=local&customer=none")).await;
    assert_eq!(ids(&rows, "run_id"), ["turn_1"]);
    let rows = get_json(format!("{base}/api/runs/agents?host=local&customer=acme")).await;
    assert_eq!(ids(&rows, "run_id"), ["turn_2"]);
    // The usage aggregates attribute the same way.
    let usage = get_json(format!("{base}/api/usage?customer=none")).await;
    assert_eq!(usage["summary"]["input_tokens"], 1_000_000, "{usage}");
    let usage = get_json(format!("{base}/api/usage?customer=acme")).await;
    assert_eq!(usage["summary"]["input_tokens"], 1_000_000, "{usage}");
}

// ── session turns ───────────────────────────────────────────────────────

/// A session turn is priced with its customer's pricing on BOTH the session
/// list and the agent-run list; a turn whose own transcript recorded no
/// customer inherits the session's and says so (`customer_derived`).
#[tokio::test]
async fn a_session_turn_prices_and_attributes_alike_on_both_lists() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_none", &proj.path().join("ws_none"));
    create_customer(global, "acme");
    let toml = |p: f64| {
        format!("[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {p}\noutput_per_mtok = 1.0\n")
    };
    std::fs::write(global.join("config.toml"), toml(1.0)).unwrap();
    std::fs::write(CustomerStore::new(global).config_path("acme"), toml(5.0)).unwrap();

    // The turn's transcript predates customers; the session records Acme.
    let at = Utc::now() - Duration::hours(1);
    let tdir = global.join("transcripts");
    std::fs::create_dir_all(&tdir).unwrap();
    let tpath = tdir.join("turn_1.jsonl");
    write_transcript(&tpath, "turn_1", "ws_none", None, at);
    std::fs::write(
        tdir.join("turn_1.meta.json"),
        json!({"run_id": "turn_1", "session_id": "s_1", "trigger_source": "session_turn"})
            .to_string(),
    )
    .unwrap();
    let sdir = global.join("sessions").join("s_1");
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("session.json"),
        json!({
            "session_id": "s_1",
            "agent_name": "reviewer",
            "provider_name": "anthropic",
            "model": MODEL,
            "workspace_id": "ws_none",
            "customer": "acme",
            "created_at": at.to_rfc3339(),
            "updated_at": at.to_rfc3339(),
            "status": "idle",
            "runs": [{
                "run_id": "turn_1",
                "transcript_path": tpath.to_string_lossy(),
                "started_at": at.to_rfc3339(),
                "status": "ok",
            }],
        })
        .to_string(),
    )
    .unwrap();
    let base = spawn(global).await;

    let sessions = get_json(format!("{base}/api/sessions?host=local&customer=acme")).await;
    let session_cost = sessions[0]["usage"]["cost_usd"].as_f64().unwrap();
    let agents = get_json(format!("{base}/api/runs/agents?host=local&customer=acme")).await;
    let turn = by_id(&agents, "run_id", "turn_1");
    assert_eq!(turn["customer"], "acme");
    assert_eq!(turn["customer_derived"], true, "inherited from the session");
    let turn_cost = turn["usage"]["cost_usd"].as_f64().unwrap();
    assert!(
        (session_cost - 5.0).abs() < 1e-9,
        "session at Acme's $5: {session_cost}"
    );
    assert!(
        (turn_cost - session_cost).abs() < 1e-9,
        "{turn_cost} vs {session_cost}"
    );
}

/// A tunnel-mirrored run that recorded `acme` costs the same through the
/// tunnel host's list, detail, live usage and graph as everywhere else: the
/// coordinator's acme pricing (M1) — never the flat global rates.
#[tokio::test]
async fn a_mirrored_runs_cost_is_the_same_on_every_host_surface() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let global = tmp.path();
    seed_workspace(global, "ws_p", &proj.path().join("ws_p"));
    create_customer(global, "acme");
    let toml = |p: f64| {
        format!("[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {p}\noutput_per_mtok = 1.0\n")
    };
    std::fs::write(global.join("config.toml"), toml(1.0)).unwrap();
    std::fs::write(CustomerStore::new(global).config_path("acme"), toml(5.0)).unwrap();
    let (host, _token) = rupu_workspace::enroll_node(
        &rupu_workspace::HostStore {
            root: global.join("hosts"),
        },
        "node-a",
    )
    .unwrap();
    let node_id = match &host.transport {
        rupu_workspace::HostTransport::Tunnel { node_id } => node_id.clone(),
        other => panic!("expected a tunnel host: {other:?}"),
    };
    let at = Utc::now() - Duration::hours(1);
    seed_run_rec(global, "m_acme", "ws_p", Some(Some("acme".into())), at);
    let store = RunStore::new(global.join("runs"));
    let mut rec = store.load("m_acme").unwrap();
    rec.worker_id = Some(node_id);
    store.update(&rec).unwrap();
    // A snapshot the graph can parse (the shared seed's is a bare name).
    std::fs::write(
        store.workflow_snapshot_path("m_acme"),
        "name: wf\nsteps:\n  - id: s1\n    agent: reviewer\n    prompt: hi\n",
    )
    .unwrap();
    let base = spawn(global).await;
    let h = &host.id;

    let cost = |v: &Value| v["cost_usd"].as_f64().unwrap_or_else(|| panic!("{v}"));
    let rows = get_json(format!("{base}/api/runs?host={h}")).await;
    let row = by_id(&rows, "id", "m_acme");
    assert_eq!(row["customer"], "acme");
    assert!((cost(&row["usage"]) - 5.0).abs() < 1e-9, "list: {row}");
    let detail = get_json(format!("{base}/api/runs/m_acme?host={h}")).await;
    assert_eq!(
        cost(&detail["usage"]),
        cost(&row["usage"]),
        "detail: {detail}"
    );
    let live = get_json(format!("{base}/api/runs/m_acme/usage?host={h}")).await;
    assert_eq!(cost(&live["summary"]), cost(&row["usage"]), "usage: {live}");
    let graph = get_json(format!("{base}/api/runs/m_acme/graph?host={h}")).await;
    assert_eq!(cost(&graph["usage"]), cost(&row["usage"]), "graph");
}
