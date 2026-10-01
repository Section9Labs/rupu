//! Integration tests for `GET /api/usage` — Task 3: the unpriced gap as an
//! explicit named number, and host fan-out.
//!
//! Before this task `UsageSummary.priced == false` meant "spend is partial"
//! but named neither which models nor how many rows were behind that partial
//! total. `unpriced` in the response now names both. Task 3 also fans the
//! endpoint out across every registered host, mirroring `/api/dashboard`'s
//! rule: a host that cannot report contributes nothing, never a zero, and its
//! state is carried in `hosts[]`.

// Throwaway in-process mock-server client, not rupu's egress
// (choke_point.rs's guard test already exempts everything under `/tests/`
// on that basis).
#![allow(clippy::disallowed_methods)]

// ---------------------------------------------------------------------------
// Spawn helpers (mirrors tests/it/dashboard.rs; helpers are duplicated per file
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
        schema: None,
        system_prompt: None,
        codename: None,
    };
    let usage = rupu_transcript::Event::Usage {
        provider: "internal-vllm".into(),
        model: model.into(),
        served_model: None,
        input_tokens: 1000,
        output_tokens: 200,
        cached_tokens: 0,
        cache_write_tokens: 0,
        purpose: None,
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
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        codename: None,
    };
    let transcript_path = dir.join(format!("{run_id}.jsonl"));
    run_store.create(record, "name: wf\n").unwrap();
    write_run_transcript(&transcript_path, model);
    run_store
        .append_step_result(
            run_id,
            &rupu_orchestrator::runs::StepResultRecord {
                run_outcome: None,
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
                loop_iteration: None,
                host: None,
                codename: None,
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
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        codename: None,
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
                schema: None,
                system_prompt: None,
                codename: None,
            })
            .unwrap(),
            serde_json::to_string(&rupu_transcript::Event::Usage {
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                served_model: None,
                input_tokens: 1000,
                output_tokens: 200,
                cached_tokens: 0,
                cache_write_tokens: 0,
                purpose: None,
            })
            .unwrap(),
        ),
    )
    .unwrap();
    run_store
        .append_step_result(
            "run_priced",
            &rupu_orchestrator::runs::StepResultRecord {
                run_outcome: None,
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
                loop_iteration: None,
                host: None,
                codename: None,
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
        schema: None,
        system_prompt: None,
        codename: None,
    };
    let usage = rupu_transcript::Event::Usage {
        provider: provider.into(),
        model: model.into(),
        served_model: None,
        input_tokens,
        output_tokens,
        cached_tokens: 0,
        cache_write_tokens: 0,
        purpose: None,
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
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
        codename: None,
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
                run_outcome: None,
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
                loop_iteration: None,
                host: None,
                codename: None,
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
async fn usage_runs_rows_carry_cache_write_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let started = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    seed_run_with_usage(
        dir.path(),
        "run_cw",
        "nightly-review",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1000,
        20,
        started,
    );
    // A second call on the same transcript that wrote 30 prompt tokens to
    // the provider's cache (a subset of its 500 input tokens).
    let mut line = serde_json::to_vec(&rupu_transcript::Event::Usage {
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        served_model: None,
        input_tokens: 500,
        output_tokens: 5,
        cached_tokens: 0,
        cache_write_tokens: 30,
        purpose: None,
    })
    .unwrap();
    line.push(b'\n');
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join("run_cw.jsonl"))
        .unwrap()
        .write_all(&line)
        .unwrap();

    let srv = spawn_server(dir.path()).await;
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
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["cache_write_tokens"].as_u64(), Some(30), "{rows:?}");
    assert_eq!(rows[0]["input_tokens"].as_u64(), Some(1500));
    assert_eq!(
        rows[0]["total_tokens"].as_u64(),
        Some(1525),
        "input + output; cache writes are not added again"
    );
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

#[tokio::test]
async fn usage_runs_since_and_until_bound_the_window_on_both_ends() {
    // Task W1: `/api/usage/runs` gains `until`. Three runs: one before
    // `since`, one inside `[since, until]`, one after `until` — only the
    // middle run must survive.
    let dir = tempfile::tempdir().unwrap();
    let before = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let inside = chrono::DateTime::parse_from_rfc3339("2026-06-15T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let after = chrono::DateTime::parse_from_rfc3339("2026-07-15T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    seed_run_with_usage(
        dir.path(),
        "run_before",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1000,
        0,
        before,
    );
    seed_run_with_usage(
        dir.path(),
        "run_inside",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        2000,
        0,
        inside,
    );
    seed_run_with_usage(
        dir.path(),
        "run_after",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        3000,
        0,
        after,
    );

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!(
        "{}/api/usage/runs?since=2026-06-01T00:00:00Z&until=2026-07-01T00:00:00Z",
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
        "runs before `since` and after `until` must both be excluded: {rows:?}"
    );
    assert_eq!(rows[0]["run_id"], "run_inside");
}

#[tokio::test]
async fn usage_runs_unparseable_until_returns_400() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/usage/runs?until=notadate", srv.base_url))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "an unparseable `until` must 400, not silently fall back to now"
    );
}

