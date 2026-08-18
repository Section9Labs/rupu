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

use assert_cmd::Command;
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

/// Important 4 (netflow-per-run Plan 3 Task 2 review round 1): the
/// requirement-4 machinery (partial failure -> non-zero exit, report
/// printed first) lives in `cmd::netflow::prune`/`handle`, which none
/// of `prune_ledgers`'s own unit tests exercise — those stop at the
/// library function. Drives the real `rupu` binary end to end, the same
/// pattern `cli_transcript.rs`'s `prune_deletes_old_archived_standalone_
/// transcripts_and_uses_config_default` uses for `transcript prune`.
///
/// Strips write permission from the ledger directory so `remove_file`
/// fails deterministically for the one eligible ledger inside it, then
/// asserts BOTH halves of requirement 4: the process exits non-zero,
/// Can this process actually be denied by directory permissions? Root
/// holds `CAP_DAC_OVERRIDE`, so stripping write permission from a parent
/// directory does not stop `remove_file` — and CI runs as root, which is
/// where this first bit. Probe by attempting the operation rather than
/// reading the uid: `geteuid` needs `libc` and `unsafe`, forbidden
/// workspace-wide, and the probe answers whether the denial is enforced
/// *here* rather than a proxy for it.
#[cfg(unix)]
fn permission_denial_is_enforced() -> bool {
    use std::os::unix::fs::PermissionsExt;

    let probe = assert_fs::TempDir::new().unwrap();
    let dir = probe.path();
    let victim = dir.join("probe.jsonl");
    std::fs::write(&victim, "{}\n").unwrap();

    let original = std::fs::metadata(dir).unwrap().permissions();
    let mut locked = original.clone();
    locked.set_mode(0o555);
    std::fs::set_permissions(dir, locked).unwrap();
    let denied = std::fs::remove_file(&victim).is_err();
    std::fs::set_permissions(dir, original).unwrap();
    denied
}

/// AND the report (naming the failed ledger) was still printed to
/// stdout before that exit — a failure must not look like silence.
#[cfg(unix)]
#[tokio::test]
async fn prune_reports_a_removal_failure_and_exits_non_zero() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = ENV_LOCK.lock().await;

    if !permission_denial_is_enforced() {
        eprintln!(
            "skipping prune_reports_a_removal_failure_and_exits_non_zero: this process can \
             bypass directory permissions (running as root?), so the removal cannot be made \
             to fail and the non-zero-exit assertion would be vacuous"
        );
        return;
    }

    let tmp = assert_fs::TempDir::new().unwrap();
    let global = tmp.child(".rupu");
    let netflow_dir = global.path().join("netflow");
    std::fs::create_dir_all(&netflow_dir).unwrap();

    let stuck = netflow_dir.join("run_stuck.jsonl");
    std::fs::write(&stuck, "{}\n").unwrap();
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 60 * 60);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&stuck)
        .unwrap()
        .set_modified(then)
        .unwrap();

    let original_perms = std::fs::metadata(&netflow_dir).unwrap().permissions();
    let mut locked = original_perms.clone();
    locked.set_mode(0o555); // read+execute, no write — remove_file fails EACCES
    std::fs::set_permissions(&netflow_dir, locked).unwrap();

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", global.path())
        .current_dir(tmp.path())
        .args(["netflow", "prune", "--older-than", "30d"])
        .output()
        .unwrap();

    // Restore before any assertion can panic, so the TempDir's own
    // cleanup on drop never fails.
    std::fs::set_permissions(&netflow_dir, original_perms).unwrap();

    assert!(
        !output.status.success(),
        "a removal failure must exit non-zero; status: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("run_stuck"),
        "the report must name the ledger that failed to remove, printed \
         BEFORE the non-zero exit, not swallowed by it; stdout: {stdout}"
    );
    assert!(
        stuck.exists(),
        "the failed removal must leave the file in place"
    );
}

