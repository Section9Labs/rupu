//! Integration tests for `GET /api/usage` — Task 3: the unpriced gap as an
//! explicit named number, and host fan-out.
//!
//! Before this task `UsageSummary.priced == false` meant "spend is partial"
//! but named neither which models nor how many rows were behind that partial
//! total. `unpriced` in the response now names both. Task 3 also fans the
//! endpoint out across every registered host, mirroring `/api/dashboard`'s
//! rule: a host that cannot report contributes nothing, never a zero, and its
//! state is carried in `hosts[]`.

// ---------------------------------------------------------------------------
// Spawn helpers (mirrors tests/dashboard.rs; helpers are duplicated per file
// — there is no shared `tests/common/` module in this crate).
// ---------------------------------------------------------------------------

struct TestServer {
    base_url: String,
}

/// Spin up a read-only local-only server.
async fn spawn_server(dir: &std::path::Path) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

/// Spin up a server with one remote host pre-registered via the registry.
async fn spawn_server_with_remote(dir: &std::path::Path, mock_base_url: &str) -> TestServer {
    let state = rupu_cp::state::AppState::new(dir.into(), rupu_config::PricingConfig::default());
    state
        .hosts
        .add_host("mock-remote", mock_base_url, None)
        .expect("add_host should succeed");
    let app = rupu_cp::server::router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{addr}"),
    }
}

// ---------------------------------------------------------------------------
// Seeders (mirrors src/api/usage.rs's own `#[cfg(test)]` helpers — helpers
// are duplicated per file, no shared `tests/common/`).
// ---------------------------------------------------------------------------

/// Write a two-line transcript: `RunStart` (anchors provider/model/agent,
/// using a provider with no configured price) followed by one `Usage` event.
fn write_run_transcript(path: &std::path::Path, model: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let start = rupu_transcript::Event::RunStart {
        run_id: "r".into(),
        workspace_id: "ws".into(),
        agent: "reviewer".into(),
        // "internal-vllm" carries no entry in the default `PricingConfig` —
        // see `crate::usage`'s own tests (`summarize_unpriced_model_yields_no_cost`)
        // — so any model under it is guaranteed unpriced regardless of name.
        provider: "internal-vllm".into(),
        model: model.into(),
        started_at: chrono::Utc::now(),
        mode: rupu_transcript::RunMode::Ask,
    };
    let usage = rupu_transcript::Event::Usage {
        provider: "internal-vllm".into(),
        model: model.into(),
        served_model: None,
        input_tokens: 1000,
        output_tokens: 200,
        cached_tokens: 0,
    };
    let mut buf = Vec::new();
    for ev in [&start, &usage] {
        let mut line = serde_json::to_vec(ev).unwrap();
        line.push(b'\n');
        buf.extend(line);
    }
    std::fs::write(path, &buf).unwrap();
}

/// Register a completed run bound to `dir`, with one step whose transcript
/// reports usage for `model` under the unpriced "internal-vllm" provider.
fn seed_transcript_with_model(dir: &std::path::Path, run_id: &str, model: &str) {
    let run_store = rupu_orchestrator::runs::RunStore::new(dir.join("runs"));
    let record = rupu_orchestrator::RunRecord {
        id: run_id.into(),
        workflow_name: "wf".into(),
        status: rupu_orchestrator::RunStatus::Completed,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws".into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: None,
        error_message: None,
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
        final_output: None,
    };
    let transcript_path = dir.join(format!("{run_id}.jsonl"));
    run_store.create(record, "name: wf\n").unwrap();
    write_run_transcript(&transcript_path, model);
    run_store
        .append_step_result(
            run_id,
            &rupu_orchestrator::runs::StepResultRecord {
                step_id: "s1".into(),
                run_id: run_id.into(),
                transcript_path,
                output: String::new(),
                success: true,
                skipped: false,
                rendered_prompt: String::new(),
                kind: rupu_orchestrator::runs::StepKind::Linear,
                items: vec![],
                findings: vec![],
                iterations: 0,
                resolved: true,
                finished_at: chrono::Utc::now(),
            },
        )
        .unwrap();
}