// ---------------------------------------------------------------------------
// Part D: `GET /api/usage/outliers?until=` (Task W1).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn usage_outliers_until_excludes_an_outlier_eligible_run_after_the_bound() {
    // Three cheap baseline runs (~$0.3 each) inside the window establish the
    // median. A spike run inside the window (~$3, 10x) must be flagged. An
    // identical spike run started AFTER `until` must be excluded entirely —
    // not flagged, and not folded into the baseline either.
    let dir = tempfile::tempdir().unwrap();
    let base_1 = chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let base_2 = chrono::DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let base_3 = chrono::DateTime::parse_from_rfc3339("2026-06-03T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let spike_in_window = chrono::DateTime::parse_from_rfc3339("2026-06-10T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let spike_after_until = chrono::DateTime::parse_from_rfc3339("2026-07-15T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);

    for (i, ts) in [base_1, base_2, base_3].into_iter().enumerate() {
        seed_run_with_usage(
            dir.path(),
            &format!("base_{i}"),
            "wf",
            "ws_a",
            "anthropic",
            "claude-sonnet-4-6",
            100_000,
            0,
            ts,
        );
    }
    seed_run_with_usage(
        dir.path(),
        "spike_in_window",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1_000_000,
        0,
        spike_in_window,
    );
    seed_run_with_usage(
        dir.path(),
        "spike_after_until",
        "wf",
        "ws_a",
        "anthropic",
        "claude-sonnet-4-6",
        1_000_000,
        0,
        spike_after_until,
    );

    let srv = spawn_server(dir.path()).await;
    let body: serde_json::Value = reqwest::get(format!(
        "{}/api/usage/outliers?since=2026-05-01T00:00:00Z&until=2026-07-01T00:00:00Z",
        srv.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let rows = body.as_array().expect("flat array of outlier rows");
    let ids: Vec<&str> = rows.iter().map(|r| r["run_id"].as_str().unwrap()).collect();
    assert!(
        ids.contains(&"spike_in_window"),
        "the in-window spike must be flagged: {ids:?}"
    );
    assert!(
        !ids.contains(&"spike_after_until"),
        "the spike started after `until` must be excluded from the window entirely: {ids:?}"
    );
}

#[tokio::test]
async fn usage_outliers_unparseable_until_returns_400() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!(
        "{}/api/usage/outliers?until=notadate",
        srv.base_url
    ))
    .await
    .unwrap();
    assert_eq!(
        resp.status(),
        400,
        "an unparseable `until` must 400, not silently fall back to now"
    );
}

// ---------------------------------------------------------------------------
// Part D: every usage surface reads the fold (live usage ledger, Plan 1
// Task 9). Aggregates also count standalone agent runs and session turns —
// each transcript exactly once — and single-entity surfaces (a run's
// timeline, a session, an agent run) are live and include dispatch children.
// ---------------------------------------------------------------------------

const FOLD_PROVIDER: &str = "anthropic";
const FOLD_MODEL: &str = "claude-sonnet-4-6";

/// A transcript: `RunStart` for `agent` in `workspace_id`, then one `Usage`
/// event per `(input, output)` pair. Built from serialized
/// `rupu_transcript::Event` values.
fn write_fold_transcript(
    path: &std::path::Path,
    agent: &str,
    workspace_id: &str,
    usages: &[(u32, u32)],
) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut events = vec![rupu_transcript::Event::RunStart {
        codename: None,
        run_id: path.file_stem().unwrap().to_string_lossy().into_owned(),
        workspace_id: workspace_id.into(),
        agent: agent.into(),
        provider: FOLD_PROVIDER.into(),
        model: FOLD_MODEL.into(),
        started_at: chrono::Utc::now(),
        mode: rupu_transcript::RunMode::Ask,
        schema: None,
        system_prompt: None,
    }];
    for (input, output) in usages {
        events.push(rupu_transcript::Event::Usage {
            provider: FOLD_PROVIDER.into(),
            model: FOLD_MODEL.into(),
            served_model: None,
            input_tokens: *input,
            output_tokens: *output,
            cached_tokens: 0,
            cache_write_tokens: 0,
            purpose: None,
        });
    }
    let mut buf = Vec::new();
    for ev in &events {
        buf.extend(serde_json::to_vec(ev).unwrap());
        buf.push(b'\n');
    }
    std::fs::write(path, &buf).unwrap();
}