/// Important 2 (netflow-per-run Plan 3 Task 2 review round 2): every
/// `prune_ledgers` unit test drives that function directly against ONE
/// temp directory, and the only prior integration test seeds just the
/// global root — nothing pinned `prune()`'s OWN candidate resolution
/// (`crates/rupu-cli/src/cmd/netflow.rs`'s `prune` function), so
/// deleting the entire project-local branch, its scope label, or its
/// `is_dir()` gate would have left the suite green. This drives the
/// real `rupu` binary against a project whose OWN `.rupu/netflow/` is
/// GENUINELY DISTINCT from the global root (a separate `home/` subtree
/// entirely, not the `project_root == ~` collision Important 1's dedup
/// fix addresses) with one stale ledger in each, and asserts both are
/// swept: both run ids gone from disk, both scope labels present in
/// the report. A genuinely-different pair of roots also guards the
/// dedup fix from the other direction — it must never collapse two
/// roots that are NOT the same directory.
#[tokio::test]
async fn prune_sweeps_both_a_distinct_project_local_root_and_the_global_root() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let project_dir = tmp.child("project");
    project_dir.create_dir_all().unwrap();
    let project_netflow = project_dir.path().join(".rupu/netflow");
    std::fs::create_dir_all(&project_netflow).unwrap();

    // A `home/` subtree entirely separate from `project/` — canonicalizing
    // either root must never make them collide.
    let home = tmp.child("home");
    let global_netflow = home.path().join(".rupu/netflow");
    std::fs::create_dir_all(&global_netflow).unwrap();

    let project_ledger = project_netflow.join("run_project.jsonl");
    let global_ledger = global_netflow.join("run_global.jsonl");
    std::fs::write(&project_ledger, "{}\n").unwrap();
    std::fs::write(&global_ledger, "{}\n").unwrap();
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 60 * 60);
    for p in [&project_ledger, &global_ledger] {
        std::fs::OpenOptions::new()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(then)
            .unwrap();
    }

    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path().join(".rupu"))
        .current_dir(project_dir.path())
        .args([
            "--format",
            "json",
            "netflow",
            "prune",
            "--older-than",
            "30d",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "status: {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""run_id": "run_project""#),
        "project-local ledger missing from the report: {stdout}"
    );
    assert!(
        stdout.contains(r#""run_id": "run_global""#),
        "global ledger missing from the report: {stdout}"
    );
    assert!(
        stdout.contains(r#""scope": "project""#),
        "project scope label missing: {stdout}"
    );
    assert!(
        stdout.contains(r#""scope": "global""#),
        "global scope label missing: {stdout}"
    );

    assert!(
        !project_ledger.exists(),
        "the project-local ledger must be removed"
    );
    assert!(!global_ledger.exists(), "the global ledger must be removed");
}

/// Important 1 (netflow-per-run Plan 3 Task 2 review round 2): when
/// `project_root_for` resolves the project root to `$HOME` itself (the
/// common case for `cwd == $HOME` or anywhere under it that isn't
/// inside a rupu project — the global root literally IS `~/.rupu`),
/// `project_local_netflow_dir(project_root)` and `global_netflow_dir`
/// are the SAME directory. Before the dedup fix, `candidates` held
/// that directory twice, so `--dry-run` reported every eligible ledger
/// TWICE and doubled the reclaimable-bytes total — the one number an
/// operator sizing a deletion is reading the preview for. Asserts the
/// dry-run report lists the one stale ledger exactly once.
#[tokio::test]
async fn dry_run_does_not_double_report_when_the_project_root_is_home() {
    let _guard = ENV_LOCK.lock().await;

    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.child("home");
    let netflow_dir = home.path().join(".rupu/netflow");
    std::fs::create_dir_all(&netflow_dir).unwrap();

    let ledger = netflow_dir.join("run_home.jsonl");
    std::fs::write(&ledger, "{}\n").unwrap();
    let then = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 60 * 60);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&ledger)
        .unwrap()
        .set_modified(then)
        .unwrap();

    // `cwd == RUPU_HOME`'s parent (i.e. `cwd` IS `$HOME`): `project_root_for`
    // finds `home/.rupu` walking up from `home` itself, so the project-local
    // and global candidates resolve to the exact same directory — the
    // collision this test pins.
    let output = Command::cargo_bin("rupu")
        .unwrap()
        .env("RUPU_HOME", home.path().join(".rupu"))
        .current_dir(home.path())
        .args([
            "--format",
            "json",
            "netflow",
            "prune",
            "--older-than",
            "30d",
            "--dry-run",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "status: {:?}, stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches(r#""run_id": "run_home""#).count(),
        1,
        "the colliding project-local/global roots must be swept ONCE, not \
         twice, or --dry-run doubles the reported reclaimable bytes: {stdout}"
    );

    assert!(ledger.exists(), "dry-run must never delete");
}
