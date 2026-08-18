//! Two different claims, two different tests — do not conflate them.
//!
//! `a_flow_reaches_both_the_ledger_and_the_transcript` proves that
//! `FanoutSink` + `TranscriptSink` + `NetflowWriterHandle` compose
//! correctly: a flow recorded through a hand-built `FanoutSink` (passed
//! explicitly to `rupu_netflow::http::client_with`) lands in the ledger,
//! the transcript, AND a plain observer. It does NOT exercise `rupu run`
//! or `crate::netflow_sink::for_run` at all.
//!
//! `rupu_run_through_a_real_http_client_populates_the_project_rooted_ledger`
//! is the one that actually proves the wiring: it drives the real
//! `rupu_cli::run(...)` entry point end to end — real `run_inner`, real
//! `crate::netflow_sink::for_run` call, a real (migrated)
//! `OpenAiCompatibleClient` provider constructed by the real
//! `provider_factory`, making a real HTTP request against a local
//! `httpmock` server — and asserts the resulting per-run ledger file
//! exists, rooted at the *project*, not the invoking (nested) `pwd`. This
//! is the test that would catch a regression moving the sink build after
//! the provider build, or reverting `paths::netflow_dir` back to raw `pwd`.

use assert_fs::prelude::*;
use rupu_netflow::{FlowCtx, MemorySink, Origin};
use std::sync::Arc;
use tokio::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