/// A standalone run's `<run_id>.meta.json` sidecar (the fields the CP reads).
fn write_standalone_meta(
    transcripts_dir: &std::path::Path,
    run_id: &str,
    session_id: Option<&str>,
    trigger_source: &str,
) {
    std::fs::create_dir_all(transcripts_dir).unwrap();
    let meta = serde_json::json!({
        "run_id": run_id,
        "session_id": session_id,
        "trigger_source": trigger_source,
    });
    std::fs::write(
        transcripts_dir.join(format!("{run_id}.meta.json")),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
}

/// A `session.json` under `<global>/sessions/<id>/` whose own token totals
/// are zero (a turn still in flight) and whose `runs[]` name `turns`
/// (`(run_id, transcript_path)`).
fn write_session(global: &std::path::Path, id: &str, turns: &[(&str, &std::path::Path)]) {
    let dir = global.join("sessions").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let runs: Vec<serde_json::Value> = turns
        .iter()
        .map(|(run_id, path)| {
            serde_json::json!({
                "run_id": run_id,
                "prompt": "go",
                "transcript_path": path.to_str().unwrap(),
                "started_at": chrono::Utc::now(),
                "total_tokens_in": 0,
                "total_tokens_out": 0,
                "total_tokens_cached": 0,
            })
        })
        .collect();
    let payload = serde_json::json!({
        "session_id": id,
        "agent_name": "chatter",
        "model": FOLD_MODEL,
        "provider_name": FOLD_PROVIDER,
        "status": "active",
        "total_turns": 0,
        "total_tokens_in": 0,
        "total_tokens_out": 0,
        "total_tokens_cached": 0,
        "created_at": chrono::Utc::now(),
        "updated_at": chrono::Utc::now(),
        "workspace_id": "ws_sess",
        "runs": runs,
    });
    std::fs::write(
        dir.join("session.json"),
        serde_json::to_vec(&payload).unwrap(),
    )
    .unwrap();
}

/// A workflow run record in `<global>/runs`, started now.
fn create_workflow_run(
    global: &std::path::Path,
    run_id: &str,
    status: rupu_orchestrator::RunStatus,
) -> rupu_orchestrator::runs::RunStore {
    let run_store = rupu_orchestrator::runs::RunStore::new(global.join("runs"));
    let record = rupu_orchestrator::RunRecord {
        codename: None,
        id: run_id.into(),
        workflow_name: "wf-fold".into(),
        status,
        inputs: std::collections::BTreeMap::new(),
        event: None,
        workspace_id: "ws_wf".into(),
        workspace_path: std::path::PathBuf::from("/tmp/proj"),
        transcript_dir: global.join("transcripts"),
        started_at: chrono::Utc::now(),
        finished_at: None,
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
        reject_cleanup_pending: None,
        permission_mode: None,
        final_output: None,
        loop_progress: Default::default(),
    };
    run_store.create(record, "name: wf-fold\n").unwrap();
    run_store
}

fn step_result(
    run_id: &str,
    step_id: &str,
    transcript_path: std::path::PathBuf,
) -> rupu_orchestrator::runs::StepResultRecord {
    rupu_orchestrator::runs::StepResultRecord {
        codename: None,
        run_outcome: None,
        step_id: step_id.into(),
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
        loop_iteration: None,
        host: None,
    }
}

async fn get_json(url: String) -> serde_json::Value {
    let resp = reqwest::get(&url).await.unwrap();
    assert!(resp.status().is_success(), "GET {url}: {}", resp.status());
    resp.json().await.unwrap()
}

#[tokio::test]
async fn usage_endpoint_counts_inflight_step_from_ledger() {
    use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let store = create_workflow_run(global, "run_LIVE", rupu_orchestrator::RunStatus::Running);
    // The step is still running: no step_results yet, and its transcript is
    // not readable from here — the ledger alone carries its spend.
    let unseen = global.join("elsewhere").join("run_LIVE_step.jsonl");
    let row = |id: &str, input: u64, output: u64| LedgerRow {
        v: LEDGER_VERSION,
        id: id.into(),
        at: chrono::Utc::now(),
        kind: LedgerKind::Turn,
        step_id: Some("build".into()),
        unit_index: None,
        unit_key: None,
        agent_run_id: "run_LIVE_step".into(),
        parent_agent_run_id: None,
        transcript: unseen.clone(),
        agent: "builder".into(),
        provider: FOLD_PROVIDER.into(),
        model: FOLD_MODEL.into(),
        input_tokens: input,
        output_tokens: output,
        cached_tokens: 0,
        cache_write_tokens: 0,
    };
    let mut ledger = Vec::new();
    for r in [row("u1", 1000, 100), row("u2", 2000, 200)] {
        ledger.extend(serde_json::to_vec(&r).unwrap());
        ledger.push(b'\n');
    }
    std::fs::write(store.usage_ledger_path("run_LIVE"), ledger).unwrap();

    let srv = spawn_server(global).await;
    let body = get_json(format!("{}/api/usage?host=local", srv.base_url)).await;
    assert_eq!(
        body["summary"]["total_tokens"].as_u64(),
        Some(3300),
        "the in-flight step's ledger rows are the run's spend: {body}"
    );
}

#[tokio::test]
async fn usage_endpoint_includes_standalone_and_session_transcripts_once() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let tdir = global.join("transcripts");

    // S: a standalone `rupu run`.
    write_fold_transcript(&tdir.join("run_S.jsonl"), "solo", "ws_s", &[(100, 10)]);
    write_standalone_meta(&tdir, "run_S", None, "run_cli");

    // T: a session turn, recorded both as a standalone transcript (+ meta
    // naming the session) and in the session's `runs[]`.
    let t_path = tdir.join("run_T.jsonl");
    write_fold_transcript(&t_path, "chatter", "ws_sess", &[(200, 20)]);
    write_standalone_meta(&tdir, "run_T", Some("ses_T"), "session_turn");
    write_session(global, "ses_T", &[("run_T", &t_path)]);

    // W: a workflow run whose step transcript also sits in the global
    // transcripts dir — it belongs to the workflow, never to "standalone".
    let w_path = tdir.join("run_W.jsonl");
    write_fold_transcript(&w_path, "reviewer", "ws_wf", &[(400, 40)]);
    let store = create_workflow_run(global, "run_WF", rupu_orchestrator::RunStatus::Completed);
    store
        .append_step_result("run_WF", &step_result("run_WF", "review", w_path))
        .unwrap();

    let srv = spawn_server(global).await;
    let body = get_json(format!("{}/api/usage?host=local", srv.base_url)).await;
    assert_eq!(
        body["summary"]["total_tokens"].as_u64(),
        Some(110 + 220 + 440),
        "S + T + W, each transcript counted once: {body}"
    );

    let rows = get_json(format!("{}/api/usage/runs", srv.base_url)).await;
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 3, "one row per source: {rows:?}");
    let kind_of = |id: &str| {
        rows.iter()
            .find(|r| r["run_id"] == id)
            .unwrap_or_else(|| panic!("row {id} missing: {rows:?}"))["kind"]
            .clone()
    };
    assert_eq!(kind_of("run_WF"), "workflow");
    assert_eq!(kind_of("run_S"), "agent");
    assert_eq!(kind_of("run_T"), "session");
    let s_row = rows.iter().find(|r| r["run_id"] == "run_S").unwrap();
    assert_eq!(
        s_row["workflow_name"], "",
        "a standalone row has no workflow (\"\", an additive-only wire change): {s_row}"
    );
    assert_eq!(s_row["total_tokens"].as_u64(), Some(110));
    assert_eq!(s_row["host_id"], "local");
    assert_eq!(s_row["workspace_id"], "ws_s");

    // The timeline buckets the same three sources.
    let buckets = get_json(format!("{}/api/usage/timeline", srv.base_url)).await;
    let total: u64 = buckets
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["rows"].as_array().unwrap().clone())
        .map(|r| r["total_tokens"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 770, "timeline includes S + T + W once: {buckets}");
}