// ---------------------------------------------------------------------------
// Part A: the unpriced gap is a named number.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_reports_unpriced_models_explicitly() {
    let dir = tempfile::tempdir().unwrap();
    // Seed a transcript using a model with no configured price.
    seed_transcript_with_model(dir.path(), "run_1", "some-unpriced-model");
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let unpriced = body["unpriced"]["models"].as_array().unwrap();
    assert!(
        unpriced.iter().any(|m| m == "some-unpriced-model"),
        "an unpriced model must be named, not hidden behind a '*': {body}"
    );
    assert!(body["unpriced"]["rows"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn usage_priced_only_reports_empty_unpriced_gap() {
    // A run under a fully-priced model must not show up in `unpriced` at all
    // — the gap is the exception, not the default shape.
    let dir = tempfile::tempdir().unwrap();
    let run_store = rupu_orchestrator::runs::RunStore::new(dir.path().join("runs"));
    let record = rupu_orchestrator::RunRecord {
        id: "run_priced".into(),
        workflow_name: "wf".into(),
        status: rupu_orchestrator::RunStatus::Completed,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws".into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: None,
        error_message: None,
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
        final_output: None,
    };
    let transcript_path = dir.path().join("run_priced.jsonl");
    run_store.create(record, "name: wf\n").unwrap();
    std::fs::write(
        &transcript_path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&rupu_transcript::Event::RunStart {
                run_id: "run_priced".into(),
                workspace_id: "ws".into(),
                agent: "reviewer".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: chrono::Utc::now(),
                mode: rupu_transcript::RunMode::Ask,
            })
            .unwrap(),
            serde_json::to_string(&rupu_transcript::Event::Usage {
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                served_model: None,
                input_tokens: 1000,
                output_tokens: 200,
                cached_tokens: 0,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    run_store
        .append_step_result(
            "run_priced",
            &rupu_orchestrator::runs::StepResultRecord {
                step_id: "s1".into(),
                run_id: "run_priced".into(),
                transcript_path,
                output: String::new(),
                success: true,
                skipped: false,
                rendered_prompt: String::new(),
                kind: rupu_orchestrator::runs::StepKind::Linear,
                items: vec![],
                findings: vec![],
                iterations: 0,
                resolved: true,
                finished_at: chrono::Utc::now(),
            },
        )
        .unwrap();

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        body["unpriced"]["models"].as_array().unwrap().len(),
        0,
        "a fully-priced run must not surface any unpriced models: {body}"
    );
    assert_eq!(body["unpriced"]["rows"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn usage_rejects_unknown_group_by() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/usage?group_by=workflw", srv.base_url))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "a typo must 400, not silently return a model breakdown"
    );
}

// ---------------------------------------------------------------------------
// Part B: host fan-out.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_reports_per_host_freshness_and_local_is_always_ok() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert!(!hosts.is_empty(), "local must always appear");
    let local = &hosts[0];
    assert_eq!(local["host_id"], "local");
    assert_eq!(local["state"], "ok");
    assert!(
        local["captured_at"].as_str().unwrap().contains('T'),
        "captured_at must be RFC-3339 for the freshness strip"
    );
}

#[tokio::test]
async fn usage_unknown_host_returns_404() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/usage?host=nope", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404, "an unknown host id must 404");
}

#[tokio::test]
async fn usage_scoped_to_host_local_returns_only_local() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage?host=local", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().expect("hosts array required");
    assert_eq!(
        hosts.len(),
        1,
        "?host=local must not also probe the registered remote"
    );
    assert_eq!(hosts[0]["host_id"], "local");
}

#[tokio::test]
async fn usage_unreachable_remote_renders_unavailable_not_omitted() {
    // A host that cannot report is NOT a host with no usage. Register an
    // unreachable remote and assert it still appears in `hosts[]`, never
    // silently dropped, and never folded in as a zero.
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server_with_remote(dir.path(), "http://127.0.0.1:1/").await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().unwrap();
    let remote = hosts
        .iter()
        .find(|h| h["host_id"] != "local")
        .expect("the unreachable remote must still appear in the freshness strip");
    assert_ne!(
        remote["state"], "ok",
        "an unreachable host must not report ok"
    );
    assert!(
        remote["captured_at"].is_null(),
        "an unreachable host has no captured_at — it never reported"
    );
}

#[tokio::test]
async fn usage_fans_out_across_a_real_remote_host_and_sums_tokens() {
    // Two real CP servers: "remote" seeded with its own unpriced-model run,
    // "central" has it registered as a host. Hitting central's /api/usage
    // (no ?host=) must include the remote's tokens in the merged summary and
    // its unpriced model in the merged gap — spend that is local-only is
    // wrong for the same reason the dashboard was.
    let remote_dir = tempfile::tempdir().unwrap();
    seed_transcript_with_model(remote_dir.path(), "remote_run", "remote-unpriced-model");
    let remote_srv = spawn_server(remote_dir.path()).await;

    let central_dir = tempfile::tempdir().unwrap();
    seed_transcript_with_model(central_dir.path(), "central_run", "central-unpriced-model");
    let central_srv = spawn_server_with_remote(central_dir.path(), &remote_srv.base_url).await;

    let body: serde_json::Value = reqwest::get(format!("{}/api/usage", central_srv.base_url))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let hosts = body["hosts"].as_array().unwrap();
    assert_eq!(
        hosts.len(),
        2,
        "both local and the remote must report: {body}"
    );
    assert!(
        hosts.iter().all(|h| h["state"] == "ok"),
        "both hosts must report ok: {hosts:?}"
    );

    // Merged summary sums input tokens across both hosts (1000 each).
    assert_eq!(
        body["summary"]["input_tokens"].as_u64().unwrap(),
        2000,
        "central + remote input tokens must sum: {body}"
    );

    // Merged unpriced gap names both hosts' unpriced models.
    let models = body["unpriced"]["models"].as_array().unwrap();
    assert!(
        models.iter().any(|m| m == "central-unpriced-model"),
        "the local model must be named: {models:?}"
    );
    assert!(
        models.iter().any(|m| m == "remote-unpriced-model"),
        "the remote model must be named too, not dropped: {models:?}"
    );
    assert_eq!(body["unpriced"]["rows"].as_u64().unwrap(), 2);
}

#[tokio::test]
async fn usage_group_by_host_tags_remote_rows_with_the_real_host_id_not_local() {
    // Both hosts hardcode their OWN rows' `host_id` to "local" from their own
    // point of view (Task 2). Without the fan-out override, grouping by host
    // would collapse both hosts' contributions into a single "local" bucket.
    let remote_dir = tempfile::tempdir().unwrap();
    seed_transcript_with_model(remote_dir.path(), "remote_run", "remote-unpriced-model");
    let remote_srv = spawn_server(remote_dir.path()).await;

    let central_dir = tempfile::tempdir().unwrap();
    seed_transcript_with_model(central_dir.path(), "central_run", "central-unpriced-model");
    let central_srv = spawn_server_with_remote(central_dir.path(), &remote_srv.base_url).await;

    let body: serde_json::Value =
        reqwest::get(format!("{}/api/usage?group_by=host", central_srv.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

    let breakdown = body["breakdown"].as_array().expect("breakdown array");
    assert_eq!(
        breakdown.len(),
        2,
        "two distinct hosts must not collapse into one 'local' bucket: {breakdown:?}"
    );
    let host_ids: std::collections::BTreeSet<&str> = breakdown
        .iter()
        .map(|r| r["host_id"].as_str().unwrap())
        .collect();
    assert!(
        host_ids.contains("local"),
        "the central host's own rows must be tagged local: {host_ids:?}"
    );
    assert!(
        !host_ids.contains(&""),
        "no row should be left with an untagged empty host_id: {host_ids:?}"
    );
    assert!(
        host_ids.iter().any(|id| *id != "local"),
        "the remote's row must carry the REAL registered host id, not 'local': {host_ids:?}"
    );
}

// ---------------------------------------------------------------------------
// Part C: `GET /api/usage/runs` — flat per-(run × model) rows (Task U1).
// ---------------------------------------------------------------------------

/// Write a two-line transcript (`RunStart` + one `Usage` event) for
/// `provider`/`model`, reporting `input_tokens`/`output_tokens`.
fn write_run_transcript_for(
    path: &std::path::Path,
    workspace_id: &str,
    provider: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let start = rupu_transcript::Event::RunStart {
        run_id: "r".into(),
        workspace_id: workspace_id.into(),
        agent: "reviewer".into(),
        provider: provider.into(),
        model: model.into(),
        started_at: chrono::Utc::now(),
        mode: rupu_transcript::RunMode::Ask,
    };
    let usage = rupu_transcript::Event::Usage {
        provider: provider.into(),
        model: model.into(),
        served_model: None,
        input_tokens,
        output_tokens,
        cached_tokens: 0,
    };
    let mut buf = Vec::new();
    for ev in [&start, &usage] {
        let mut line = serde_json::to_vec(ev).unwrap();
        line.push(b'\n');
        buf.extend(line);
    }
    std::fs::write(path, &buf).unwrap();
}

/// Register a completed run under `dir`'s run store, with one step whose
/// transcript reports usage for `provider`/`model`. `started_at` is caller
/// controlled so `?since` filtering can be exercised.
#[allow(clippy::too_many_arguments)]
fn seed_run_with_usage(
    dir: &std::path::Path,
    run_id: &str,
    workflow_name: &str,
    workspace_id: &str,
    provider: &str,
    model: &str,
    input_tokens: u32,
    output_tokens: u32,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    let run_store = rupu_orchestrator::runs::RunStore::new(dir.join("runs"));
    let record = rupu_orchestrator::RunRecord {
        id: run_id.into(),
        workflow_name: workflow_name.into(),
        status: rupu_orchestrator::RunStatus::Completed,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: workspace_id.into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: std::path::PathBuf::from("/tmp/proj/.rupu/transcripts"),
        started_at,
        finished_at: None,
        error_message: None,
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
        final_output: None,
    };
    let transcript_path = dir.join(format!("{run_id}.jsonl"));
    run_store.create(record, "name: wf\n").unwrap();
    write_run_transcript_for(
        &transcript_path,
        workspace_id,
        provider,
        model,
        input_tokens,
        output_tokens,
    );
    run_store
        .append_step_result(
            run_id,
            &rupu_orchestrator::runs::StepResultRecord {
                step_id: "s1".into(),
                run_id: run_id.into(),
                transcript_path,
                output: String::new(),
                success: true,
                skipped: false,
                rendered_prompt: String::new(),
                kind: rupu_orchestrator::runs::StepKind::Linear,
                items: vec![],
                findings: vec![],
                iterations: 0,
                resolved: true,
                finished_at: chrono::Utc::now(),
            },
        )
        .unwrap();
}

#[tokio::test]
async fn usage_runs_returns_flat_per_run_rows_with_run_id_and_priced_cost() {
    let dir = tempfile::tempdir().unwrap();
    let started_1 = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let started_2 = chrono::DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    seed_run_with_usage(
        dir.path(),
        "run_1",
        "nightly-review",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1_000_000,
        0,
        started_1,
    );
    seed_run_with_usage(
        dir.path(),
        "run_2",
        "hotfix",
        "ws_b",
        "internal-vllm",
        "llama-3-70b",
        1000,
        200,
        started_2,
    );

    let srv = spawn_server(dir.path()).await;
    // Explicit `since` (rather than relying on the default 30-day window) so
    // this test is not sensitive to the gap between these fixed 2026-06
    // timestamps and whatever `Utc::now()` the CI/dev clock reports.
    let body: serde_json::Value = reqwest::get(format!(
        "{}/api/usage/runs?since=2026-01-01T00:00:00Z",
        srv.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let rows = body.as_array().expect("flat array of rows");
    assert_eq!(rows.len(), 2, "one row per run: {rows:?}");

    let r1 = rows
        .iter()
        .find(|r| r["run_id"] == "run_1")
        .expect("run_1 row present");
    assert_eq!(r1["workflow_name"], "nightly-review");
    assert_eq!(r1["model"], "claude-sonnet-4-6");
    assert_eq!(r1["provider"], "anthropic");
    assert_eq!(r1["workspace_id"], "ws_a");
    assert_eq!(r1["host_id"], "local");
    assert_eq!(r1["input_tokens"].as_u64().unwrap(), 1_000_000);
    assert_eq!(r1["priced"], true);
    assert!(
        (r1["cost_usd"].as_f64().unwrap() - 3.0).abs() < 1e-9,
        "1M anthropic input tokens at $3/M: {r1:?}"
    );
    assert!(
        r1["started_at"].as_str().unwrap().ends_with('Z'),
        "started_at must be Z-suffixed RFC-3339, matching RunListRow: {r1:?}"
    );

    let r2 = rows
        .iter()
        .find(|r| r["run_id"] == "run_2")
        .expect("run_2 row present");
    assert_eq!(r2["workflow_name"], "hotfix");
    assert_eq!(r2["model"], "llama-3-70b");
    assert_eq!(r2["workspace_id"], "ws_b");
    assert_eq!(r2["priced"], false);
    assert!(
        r2["cost_usd"].is_null(),
        "unpriced row must report null cost, never a fabricated number: {r2:?}"
    );
    assert_eq!(r2["input_tokens"].as_u64().unwrap(), 1000);
    assert_eq!(r2["output_tokens"].as_u64().unwrap(), 200);
}

#[tokio::test]
async fn usage_runs_workspace_id_scopes_to_that_project_only() {
    let dir = tempfile::tempdir().unwrap();
    let now = chrono::Utc::now();
    seed_run_with_usage(
        dir.path(),
        "run_a",
        "wf-a",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1000,
        0,
        now,
    );
    seed_run_with_usage(
        dir.path(),
        "run_b",
        "wf-b",
        "ws_b",
        "anthropic",
        "claude-sonnet-4-6",
        2000,
        0,
        now,
    );

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value =
        reqwest::get(format!("{}/api/usage/runs?workspace_id=ws_a", srv.base_url))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
    let rows = body.as_array().expect("flat array of rows");
    assert_eq!(
        rows.len(),
        1,
        "workspace_id must scope out the other project's run: {rows:?}"
    );
    assert_eq!(rows[0]["run_id"], "run_a");
    assert_eq!(rows[0]["workspace_id"], "ws_a");
}

#[tokio::test]
async fn usage_runs_since_excludes_a_run_started_before_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let old = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let recent = chrono::DateTime::parse_from_rfc3339("2026-06-15T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    seed_run_with_usage(
        dir.path(),
        "run_old",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1000,
        0,
        old,
    );
    seed_run_with_usage(
        dir.path(),
        "run_recent",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        2000,
        0,
        recent,
    );

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!(
        "{}/api/usage/runs?since=2026-06-01T00:00:00Z",
        srv.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let rows = body.as_array().expect("flat array of rows");
    assert_eq!(
        rows.len(),
        1,
        "the run started before `since` must be excluded: {rows:?}"
    );
    assert_eq!(rows[0]["run_id"], "run_recent");
}
