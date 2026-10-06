//! Integration tests for the customers API (`/api/customers*`): CRUD,
//! project assignment, list rollups (recorded + derived attribution,
//! per-customer pricing), `default_account`, and archived handling.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use reqwest::StatusCode;
use rupu_cp::launcher::{LaunchError, LaunchRequest, RunLauncher};
use rupu_orchestrator::runs::{RunRecord, RunStatus, RunStore, StepKind, StepResultRecord};
use serde_json::{json, Value};

/// Just enough launcher to make the state writable (`cp serve` mode).
struct MockLauncher;

#[async_trait::async_trait]
impl RunLauncher for MockLauncher {
    async fn launch(&self, _req: LaunchRequest) -> Result<String, LaunchError> {
        Ok("mock_run_id".into())
    }
}

async fn spawn(dir: &Path, writable: bool) -> String {
    let mut state =
        rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    if writable {
        state = state.with_launcher(Some(Arc::new(MockLauncher)));
    }
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
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

async fn create(base: &str, slug: &str, name: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/api/customers"))
        .json(&json!({ "slug": slug, "name": name }))
        .send()
        .await
        .unwrap()
}

async fn put_assign(base: &str, slug: &str, ws: &str) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!("{base}/api/customers/{slug}/projects/{ws}"))
        .send()
        .await
        .unwrap()
}

async fn get_json(url: String) -> Value {
    let resp = reqwest::get(url).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    resp.json().await.unwrap()
}

fn row<'a>(rows: &'a Value, slug: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["slug"] == slug)
        .unwrap_or_else(|| panic!("no row for {slug} in {rows}"))
}

#[tokio::test]
async fn create_validates_and_is_writable_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let base = spawn(tmp.path(), true).await;

    let resp = create(&base, "acme", "Acme Corp").await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let dto: Value = resp.json().await.unwrap();
    assert_eq!(dto["slug"], "acme");
    assert_eq!(dto["name"], "Acme Corp");
    assert_eq!(dto["archived"], false);
    assert!(dto["color"].is_null());
    assert!(dto["tint"]["light"].as_str().unwrap().starts_with('#'));
    assert!(dto["tint"]["dark"].as_str().unwrap().starts_with('#'));

    assert_eq!(
        create(&base, "acme", "Again").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        create(&base, "Bad Slug", "x").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        create(&base, "blank", "  ").await.status(),
        StatusCode::BAD_REQUEST
    );
    // `none` is the `?customer=` filter's "no customer": reserved.
    let resp = create(&base, "none", "Nobody").await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("reserved"),
        "{body}"
    );

    // A read-only deployment refuses every write.
    let ro = tempfile::tempdir().unwrap();
    let ro_base = spawn(ro.path(), false).await;
    assert_eq!(
        create(&ro_base, "acme", "Acme").await.status(),
        StatusCode::NOT_IMPLEMENTED
    );
    let client = reqwest::Client::new();
    for req in [
        client
            .patch(format!("{ro_base}/api/customers/acme"))
            .json(&json!({"name": "x"})),
        client.post(format!("{ro_base}/api/customers/acme/archive")),
        client.post(format!("{ro_base}/api/customers/acme/unarchive")),
        client.delete(format!("{ro_base}/api/customers/acme")),
        client.put(format!("{ro_base}/api/customers/acme/projects/ws_a")),
        client.delete(format!("{ro_base}/api/customers/acme/projects/ws_a")),
    ] {
        assert_eq!(
            req.send().await.unwrap().status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }
}