#[tokio::test]
async fn run_usage_timeline_includes_inflight_points() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let store = create_workflow_run(global, "run_TL", rupu_orchestrator::RunStatus::Running);
    let t_path = global.join("proj").join("run_TL_build.jsonl");
    write_fold_transcript(&t_path, "builder", "ws_wf", &[(10, 1), (20, 2)]);
    let ev = rupu_orchestrator::executor::Event::StepWorking {
        run_id: "run_TL".into(),
        step_id: "build".into(),
        note: None,
        transcript_path: Some(t_path),
    };
    let mut line = serde_json::to_vec(&ev).unwrap();
    line.push(b'\n');
    std::fs::write(store.events_path("run_TL"), line).unwrap();

    let srv = spawn_server(global).await;
    let points = get_json(format!("{}/api/runs/run_TL/usage-timeline", srv.base_url)).await;
    let points = points.as_array().expect("array");
    assert_eq!(points.len(), 2, "both in-flight turns: {points:?}");
    assert!(points.iter().all(|p| p["label"] == "build"), "{points:?}");
    assert_eq!(points[1]["tokens_in"].as_u64(), Some(20));
}

#[tokio::test]
async fn session_usage_is_live_from_transcripts() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let t_path = global
        .join("proj")
        .join(".rupu")
        .join("transcripts")
        .join("run_L.jsonl");
    write_fold_transcript(&t_path, "chatter", "ws_sess", &[(300, 30), (5, 1)]);
    write_session(global, "ses_L", &[("run_L", &t_path)]);

    let srv = spawn_server(global).await;
    let body = get_json(format!("{}/api/sessions/ses_L", srv.base_url)).await;
    assert_eq!(
        body["usage"]["total_tokens"].as_u64(),
        Some(336),
        "session usage is the transcripts' live sum, not session.json's zero totals: {body}"
    );
    assert_eq!(body["usage"]["runs"].as_u64(), Some(1));

    let points = get_json(format!(
        "{}/api/sessions/ses_L/usage-timeline",
        srv.base_url
    ))
    .await;
    assert_eq!(points.as_array().map(Vec::len), Some(2), "{points}");
}