/// Proves `FanoutSink`/`TranscriptSink`/`NetflowWriterHandle` compose —
/// see the module doc comment for what this test does and does NOT prove.
#[tokio::test]
async fn a_flow_reaches_both_the_ledger_and_the_transcript() {
    use rupu_netflow::{FanoutSink, NetflowPaths, NetflowWriterHandle};
    use rupu_transcript::{JsonlWriter, TranscriptSink};

    let tmp = tempfile::TempDir::new().unwrap();
    let transcript = tmp.path().join("transcript.jsonl");
    JsonlWriter::create(&transcript).unwrap();

    let paths = NetflowPaths::for_run(tmp.path(), "run-x");
    let handle = NetflowWriterHandle::spawn(paths.clone()).unwrap();

    let observed = Arc::new(MemorySink::default());
    let fanout = FanoutSink::new(vec![
        handle.writer.clone(),
        Arc::new(TranscriptSink::new(transcript.clone())),
        observed.clone(),
    ]);

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET).path("/ping");
            then.status(200).body("pong");
        })
        .await;

    let client = rupu_netflow::http::client_with(
        FlowCtx {
            run_id: Some("run-x".into()),
            step_id: None,
            agent: None,
            workspace_id: None,
            origin: Origin::Provider("anthropic".into()),
        },
        reqwest::Client::builder(),
        Arc::new(fanout),
    )
    .unwrap();

    client.get(server.url("/ping")).send().await.unwrap();
    handle.shutdown().await;

    // Ledger
    let flows = rupu_netflow::ledger::read_flows(&paths.flows).unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0].ctx.run_id.as_deref(), Some("run-x"));

    // Transcript
    let text = std::fs::read_to_string(&transcript).unwrap();
    assert!(text.contains(r#""type":"net_flow""#));

    // And the observer saw it too — fanout reaches every child.
    assert_eq!(observed.records().len(), 1);
}

/// The decisive test: drives the REAL `rupu run` entry point (not a
/// hand-built sink) against a real, migrated `OpenAiCompatibleClient`
/// provider talking to a local `httpmock` server, and asserts the ledger
/// gets populated at the project root — not at the nested directory
/// `rupu run` was actually invoked from.
///
/// There is no process-global sink any more (see `crate::netflow_sink::
/// for_run`) so a second test calling `rupu_cli::run` in this binary would
/// not race this one's sink — `ENV_LOCK` below only serializes the
/// process-wide `set_current_dir`/env-var mutations, not sink resolution.
#[tokio::test(flavor = "multi_thread")]
async fn rupu_run_through_a_real_http_client_populates_the_project_rooted_ledger() {
    let _guard = ENV_LOCK.lock().await;

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "id": "cmpl_1",
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hello from mock"},
                        "finish_reason": "stop"
                    }]
                }));
        })
        .await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    // No `provider:`/`model:` pinned — resolves through
    // `[providers.netflowmock]` below, same path
    // `dispatch_resolves_openai_compatible_provider_default_model`
    // (crates/rupu-cli/src/cmd/dispatch.rs) already exercises.
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: netflowmock\nmaxTurns: 1\n---\nyou echo.")
        .unwrap();
    global
        .child("config.toml")
        .write_str(&format!(
            "[providers.netflowmock]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\ndefault_model = \"mock-model\"\nstream = false\n",
            server.base_url(),
        ))
        .unwrap();

    // Project root: a `.rupu/` marker directory is all `project_root_for`
    // requires. `rupu run` is invoked from a NESTED subdirectory of it —
    // the exact "common invocation pattern" fix-round-1 Finding 1 flagged.
    // `.rupu/netflow/` is pre-created too — `paths::netflow_dir` only
    // routes project-locally when that directory ALREADY exists (same
    // opt-in gate as `transcripts_dir`), so without this the ledger would
    // fall back to global and this test would no longer be exercising the
    // project-vs-nested-pwd routing it exists to prove.
    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu/netflow").create_dir_all().unwrap();
    let nested_pwd = project.child("src/deep/nested");
    nested_pwd.create_dir_all().unwrap();

    // Hermetic credential store — a nonexistent file is a valid empty
    // store (`KeychainResolver::read_file_map`), so `netflowmock`'s
    // credential resolves purely from the `RUPU_NETFLOWMOCK_API_KEY`
    // env-var fallback below, never touching a real keychain or a real
    // `~/.rupu/auth.json`.
    let auth_file = tmp.child("auth.json");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_AUTH_FILE", auth_file.path());
    std::env::set_var("RUPU_NETFLOWMOCK_API_KEY", "test-key");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::set_current_dir(nested_pwd.path()).unwrap();

    // Pinned so the test can predict the per-run ledger's filename
    // (`NetflowPaths::for_run` names it `<run_id>.jsonl`) without having
    // to discover a ULID `run_inner` minted internally.
    let run_id = "run-netflow-fixed-id";

    let exit = rupu_cli::run(vec![
        "rupu".into(),
        "run".into(),
        "echo".into(),
        "--mode".into(),
        "bypass".into(),
        "--run-id".into(),
        run_id.into(),
        "say hi".into(),
    ])
    .await;

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_NETFLOWMOCK_API_KEY");
    std::env::remove_var("RUPU_AUTH_FILE");
    std::env::remove_var("RUPU_HOME");

    assert_eq!(
        format!("{exit:?}"),
        format!("{:?}", std::process::ExitCode::from(0)),
        "rupu run should exit 0 against the mocked openai-compatible endpoint"
    );

    // Poll for the background ledger writer task to land the write —
    // `rupu run` (like `crate::netflow_sink::for_run`) doesn't wait on an
    // explicit ledger `shutdown()` before returning.
    let ledger_path = project
        .path()
        .join(".rupu/netflow")
        .join(format!("{run_id}.jsonl"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let text = loop {
        if let Ok(t) = std::fs::read_to_string(&ledger_path) {
            if !t.trim().is_empty() {
                break t;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "netflow ledger at {ledger_path:?} was never written"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };

    assert!(
        text.contains(r#""path":"/v1/chat/completions""#),
        "expected the real provider request in the ledger, got: {text}"
    );
    // `OpenAiCompatibleClient::new` attributes every openai-compatible
    // provider generically as `"openai_compatible"` regardless of the
    // config-declared name ("netflowmock" here) — see
    // `crates/rupu-providers/src/openai_compatible.rs`.
    assert!(
        text.contains(r#""kind":"provider","name":"openai_compatible""#),
        "expected provider-attributed origin, got: {text}"
    );

    // The ledger must NOT fragment into the nested invocation directory.
    assert!(
        !nested_pwd
            .path()
            .join(".rupu/netflow")
            .join(format!("{run_id}.jsonl"))
            .exists(),
        "netflow must not fragment into the nested pwd when a project root exists"
    );
}

/// The actual regression guard for the class of bug this whole plan
/// exists to kill, driven through a REAL entry point rather than calling
/// `netflow_sink::for_run` directly (that narrower claim belongs to
/// `crate::netflow_sink::tests::two_runs_in_one_process_get_separate_
/// ledgers`, `crates/rupu-cli/src/netflow_sink.rs` — see its own comment
/// for what it does and does not prove). A process-global sink built
/// once — the exact shape of the deleted `OnceLock` — would route the
/// second `rupu run` invocation's flow into the first run's ledger file
/// (or the reverse, depending on which one happened to install last).
/// `rupu session`'s per-turn worker and `DefaultStepFactory`'s per-step
/// build follow the identical "build fresh every call, never cache"
/// pattern `run_inner` uses here; this test exercises `run_inner`'s own
/// copy of that pattern twice in one process, which is what "two runs in
/// one process" concretely means for a CLI binary.
#[tokio::test(flavor = "multi_thread")]
async fn two_sequential_rupu_run_invocations_in_one_process_get_separate_ledgers() {
    let _guard = ENV_LOCK.lock().await;

    let server = httpmock::MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "id": "cmpl_1",
                    "model": "mock-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hello from mock"},
                        "finish_reason": "stop"
                    }]
                }));
        })
        .await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    global.create_dir_all().unwrap();
    global.child("agents").create_dir_all().unwrap();
    global
        .child("agents/echo.md")
        .write_str("---\nname: echo\nprovider: netflowmock\nmaxTurns: 1\n---\nyou echo.")
        .unwrap();
    global
        .child("config.toml")
        .write_str(&format!(
            "[providers.netflowmock]\nkind = \"openai-compatible\"\nbase_url = \"{}\"\ndefault_model = \"mock-model\"\nstream = false\n",
            server.base_url(),
        ))
        .unwrap();

    // One shared project for both runs -- `.rupu/netflow/` pre-created so
    // both land project-locally, matching `NetflowPaths::for_run`'s
    // "one file per run id" contract under the SAME netflow directory.
    let project = assert_fs::TempDir::new().unwrap();
    project.child(".rupu/netflow").create_dir_all().unwrap();

    let auth_file = tmp.child("auth.json");

    std::env::set_var("RUPU_HOME", global.path());
    std::env::set_var("RUPU_AUTH_FILE", auth_file.path());
    std::env::set_var("RUPU_NETFLOWMOCK_API_KEY", "test-key");
    std::env::remove_var("RUPU_MOCK_PROVIDER_SCRIPT");
    std::env::set_current_dir(project.path()).unwrap();

    for run_id in ["run-first", "run-second"] {
        let exit = rupu_cli::run(vec![
            "rupu".into(),
            "run".into(),
            "echo".into(),
            "--mode".into(),
            "bypass".into(),
            "--run-id".into(),
            run_id.into(),
            "say hi".into(),
        ])
        .await;
        assert_eq!(
            format!("{exit:?}"),
            format!("{:?}", std::process::ExitCode::from(0)),
            "rupu run ({run_id}) should exit 0 against the mocked endpoint"
        );
    }

    std::env::set_current_dir(tmp.path()).unwrap();
    std::env::remove_var("RUPU_NETFLOWMOCK_API_KEY");
    std::env::remove_var("RUPU_AUTH_FILE");
    std::env::remove_var("RUPU_HOME");

    let ledger_dir = project.path().join(".rupu/netflow");
    let first_path = ledger_dir.join("run-first.jsonl");
    let second_path = ledger_dir.join("run-second.jsonl");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let both_ready = std::fs::read_to_string(&first_path)
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false)
            && std::fs::read_to_string(&second_path)
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false);
        if both_ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "both per-run ledgers should have been written by now: {first_path:?} / {second_path:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let first_text = std::fs::read_to_string(&first_path).unwrap();
    let second_text = std::fs::read_to_string(&second_path).unwrap();

    assert_ne!(
        first_path, second_path,
        "sanity: the two runs must resolve to different ledger files"
    );
    assert_eq!(
        first_text
            .matches(r#""path":"/v1/chat/completions""#)
            .count(),
        1,
        "run-first's ledger must contain exactly its own one request, got: {first_text}"
    );
    assert_eq!(
        second_text
            .matches(r#""path":"/v1/chat/completions""#)
            .count(),
        1,
        "run-second's ledger must contain exactly its own one request, got: {second_text}"
    );
}