#[tokio::test]
async fn patch_updates_and_empty_string_clears() {
    let tmp = tempfile::tempdir().unwrap();
    let base = spawn(tmp.path(), true).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/customers"))
        .json(&json!({"slug": "acme", "name": "Acme", "contact": "ops@acme.test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = client
        .patch(format!("{base}/api/customers/acme"))
        .json(&json!({"name": "Acme Inc", "color": "#112233", "contact": ""}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dto: Value = resp.json().await.unwrap();
    assert_eq!(dto["name"], "Acme Inc");
    assert_eq!(dto["color"], "#112233");
    assert_eq!(dto["tint"]["light"], "#112233");
    assert!(dto["contact"].is_null());

    let bad = client
        .patch(format!("{base}/api/customers/acme"))
        .json(&json!({"color": "red"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let missing = client
        .patch(format!("{base}/api/customers/ghost"))
        .json(&json!({"name": "x"}))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn assign_shows_on_projects_and_blocks_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path(), "ws_a", &proj.path().join("a"));
    let base = spawn(tmp.path(), true).await;
    let client = reqwest::Client::new();
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );

    // Before assignment the row says "no customer" explicitly.
    let rows = get_json(format!("{base}/api/projects")).await;
    assert!(rows[0].as_object().unwrap().contains_key("customer"));
    assert!(rows[0]["customer"].is_null());

    let resp = put_assign(&base, "acme", "ws_a").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let assigned: Value = resp.json().await.unwrap();
    assert_eq!(assigned["ws_id"], "ws_a");
    assert_eq!(assigned["customer"]["slug"], "acme");

    let rows = get_json(format!("{base}/api/projects")).await;
    assert_eq!(rows[0]["customer"]["slug"], "acme");
    assert_eq!(rows[0]["customer"]["name"], "Acme");
    let detail = get_json(format!("{base}/api/projects/ws_a")).await;
    assert_eq!(detail["project"]["customer"]["slug"], "acme");

    // Unknown project / customer ⇒ 404.
    assert_eq!(
        put_assign(&base, "acme", "ws_nope").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        put_assign(&base, "ghost", "ws_a").await.status(),
        StatusCode::NOT_FOUND
    );

    // Delete with an assigned project ⇒ 409 naming it.
    let resp = client
        .delete(format!("{base}/api/customers/acme"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("acme"));
    assert_eq!(body["projects"][0]["ws_id"], "ws_a");
    assert!(body["projects"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("/a"));

    // Unassigning from a customer the project is not assigned to ⇒ 404.
    assert_eq!(
        create(&base, "globex", "Globex").await.status(),
        StatusCode::CREATED
    );
    let resp = client
        .delete(format!("{base}/api/customers/globex/projects/ws_a"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = client
        .delete(format!("{base}/api/customers/acme/projects/ws_a"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let rows = get_json(format!("{base}/api/projects")).await;
    assert!(rows[0]["customer"].is_null());

    let resp = client
        .delete(format!("{base}/api/customers/acme"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let resp = reqwest::get(format!("{base}/api/customers/acme"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── rollups ──────────────────────────────────────────────────────────────

const MODEL: &str = "claude-rollup-test";

/// A transcript with one `Usage` event: 1M input tokens of `anthropic/MODEL`.
fn write_transcript(path: &Path, ws: &str, customer: Option<&str>) {
    write_transcript_at(path, ws, customer, chrono::Utc::now());
}

fn write_transcript_at(
    path: &Path,
    ws: &str,
    customer: Option<&str>,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let start = rupu_transcript::Event::RunStart {
        run_id: "r".into(),
        workspace_id: ws.into(),
        agent: "reviewer".into(),
        provider: "anthropic".into(),
        model: MODEL.into(),
        started_at,
        mode: rupu_transcript::RunMode::Ask,
        schema: None,
        system_prompt: None,
        codename: None,
        // `None` = a legacy record (no key); see the tri-state tests.
        customer: customer.map(|c| Some(c.to_string())),
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
    customer: Option<&str>,
    started_at: chrono::DateTime<chrono::Utc>,
) -> RunRecord {
    RunRecord {
        // `None` = a legacy record (no key); see the tri-state tests.
        customer: customer.map(|c| Some(c.to_string())),
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
fn seed_run(global: &Path, id: &str, ws: &str, customer: Option<&str>) {
    seed_run_at(global, id, ws, customer, chrono::Utc::now());
}

fn seed_run_at(
    global: &Path,
    id: &str,
    ws: &str,
    customer: Option<&str>,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    let store = RunStore::new(global.join("runs"));
    store
        .create(record(id, ws, customer, started_at), "name: wf\n")
        .unwrap();
    let transcript_path = global.join("tx").join(format!("{id}.jsonl"));
    write_transcript_at(&transcript_path, ws, customer, started_at);
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
                finished_at: chrono::Utc::now(),
                loop_iteration: None,
                host: None,
                codename: None,
                cause: None,
                error: None,
            },
        )
        .unwrap();
}

fn pricing_toml(input_per_mtok: f64) -> String {
    format!(
        "[pricing.anthropic.\"{MODEL}\"]\ninput_per_mtok = {input_per_mtok}\noutput_per_mtok = 1.0\n"
    )
}

fn approx(v: &Value, want: f64) {
    let got = v.as_f64().unwrap_or_else(|| panic!("not a number: {v}"));
    assert!((got - want).abs() < 1e-9, "want {want}, got {got}");
}

#[tokio::test]
async fn list_rolls_up_recorded_and_derived_runs_with_customer_pricing() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(global, "ws_acme", &proj.path().join("acme"));
    seed_workspace(global, "ws_other", &proj.path().join("other"));
    // Global prices MODEL at $1/Mtok input; Acme's layer at $5.
    std::fs::write(global.join("config.toml"), pricing_toml(1.0)).unwrap();

    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        create(&base, "globex", "Globex").await.status(),
        StatusCode::CREATED
    );
    std::fs::write(
        rupu_workspace::CustomerStore::new(global).config_path("acme"),
        pricing_toml(5.0),
    )
    .unwrap();
    assert_eq!(
        put_assign(&base, "acme", "ws_acme").await.status(),
        StatusCode::OK
    );

    seed_run(global, "run_recorded", "ws_other", Some("acme"));
    seed_run(global, "run_derived", "ws_acme", None);
    seed_run(global, "run_none", "ws_other", None);
    // A standalone agent run in Acme's project (no recorded customer):
    // spend + activity, not a workflow run.
    write_transcript(
        &global.join("transcripts").join("agent_1.jsonl"),
        "ws_acme",
        None,
    );

    let rows = get_json(format!("{base}/api/customers")).await;
    let acme = row(&rows, "acme");
    assert_eq!(acme["name"], "Acme");
    assert_eq!(acme["rollup"]["projects"], 1);
    assert_eq!(acme["rollup"]["run_count"], 2);
    assert_eq!(acme["rollup"]["usage"]["input_tokens"], 3_000_000);
    // 3 × 1M input at Acme's $5 — global pricing would make it $3.
    approx(&acme["rollup"]["usage"]["cost_usd"], 15.0);
    assert!(acme["rollup"]["last_active"].is_string());
    assert_eq!(acme["rollup"]["findings_open"], 0);

    let globex = row(&rows, "globex");
    assert_eq!(globex["rollup"]["run_count"], 0);
    assert_eq!(globex["rollup"]["projects"], 0);
    assert_eq!(globex["rollup"]["usage"]["input_tokens"], 0);
    assert!(globex["rollup"]["last_active"].is_null());

    // The detail endpoint agrees and lists the projects.
    let detail = get_json(format!("{base}/api/customers/acme")).await;
    assert_eq!(detail["customer"]["slug"], "acme");
    assert_eq!(detail["rollup"]["run_count"], 2);
    approx(&detail["rollup"]["usage"]["cost_usd"], 15.0);
    assert_eq!(detail["projects"][0]["ws_id"], "ws_acme");
    assert_eq!(detail["projects"][0]["customer"]["slug"], "acme");
    assert!(detail["layer_error"].is_null());

    // A bad range is a 400.
    let resp = reqwest::get(format!("{base}/api/customers?range=1y"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// One finding in a project's coverage ledger.
fn seed_finding(project: &Path) {
    let dir = project.join(".rupu").join("coverage").join("t1");
    std::fs::create_dir_all(&dir).unwrap();
    let finding = "{\"id\":\"f1\",\"file_path\":\"src/a.rs\",\"line_range\":[1,5],\
\"scope\":\"line\",\"summary\":\"thing\",\"severity\":\"high\",\
\"concern_id\":\"ssrf\",\
\"evidence\":{\"rationale\":\"because\"},\
\"declared_by\":{\"run_id\":\"run_x\",\"model\":\"claude\",\"surface\":\"workflow\"},\
\"declared_at\":\"2026-06-19T00:00:00Z\"}\n";
    std::fs::write(dir.join("findings.jsonl"), finding).unwrap();
}

#[tokio::test]
async fn findings_count_only_the_customers_current_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let a = proj.path().join("a");
    let b = proj.path().join("b");
    seed_workspace(tmp.path(), "ws_a", &a);
    seed_workspace(tmp.path(), "ws_b", &b);
    seed_finding(&a);
    seed_finding(&b);
    let base = spawn(tmp.path(), true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        put_assign(&base, "acme", "ws_a").await.status(),
        StatusCode::OK
    );

    let rows = get_json(format!("{base}/api/customers")).await;
    assert_eq!(row(&rows, "acme")["rollup"]["findings_open"], 1);
}

#[tokio::test]
async fn default_account_reports_layer_lock_and_inheritance() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    std::fs::write(
        global.join("config.toml"),
        "default_provider = \"anthropic\"\n",
    )
    .unwrap();
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        create(&base, "globex", "Globex").await.status(),
        StatusCode::CREATED
    );
    let store = rupu_workspace::CustomerStore::new(global);
    std::fs::write(
        store.config_path("acme"),
        "default_provider = \"anthropic-acme\"\n\n[policy]\nlock = [\"default_provider\"]\n",
    )
    .unwrap();

    let rows = get_json(format!("{base}/api/customers")).await;
    assert_eq!(
        row(&rows, "acme")["default_account"],
        json!({"account": "anthropic-acme", "locked_by": "customer", "inherited": false})
    );
    assert_eq!(
        row(&rows, "globex")["default_account"],
        json!({"account": "anthropic", "locked_by": null, "inherited": true})
    );

    // A malformed layer: the row stays listed with no default account, and
    // the detail names the parse error.
    std::fs::write(store.config_path("globex"), "this is = = not toml").unwrap();
    let rows = get_json(format!("{base}/api/customers")).await;
    assert!(row(&rows, "globex")["default_account"].is_null());
    let detail = get_json(format!("{base}/api/customers/globex")).await;
    assert!(detail["default_account"].is_null());
    assert!(detail["layer_error"].is_string());
    let detail = get_json(format!("{base}/api/customers/acme")).await;
    assert!(detail["layer_error"].is_null());
    assert_eq!(detail["default_account"]["account"], "anthropic-acme");
}

/// A customer whose layer does not resolve is priced at the GLOBAL rates —
/// and every surface that priced its work says so: the customer row carries
/// `layer_error`, its rollup / run rows / usage carry `pricing_error`
/// (naming the customer); work of other customers carries none.
#[tokio::test]
async fn a_malformed_layer_prices_at_global_rates_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(global, "ws_other", &proj.path().join("other"));
    std::fs::write(global.join("config.toml"), pricing_toml(1.0)).unwrap();
    let base = spawn(global, true).await;
    for slug in ["acme", "globex"] {
        assert_eq!(
            create(&base, slug, slug).await.status(),
            StatusCode::CREATED
        );
    }
    let store = rupu_workspace::CustomerStore::new(global);
    std::fs::write(store.config_path("acme"), "this is = = not toml").unwrap();
    std::fs::write(store.config_path("globex"), pricing_toml(5.0)).unwrap();
    seed_run(global, "run_acme", "ws_other", Some("acme"));
    seed_run(global, "run_globex", "ws_other", Some("globex"));

    let rows = get_json(format!("{base}/api/customers")).await;
    let acme = row(&rows, "acme");
    assert!(acme["layer_error"].is_string(), "{acme}");
    assert!(acme["default_account"].is_null());
    // 1M input at the GLOBAL $1, flagged.
    approx(&acme["rollup"]["usage"]["cost_usd"], 1.0);
    let why = acme["rollup"]["usage"]["pricing_error"].as_str().unwrap();
    assert!(why.contains("acme") && why.contains("global"), "{why}");
    let globex = row(&rows, "globex");
    assert!(globex["layer_error"].is_null());
    approx(&globex["rollup"]["usage"]["cost_usd"], 5.0);
    assert!(globex["rollup"]["usage"].get("pricing_error").is_none());

    let runs = get_json(format!("{base}/api/runs?host=local")).await;
    let run = |id: &str| {
        runs.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()
            .clone()
    };
    assert!(run("run_acme")["usage"]["pricing_error"].is_string());
    assert!(run("run_globex")["usage"].get("pricing_error").is_none());

    let usage = get_json(format!("{base}/api/usage?customer=acme")).await;
    assert!(usage["summary"]["pricing_error"].is_string(), "{usage}");
    let usage = get_json(format!("{base}/api/usage?customer=globex")).await;
    assert!(usage["summary"].get("pricing_error").is_none(), "{usage}");
    // Unfiltered, the merged total says some of it was mispriced.
    let usage = get_json(format!("{base}/api/usage")).await;
    assert!(usage["summary"]["pricing_error"].is_string(), "{usage}");

    let rows = get_json(format!("{base}/api/usage/runs?customer=acme")).await;
    assert!(rows[0]["pricing_error"].is_string(), "{rows}");
}

/// A run store that cannot be listed fails the rollups (500) — never a
/// rollup of zero spend.
#[tokio::test]
async fn an_unreadable_run_store_fails_the_rollups() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    // `runs` exists but is not a directory: listing it fails (not NotFound).
    let runs = global.join("runs");
    let _ = std::fs::remove_dir_all(&runs);
    std::fs::write(&runs, "not a directory").unwrap();
    for path in ["/api/customers", "/api/customers/acme", "/api/projects"] {
        let (status, body) = error_of(format!("{base}{path}")).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{path}: {body}");
        assert!(body.contains("cannot list runs"), "{path}: {body}");
    }
}

#[tokio::test]
async fn archived_customers_are_hidden_unless_asked_for() {
    let tmp = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(tmp.path(), "ws_a", &proj.path().join("a"));
    let base = spawn(tmp.path(), true).await;
    let client = reqwest::Client::new();
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        create(&base, "globex", "Globex").await.status(),
        StatusCode::CREATED
    );

    let resp = client
        .post(format!("{base}/api/customers/globex/archive"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let dto: Value = resp.json().await.unwrap();
    assert_eq!(dto["archived"], true);

    let rows = get_json(format!("{base}/api/customers")).await;
    let slugs: Vec<&str> = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["acme"]);
    let rows = get_json(format!("{base}/api/customers?archived=1")).await;
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(row(&rows, "globex")["archived"], true);

    // An archived customer still has a detail page, and takes no projects.
    let detail = get_json(format!("{base}/api/customers/globex")).await;
    assert_eq!(detail["customer"]["archived"], true);
    assert_eq!(
        put_assign(&base, "globex", "ws_a").await.status(),
        StatusCode::CONFLICT
    );

    let resp = client
        .post(format!("{base}/api/customers/globex/unarchive"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let rows = get_json(format!("{base}/api/customers")).await;
    assert_eq!(rows.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn project_rows_are_priced_per_customer_and_sum_to_its_rollup() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(global, "ws_acme", &proj.path().join("acme"));
    seed_workspace(global, "ws_other", &proj.path().join("other"));
    std::fs::write(global.join("config.toml"), pricing_toml(1.0)).unwrap();
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    std::fs::write(
        rupu_workspace::CustomerStore::new(global).config_path("acme"),
        pricing_toml(5.0),
    )
    .unwrap();
    assert_eq!(
        put_assign(&base, "acme", "ws_acme").await.status(),
        StatusCode::OK
    );

    seed_run(global, "run_recorded", "ws_acme", Some("acme"));
    seed_run(global, "run_derived", "ws_acme", None);
    seed_run(global, "run_none", "ws_other", None);
    write_transcript(
        &global.join("transcripts").join("agent_1.jsonl"),
        "ws_acme",
        None,
    );

    let detail = get_json(format!("{base}/api/customers/acme?range=all")).await;
    approx(&detail["rollup"]["usage"]["cost_usd"], 15.0);
    let projects = detail["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    let sum: f64 = projects
        .iter()
        .map(|p| p["usage"]["cost_usd"].as_f64().unwrap())
        .sum();
    assert!((sum - 15.0).abs() < 1e-9, "projects sum {sum}");
    assert_eq!(projects[0]["run_count"], 2);

    // The Projects page prices the same way: Acme's runs at $5, the
    // unassigned project's at the global $1.
    let rows = get_json(format!("{base}/api/projects")).await;
    let by_ws = |ws: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["ws_id"] == ws)
            .unwrap()
            .clone()
    };
    approx(&by_ws("ws_acme")["usage"]["cost_usd"], 15.0);
    approx(&by_ws("ws_other")["usage"]["cost_usd"], 1.0);
    let one = get_json(format!("{base}/api/projects/ws_acme")).await;
    approx(&one["usage"]["cost_usd"], 15.0);
}

#[tokio::test]
async fn range_7d_excludes_older_runs_and_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(global, "ws_acme", &proj.path().join("acme"));
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        put_assign(&base, "acme", "ws_acme").await.status(),
        StatusCode::OK
    );

    let old = chrono::Utc::now() - chrono::Duration::days(10);
    seed_run(global, "run_new", "ws_acme", None);
    seed_run_at(global, "run_old", "ws_acme", None, old);
    write_transcript(
        &global.join("transcripts").join("agent_new.jsonl"),
        "ws_acme",
        None,
    );
    write_transcript_at(
        &global.join("transcripts").join("agent_old.jsonl"),
        "ws_acme",
        None,
        old,
    );

    let rows = get_json(format!("{base}/api/customers?range=7d")).await;
    let acme = row(&rows, "acme");
    assert_eq!(acme["rollup"]["run_count"], 1);
    assert_eq!(acme["rollup"]["usage"]["input_tokens"], 2_000_000);

    let rows = get_json(format!("{base}/api/customers?range=all")).await;
    let acme = row(&rows, "acme");
    assert_eq!(acme["rollup"]["run_count"], 2);
    assert_eq!(acme["rollup"]["usage"]["input_tokens"], 4_000_000);
}

async fn error_of(url: String) -> (StatusCode, String) {
    let resp = reqwest::get(url).await.unwrap();
    let status = resp.status();
    let body: Value = resp.json().await.unwrap();
    (
        status,
        body["error"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test]
async fn an_unreadable_assignment_fails_the_listings_naming_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let proj = tempfile::tempdir().unwrap();
    seed_workspace(global, "ws_broken", &proj.path().join("broken"));
    // A directory where the assignment sidecar belongs: unreadable, which
    // must never read as "no customer".
    std::fs::create_dir(global.join("workspaces").join("ws_broken.customer")).unwrap();
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );

    for url in [
        format!("{base}/api/projects"),
        format!("{base}/api/projects/ws_broken"),
        format!("{base}/api/customers"),
        format!("{base}/api/customers/acme"),
    ] {
        let (status, msg) = error_of(url.clone()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{url}");
        assert!(msg.contains("ws_broken"), "{url}: {msg}");
    }
}

#[tokio::test]
async fn an_invalid_legacy_workspace_id_lists_as_unassigned() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path();
    let base = spawn(global, true).await;
    assert_eq!(
        create(&base, "acme", "Acme").await.status(),
        StatusCode::CREATED
    );
    seed_run(global, "run_legacy", "not a valid id!", None);

    let rows = get_json(format!("{base}/api/customers")).await;
    assert_eq!(row(&rows, "acme")["rollup"]["run_count"], 0);
    get_json(format!("{base}/api/projects")).await;
}

#[tokio::test]
async fn read_only_customer_writes_name_the_action() {
    let tmp = tempfile::tempdir().unwrap();
    let base = spawn(tmp.path(), false).await;
    let resp = create(&base, "acme", "Acme").await;
    assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "managing customers requires `rupu cp serve`");
}