#[tokio::test]
async fn agent_run_usage_includes_dispatch_children() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let tdir = global.join("transcripts");
    write_fold_transcript(&tdir.join("run_A.jsonl"), "lead", "ws_a", &[(1000, 100)]);
    write_standalone_meta(&tdir, "run_A", None, "run_cli");
    let child = global
        .join("runs")
        .join("run_A")
        .join("sub")
        .join("sub_1")
        .join("transcript.jsonl");
    write_fold_transcript(&child, "helper", "ws_a", &[(50, 5)]);

    let srv = spawn_server(global).await;
    let rows = get_json(format!("{}/api/runs/agents?host=local", srv.base_url)).await;
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["run_id"] == "run_A")
        .unwrap_or_else(|| panic!("run_A row missing: {rows}"))
        .clone();
    assert_eq!(
        row["usage"]["total_tokens"].as_u64(),
        Some(1100 + 55),
        "the agent run's usage includes its dispatched child: {row}"
    );
    assert_eq!(row["turns"].as_u64(), Some(2));

    // The Usage page counts the child too.
    let body = get_json(format!("{}/api/usage?host=local", srv.base_url)).await;
    assert_eq!(
        body["summary"]["total_tokens"].as_u64(),
        Some(1155),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// Part E: the entity rollups and outliers count standalone agent runs and
// session turns too — each transcript once (Task 9 fix round 1).
// ---------------------------------------------------------------------------

fn write_agent_md(global: &std::path::Path, name: &str) {
    let dir = global.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: agent {name}\nprovider: {FOLD_PROVIDER}\nmodel: {FOLD_MODEL}\n---\n\nYou are {name}.\n"
        ),
    )
    .unwrap();
}

fn write_workspace(global: &std::path::Path, id: &str) {
    let dir = global.join("workspaces");
    std::fs::create_dir_all(&dir).unwrap();
    let path = global.join("proj");
    std::fs::write(
        dir.join(format!("{id}.toml")),
        format!(
            "id = \"{id}\"\npath = \"{}\"\ncreated_at = \"2026-01-01T00:00:00Z\"\n",
            path.display()
        ),
    )
    .unwrap();
}

/// A standalone `rupu run` transcript + its meta in `<global>/transcripts`.
fn write_standalone_run(
    global: &std::path::Path,
    run_id: &str,
    agent: &str,
    workspace_id: &str,
    usages: &[(u32, u32)],
) {
    let tdir = global.join("transcripts");
    write_fold_transcript(
        &tdir.join(format!("{run_id}.jsonl")),
        agent,
        workspace_id,
        usages,
    );
    write_standalone_meta(&tdir, run_id, None, "run_cli");
}

/// A completed workflow run (workspace `ws_wf`) whose one step transcript sits
/// in `<global>/transcripts` — claimed by the run, never standalone spend.
fn write_workflow_run_in_global_transcripts(
    global: &std::path::Path,
    run_id: &str,
    agent: &str,
    usages: &[(u32, u32)],
) {
    let t_path = global
        .join("transcripts")
        .join(format!("{run_id}_step.jsonl"));
    write_fold_transcript(&t_path, agent, "ws_wf", usages);
    let store = create_workflow_run(global, run_id, rupu_orchestrator::RunStatus::Completed);
    store
        .append_step_result(run_id, &step_result(run_id, "review", t_path))
        .unwrap();
}

#[tokio::test]
async fn agent_and_project_rollups_include_standalone_spend_once() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    write_agent_md(global, "solo");
    write_agent_md(global, "reviewer");
    write_workspace(global, "ws_wf");
    write_standalone_run(global, "run_S1", "solo", "ws_wf", &[(100, 10)]);
    write_workflow_run_in_global_transcripts(global, "run_WF", "reviewer", &[(400, 40)]);

    let srv = spawn_server(global).await;

    // Per-agent: the standalone run counts for its agent, the workflow step
    // transcript counts once — and both match `/api/usage?group_by=agent`.
    let agents = get_json(format!("{}/api/agents", srv.base_url)).await;
    let agent_total = |name: &str| {
        agents
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap_or_else(|| panic!("agent {name} missing: {agents}"))["usage"]["total_tokens"]
            .as_u64()
    };
    let by_agent = get_json(format!(
        "{}/api/usage?host=local&group_by=agent",
        srv.base_url
    ))
    .await;
    let usage_total = |name: &str| {
        by_agent["breakdown"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["agent"] == name)
            .and_then(|r| r["total_tokens"].as_u64())
    };
    assert_eq!(agent_total("solo"), Some(110), "{agents}");
    assert_eq!(agent_total("solo"), usage_total("solo"), "{by_agent}");
    assert_eq!(
        agent_total("reviewer"),
        Some(440),
        "not double counted: {agents}"
    );
    assert_eq!(
        agent_total("reviewer"),
        usage_total("reviewer"),
        "{by_agent}"
    );

    // Per-project: list and detail both include the standalone run's spend,
    // the workflow step once; the run count stays the workflow-run count.
    let projects = get_json(format!("{}/api/projects", srv.base_url)).await;
    let p = projects
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["ws_id"] == "ws_wf")
        .unwrap_or_else(|| panic!("project missing: {projects}"))
        .clone();
    assert_eq!(p["usage"]["total_tokens"].as_u64(), Some(550), "{p}");
    assert_eq!(p["run_count"].as_u64(), Some(1), "{p}");
    // Every contributing model is priced, so the rollup is priced and costed
    // (an empty `EntityRollup` must start priced, like `rollup(empty)`).
    assert_eq!(p["usage"]["priced"], true, "{p}");
    assert!(p["usage"]["cost_usd"].as_f64().is_some(), "{p}");
    let detail = get_json(format!("{}/api/projects/ws_wf", srv.base_url)).await;
    assert_eq!(
        detail["usage"]["total_tokens"].as_u64(),
        Some(550),
        "{detail}"
    );
}

#[tokio::test]
async fn workflow_list_rollup_is_priced_for_priced_models() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    write_agent_md(global, "reviewer");
    let wf_dir = global.join("workflows");
    std::fs::create_dir_all(&wf_dir).unwrap();
    std::fs::write(
        wf_dir.join("wf-fold.yaml"),
        "name: wf-fold\nsteps:\n  - id: review\n    agent: reviewer\n    prompt: hi\n",
    )
    .unwrap();
    write_workflow_run_in_global_transcripts(global, "run_WF", "reviewer", &[(400, 40)]);

    let srv = spawn_server(global).await;
    let rows = get_json(format!("{}/api/workflows", srv.base_url)).await;
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "wf-fold")
        .unwrap_or_else(|| panic!("workflow row missing: {rows}"))
        .clone();
    assert_eq!(row["usage"]["total_tokens"].as_u64(), Some(440), "{row}");
    assert_eq!(row["run_count"].as_u64(), Some(1), "{row}");
    assert_eq!(row["usage"]["priced"], true, "{row}");
    assert!(row["usage"]["cost_usd"].as_f64().is_some(), "{row}");
}

#[tokio::test]
async fn outliers_include_standalone_runs_and_never_double_count() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    for id in ["run_C1", "run_C2", "run_C3"] {
        write_standalone_run(global, id, "solo", "ws_a", &[(1000, 0)]);
    }
    write_standalone_run(global, "run_SPIKE", "solo", "ws_a", &[(10_000, 0)]);
    // An expensive workflow step by the same agent, in the global transcripts
    // dir: if it leaked into the standalone population it would be a second
    // "solo" outlier.
    write_workflow_run_in_global_transcripts(global, "run_WF", "solo", &[(50_000, 0)]);

    let srv = spawn_server(global).await;
    let out = get_json(format!("{}/api/usage/outliers", srv.base_url)).await;
    let out = out.as_array().expect("array");
    assert_eq!(out.len(), 1, "only the standalone spike: {out:?}");
    assert_eq!(out[0]["run_id"], "run_SPIKE");
    assert_eq!(out[0]["kind"], "agent");
    assert_eq!(out[0]["agent"], "solo");
    assert_eq!(out[0]["workflow_name"], "");
}

/// Outlier rows carry the target the UI links them to: a standalone run its
/// own transcript, a session turn its session (and transcript), a workflow run
/// neither (it links to `/runs/:id`, which standalone/session runs never have).
#[tokio::test]
async fn outlier_rows_carry_their_link_targets() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let now = chrono::Utc::now();

    // Standalone agent runs of `solo`: three baseline + one spike.
    for id in ["run_A1", "run_A2", "run_A3"] {
        write_standalone_run(global, id, "solo", "ws_a", &[(1000, 0)]);
    }
    write_standalone_run(global, "run_ASPIKE", "solo", "ws_a", &[(10_000, 0)]);

    // Session turns of `chatty` in session `sess_S1`: three baseline + a spike.
    let tdir = global.join("transcripts");
    for (id, tokens) in [
        ("run_T1", 1000),
        ("run_T2", 1000),
        ("run_T3", 1000),
        ("run_TSPIKE", 10_000),
    ] {
        write_fold_transcript(
            &tdir.join(format!("{id}.jsonl")),
            "chatty",
            "ws_a",
            &[(tokens, 0)],
        );
        write_standalone_meta(&tdir, id, Some("sess_S1"), "session_turn");
    }

    // A workflow: three baseline runs + a spike.
    for (i, tokens) in [100_000, 100_000, 100_000, 1_000_000]
        .into_iter()
        .enumerate()
    {
        seed_run_with_usage(
            global,
            &format!("run_W{i}"),
            "wf",
            "ws_a",
            "anthropic",
            "claude-sonnet-4-6",
            tokens,
            0,
            now - chrono::Duration::hours(1),
        );
    }

    let srv = spawn_server(global).await;
    let out = get_json(format!("{}/api/usage/outliers", srv.base_url)).await;
    let out = out.as_array().expect("array");
    let row = |id: &str| {
        out.iter()
            .find(|r| r["run_id"] == id)
            .unwrap_or_else(|| panic!("{id} not flagged: {out:?}"))
    };

    let agent = row("run_ASPIKE");
    assert_eq!(agent["kind"], "agent");
    let agent_path = agent["transcript_path"]
        .as_str()
        .expect("agent transcript_path");
    assert!(agent_path.ends_with("run_ASPIKE.jsonl"), "{agent}");
    assert!(agent.get("session_id").is_none(), "no session: {agent}");

    let session = row("run_TSPIKE");
    assert_eq!(session["kind"], "session");
    assert_eq!(session["session_id"], "sess_S1", "{session}");
    let session_path = session["transcript_path"]
        .as_str()
        .expect("session transcript_path");
    assert!(session_path.ends_with("run_TSPIKE.jsonl"), "{session}");

    let wf = row("run_W3");
    assert_eq!(wf["kind"], "workflow");
    assert!(wf.get("session_id").is_none(), "workflow row: {wf}");
    assert!(wf.get("transcript_path").is_none(), "workflow row: {wf}");
}

// ---------------------------------------------------------------------------
// Part E: `GET /api/runs/:id/usage` — the live run usage endpoint (live usage
// ledger, Plan 2 Task 3): summary, per-step summaries, turns, partial, a
// STRING epoch, and an append-only incremental `points` series.
// ---------------------------------------------------------------------------

/// One serialized `LedgerRow` line (a turn of step `step`).
fn ledger_line(id: &str, step: &str, input: u64, output: u64) -> Vec<u8> {
    use rupu_orchestrator::usage_ledger::{LedgerKind, LedgerRow, LEDGER_VERSION};
    let row = LedgerRow {
        v: LEDGER_VERSION,
        id: id.into(),
        at: chrono::Utc::now(),
        kind: LedgerKind::Turn,
        step_id: Some(step.into()),
        unit_index: None,
        unit_key: None,
        agent_run_id: format!("run_AGENT_{step}"),
        parent_agent_run_id: None,
        transcript: std::path::PathBuf::from(format!("/nowhere/run_AGENT_{step}.jsonl")),
        agent: "builder".into(),
        provider: FOLD_PROVIDER.into(),
        model: FOLD_MODEL.into(),
        input_tokens: input,
        output_tokens: output,
        cached_tokens: 0,
        cache_write_tokens: 0,
    };
    let mut line = serde_json::to_vec(&row).unwrap();
    line.push(b'\n');
    line
}

fn append_bytes(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    f.write_all(bytes).unwrap();
}

#[tokio::test]
async fn run_usage_endpoint_serves_summary_steps_and_incremental_points() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let store = create_workflow_run(global, "run_LU", rupu_orchestrator::RunStatus::Running);
    let ledger = store.usage_ledger_path("run_LU");
    for (id, step, input) in [("u1", "a", 10), ("u2", "a", 20), ("u3", "b", 30)] {
        append_bytes(&ledger, &ledger_line(id, step, input, 1));
    }

    let srv = spawn_server(global).await;
    let v = get_json(format!("{}/api/runs/run_LU/usage", srv.base_url)).await;
    assert_eq!(v["summary"]["total_tokens"], 63, "{v}");
    assert_eq!(v["steps"]["a"]["input_tokens"], 30, "{v}");
    assert_eq!(v["steps"]["b"]["input_tokens"], 30, "{v}");
    assert_eq!(v["turns"], 3, "{v}");
    assert_eq!(v["partial"], false, "{v}");
    assert_eq!(v["points_from"], 0, "{v}");
    assert_eq!(v["points"].as_array().unwrap().len(), 3, "{v}");
    let epoch = v["epoch"]
        .as_str()
        .expect("epoch is a decimal string (u64 beyond JS's 2^53)")
        .to_string();
    assert!(epoch.parse::<u64>().is_ok(), "epoch {epoch:?}");

    // One more row → an incremental fetch returns only the new point.
    append_bytes(&ledger, &ledger_line("u4", "b", 40, 1));
    let v2 = get_json(format!(
        "{}/api/runs/run_LU/usage?since=3&epoch={epoch}",
        srv.base_url
    ))
    .await;
    assert_eq!(
        v2["epoch"],
        epoch.as_str(),
        "pure growth keeps the epoch: {v2}"
    );
    assert_eq!(v2["points_from"], 3, "{v2}");
    let pts = v2["points"].as_array().unwrap();
    assert_eq!(pts.len(), 1, "{v2}");
    assert_eq!(pts[0]["turn"], 4, "{v2}");
    assert_eq!(pts[0]["label"], "b", "{v2}");
    assert_eq!(pts[0]["tokens_in"], 40, "{v2}");
    assert_eq!(v2["summary"]["total_tokens"], 104, "{v2}");
    assert_eq!(v2["steps"]["b"]["input_tokens"], 70, "{v2}");

    // Caught up: `since == len` is an empty tail, not a full resend.
    let v_tail = get_json(format!(
        "{}/api/runs/run_LU/usage?since=4&epoch={epoch}",
        srv.base_url
    ))
    .await;
    assert_eq!(v_tail["points_from"], 4, "{v_tail}");
    assert_eq!(v_tail["points"].as_array().unwrap().len(), 0, "{v_tail}");

    // A wrong epoch, a `since` past the end, or a `since` without an epoch →
    // the full series from 0.
    for q in [
        "since=3&epoch=1".to_string(),
        format!("since=9&epoch={epoch}"),
        "since=3".to_string(),
    ] {
        let v3 = get_json(format!("{}/api/runs/run_LU/usage?{q}", srv.base_url)).await;
        assert!(v3["epoch"].is_string(), "{q}: {v3}");
        assert_eq!(v3["points_from"], 0, "{q}: {v3}");
        assert_eq!(v3["points"].as_array().unwrap().len(), 4, "{q}: {v3}");
    }
}

#[tokio::test]
async fn run_usage_endpoint_404s_for_unknown_run() {
    let dir = tempfile::tempdir().unwrap();
    let srv = spawn_server(dir.path()).await;
    let resp = reqwest::get(format!("{}/api/runs/run_nope/usage", srv.base_url))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn run_usage_endpoint_serves_a_project_local_run() {
    // A run under a registered project's `.rupu/runs` resolves through
    // `resolve_run_location` exactly like the usage-timeline endpoint.
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path();
    let proj = tempfile::tempdir().unwrap();
    let store = create_workflow_run(
        &proj.path().join(".rupu"),
        "run_PL",
        rupu_orchestrator::RunStatus::Completed,
    );
    append_bytes(
        &store.usage_ledger_path("run_PL"),
        &ledger_line("p1", "review", 7, 3),
    );
    let ws_dir = global.join("workspaces");
    std::fs::create_dir_all(&ws_dir).unwrap();
    std::fs::write(
        ws_dir.join("ws_pl.toml"),
        format!(
            "id = \"ws_pl\"\npath = \"{}\"\ncreated_at = \"2026-09-30T00:00:00Z\"\n",
            proj.path().display()
        ),
    )
    .unwrap();

    let srv = spawn_server(global).await;
    let v = get_json(format!("{}/api/runs/run_PL/usage", srv.base_url)).await;
    assert_eq!(v["summary"]["total_tokens"], 10, "{v}");
    assert_eq!(v["steps"]["review"]["output_tokens"], 3, "{v}");
}
